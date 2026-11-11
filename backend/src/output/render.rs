//! Renders a slideshow mp4 from images plus one audio track with ffmpeg.

use std::path::{Path, PathBuf};

use crate::engines::proc::{self, Cancel, Result};
use crate::output::media::{self, VideoEncoder};

const FALLBACK_SECONDS_PER_IMAGE: f64 = 3.0;
const MAX_SIDE: u32 = 1920;

/// Each image is shown for an equal share of the audio. `on_progress` gets 0-100.
pub async fn render(
    ffmpeg: &Path,
    images: &[PathBuf],
    audio: &Path,
    output: &Path,
    cancel: &Cancel,
    on_progress: impl Fn(f64),
) -> Result<()> {
    if images.is_empty() {
        return Err("No images to render".into());
    }
    let audio_secs = media::inspect(ffmpeg, audio).await.duration;
    let total = audio_secs.unwrap_or(FALLBACK_SECONDS_PER_IMAGE * images.len() as f64);
    let per_image = (total / images.len() as f64).max(0.5);
    let (width, height) = output_size(media::inspect(ffmpeg, &images[0]).await.dimensions);

    let mut args: Vec<String> = vec!["-y".into(), "-hide_banner".into()];
    for image in images {
        args.extend([
            "-loop".into(),
            "1".into(),
            "-t".into(),
            format!("{per_image:.3}"),
            "-i".into(),
            image.to_string_lossy().into(),
        ]);
    }
    args.extend(["-i".into(), audio.to_string_lossy().into()]);

    // Letterbox every image into the same frame, then join them end to end.
    let mut filter = String::new();
    for i in 0..images.len() {
        filter.push_str(&format!(
            "[{i}:v]scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black,setsar=1,fps=30,format=yuv420p[v{i}];"
        ));
    }
    for i in 0..images.len() {
        filter.push_str(&format!("[v{i}]"));
    }
    filter.push_str(&format!("concat=n={}:v=1:a=0[v]", images.len()));

    args.extend(["-filter_complex".into(), filter, "-map".into(), "[v]".into(), "-map".into(), format!("{}:a", images.len())]);

    // Try each H.264 encoder this machine has (VideoToolbox on macOS, then x264).
    let mut encoders = VideoEncoder::h264_for_this_machine();
    let mut last_error = String::new();
    while !encoders.is_empty() {
        let encoder = encoders.remove(0);
        let mut run_args = args.clone();
        run_args.extend(encoder.args((width, height)));
        run_args.extend(
            ["-c:a", "aac", "-b:a", "192k", "-shortest", "-movflags", "+faststart", "-progress", "pipe:1", "-nostats"]
                .map(String::from),
        );
        run_args.push(output.to_string_lossy().into());

        let out = proc::run(ffmpeg, &run_args, Some(cancel), |line| {
            // `-progress` reports out_time_us=<microseconds rendered so far>.
            if let Some(us) = line.strip_prefix("out_time_us=").and_then(|v| v.parse::<f64>().ok()) {
                on_progress((us / 1e6 / total * 100.0).clamp(0.0, 100.0));
            }
        })
        .await;
        match out {
            Ok(o) if o.success => return Ok(()),
            Ok(o) => last_error = o.error_message(),
            Err(e) => {
                let _ = std::fs::remove_file(output);
                return Err(e);
            }
        }
    }
    let _ = std::fs::remove_file(output);
    Err(last_error)
}

/// Keeps the first image's aspect ratio, capped at 1920px and rounded to even numbers
/// (required by yuv420p). Defaults to vertical 1080x1920 like most photo posts.
fn output_size(first_image: Option<(u32, u32)>) -> (u32, u32) {
    let (w, h) = first_image.filter(|(w, h)| *w > 0 && *h > 0).unwrap_or((1080, 1920));
    let scale = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    let even = |v: f64| ((v.round() as u32) / 2 * 2).max(2);
    (even(w as f64 * scale), even(h as f64 * scale))
}

#[cfg(test)]
mod tests {
    use super::output_size;

    #[test]
    fn output_size_caps_and_evens() {
        assert_eq!(output_size(None), (1080, 1920));
        assert_eq!(output_size(Some((4000, 3000))), (1920, 1440));
        assert_eq!(output_size(Some((1081, 721))), (1080, 720));
    }
}
