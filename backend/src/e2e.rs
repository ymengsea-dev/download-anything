//! End-to-end checks against real sites and tools: `cargo test -- --ignored`.

use std::path::PathBuf;

use tokio::sync::watch;

use crate::engines::binaries::{self, Engine};
use crate::engines::proc;
use crate::output::{download, media, render};
use crate::sources::probe::{self, Kind};

async fn engine(dir: &std::path::Path) -> Engine {
    // Shared between tests: a new copy of the macOS yt-dlp takes seconds on its first run.
    let engines = std::env::temp_dir().join("download-anything-test-engines");
    binaries::locate(&dir.join("updates"), &engines).await.expect("bundled engines (run `pnpm engines`)")
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("download-anything-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
#[ignore]
async fn youtube_video_and_audio() {
    let dir = temp_dir("yt");
    let engine = engine(&dir).await;
    let (_tx, cancel) = watch::channel(false);
    let found = probe::probe(&engine, "https://www.youtube.com/watch?v=jNQXAC9IVRw", &cancel).await.unwrap();
    assert!(found.items.iter().any(|i| i.kind == Kind::Video));
    let audio = found.items.iter().find(|i| i.kind == Kind::Audio).unwrap();

    let path = download::download(&engine, audio, &dir, &cancel, |_| {}).await.unwrap();
    assert_eq!(path.extension().unwrap(), "mp3");
}

#[tokio::test]
#[ignore]
async fn direct_image_link() {
    let dir = temp_dir("direct");
    let engine = engine(&dir).await;
    let url = "https://upload.wikimedia.org/wikipedia/commons/a/a9/Example.jpg";
    let (_tx, cancel) = watch::channel(false);
    let found = probe::probe(&engine, url, &cancel).await.unwrap();
    assert_eq!(found.items.len(), 1);
    assert_eq!(found.items[0].kind, Kind::Image);

    let path = download::download(&engine, &found.items[0], &dir, &cancel, |_| {}).await.unwrap();
    assert!(path.metadata().unwrap().len() > 1000);
}

/// A video whose codecs QuickTime can't play (here MPEG-4 Part 2 + MP2 in .mkv; in the wild VP9/AV1 +
/// Opus) must come out as H.264 + AAC mp4. Uses only encoders the lean bundled FFmpeg has.
#[tokio::test]
#[ignore]
async fn incompatible_video_is_converted_to_h264_aac() {
    let dir = temp_dir("convert");
    let engine = engine(&dir).await;
    let clip = dir.join("clip.mkv");
    let args = [
        "-y", "-f", "lavfi", "-i", "testsrc=size=640x360:duration=2", "-f", "lavfi", "-i",
        "sine=frequency=440:duration=2", "-c:v", "mpeg4", "-c:a", "mp2",
    ];
    let made = proc::run(&engine.ffmpeg, args.iter().map(|a| a.to_string()).chain([clip.to_string_lossy().into()]), None, |_| {})
        .await
        .unwrap();
    assert!(made.success, "couldn't create test clip: {}", made.error_message());

    let (_tx, cancel) = watch::channel(false);
    let out = media::make_playable(&engine.ffmpeg, &clip, &cancel, |_| {}).await.unwrap();
    assert_eq!(out, dir.join("clip.mp4"));
    assert!(!clip.exists(), "original should be replaced");
    let info = media::inspect(&engine.ffmpeg, &out).await;
    assert_eq!(info.video_codec.as_deref(), Some("h264"));
    assert_eq!(info.audio_codec.as_deref(), Some("aac"));
}

#[tokio::test]
#[ignore]
async fn status_lists_every_tool_with_versions() {
    let dir = temp_dir("status");
    let engine = engine(&dir).await;
    let rows = binaries::status(&engine, &dir.join("updates")).await;
    for row in &rows {
        println!("{:?} {:?} updated={} {}", row.tool, row.version, row.updated, row.path.display());
    }
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.version.is_some() && !r.updated), "bundled yt-dlp and ffmpeg report versions");
}

/// What the app finds for a link, without downloading:
/// `DA_TEST_URL=<url> cargo test probe_from_env -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn probe_from_env() {
    let Ok(url) = std::env::var("DA_TEST_URL") else { return };
    let engine = engine(&temp_dir("probe-env")).await;
    let (_tx, cancel) = watch::channel(false);
    match probe::probe(&engine, &url, &cancel).await {
        Ok(found) => {
            println!("title: {}", found.title);
            for i in &found.items {
                println!("  {:?} {:?} {:?} {:?} {:?} {}", i.kind, i.format, i.detail, i.size, i.qualities, i.title);
            }
        }
        Err(e) => println!("error: {e}"),
    }
}

