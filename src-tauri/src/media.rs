//! Inspecting media files with ffmpeg, and converting videos to codecs every player supports.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

use crate::proc::{self, err, Cancel, Result};

static DURATION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Duration: (\d+):(\d{2}):(\d{2}(?:\.\d+)?)").unwrap());
static VIDEO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Stream #[^\n]*?Video: (\w+)").unwrap());
static AUDIO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"Stream #[^\n]*?Audio: (\w+)").unwrap());
static DIMENSIONS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Video: [^\n]*?, (\d{2,5})x(\d{2,5})").unwrap());

#[derive(Debug, Default)]
pub struct Info {
    pub duration: Option<f64>,
    pub dimensions: Option<(u32, u32)>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
}

/// Reads `ffmpeg -i`'s banner. Missing fields stay `None`.
pub async fn inspect(ffmpeg: &Path, file: &Path) -> Info {
    let args = [std::ffi::OsStr::new("-hide_banner"), "-i".as_ref(), file.as_os_str()];
    // ffmpeg exits non-zero without an output file; the banner is all we need.
    let Ok(out) = proc::run(ffmpeg, args, None, |_| {}).await else {
        return Info::default();
    };
    parse(&out.stderr)
}

fn parse(banner: &str) -> Info {
    let duration = DURATION.captures(banner).and_then(|c| {
        let h: f64 = c[1].parse().ok()?;
        let m: f64 = c[2].parse().ok()?;
        let s: f64 = c[3].parse().ok()?;
        Some(h * 3600.0 + m * 60.0 + s).filter(|d| *d > 0.0)
    });
    Info {
        duration,
        dimensions: DIMENSIONS.captures(banner).and_then(|c| Some((c[1].parse().ok()?, c[2].parse().ok()?))),
        video_codec: VIDEO.captures(banner).map(|c| c[1].to_string()),
        audio_codec: AUDIO.captures(banner).map(|c| c[1].to_string()),
    }
}

/// QuickTime, Windows' players and every browser handle H.264 video with AAC/MP3 audio.
/// VP9, AV1 and Opus (common on Instagram/YouTube) are not supported everywhere.
fn video_ok(codec: &str) -> bool {
    codec == "h264"
}

fn audio_ok(codec: &str) -> bool {
    matches!(codec, "aac" | "mp3")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoEncoder {
    Copy,
    /// Apple's H.264 encoder, part of macOS. The bundled Mac FFmpeg is built without x264
    /// (to stay redistributable), so this is the encoder there; it's also much faster.
    VideoToolbox,
    /// x264, included in the Windows/Linux FFmpeg builds.
    X264,
}

impl VideoEncoder {
    /// H.264 encoders to try, in order, on this OS.
    pub fn h264_for_this_machine() -> Vec<Self> {
        if cfg!(target_os = "macos") {
            vec![Self::VideoToolbox, Self::X264]
        } else {
            vec![Self::X264]
        }
    }

    /// `-c:v …` arguments for a `width`x`height` H.264 output.
    pub fn args(self, (w, h): (u32, u32)) -> Vec<String> {
        match self {
            Self::Copy => vec!["-c:v".into(), "copy".into()],
            Self::VideoToolbox => {
                // VideoToolbox has no constant-quality mode everywhere; size the bitrate to the frame
                // (~0.15 bit per pixel at 30 fps: 1080p ≈ 9 Mb/s, 4K ≈ 37 Mb/s).
                let kbps = ((w as f64 * h as f64 * 30.0 * 0.15) / 1000.0).clamp(2_000.0, 60_000.0) as u32;
                let mut a: Vec<String> =
                    ["-c:v", "h264_videotoolbox", "-profile:v", "high", "-pix_fmt", "yuv420p"].map(String::from).into();
                a.extend(["-b:v".into(), format!("{kbps}k"), "-maxrate".into(), format!("{}k", kbps * 3 / 2)]);
                a
            }
            Self::X264 => ["-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p"].map(String::from).into(),
        }
    }
}

fn convert_args(input: &Path, output: &Path, encoder: VideoEncoder, info: &Info, audio_fine: bool) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-y".into(),
        "-hide_banner".into(),
        "-i".into(),
        input.to_string_lossy().into(),
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        "0:a:0?".into(),
    ];
    args.extend(encoder.args(info.dimensions.unwrap_or((1920, 1080))));
    if audio_fine {
        args.extend(["-c:a".into(), "copy".into()]);
    } else {
        args.extend(["-c:a", "aac", "-b:a", "192k"].map(String::from));
    }
    args.extend(["-movflags", "+faststart", "-progress", "pipe:1", "-nostats"].map(String::from));
    args.push(output.to_string_lossy().into());
    args
}

