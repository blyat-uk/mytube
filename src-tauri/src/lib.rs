pub mod commands;
pub mod config;
pub mod db;
pub mod models;
pub mod player;
pub mod poll;
pub mod queue;
pub mod resolve;
pub mod rss;
pub mod ytdlp;

use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            config::ensure_dirs()?;
            let settings = config::load().unwrap_or_default();
            std::fs::create_dir_all(&settings.download_dir).ok();

            let db = Arc::new(db::Db::open(&config::db_path())?);
            // A yt-dlp child never survives an app restart.
            db.reset_stale_downloads()?;

            let http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?;

            let queue = Arc::new(queue::Queue::new(
                db.clone(),
                app.handle().clone(),
                settings.max_concurrent_downloads,
            ));

            let state = Arc::new(poll::AppState {
                db,
                http,
                queue,
                poll_lock: Arc::new(tokio::sync::Mutex::new(())),
            });
            app.manage(state.clone());

            // Startup poll, then a repeating background timer.
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                if settings.poll_on_startup {
                    let channels = state.db.list_subscribed_channels().unwrap_or_default();
                    if !channels.is_empty() {
                        poll::poll_channels(&state, &handle, channels).await;
                    }
                }
                loop {
                    let mins = config::load()
                        .map(|s| s.poll_interval_minutes)
                        .unwrap_or(30);
                    tokio::time::sleep(std::time::Duration::from_secs(mins * 60)).await;
                    let channels = state.db.list_subscribed_channels().unwrap_or_default();
                    if !channels.is_empty() {
                        poll::poll_channels(&state, &handle, channels).await;
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::list_channels,
            commands::add_channel,
            commands::add_video,
            commands::classify_add_input,
            commands::remove_channel,
            commands::import_takeout_csv,
            commands::poll_all,
            commands::poll_channel,
            commands::list_videos,
            commands::set_watched,
            commands::enqueue_download,
            commands::cancel_download,
            commands::open_in_player,
            commands::delete_download,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