/// Images + sound from a link, rendered into a video like the "Make a video" row:
/// `DA_TEST_URL=<url> cargo test render_from_env -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn render_from_env() {
    let Ok(url) = std::env::var("DA_TEST_URL") else { return };
    let dir = temp_dir("render-env");
    let engine = engine(&dir).await;
    let (_tx, cancel) = watch::channel(false);
    let found = probe::probe(&engine, &url, &cancel).await.unwrap();
    let mut images = Vec::new();
    for (i, item) in found.items.iter().filter(|i| i.kind == Kind::Image).enumerate() {
        images.push(download::download(&engine, item, &dir.join(format!("img{i}")), &cancel, |_| {}).await.unwrap());
    }
    let audio = found.items.iter().find(|i| i.kind == Kind::Audio).expect("no sound found");
    let audio = download::download(&engine, audio, &dir.join("audio"), &cancel, |_| {}).await.unwrap();
    let out = dir.join("out.mp4");
    render::render(&engine.ffmpeg, &images, &audio, &out, &cancel, |_| {}).await.unwrap();
    let info = media::inspect(&engine.ffmpeg, &out).await;
    println!("{} image(s) + {} -> {:?} {:?} {:?}s", images.len(), audio.display(), info.video_codec, info.dimensions, info.duration);
}

/// Manual check for any link: `DA_TEST_URL=<url> cargo test download_from_env -- --ignored --nocapture`.
#[tokio::test]
#[ignore]
async fn download_from_env() {
    let Ok(url) = std::env::var("DA_TEST_URL") else { return };
    let dir = temp_dir("from-env");
    let engine = engine(&dir).await;
    let (_tx, cancel) = watch::channel(false);
    let found = probe::probe(&engine, &url, &cancel).await.unwrap();
    let mut item = found.items.iter().find(|i| i.kind == Kind::Video).unwrap_or(&found.items[0]).clone();
    println!("qualities: {:?}", item.qualities);
    // Optional DA_TEST_HEIGHT=1080 picks that resolution, as the UI's quality menu does.
    if let (Ok(h), probe::Source::YtDlp { max_height, .. }) = (std::env::var("DA_TEST_HEIGHT"), &mut item.source) {
        *max_height = h.parse().ok();
    }
    let percents = std::sync::Mutex::new(Vec::new());
    let path = download::download(&engine, &item, &dir, &cancel, |p| percents.lock().unwrap().push(p)).await.unwrap();
    let percents = percents.into_inner().unwrap();
    assert!(percents.windows(2).all(|w| w[1] >= w[0]), "progress went backwards: {percents:?}");
    println!("progress: {} updates, last {:?}", percents.len(), percents.last());
    let info = media::inspect(&engine.ffmpeg, &path).await;
    println!("saved {} | video={:?} audio={:?} size={:?}", path.display(), info.video_codec, info.audio_codec, info.dimensions);
}

#[tokio::test]
#[ignore]
async fn slideshow_from_images_and_audio() {
    let dir = temp_dir("render");
    let engine = engine(&dir).await;
    let run = |args: Vec<String>| proc::run(&engine.ffmpeg, args, None, |_| {});
    let s = |v: &str| v.to_string();
    for (i, size) in ["640x480", "480x640"].iter().enumerate() {
        let out = dir.join(format!("img{i}.png")).to_string_lossy().into_owned();
        let src = format!("testsrc=size={size}:duration=1");
        run(vec![s("-y"), s("-f"), s("lavfi"), s("-i"), src, s("-frames:v"), s("1"), out]).await.unwrap();
    }
    let audio = dir.join("tone.mp3").to_string_lossy().into_owned();
    let tone = s("sine=frequency=440:duration=4");
    run(vec![s("-y"), s("-f"), s("lavfi"), s("-i"), tone, audio.clone()]).await.unwrap();

    let images = [dir.join("img0.png"), dir.join("img1.png")];
    let output = dir.join("out.mp4");
    let (_tx, cancel) = watch::channel(false);
    render::render(&engine.ffmpeg, &images, std::path::Path::new(&audio), &output, &cancel, |_| {})
        .await
        .unwrap();
    assert!(output.metadata().unwrap().len() > 1000);
}
