//! The Tauri commands the UI calls (typed wrappers in frontend/shared/api.ts).

use std::path::PathBuf;
use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, State};

use crate::engines::binaries::{self, Engine};
use crate::engines::proc::{err, Result};
use crate::jobs::{progress_emitter, Jobs};
use crate::output::{download, render};
use crate::sources::probe::{self, Kind, MediaItem, ProbeResult};

#[derive(Default)]
pub struct AppState {
    engine: tokio::sync::Mutex<Option<Arc<Engine>>>,
    jobs: Jobs,
}

impl AppState {
    async fn engine(&self) -> Result<Arc<Engine>> {
        self.engine.lock().await.clone().ok_or_else(|| "Engine is still setting up".into())
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
pub async fn setup_engine(app: AppHandle, state: State<'_, AppState>) -> Result<Engine> {
    let mut slot = state.engine.lock().await;
    if let Some(engine) = slot.as_ref() {
        return Ok((**engine).clone());
    }
    let engine = binaries::locate(&updates_dir(&app)?, &app.path().app_data_dir().map_err(err)?.join("engines")).await?;
    *slot = Some(Arc::new(engine.clone()));
    Ok(engine)
}

/// Where "Update yt-dlp" keeps the newer yt-dlp (the bundled one stays untouched).
fn updates_dir(app: &AppHandle) -> Result<PathBuf> {
    Ok(app.path().app_data_dir().map_err(err)?.join("updates"))
}

#[tauri::command]
pub async fn engine_status(app: AppHandle, state: State<'_, AppState>) -> Result<Vec<binaries::ToolStatus>> {
    let engine = state.engine().await?;
    Ok(binaries::status(&engine, &updates_dir(&app)?).await)
}

/// Settings → "Update yt-dlp". Runs only when clicked; the download is checked against
/// yt-dlp's official SHA-256 list before the app switches to it. Progress: `update-progress`.
#[tauri::command]
pub async fn update_yt_dlp(app: AppHandle, state: State<'_, AppState>) -> Result<binaries::UpdateOutcome> {
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
pub fn default_out_dir(app: AppHandle) -> Result<String> {
    let dir = default_dir(&app)?;
    // Created up front so "open folder" works before the first download.
    std::fs::create_dir_all(&dir).map_err(err)?;
    Ok(dir.to_string_lossy().into())
}

#[tauri::command]
pub async fn probe_url(job_id: String, url: String, state: State<'_, AppState>) -> Result<ProbeResult> {
    let url = url.trim();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Paste a link that starts with http:// or https://".into());
    }
    let engine = state.engine().await?;
    let cancel = state.jobs.start(&job_id)?;
    let result = probe::probe(&engine, url, &cancel).await;
    state.jobs.finish(&job_id);
    result
}

#[tauri::command]
pub async fn download_item(
    app: AppHandle,
    state: State<'_, AppState>,
    job_id: String,
    item: MediaItem,
    out_dir: Option<String>,
) -> Result<String> {
    let engine = state.engine().await?;
    let dir = self::out_dir(&app, out_dir)?;
    let cancel = state.jobs.start(&job_id)?;
    let result = download::download(&engine, &item, &dir, &cancel, progress_emitter(&app, &job_id)).await;
    state.jobs.finish(&job_id);
    Ok(result?.to_string_lossy().into())
}

#[tauri::command]
pub async fn render_slideshow(
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
    let cancel = state.jobs.start(&job_id)?;
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

    state.jobs.finish(&job_id);
    let _ = std::fs::remove_dir_all(&work);
    Ok(result?.to_string_lossy().into())
}

#[tauri::command]
pub fn cancel_job(job_id: String, state: State<'_, AppState>) {
    state.jobs.cancel(&job_id);
}