/// Makes sure a downloaded video plays everywhere: if it isn't H.264 + AAC/MP3 in an mp4,
/// converts it (re-encoding only the streams that need it) and replaces the original.
/// Returns the final path, which ends in `.mp4`.
pub async fn make_playable(
    ffmpeg: &Path,
    path: &Path,
    cancel: &Cancel,
    on_progress: impl Fn(f64),
) -> Result<PathBuf> {
    let info = inspect(ffmpeg, path).await;
    let Some(video) = info.video_codec.as_deref() else {
        return Ok(path.to_path_buf()); // not a video (or unreadable); leave it alone
    };
    let video_fine = video_ok(video);
    let audio_fine = info.audio_codec.as_deref().is_none_or(audio_ok);
    let is_mp4 = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("mp4"));
    if video_fine && audio_fine && is_mp4 {
        return Ok(path.to_path_buf());
    }

    let temp = path.with_extension("converting.mp4");
    let total = info.duration.unwrap_or(0.0);
    let mut encoders = if video_fine { vec![VideoEncoder::Copy] } else { VideoEncoder::h264_for_this_machine() };
    let result = loop {
        let encoder = encoders.remove(0);
        let args = convert_args(path, &temp, encoder, &info, audio_fine);
        let out = proc::run(ffmpeg, &args, Some(cancel), |line| {
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                if total > 0.0 {
                    on_progress((us / 1e6 / total * 100.0).clamp(0.0, 100.0));
                }
            }
        })
        .await;
        match out {
            Ok(o) if o.success => break Ok(()),
            // The hardware encoder can refuse some inputs (sizes, pixel formats): fall back.
            Ok(_) if !encoders.is_empty() => continue,
            Ok(o) => break Err(format!("Couldn't convert the video: {}", o.error_message())),
            Err(e) => break Err(e),
        }
    };
    if let Err(e) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }

    // `clip.webm` becomes `clip.mp4`, or `clip (2).mp4` if that name is taken.
    let final_path = if is_mp4 {
        path.to_path_buf()
    } else {
        crate::download::reserve_unique(&path.with_extension("mp4"))?.0
    };
    std::fs::remove_file(path).map_err(err)?;
    std::fs::rename(&temp, &final_path).map_err(err)?;
    Ok(final_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_banner() {
        let banner = "Input #0, matroska,webm, from 'x.webm':\n  Duration: 00:01:02.50, start: 0.000000, bitrate: 1 kb/s\n  \
                      Stream #0:0(eng): Video: vp9 (Profile 0), yuv420p(tv), 1920x1080, SAR 1:1 DAR 16:9, 30 fps\n  \
                      Stream #0:1(eng): Audio: opus, 48000 Hz, stereo, fltp (default)\n";
        let info = parse(banner);
        assert_eq!(info.duration, Some(62.5));
        assert_eq!(info.dimensions, Some((1920, 1080)));
        assert_eq!(info.video_codec.as_deref(), Some("vp9"));
        assert_eq!(info.audio_codec.as_deref(), Some("opus"));
    }
}
