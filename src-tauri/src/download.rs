//! Downloads a single probed item into a folder and returns the saved file's path.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::binaries::Engine;
use crate::http;
use crate::media;
use crate::probe::{ext_of, file_name_of, Kind, MediaItem, Source};
use crate::proc::{self, err, Cancel, Result};

const PROGRESS_PREFIX: &str = "[da]";

/// `on_progress` gets one overall 0-100 that only moves forward, or -1 when progress can't
/// be measured. Separate video/audio files and any conversion all count towards it.
pub async fn download(
    engine: &Engine,
    item: &MediaItem,
    out_dir: &Path,
    cancel: &Cancel,
    on_progress: impl Fn(f64),
) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir).map_err(err)?;
    let overall = OverallProgress::new(&on_progress, expects_conversion(item));
    let downloading = |p| overall.download(p);
    let saved = match &item.source {
        Source::YtDlp { url, playlist_index, audio_only, max_height } => {
            let opts = YtDlpOptions { playlist_index: *playlist_index, audio_only: *audio_only, max_height: *max_height };
            yt_dlp(engine, url, &item.title, opts, out_dir, cancel, downloading).await
        }
        Source::Direct { url, referer } => direct(url, referer.as_deref(), out_dir, cancel, downloading).await,
    }?;
    // Sites often serve VP9/AV1/Opus, which QuickTime and Windows can't play.
    if item.kind == Kind::Video {
        return media::make_playable(&engine.ffmpeg, &saved, cancel, |p| overall.convert(p)).await;
    }
    Ok(saved)
}

/// The chosen resolution only exists as VP9/AV1, so media::make_playable will re-encode it.
fn expects_conversion(item: &MediaItem) -> bool {
    let Source::YtDlp { max_height: Some(h), .. } = item.source else { return false };
    item.qualities.iter().any(|q| q.height == h && !q.compatible)
}

/// Folds the steps of one download into a single bar:
/// - yt-dlp fetches video and audio as two files, each reporting 0-100. The first (video) is
///   the bulk; a drop in percent means the next file started.
/// - an expected conversion gets the last 30%.
/// The bar never moves backwards.
struct OverallProgress<'a, F: Fn(f64)> {
    emit: &'a F,
    /// Share of the bar for downloading (the rest is conversion).
    download_share: f64,
    /// (files started, last raw percent, last emitted percent)
    state: Mutex<(u32, f64, f64)>,
}

impl<'a, F: Fn(f64)> OverallProgress<'a, F> {
    fn new(emit: &'a F, expect_conversion: bool) -> Self {
        let download_share = if expect_conversion { 70.0 } else { 100.0 };
        Self { emit, download_share, state: Mutex::new((0, 0.0, 0.0)) }
    }

    fn download(&self, raw: f64) {
        if raw < 0.0 {
            return (self.emit)(-1.0);
        }
        let value = {
            let mut st = self.state.lock().unwrap();
            if raw + 30.0 < st.1 {
                st.0 += 1;
            }
            st.1 = raw;
            let first_part = self.download_share * 0.9;
            if st.0 == 0 {
                raw / 100.0 * first_part
            } else {
                first_part + raw / 100.0 * (self.download_share - first_part)
            }
        };
        self.forward(value);
    }

    fn convert(&self, raw: f64) {
        let value = if self.download_share < 100.0 {
            self.download_share + raw.max(0.0) / 100.0 * (100.0 - self.download_share)
        } else {
            // Unplanned conversion: creep towards the end without going backwards.
            let last = self.state.lock().unwrap().2;
            last + (99.0 - last).max(0.0) * raw.max(0.0) / 100.0
        };
        self.forward(value);
    }

    fn forward(&self, value: f64) {
        let mut st = self.state.lock().unwrap();
        if value > st.2 {
            st.2 = value;
            (self.emit)(value);
        }
    }
}

struct YtDlpOptions {
    playlist_index: Option<u32>,
    audio_only: bool,
    max_height: Option<u32>,
}

