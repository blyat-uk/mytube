pub mod commands;
pub mod config;
pub mod db;
pub mod models;
pub mod player;
pub mod poll;
pub mod queue;
pub mod resolve;
pub mod rss;
pub mod siblings;
pub mod transfer;
pub mod tray;
pub mod upload_date;
pub mod window;
pub mod ytdlp;

use std::sync::Arc;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // First, before anything else has been built: a second launch must be
        // turned away before it registers a tray of its own. Two copies share
        // the tray id, and therefore the same
        // `$XDG_RUNTIME_DIR/tray-icon/tray-icon-mytube-<n>.png` -- each one's
        // `set_icon` deletes the file the other's indicator is still pointing
        // at, so one of the two icons goes blank and stops badging. They also
        // both poll and both write the same SQLite file.
        //
        // Launching mytube again is how you ask for the window back, so that
        // is what it does.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            tray::show_window(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            config::ensure_dirs()?;
            let settings = config::load().unwrap_or_default();
            std::fs::create_dir_all(&settings.download_dir).ok();

            // The tray first: `window::track` asks whether one exists before it
            // turns a close into a hide. Only logged on failure — an app with
            // no tray is worth having, and it then closes the ordinary way.
            if let Err(err) = tray::init(app.handle()) {
                eprintln!("mytube: no tray icon: {err}");
            }

            // Before the database and the poller: the window is created hidden,
            // so nothing is on screen until this shows it.
            if let Some(win) = window::main_window(app.handle()) {
                window::restore(&win, &settings);
                window::track(&win, window::Geometry::from_settings(&settings));
            }

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
                // Replaces absent and approximate-bucket dates with real ones.
                let fixed = poll::repair_dates(&state).await;
                if fixed > 0 {
                    eprintln!("mytube: corrected {fixed} upload dates");
                }
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
            commands::save_view_state,
            commands::list_channels,
            commands::add_channel,
            commands::add_video,
            commands::classify_add_input,
            commands::remove_channel,
            commands::import_takeout_csv,
            commands::preview_takeout_csv,
            commands::set_video_hidden,
            commands::delete_video,
            commands::poll_all,
            commands::poll_channel,
            commands::set_channel_member,
            commands::list_videos,
            commands::list_video_groups,
            commands::mark_siblings,
            commands::unlink_siblings,
            commands::set_watched,
            commands::enqueue_download,
            commands::cancel_download,
            commands::open_in_player,
            commands::delete_download,
            commands::transfer_estimate,
            commands::export_config,
            commands::read_archive,
            commands::import_config,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
