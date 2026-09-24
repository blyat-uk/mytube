use crate::models::SortOrder;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The OS's own videos folder -- `~/Videos` on Linux and Windows, `~/Movies`
/// on macOS -- falling back to `~/Videos` where the desktop names none.
fn d_download_dir() -> String {
    dirs::video_dir()
        .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mytube")
        .to_string_lossy()
        .into_owned()
}
fn d_filename_template() -> String {
    "%(uploader)s/%(title)s [%(id)s].%(ext)s".into()
}
/// Empty means "whatever this OS opens the file with" -- the one default that
/// is right on every machine. The Settings view offers the players it detects.
fn d_player_command() -> String {
    String::new()
}
/// `auto` resolves at call time to Firefox when a Firefox profile exists and to
/// no cookies otherwise; see `detect::resolve_cookies`.
fn d_cookies_browser() -> String {
    COOKIES_AUTO.into()
}
fn d_ytdlp_channel() -> String {
    "nightly".into()
}
fn d_true() -> bool {
    true
}

/// The `cookies_browser` value that means "pick for me".
pub const COOKIES_AUTO: &str = "auto";
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
    /// `auto`, `""` for none, or a `--cookies-from-browser` spec verbatim
    /// (`firefox`, `chrome:Profile 1`). Machine-local: never exported.
    #[serde(default = "d_cookies_browser")]
    pub cookies_browser: String,
    /// A Netscape cookies.txt. Non-empty wins over `cookies_browser`.
    #[serde(default)]
    pub cookies_file: String,
    /// `nightly` or `stable`; anything else is repaired to nightly on load.
    #[serde(default = "d_ytdlp_channel")]
    pub ytdlp_channel: String,
    #[serde(default = "d_true")]
    pub ytdlp_auto_update: bool,
    /// Hand-edit-only overrides for the three external tools. Non-empty always
    /// wins over both the managed and the system copy.
    #[serde(default)]
    pub ytdlp_path: String,
    #[serde(default)]
    pub ffmpeg_path: String,
    #[serde(default)]
    pub deno_path: String,
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
        if v.ytdlp_channel != "stable" {
            v.ytdlp_channel = "nightly".into();
        }
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

    /// The same snapshot problem for the three tool overrides, which have no
    /// UI at all: they are set by editing settings.json by hand. A Settings
    /// view mounted before that edit still holds the old value, and saving
    /// anything from it -- a new download folder, the update channel -- would
    /// silently put the old path back. Nothing in the app ever changes these,
    /// so the file's copy always wins.
    pub fn keep_machine_overrides_of(&mut self, disk: &Settings) {
        self.ytdlp_path = disk.ytdlp_path.clone();
        self.ffmpeg_path = disk.ffmpeg_path.clone();
        self.deno_path = disk.deno_path.clone();
    }

    /// Takes the portable half of `incoming`, keeping this machine's window
    /// geometry and feed filters. The mirror of `keep_view_of`.
    ///
    /// The split is between settings that describe *the library* — where
    /// downloads land, how they are named, what plays them, how hard to poll,
    /// how big the cards are — and settings that describe *this screen*. An
    /// archive written on a 4K desktop must not reopen a laptop's window at
    /// 3840x2160, and the filters the feed happened to be wearing when the
    /// archive was made are nobody else's business.
    ///
    /// `extra` is merged rather than replaced. Those keys are hand-written or
    /// from a future version; dropping the ones already here to take an
    /// archive's would lose settings this build cannot even name, and taking
    /// none of the archive's would lose the same on the way in.
    ///
    /// The player is the one portable key that can name something this machine
    /// lacks: an archive written on Windows carries `"C:\Program Files\…\vlc.exe"`.
    /// It is taken only when it would start something here (see
    /// `player::command_resolves`); otherwise the local player stays.
    pub fn adopt_portable(&mut self, incoming: &Settings) {
        self.adopt_portable_with(incoming, crate::player::command_resolves)
    }

    fn adopt_portable_with(&mut self, incoming: &Settings, resolves: impl Fn(&str) -> bool) {
        self.download_dir = incoming.download_dir.clone();
        self.filename_template = incoming.filename_template.clone();
        if resolves(&incoming.player_command) {
            self.player_command = incoming.player_command.clone();
        }
        self.max_concurrent_downloads = incoming.max_concurrent_downloads;
        self.poll_interval_minutes = incoming.poll_interval_minutes;
        self.poll_on_startup = incoming.poll_on_startup;
        self.backfill_count = incoming.backfill_count;
        self.card_size = incoming.card_size;
        for (k, v) in &incoming.extra {
            self.extra.insert(k.clone(), v.clone());
        }
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
    load_from(&settings_path())
}

