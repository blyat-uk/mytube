use crate::models::SortOrder;
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
fn d_sort() -> SortOrder {
    SortOrder::Newest
}

/// The Subscriptions feed's filters, remembered between runs.
///
/// The active tab is deliberately absent: a launch always lands on
/// Subscriptions.
///
/// Deliberately no `#[serde(flatten)] extra` here, unlike `Settings`: a key
/// this struct doesn't know about would round-trip through `get_settings`
/// but never appear in the frontend's hand-built `ViewState` literal, so the
/// "did anything change" comparison in `App.tsx` would disagree with itself
/// on every single launch -- the exact redundant write that guard exists to
/// prevent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewState {
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub search: String,
    #[serde(default)]
    pub hide_watched: bool,
    #[serde(default)]
    pub downloaded_only: bool,
    #[serde(default)]
    pub show_hidden: bool,
    #[serde(default)]
    pub grouped: bool,
    #[serde(default = "d_sort")]
    pub sort: SortOrder,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            channel_id: None,
            search: String::new(),
            hide_watched: false,
            downloaded_only: false,
            show_hidden: false,
            grouped: false,
            sort: SortOrder::Newest,
        }
    }
}

impl ViewState {
    /// Repairs values the UI can never produce but a hand-edited file can, so
    /// the pickers that read this block always land on an option they offer.
    /// `pub(crate)`, not private: `commands::save_view_state` calls this too,
    /// so the 200-character search cap (and the rest) bounds what gets
    /// *stored*, not just what `Settings::from_json_str` later *reads*.
    pub(crate) fn sanitize(&mut self) {
        // `Downloaded` belongs to the Downloads tab's own ordering, and
        // `Parts` only exists in the picker while Group siblings is on.
        if self.sort == SortOrder::Downloaded || (self.sort == SortOrder::Parts && !self.grouped) {
            self.sort = SortOrder::Newest;
        }
        let trimmed = self.search.trim();
        self.search = if trimmed.chars().count() > 200 {
            trimmed.chars().take(200).collect()
        } else {
            trimmed.to_string()
        };
        if self.channel_id.as_deref() == Some("") {
            self.channel_id = None;
        }
    }
}

fn lenient_view<'de, D: serde::Deserializer<'de>>(d: D) -> Result<ViewState, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
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
    /// The feed's filters. Deserialised leniently: settings.json is
    /// hand-editable by design, and a typo in this block should cost the
    /// filters, not the download directory.
    #[serde(default, deserialize_with = "lenient_view")]
    pub view: ViewState,
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
        v.view.sanitize();
        Ok(v)
    }

    pub fn to_json_string(&self) -> Result<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// The Settings view sends the whole object back, from a snapshot taken
    /// when it mounted -- and the feed's filters move while it is open. The
    /// file's copy of `view` therefore wins over any incoming one; only
    /// `save_view_state` writes that block.
    pub fn keep_view_of(&mut self, disk: &Settings) {
        self.view = disk.view.clone();
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

    #[test]
    fn view_defaults_when_absent_from_an_existing_settings_file() {
        let s = Settings::from_json_str(r#"{"player_command":"mpv"}"#).unwrap();
        assert_eq!(s.view, ViewState::default());
    }

    #[test]
    fn a_populated_view_round_trips_through_to_json_string_and_from_json_str() {
        let mut s = Settings::default();
        s.view = ViewState {
            channel_id: Some("UC123".into()),
            search: "cats".into(),
            hide_watched: true,
            downloaded_only: true,
            show_hidden: true,
            grouped: true,
            sort: SortOrder::Length,
        };
        let json = s.to_json_string().unwrap();
        let back = Settings::from_json_str(&json).unwrap();
        assert_eq!(back.view, s.view);
    }

    #[test]
    fn a_broken_view_block_falls_back_to_default_and_keeps_other_keys() {
        let s =
            Settings::from_json_str(r#"{"player_command":"mpv","view":{"sort":"sideways"}}"#)
                .unwrap();
        assert_eq!(s.view, ViewState::default());
        assert_eq!(s.player_command, "mpv");
    }

    #[test]
    fn a_view_of_the_wrong_type_falls_back_to_default_and_keeps_other_keys() {
        let s = Settings::from_json_str(r#"{"player_command":"mpv","view":42}"#).unwrap();
        assert_eq!(s.view, ViewState::default());
        assert_eq!(s.player_command, "mpv");
    }

    #[test]
    fn sort_parts_with_grouped_false_sanitises_to_newest() {
        let s = Settings::from_json_str(r#"{"view":{"sort":"parts","grouped":false}}"#).unwrap();
        assert_eq!(s.view.sort, SortOrder::Newest);
    }

    #[test]
    fn sort_parts_with_grouped_true_is_kept() {
        let s = Settings::from_json_str(r#"{"view":{"sort":"parts","grouped":true}}"#).unwrap();
        assert_eq!(s.view.sort, SortOrder::Parts);
    }

    #[test]
    fn sort_downloaded_sanitises_to_newest_regardless_of_grouped() {
        let ungrouped =
            Settings::from_json_str(r#"{"view":{"sort":"downloaded","grouped":false}}"#).unwrap();
        assert_eq!(ungrouped.view.sort, SortOrder::Newest);
        let grouped =
            Settings::from_json_str(r#"{"view":{"sort":"downloaded","grouped":true}}"#).unwrap();
        assert_eq!(grouped.view.sort, SortOrder::Newest);
    }

    #[test]
    fn search_is_trimmed_and_capped_at_200_characters() {
        let s = Settings::from_json_str(r#"{"view":{"search":"  cats  "}}"#).unwrap();
        assert_eq!(s.view.search, "cats");

        let long = "a".repeat(500);
        let s2 =
            Settings::from_json_str(&format!(r#"{{"view":{{"search":"{long}"}}}}"#)).unwrap();
        assert_eq!(s2.view.search.len(), 200);
    }

    #[test]
    fn search_cap_counts_characters_not_bytes() {
        // "é" is two bytes in UTF-8. A byte-oriented cap (`String::truncate(200)`)
        // would keep only 100 of these; the intended behaviour keeps 200.
        let long = "é".repeat(500);
        let s = Settings::from_json_str(&format!(r#"{{"view":{{"search":"{long}"}}}}"#)).unwrap();
        assert_eq!(s.view.search.chars().count(), 200);
    }

    #[test]
    fn keep_view_of_takes_the_files_block_and_discards_the_incoming_one() {
        let mut incoming = Settings::default();
        incoming.view.search = "incoming".into();

        let mut disk = Settings::default();
        disk.view.search = "disk".into();

        incoming.keep_view_of(&disk);
        assert_eq!(incoming.view.search, "disk");
    }
}