async fn yt_dlp(
    engine: &Engine,
    url: &str,
    title: &str,
    YtDlpOptions { playlist_index, audio_only, max_height }: YtDlpOptions,
    out_dir: &Path,
    cancel: &Cancel,
    on_progress: impl Fn(f64),
) -> Result<PathBuf> {
    // Name the file after the cleaned title the UI shows, not the site's raw one
    // ("393K views · 16K reactions | …"). `%` is yt-dlp's template marker, so escape it.
    let name = sanitize(title).replace('%', "%%");
    let template = out_dir.join(format!("{name} [%(id)s].%(ext)s"));
    let mut args: Vec<String> = vec![
        "--no-warnings".into(),
        "--color".into(),
        "never".into(),
        "--newline".into(),
        "--progress".into(),
        "--progress-template".into(),
        format!("download:{PROGRESS_PREFIX}%(progress._percent_str)s"),
        "--print".into(),
        "after_move:filepath".into(),
        "--ffmpeg-location".into(),
        engine.ffmpeg.to_string_lossy().into(),
        "-o".into(),
        template.to_string_lossy().into(),
    ];
    match playlist_index {
        Some(i) => args.extend(["--yes-playlist".into(), "--playlist-items".into(), i.to_string()]),
        None => args.push("--no-playlist".into()),
    }
    if audio_only {
        args.extend(["-f", "ba/b", "-x", "--audio-format", "mp3", "--audio-quality", "0"].map(String::from));
    } else {
        // Default: the best H.264 + AAC, which plays everywhere without converting. With a
        // chosen resolution: exactly that (H.264 if offered at it); VP9/AV1 picks are converted
        // afterwards by media::make_playable.
        let sort = match max_height {
            Some(h) => format!("res:{h},vcodec:h264,acodec:aac,ext:mp4:m4a"),
            None => "vcodec:h264,res,acodec:aac,ext:mp4:m4a".into(),
        };
        args.extend(["-f".into(), "bv*+ba/b".into(), "-S".into(), sort, "--merge-output-format".into(), "mp4".into()]);
    }
    args.push(url.into());

    let out = proc::run(&engine.yt_dlp, &args, Some(cancel), |line| {
        if let Some(pct) = line.strip_prefix(PROGRESS_PREFIX) {
            if let Ok(p) = pct.trim().trim_end_matches('%').parse::<f64>() {
                on_progress(p);
            }
        }
    })
    .await?;
    if !out.success {
        return Err(out.error_message());
    }
    last_existing_path(&out.stdout).ok_or_else(|| "yt-dlp finished but no file was saved".into())
}

async fn direct(
    url: &str,
    referer: Option<&str>,
    out_dir: &Path,
    cancel: &Cancel,
    on_progress: impl Fn(f64),
) -> Result<PathBuf> {
    let mut req = http::client().get(url);
    if let Some(r) = referer {
        req = req.header(reqwest::header::REFERER, r);
    }
    let resp = req.send().await.and_then(|r| r.error_for_status()).map_err(err)?;

    let mut name = sanitize(&file_name_of(resp.url().as_str()));
    if ext_of(&name).is_empty() {
        let ext = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(ext_for_mime);
        if let Some(ext) = ext {
            name = format!("{name}.{ext}");
        }
    }

    // Write straight into a freshly reserved file so parallel downloads of same-named
    // files can never share a path; remove it again if anything goes wrong.
    let (dest, file) = reserve_unique(&out_dir.join(name))?;
    let mut file = tokio::fs::File::from_std(file);
    let result = http::stream_to_file(resp, &mut file, Some(cancel), on_progress).await;
    drop(file);
    if let Err(e) = result {
        let _ = std::fs::remove_file(&dest);
        return Err(e);
    }
    Ok(dest)
}

fn last_existing_path(stdout: &str) -> Option<PathBuf> {
    stdout
        .lines()
        .rev()
        .map(|l| PathBuf::from(l.trim()))
        .find(|p| p.is_absolute() && p.is_file())
}

pub fn ext_for_mime(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next()?.trim();
    Some(match mime {
        "image/jpeg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/avif" => "avif",
        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "audio/mpeg" => "mp3",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/ogg" => "ogg",
        "audio/wav" | "audio/x-wav" => "wav",
        _ => return None,
    })
}

pub fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if r#"<>:"/\|?*"#.contains(c) || c.is_control() { '_' } else { c })
        .collect();
    let cleaned = cleaned.trim().trim_matches('.');
    let cleaned: String = cleaned.chars().take(150).collect();
    if cleaned.is_empty() { "download".into() } else { cleaned }
}

/// `name.ext`, then `name (2).ext`, `name (3).ext`...
fn numbered(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    std::iter::once(path.to_path_buf())
        .chain((2..).map(move |n| path.with_file_name(format!("{stem} ({n}){ext}"))))
}

/// Atomically creates the first free name from [`numbered`], so no other job can take it.
pub fn reserve_unique(path: &Path) -> Result<(PathBuf, std::fs::File)> {
    for candidate in numbered(path).take(10_000) {
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&candidate) {
            Ok(file) => return Ok((candidate, file)),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(err(e)),
        }
    }
    Err(format!("No free file name for {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overall_progress_is_one_forward_bar() {
        let seen = Mutex::new(Vec::new());
        let emit = |p: f64| seen.lock().unwrap().push(p.round());
        let bar = OverallProgress::new(&emit, true);
        for p in [0.0, 50.0, 100.0] {
            bar.download(p); // video file
        }
        for p in [0.0, 50.0, 100.0] {
            bar.download(p); // audio file starts again at 0
        }
        for p in [0.0, 50.0, 100.0] {
            bar.convert(p);
        }
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen, [32.0, 63.0, 67.0, 70.0, 85.0, 100.0]);
        assert!(seen.windows(2).all(|w| w[1] > w[0]));
    }

    #[test]
    fn reservations_never_share_a_name() {
        let dir = std::env::temp_dir().join("download-anything-reserve");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = dir.join("photo.jpg");
        let names: Vec<_> = (0..3).map(|_| reserve_unique(&target).unwrap().0).collect();
        assert_eq!(names, [dir.join("photo.jpg"), dir.join("photo (2).jpg"), dir.join("photo (3).jpg")]);
    }
}
