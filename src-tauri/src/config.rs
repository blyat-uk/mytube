use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn d_download_dir() -> String {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Videos")
        .join("mytube")
        .to_string_lossy()
        .into_owned()
}
fn d_filename_template() -> String {
    "%(uploader)s/%(title)s [%(id)s].%(ext)s".into()
}
fn d_player_command() -> String {
    "smplayer".into()
}
fn d_max_concurrent() -> usize {
    5
}
fn d_poll_interval() -> u64 {
    30
}
fn d_poll_on_startup() -> bool {
    true
}
fn d_backfill_count() -> u32 {
    30
}
/// Minimum grid card width in px, driven by Ctrl+scroll on the grid.
fn d_card_size() -> u32 {
    260
}
/// Window geometry in logical px. The defaults match `app.windows[0]` in
/// tauri.conf.json, so a first run and a deleted settings.json look alike.
fn d_window_width() -> u32 {
    1280
}
fn d_window_height() -> u32 {
    840
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "d_download_dir")]
    pub download_dir: String,
    #[serde(default = "d_filename_template")]
    pub filename_template: String,
    #[serde(default = "d_player_command")]
    pub player_command: String,
    #[serde(default = "d_max_concurrent")]
    pub max_concurrent_downloads: usize,
    #[serde(default = "d_poll_interval")]
    pub poll_interval_minutes: u64,
    #[serde(default = "d_poll_on_startup")]
    pub poll_on_startup: bool,
    #[serde(default = "d_backfill_count")]
    pub backfill_count: u32,
    #[serde(default = "d_card_size")]
    pub card_size: u32,
    #[serde(default = "d_window_width")]
    pub window_width: u32,
    #[serde(default = "d_window_height")]
    pub window_height: u32,
    /// Absent until a position is known. Wayland never reports one, so it stays
    /// null there however long the app is used.
    #[serde(default)]
    pub window_x: Option<i32>,
    #[serde(default)]
    pub window_y: Option<i32>,
    #[serde(default)]
    pub window_maximized: bool,
    /// Preserves keys written by future versions or by hand.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_json_str("{}").expect("empty object is valid")
    }
}

impl Settings {
    pub fn from_json_str(s: &str) -> Result<Self> {
        let mut v: Settings =
            serde_json::from_str(s).context("settings.json is not valid JSON")?;
        v.max_concurrent_downloads = v.max_concurrent_downloads.clamp(1, 16);
        v.poll_interval_minutes = v.poll_interval_minutes.clamp(1, 1440);
        v.backfill_count = v.backfill_count.clamp(1, 500);
        v.card_size = v.card_size.clamp(160, 640);
        // Floors match minWidth/minHeight in tauri.conf.json; a hand-edited or
        // corrupt value would otherwise restore a window too small to use.
        v.window_width = v.window_width.clamp(720, 16_384);
        v.window_height = v.window_height.clamp(480, 16_384);
        Ok(v)
    }

    pub fn to_json_string(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }
}

/// `~/.config/mytube` — deliberately not Tauri's app_config_dir(), which would
/// give `~/.config/uk.blyat.mytube`.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mytube")
}
pub fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}
pub fn db_path() -> PathBuf {
    config_dir().join("mytube.db")
}
pub fn thumbs_dir() -> PathBuf {
    config_dir().join("thumbs")
}

pub fn ensure_dirs() -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(thumbs_dir())?;
    Ok(())
}

pub fn load() -> Result<Settings> {
    let p = settings_path();
    if !p.exists() {
        return Ok(Settings::default());
    }
    let raw = std::fs::read_to_string(&p)?;
    Settings::from_json_str(&raw)
}

pub fn save(s: &Settings) -> Result<()> {
    ensure_dirs()?;
    write_atomic(&settings_path(), s.to_json_string()?.as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_apply_when_file_missing() {
        let s = Settings::from_json_str("{}").unwrap();
        assert_eq!(s.max_concurrent_downloads, 5);
        assert_eq!(s.poll_interval_minutes, 30);
        assert_eq!(s.backfill_count, 30);
        assert_eq!(s.card_size, 260);
        assert_eq!(s.player_command, "smplayer");
        assert!(s.poll_on_startup);
        assert_eq!(s.filename_template, "%(uploader)s/%(title)s [%(id)s].%(ext)s");
    }

    #[test]
    fn partial_file_keeps_defaults_for_missing_keys() {
        let s = Settings::from_json_str(r#"{"player_command":"mpv --fs"}"#).unwrap();
        assert_eq!(s.player_command, "mpv --fs");
        assert_eq!(s.max_concurrent_downloads, 5);
    }

    #[test]
    fn unknown_keys_survive_a_save_round_trip() {
        let s = Settings::from_json_str(r#"{"future_option":true}"#).unwrap();
        let out = s.to_json_string().unwrap();
        assert!(out.contains("future_option"), "unknown key was dropped: {out}");
    }

    #[test]
    fn card_size_is_clamped_to_the_zoom_range() {
        assert_eq!(Settings::from_json_str(r#"{"card_size":10}"#).unwrap().card_size, 160);
        assert_eq!(Settings::from_json_str(r#"{"card_size":9999}"#).unwrap().card_size, 640);
        assert_eq!(Settings::from_json_str(r#"{"card_size":300}"#).unwrap().card_size, 300);
    }

    #[test]
    fn window_geometry_defaults_match_the_tauri_config() {
        let s = Settings::default();
        assert_eq!(s.window_width, 1280);
        assert_eq!(s.window_height, 840);
        assert_eq!(s.window_x, None);
        assert_eq!(s.window_y, None);
        assert!(!s.window_maximized);
    }

    #[test]
    fn window_size_is_clamped_to_something_usable() {
        let tiny = Settings::from_json_str(r#"{"window_width":1,"window_height":1}"#).unwrap();
        assert_eq!((tiny.window_width, tiny.window_height), (720, 480));
        let huge =
            Settings::from_json_str(r#"{"window_width":999999,"window_height":999999}"#).unwrap();
        assert_eq!((huge.window_width, huge.window_height), (16_384, 16_384));
    }

    #[test]
    fn a_negative_window_position_survives_a_round_trip() {
        // A second monitor left of the primary one gives genuine negatives.
        let s = Settings::from_json_str(r#"{"window_x":-1920,"window_y":-40}"#).unwrap();
        assert_eq!((s.window_x, s.window_y), (Some(-1920), Some(-40)));
        let back = Settings::from_json_str(&s.to_json_string().unwrap()).unwrap();
        assert_eq!((back.window_x, back.window_y), (Some(-1920), Some(-40)));
    }

    #[test]
    fn window_keys_are_optional_in_an_existing_settings_file() {
        let s = Settings::from_json_str(r#"{"player_command":"mpv"}"#).unwrap();
        assert_eq!(s.window_width, 1280);
        assert_eq!(s.window_x, None);
    }

    #[test]
    fn concurrency_is_clamped_to_a_sane_range() {
        assert_eq!(
            Settings::from_json_str(r#"{"max_concurrent_downloads":0}"#)
                .unwrap()
                .max_concurrent_downloads,
            1
        );
        assert_eq!(
            Settings::from_json_str(r#"{"max_concurrent_downloads":999}"#)
                .unwrap()
                .max_concurrent_downloads,
            16
        );
    }
}
