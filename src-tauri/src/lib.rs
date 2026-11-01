mod binaries;
mod download;
mod http;
mod media;
mod photos;
mod probe;
mod proc;
mod render;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::watch;

use binaries::Engine;
use probe::{Kind, MediaItem, ProbeResult};
use proc::{err, Cancel, Result};

#[derive(Default)]
struct AppState {
    engine: tokio::sync::Mutex<Option<Arc<Engine>>>,
    jobs: Mutex<HashMap<String, watch::Sender<bool>>>,
}

impl AppState {
    async fn engine(&self) -> Result<Arc<Engine>> {
        self.engine.lock().await.clone().ok_or_else(|| "Engine is still setting up".into())
    }

    /// Registers a cancellable job. Ids are unique per running job; a duplicate would
    /// otherwise replace (and orphan) the first job's cancel channel.
    fn start_job(&self, id: &str) -> Result<Cancel> {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.contains_key(id) {
            return Err("Already in progress".into());
        }
        let (tx, rx) = watch::channel(false);
        jobs.insert(id.to_string(), tx);
        Ok(rx)
    }

    fn finish_job(&self, id: &str) {
        self.jobs.lock().unwrap().remove(id);
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct JobProgress<'a> {
    job_id: &'a str,
    percent: f64,
}

/// Emits `job-progress` for `job_id`, at most once per whole percent.
fn progress_emitter<'a>(app: &'a AppHandle, job_id: &'a str) -> impl Fn(f64) + 'a {
    // f64 bits of the last emitted value; an atomic keeps the command future `Send`.
    let last = AtomicU64::new(u64::MAX);
    move |percent: f64| {
        let percent = if percent < 0.0 { -1.0 } else { percent.floor().min(100.0) };
        if last.swap(percent.to_bits(), Ordering::Relaxed) != percent.to_bits() {
            let _ = app.emit("job-progress", JobProgress { job_id, percent });
        }
    }
}

fn out_dir(app: &AppHandle, chosen: Option<String>) -> Result<PathBuf> {
    match chosen.filter(|d| !d.is_empty()) {
        Some(dir) => Ok(PathBuf::from(dir)),
        None => default_dir(app),
    }
}

fn default_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().download_dir().map_err(err)?.join("Download Anything"))
}

/// Finds the engines bundled with the app. Nothing is downloaded.
#[tauri::command]
async fn setup_engine(app: AppHandle, state: State<'_, AppState>) -> Result<Engine> {
    let mut slot = state.engine.lock().await;
    if let Some(engine) = slot.as_ref() {
        return Ok((**engine).clone());
    }
    let engine = binaries::locate(&updates_dir(&app)?)?;
    *slot = Some(Arc::new(engine.clone()));
    Ok(engine)
}

/// Where "Update yt-dlp" keeps the newer yt-dlp (the bundled one stays untouched).
fn updates_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir().map_err(err)?.join("updates"))
}

#[tauri::command]
async fn engine_status(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<binaries::ToolStatus>> {
    let engine = state.engine().await?;
    Ok(binaries::status(&engine, &updates_dir(&app)?).await)
}

/// Settings → "Update yt-dlp". Runs only when clicked; the download is checked against
/// yt-dlp's official SHA-256 list before the app switches to it. Progress: `update-progress`.
#[tauri::command]
async fn update_yt_dlp(app: AppHandle, state: State<'_, AppState>) -> Result<binaries::UpdateOutcome> {
    let current = state.engine().await?.yt_dlp.clone();
    let outcome = binaries::update_yt_dlp(&current, &updates_dir(&app)?, |p| {
        let _ = app.emit("update-progress", p);
    })
    .await?;
    if let binaries::UpdateOutcome::Updated { path } = &outcome {
        let mut slot = state.engine.lock().await;
        let mut engine = slot.as_deref().cloned().ok_or("Engine is still setting up")?;
        engine.yt_dlp = path.clone();
        *slot = Some(Arc::new(engine));
    }
    Ok(outcome)
}

#[tauri::command]
fn default_out_dir(app: AppHandle) -> Result<String> {
    let dir = default_dir(&app)?;
    // Created up front so "open folder" works before the first download.
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.to_string_lossy().into())
}