pub fn save(s: &Settings) -> Result<()> {
    ensure_dirs()?;
    save_to(&settings_path(), s)
}

/// `load` against an explicit file. Split out for `transfer`, whose tests point
/// at a temp directory: `config_dir()` is built from `$XDG_CONFIG_HOME`, which
/// is process-wide, so a parameter is the only way two of them can run at once.
pub fn load_from(path: &Path) -> Result<Settings> {
    if !path.exists() {
        return Ok(Settings::default());
    }
    let raw = std::fs::read_to_string(path)?;
    Settings::from_json_str(&raw)
}

/// `save` against an explicit file. Creates the parent directory, since the
/// caller may be writing somewhere `ensure_dirs` knows nothing about.
pub fn save_to(path: &Path, s: &Settings) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_atomic(path, s.to_json_string()?.as_bytes())
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
        assert_eq!(s.player_command, "", "empty = the OS default app");
        assert_eq!(s.cookies_browser, COOKIES_AUTO);
        assert_eq!(s.cookies_file, "");
        assert_eq!(s.ytdlp_channel, "nightly");
        assert!(s.ytdlp_auto_update);
        assert_eq!((s.ytdlp_path.as_str(), s.ffmpeg_path.as_str(), s.deno_path.as_str()), ("", "", ""));
        assert!(s.download_dir.ends_with("mytube"));
        assert!(s.poll_on_startup);
        assert_eq!(s.filename_template, "%(uploader)s/%(title)s [%(id)s].%(ext)s");
    }

    #[test]
    fn an_unknown_update_channel_reads_as_nightly() {
        let s = Settings::from_json_str(r#"{"ytdlp_channel":"master"}"#).unwrap();
        assert_eq!(s.ytdlp_channel, "nightly");
        let s = Settings::from_json_str(r#"{"ytdlp_channel":"stable"}"#).unwrap();
        assert_eq!(s.ytdlp_channel, "stable");
    }

    #[test]
    fn tool_and_cookie_keys_are_named_fields_not_extra() {
        let s = Settings::from_json_str(
            r#"{"cookies_browser":"chrome:Profile 1","cookies_file":"/c.txt","ytdlp_path":"/y","ytdlp_auto_update":false}"#,
        )
        .unwrap();
        assert_eq!(s.cookies_browser, "chrome:Profile 1");
        assert_eq!(s.cookies_file, "/c.txt");
        assert_eq!(s.ytdlp_path, "/y");
        assert!(!s.ytdlp_auto_update);
        assert!(s.extra.is_empty(), "{:?}", s.extra);
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

    #[test]
    fn hand_edited_tool_overrides_survive_a_save_from_a_stale_view() {
        // The view mounted with no overrides; since then, settings.json was
        // edited by hand to point at a yt-dlp and an ffmpeg of the user's own,
        // and a deno override was removed.
        let mut incoming = Settings::from_json_str(r#"{"deno_path":"/old/deno"}"#).unwrap();
        incoming.download_dir = "/new/videos".into();
        let disk = Settings::from_json_str(
            r#"{"ytdlp_path":"/opt/yt-dlp","ffmpeg_path":"C:\\ff\\ffmpeg-7.exe"}"#,
        )
        .unwrap();

        incoming.keep_machine_overrides_of(&disk);
        assert_eq!(incoming.ytdlp_path, "/opt/yt-dlp");
        assert_eq!(incoming.ffmpeg_path, r"C:\ff\ffmpeg-7.exe");
        assert_eq!(incoming.deno_path, "", "a removed override stays removed");
        assert_eq!(incoming.download_dir, "/new/videos", "what the view did change is kept");
    }

    /// A settings file as an archive from another machine would carry it: every
    /// portable field moved off its default, and every this-machine field moved
    /// too, so a test can tell "taken" from "happened to match".
    fn an_incoming_file() -> Settings {
        let mut s = Settings::from_json_str(
            r#"{
                "download_dir": "/mnt/OTHER/Videos",
                "filename_template": "%(title)s.%(ext)s",
                "player_command": "mpv --fs",
                "max_concurrent_downloads": 9,
                "poll_interval_minutes": 90,
                "poll_on_startup": false,
                "backfill_count": 120,
                "card_size": 400,
                "window_width": 3840,
                "window_height": 2160,
                "window_x": 1234,
                "window_y": 56,
                "window_maximized": true,
                "from_the_other_machine": "kept"
            }"#,
        )
        .unwrap();
        s.view.search = "incoming".into();
        s.view.grouped = true;
        s
    }

    #[test]
    fn adopt_portable_takes_everything_that_describes_the_library() {
        let mut local = Settings::default();
        local.adopt_portable_with(&an_incoming_file(), |_| true);
        assert_eq!(local.download_dir, "/mnt/OTHER/Videos");
        assert_eq!(local.filename_template, "%(title)s.%(ext)s");
        assert_eq!(local.player_command, "mpv --fs");
        assert_eq!(local.max_concurrent_downloads, 9);
        assert_eq!(local.poll_interval_minutes, 90);
        assert!(!local.poll_on_startup);
        assert_eq!(local.backfill_count, 120);
        assert_eq!(local.card_size, 400);
    }

    #[test]
    fn a_player_that_does_not_exist_here_is_not_adopted() {
        let mut local = Settings::from_json_str(r#"{"player_command":"smplayer"}"#).unwrap();
        local.adopt_portable_with(&an_incoming_file(), |cmd| cmd != "mpv --fs");
        assert_eq!(local.player_command, "smplayer", "the local player stays");
        assert_eq!(local.backfill_count, 120, "the rest of the archive still lands");
    }

    #[test]
    fn adopt_portable_keeps_this_machines_window_geometry() {
        let mut local = Settings::from_json_str(
            r#"{"window_width":1400,"window_height":900,"window_x":-40,"window_y":7}"#,
        )
        .unwrap();
        local.adopt_portable(&an_incoming_file());
        assert_eq!((local.window_width, local.window_height), (1400, 900));
        assert_eq!((local.window_x, local.window_y), (Some(-40), Some(7)));
        assert!(!local.window_maximized);
    }

    #[test]
    fn adopt_portable_keeps_this_machines_view_block() {
        let mut local = Settings::default();
        local.view.search = "local".into();
        local.view.hide_watched = true;
        local.adopt_portable(&an_incoming_file());
        assert_eq!(local.view.search, "local");
        assert!(local.view.hide_watched);
        assert!(!local.view.grouped, "the archive's grouping chip came along");
    }

    #[test]
    fn adopt_portable_merges_extra_keys_rather_than_replacing_them() {
        let mut local = Settings::from_json_str(r#"{"only_here":1,"on_both":"local"}"#).unwrap();
        let mut incoming = an_incoming_file();
        incoming.extra.insert("on_both".into(), serde_json::json!("incoming"));

        local.adopt_portable(&incoming);
        assert_eq!(local.extra.get("only_here"), Some(&serde_json::json!(1)));
        assert_eq!(
            local.extra.get("from_the_other_machine"),
            Some(&serde_json::json!("kept")),
            "an unknown key from the archive was dropped"
        );
        assert_eq!(local.extra.get("on_both"), Some(&serde_json::json!("incoming")));
    }

    #[test]
    fn load_from_and_save_to_round_trip_outside_the_config_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("settings.json");

        let mut s = Settings::default();
        s.player_command = "mpv".into();
        save_to(&path, &s).unwrap();

        assert_eq!(load_from(&path).unwrap().player_command, "mpv");
        // A file that is not there is the first-run case, not an error.
        assert_eq!(
            load_from(&dir.path().join("absent.json")).unwrap().player_command,
            ""
        );
    }
}
