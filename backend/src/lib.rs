mod commands;
mod engines;
mod http;
mod jobs;
mod output;
mod sources;

#[cfg(test)]
mod e2e;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(commands::AppState::default())
        .invoke_handler(tauri::generate_handler![
            commands::setup_engine,
            commands::engine_status,
            commands::update_yt_dlp,
            commands::default_out_dir,
            commands::probe_url,
            commands::download_item,
            commands::render_slideshow,
            commands::cancel_job
        ])
        .run(tauri::tauri_build_context!())
        .expect("error while running tauri application");
}