#[tauri::command]
async fn probe_url(job_id: String, url: String, state: State<'_, AppState>) -> Result<ProbeResult> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Paste a link that starts with http:// or https://".into());
    }
    let engine = state.engine().await?;
    let cancel = state.start_job(&job_id)?;
    let result = probe::probe(&engine, url, &cancel).await;
    state.finish_job(&job_id);
    result
}

#[tauri::command]
async fn download_item(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
    item: MediaItem,
    out_dir: Option<String>,
) -> Result<String> {
    let engine = state.engine().await?;
    let dir = self::out_dir(&app, out_dir)?;
    let cancel = state.start_job(&job_id)?;
    let result = download::download(&engine, &item, &dir, &cancel, progress_emitter(&app, &job_id)).await;
    state.finish_job(&job_id);
    Ok(result?.to_string_lossy().into())
}

#[tauri::command]
async fn render_slideshow(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
    images: Vec<MediaItem>,
    audio: MediaItem,
    title: String,
    out_dir: Option<String>,
) -> Result<String> {
    if audio.kind != Kind::Audio || images.iter().any(|i| i.kind != Kind::Image) {
        return Err("Render needs images plus one audio track".into());
    }
    let engine = state.engine().await?;
    let dir = self::out_dir(&app, out_dir)?;
    std::fs::create_dir_all(&dir).map_err(err)?;
    let work = app.path().app_cache_dir().map_err(err)?.join("render").join(&job_id);
    let cancel = state.start_job(&job_id)?;
    let emit = progress_emitter(&app, &job_id);

    let result = async {
        // Fetching inputs is the first 40% of the bar, encoding the rest.
        let steps = images.len() as f64 + 1.0;
        let mut image_paths = Vec::new();
        for (i, image) in images.iter().enumerate() {
            let dest = work.join(format!("img{i:03}"));
            let path = download::download(&engine, image, &dest, &cancel, |p| {
                emit((i as f64 + p.max(0.0) / 100.0) / steps * 40.0)
            })
            .await?;
            image_paths.push(path);
        }
        let audio_path = download::download(&engine, &audio, &work.join("audio"), &cancel, |p| {
            emit((steps - 1.0 + p.max(0.0) / 100.0) / steps * 40.0)
        })
        .await?;

        let name = format!("{} (slideshow).mp4", download::sanitize(&title));
        // Reserve the name so a parallel render of the same title can't pick it too;
        // ffmpeg then overwrites the empty file (and render() removes it on failure).
        let (output, _) = download::reserve_unique(&dir.join(name))?;
        render::render(&engine.ffmpeg, &image_paths, &audio_path, &output, &cancel, |p| {
            emit(40.0 + p * 0.6)
        })
        .await?;
        Ok::<_, String>(output)
    }
    .await;

    state.finish_job(&job_id);
    let _ = std::fs::remove_dir_all(&work);
    Ok(result?.to_string_lossy().into())
}

#[tauri::command]
fn cancel_job(job_id: String, state: State<'_, AppState>) {
    if let Some(tx) = state.jobs.lock().unwrap().get(&job_id) {
        let _ = tx.send(true);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            setup_engine,
            engine_status,
            update_yt_dlp,
            default_out_dir,
            probe_url,
            download_item,
            render_slideshow,
            cancel_job
        ])
        .run(tauri::tauri_build_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_job_ids_are_rejected_until_finished() {
        let state = AppState::default();
        assert!(state.start_job("1-0").is_ok());
        assert!(state.start_job("1-0").is_err());
        state.finish_job("1-0");
        assert!(state.start_job("1-0").is_ok());
    }
}

/// End-to-end checks against real sites and tools: `cargo test -- --ignored`.
#[cfg(test)]
mod e2e {
    use super::*;

    async fn engine(dir: &std::path::Path) -> Engine {
        binaries::locate(&dir.join("updates")).expect("bundled engines (run `pnpm engines`)")
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
                    println!("  {:?} {:?} {:?} {}", i.kind, i.format, i.detail, i.title);
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
}
