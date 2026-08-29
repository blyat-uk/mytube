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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
