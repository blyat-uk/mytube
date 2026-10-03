//! What is installed on this machine that MyTube could use: video players and
//! browsers whose cookies yt-dlp can read.
//!
//! Every probe goes through [`Env`], and every list is built by a function that
//! takes the [`Os`] as a parameter, so the Windows and macOS lists are tested on
//! Linux against a fake filesystem. Paths are `String`s joined with the target
//! OS's separator for the same reason: a `PathBuf` built on Linux would put `/`
//! into a Windows path. Nothing here spawns a process.

use crate::config::{Settings, COOKIES_AUTO};
use crate::cookie_file::{self, YOUTUBE_LOGIN_COOKIES};
use crate::models::{BrowserOption, BrowserScan, PlayerOption};
use crate::ytdlp::Cookies;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    MacOs,
    Windows,
}

impl Os {
    /// The OS this binary was built for. Any other unix reads as Linux, the
    /// same fallback yt-dlp's `cookies.py` makes.
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::MacOs
        } else {
            Os::Linux
        }
    }

    fn sep(self) -> char {
        if self == Os::Windows { '\\' } else { '/' }
    }
}

/// The questions detection asks of the machine.
pub trait Env {
    fn is_file(&self, path: &str) -> bool;
    fn is_dir(&self, path: &str) -> bool;
    /// The names of `path`'s entries; empty when it is not a readable directory.
    fn list_dir(&self, path: &str) -> Vec<String>;
    /// `program` looked up on `PATH` (with `PATHEXT` on Windows).
    fn which(&self, program: &str) -> Option<String>;
    /// An environment variable; set-but-empty reads as unset.
    fn var(&self, name: &str) -> Option<String>;
    fn home(&self) -> Option<String>;
    /// When `path` was last written, in milliseconds since the epoch.
    fn modified(&self, path: &str) -> Option<u64>;
    /// Whether `path` is there but this process is refused it -- what macOS
    /// 27 answers for another browser's data (see [`blocked_note`]).
    fn denied(&self, path: &str) -> bool;
    /// Whether the cookie database at `db` holds a YouTube sign-in; `None`
    /// when it cannot be read. See [`youtube_login_in`].
    fn youtube_login(&self, db: &str, schema: CookieDb) -> Option<bool>;
}

/// The machine MyTube is running on.
pub struct RealEnv;

impl Env for RealEnv {
    fn is_file(&self, path: &str) -> bool {
        Path::new(path).is_file()
    }
    fn is_dir(&self, path: &str) -> bool {
        Path::new(path).is_dir()
    }
    fn list_dir(&self, path: &str) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }
    fn which(&self, program: &str) -> Option<String> {
        which::which(program).ok().map(|p| p.to_string_lossy().into_owned())
    }
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }
    fn home(&self) -> Option<String> {
        dirs::home_dir().map(|p| p.to_string_lossy().into_owned())
    }
    fn modified(&self, path: &str) -> Option<u64> {
        let t = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
    }
    fn denied(&self, path: &str) -> bool {
        // EPERM and EACCES both land here. macOS 27 lets the folder be
        // stat'ed and refuses the listing, so a listing is what is asked.
        matches!(std::fs::read_dir(path), Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied)
    }
    fn youtube_login(&self, db: &str, schema: CookieDb) -> Option<bool> {
        youtube_login_in(db, schema)
    }
}

/// `base` + `rel`, where `rel` is written with `/` and converted to the target
/// OS's separator.
fn join(os: Os, base: &str, rel: &str) -> String {
    let sep = os.sep();
    let base = base.trim_end_matches(['/', '\\']);
    let rel: String = rel.chars().map(|c| if c == '/' { sep } else { c }).collect();
    format!("{base}{sep}{rel}")
}

/// A path quoted so that `player::split_command` on `os` hands it back whole.
/// Windows paths cannot contain `"`, so plain double quotes always suffice
/// there; elsewhere shell-words does the quoting it will later undo.
fn quote_path(os: Os, path: &str) -> String {
    if os == Os::Windows {
        format!("\"{path}\"")
    } else {
        shell_words::quote(path).into_owned()
    }
}

fn player(id: &str, label: &str, command: String) -> PlayerOption {
    PlayerOption { id: id.into(), label: label.into(), command }
}

// ---------------------------------------------------------------- players

/// "System default" (`command: ""`) first, then installed players in this OS's
/// preference order.
pub fn detect_players() -> Vec<PlayerOption> {
    players_for(Os::current(), &RealEnv)
}

/// Linux players found on `PATH`, in preference order: (id, label, program).
/// The command is the bare program name, which is what a hand-typed setting
/// (the old `smplayer` default among them) already says, so an existing
/// setting lands on its detected entry in the Settings select.
const LINUX_PATH_PLAYERS: &[(&str, &str, &str)] = &[
    ("mpv", "mpv", "mpv"),
    ("smplayer", "SMPlayer", "smplayer"),
    ("vlc", "VLC", "vlc"),
    ("celluloid", "Celluloid", "celluloid"),
    ("haruna", "Haruna", "haruna"),
    ("totem", "Totem", "totem"),
    ("mplayer", "MPlayer", "mplayer"),
];

/// Flathub app ids, each exported as `<exports>/bin/<app id>`, a script that
/// runs `flatpak run <app id> "$@"`. Ids checked on flathub.org 2026-09-24.
const LINUX_FLATPAK_PLAYERS: &[(&str, &str, &str)] = &[
    ("flatpak-mpv", "mpv (Flatpak)", "io.mpv.Mpv"),
    ("flatpak-vlc", "VLC (Flatpak)", "org.videolan.VLC"),
    ("flatpak-smplayer", "SMPlayer (Flatpak)", "info.smplayer.SMPlayer"),
    ("flatpak-celluloid", "Celluloid (Flatpak)", "io.github.celluloid_player.Celluloid"),
    ("flatpak-haruna", "Haruna (Flatpak)", "org.kde.haruna"),
];

/// macOS app bundles, opened with `open -a <name>`, which hands the file to
/// the app through LaunchServices. QuickTime is not offered: downloads are
/// `.mkv`, which it does not play. `mpv.app` is the upstream / Homebrew-cask
/// bundle, distinct from the Homebrew formula's command-line `mpv`.
const MACOS_APPS: &[(&str, &str, &str)] = &[
    ("iina", "IINA", "IINA"),
    ("vlc", "VLC", "VLC"),
    ("mpv-app", "mpv", "mpv"),
];

/// Windows players under `%ProgramFiles%` / `%ProgramFiles(x86)%`: (id, label,
/// candidate paths relative to a Program Files root, best first). Sources,
/// checked 2026-09-24:
/// - VLC: the official installer's `VideoLAN\VLC\vlc.exe`.
/// - MPC-HC (clsid2 fork): `distrib/mpc-hc_setup.iss` installs to `{pf}\MPC-HC`
///   as `mpc-hc64.exe` (x64) / `mpc-hc.exe` (x86). K-Lite Codec Pack ships its
///   own copy under `K-Lite Codec Pack\MPC-HC64` (file.net, strontic).
/// - MPC-BE: `distrib/mpc-be_setup.iss` installs to `{pf}\MPC-BE` as
///   `mpc-be64.exe`; older x64 installers used `MPC-BE x64`, so both are tried.
/// - PotPlayer: `DAUM\PotPlayer\PotPlayerMini64.exe`, unchanged since Kakao
///   took it over (file.net, advanceduninstaller listings of 2025 builds).
/// - SMPlayer: `setup/smplayer.nsi` sets `InstallDir "$PROGRAMFILES64\SMPlayer"`.
const WINDOWS_PF_PLAYERS: &[(&str, &str, &[&str])] = &[
    ("vlc", "VLC", &[r"VideoLAN\VLC\vlc.exe"]),
    (
        "mpc-hc",
        "MPC-HC",
        &[
            r"MPC-HC\mpc-hc64.exe",
            r"MPC-HC\mpc-hc.exe",
            r"K-Lite Codec Pack\MPC-HC64\mpc-hc64.exe",
            r"K-Lite Codec Pack\MPC-HC\mpc-hc.exe",
        ],
    ),
    (
        "mpc-be",
        "MPC-BE",
        &[r"MPC-BE\mpc-be64.exe", r"MPC-BE x64\mpc-be64.exe", r"MPC-BE\mpc-be.exe"],
    ),
    (
        "potplayer",
        "PotPlayer",
        &[r"DAUM\PotPlayer\PotPlayerMini64.exe", r"DAUM\PotPlayer\PotPlayerMini.exe"],
    ),
    ("smplayer", "SMPlayer", &[r"SMPlayer\smplayer.exe"]),
];

pub(crate) fn players_for(os: Os, env: &dyn Env) -> Vec<PlayerOption> {
    let mut out = vec![player("system", "System default", String::new())];
    match os {
        Os::Linux => linux_players(env, &mut out),
        Os::MacOs => macos_players(env, &mut out),
        Os::Windows => windows_players(env, &mut out),
    }
    out
}

fn linux_players(env: &dyn Env, out: &mut Vec<PlayerOption>) {
    for (id, label, program) in LINUX_PATH_PLAYERS {
        if env.which(program).is_some() {
            out.push(player(id, label, (*program).into()));
        }
    }
    // The system installation, then the per-user one, which lives under
    // `$XDG_DATA_HOME/flatpak` (flatpak takes it from `g_get_user_data_dir`).
    let mut exports = vec!["/var/lib/flatpak/exports/bin".to_string()];
    let data_home = env
        .var("XDG_DATA_HOME")
        .or_else(|| env.home().map(|h| join(Os::Linux, &h, ".local/share")));
    if let Some(d) = data_home {
        exports.push(join(Os::Linux, &d, "flatpak/exports/bin"));
    }
    for (id, label, app_id) in LINUX_FLATPAK_PLAYERS {
        if let Some(p) = exports.iter().map(|e| join(Os::Linux, e, app_id)).find(|p| env.is_file(p)) {
            out.push(player(id, label, quote_path(Os::Linux, &p)));
        }
    }
}

/// Where `open -a` finds an app by name, for the bundles detection offers and
/// for `player::command_resolves`.
pub(crate) fn macos_app_dirs(env: &dyn Env) -> Vec<String> {
    let mut dirs = vec![
        "/Applications".to_string(),
        "/Applications/Utilities".into(),
        "/System/Applications".into(),
        "/System/Applications/Utilities".into(),
    ];
    if let Some(h) = env.home() {
        dirs.push(join(Os::MacOs, &h, "Applications"));
    }
    dirs
}

fn macos_players(env: &dyn Env, out: &mut Vec<PlayerOption>) {
    let dirs = macos_app_dirs(env);
    for (id, label, app) in MACOS_APPS {
        let bundle = format!("{app}.app");
        if dirs.iter().any(|d| env.is_dir(&join(Os::MacOs, d, &bundle))) {
            out.push(player(id, label, format!("open -a {}", shell_words::quote(app))));
        }
    }
    // A GUI launch does not inherit the shell's PATH, so Homebrew's prefixes
    // (Apple Silicon, then Intel) are looked in directly, and the command is
    // the absolute path for the same reason.
    let mpv = env.which("mpv").or_else(|| {
        ["/opt/homebrew/bin/mpv", "/usr/local/bin/mpv"]
            .into_iter()
            .find(|p| env.is_file(p))
            .map(String::from)
    });
    if let Some(p) = mpv {
        out.push(player("mpv", "mpv (command line)", quote_path(Os::MacOs, &p)));
    }
}

/// `%ProgramFiles%`, its 64-bit alias, and `%ProgramFiles(x86)%`, deduplicated.
fn program_files_roots(env: &dyn Env) -> Vec<String> {
    let mut roots: Vec<String> = Vec::new();
    for var in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(v) = env.var(var) {
            if !roots.iter().any(|r| r.eq_ignore_ascii_case(&v)) {
                roots.push(v);
            }
        }
    }
    roots
}

fn windows_players(env: &dyn Env, out: &mut Vec<PlayerOption>) {
    let roots = program_files_roots(env);
    let find = |rels: &[&str]| -> Option<String> {
        rels.iter()
            .flat_map(|rel| roots.iter().map(move |r| join(Os::Windows, r, rel)))
            .find(|p| env.is_file(p))
    };
    let push = |out: &mut Vec<PlayerOption>, id: &str, label: &str, path: String| {
        out.push(player(id, label, quote_path(Os::Windows, &path)));
    };

    let (vlc_id, vlc_label, vlc_rels) = WINDOWS_PF_PLAYERS[0];
    if let Some(p) = find(vlc_rels) {
        push(out, vlc_id, vlc_label, p);
    }
    // mpv has no installer. Scoop's manifest (Extras/bucket/mpv.json) puts the
    // real exe on PATH with `env_add_path "."` rather than a console shim, at
    // `<scoop root>\apps\mpv\current\mpv.exe`; the root is `%SCOOP%` when set.
    let scoop = env.var("SCOOP").or_else(|| env.home().map(|h| join(Os::Windows, &h, "scoop")));
    let mpv = scoop
        .map(|s| join(Os::Windows, &s, r"apps\mpv\current\mpv.exe"))
        .filter(|p| env.is_file(p))
        .or_else(|| find(&[r"mpv\mpv.exe"]))
        .or_else(|| env.which("mpv"));
    if let Some(p) = mpv {
        push(out, "mpv", "mpv", p);
    }
    for (id, label, rels) in &WINDOWS_PF_PLAYERS[1..] {
        if let Some(p) = find(rels) {
            push(out, id, label, p);
        }
    }
}

// --------------------------------------------------------------- browsers

/// Browsers whose profile data exists, in yt-dlp's own paths, and which of
/// them Automatic would use.
pub fn detect_browsers() -> BrowserScan {
    scan_browsers(Os::current(), &RealEnv)
}

/// Which of the two cookie-database schemas a browser keeps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CookieDb {
    /// `moz_cookies`, keyed by `host`.
    Firefox,
    /// `cookies`, keyed by `host_key`.
    Chromium,
}

/// Whether the cookie database at `db` holds a YouTube sign-in
/// ([`YOUTUBE_LOGIN_COOKIES`] on youtube.com); `None` when it cannot be read.
/// Only names and hosts are read -- both stored in the clear by every browser,
/// so nothing is decrypted and macOS asks for no Keychain access.
///
/// Opened `immutable=1`: SQLite then takes no lock and writes nothing beside
/// the file, so a running browser is never disturbed. That also leaves out
/// whatever still sits in the browser's `-wal`, which is what yt-dlp sees too:
/// it copies the database file alone before reading it.
fn youtube_login_in(db: &str, schema: CookieDb) -> Option<bool> {
    use rusqlite::{Connection, OpenFlags};
    let conn = Connection::open_with_flags(
        sqlite_uri(db),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let (table, host) = match schema {
        CookieDb::Firefox => ("moz_cookies", "host"),
        CookieDb::Chromium => ("cookies", "host_key"),
    };
    let names = vec!["?"; YOUTUBE_LOGIN_COOKIES.len()].join(", ");
    let sql = format!(
        "SELECT EXISTS (SELECT 1 FROM {table} \
         WHERE ({host} = 'youtube.com' OR {host} LIKE '%.youtube.com') AND name IN ({names}))"
    );
    conn.query_row(&sql, rusqlite::params_from_iter(YOUTUBE_LOGIN_COOKIES), |r| r.get(0)).ok()
}

/// `path` as an SQLite URI, which `immutable` needs: `file:///…`, with a
/// Windows drive path given its leading slash and every byte a URI could
/// misread percent-encoded (a profile named `Profile #1?` is legal).
fn sqlite_uri(path: &str) -> String {
    let mut p = path.replace('\\', "/");
    if !p.starts_with('/') {
        p.insert(0, '/');
    }
    let mut out = String::from("file://");
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out.push_str("?immutable=1");
    out
}

/// yt-dlp's `_config_home()`: `$XDG_CONFIG_HOME`, else `~/.config`. (yt-dlp
/// would take a set-but-empty value as a relative path; [`Env::var`] reads it
/// as unset, which is what the XDG spec says it means.)
fn config_home(env: &dyn Env) -> Option<String> {
    env.var("XDG_CONFIG_HOME").or_else(|| env.home().map(|h| join(Os::Linux, &h, ".config")))
}

/// yt-dlp's `_firefox_browser_dirs()`, verbatim per OS (cookies.py, master as
/// of 2026-09-24).
fn firefox_roots(os: Os, env: &dyn Env) -> Vec<String> {
    let home = env.home();
    let under_home = |rel: &str| home.as_ref().map(|h| join(os, h, rel));
    match os {
        Os::Windows => [
            env.var("APPDATA").map(|a| join(os, &a, r"Mozilla\Firefox\Profiles")),
            // The Microsoft Store (MSIX) build.
            env.var("LOCALAPPDATA").map(|l| {
                join(os, &l, r"Packages\Mozilla.Firefox_n80bbvh6b1yt2\LocalCache\Roaming\Mozilla\Firefox\Profiles")
            }),
        ]
        .into_iter()
        .flatten()
        .collect(),
        Os::MacOs => under_home("Library/Application Support/Firefox/Profiles").into_iter().collect(),
        Os::Linux => [
            // Firefox 147+ follows the XDG base directory spec on a new install.
            config_home(env).map(|c| join(os, &c, "mozilla/firefox")),
            // Installs from Firefox 146 and earlier.
            under_home(".mozilla/firefox"),
            // Flatpak, XDG-style and legacy.
            under_home(".var/app/org.mozilla.firefox/config/mozilla/firefox"),
            under_home(".var/app/org.mozilla.firefox/.mozilla/firefox"),
            // Snap ignores XDG.
            under_home("snap/firefox/common/.mozilla/firefox"),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

/// A cookie database, and when it was last written.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Db {
    path: String,
    modified: u64,
}

/// The most recently written of `candidates` that exist -- how yt-dlp picks
/// between profiles (`_newest`, `_find_most_recently_used_file`), so the
/// database read here is the one it will read.
fn newest(env: &dyn Env, candidates: Vec<String>) -> Option<Db> {
    candidates
        .into_iter()
        .filter(|p| env.is_file(p))
        .map(|p| Db { modified: env.modified(&p).unwrap_or(0), path: p })
        .fold(None, |best, d| match best {
            Some(b) if b.modified >= d.modified => Some(b),
            _ => Some(d),
        })
}

/// yt-dlp's `_firefox_cookie_dbs()`: `cookies.sqlite` at `<root>/`,
/// `<root>/*/` or `<root>/Profiles/*/`. A root that exists but holds no
/// cookie database is exactly what yt-dlp fails on ("could not find firefox
/// cookies database"), so the directory alone does not count. Python's glob
/// `*` skips dot-names, and so does this.
fn firefox_db(os: Os, env: &dyn Env) -> Option<Db> {
    let file = "cookies.sqlite";
    let mut candidates = Vec::new();
    for root in firefox_roots(os, env) {
        candidates.push(join(os, &root, file));
        for dir in [root.clone(), join(os, &root, "Profiles")] {
            for n in env.list_dir(&dir).into_iter().filter(|n| !n.starts_with('.')) {
                candidates.push(join(os, &join(os, &dir, &n), file));
            }
        }
    }
    newest(env, candidates)
}

/// The Chromium family: (`--cookies-from-browser` name, label).
const CHROMIUM_BROWSERS: &[(&str, &str)] = &[
    ("chrome", "Chrome"),
    ("chromium", "Chromium"),
    ("brave", "Brave"),
    ("edge", "Edge"),
    ("opera", "Opera"),
    ("vivaldi", "Vivaldi"),
    ("whale", "Whale"),
];

/// yt-dlp's `_get_chromium_based_browser_settings()` `browser_dir`, verbatim
/// per OS (cookies.py, master as of 2026-09-24).
fn chromium_dir(os: Os, env: &dyn Env, browser: &str) -> Option<String> {
    match os {
        Os::Windows => {
            let (var, rel) = match browser {
                "brave" => ("LOCALAPPDATA", r"BraveSoftware\Brave-Browser\User Data"),
                "chrome" => ("LOCALAPPDATA", r"Google\Chrome\User Data"),
                "chromium" => ("LOCALAPPDATA", r"Chromium\User Data"),
                "edge" => ("LOCALAPPDATA", r"Microsoft\Edge\User Data"),
                "opera" => ("APPDATA", r"Opera Software\Opera Stable"),
                "vivaldi" => ("LOCALAPPDATA", r"Vivaldi\User Data"),
                "whale" => ("LOCALAPPDATA", r"Naver\Naver Whale\User Data"),
                _ => return None,
            };
            env.var(var).map(|b| join(os, &b, rel))
        }
        Os::MacOs => {
            let rel = match browser {
                "brave" => "BraveSoftware/Brave-Browser",
                "chrome" => "Google/Chrome",
                "chromium" => "Chromium",
                "edge" => "Microsoft Edge",
                "opera" => "com.operasoftware.Opera",
                "vivaldi" => "Vivaldi",
                "whale" => "Naver/Whale",
                _ => return None,
            };
            env.home().map(|h| join(os, &join(os, &h, "Library/Application Support"), rel))
        }
        Os::Linux => {
            let rel = match browser {
                "brave" => "BraveSoftware/Brave-Browser",
                "chrome" => "google-chrome",
                "chromium" => "chromium",
                "edge" => "microsoft-edge",
                "opera" => "opera",
                "vivaldi" => "vivaldi",
                "whale" => "naver-whale",
                _ => return None,
            };
            config_home(env).map(|c| join(os, &c, rel))
        }
    }
}

/// The newest Chromium `Cookies` database where one lives: in the data dir
/// itself or its `Network/` (Opera keeps no profiles), or in a profile dir
/// (`Default`, `Profile 1`, …) or its `Network/`. yt-dlp walks the whole tree
/// for the newest `Cookies`; every real layout keeps it at one of these
/// depths, and a bounded look keeps detection from walking gigabytes of cache.
fn chromium_db(os: Os, env: &dyn Env, dir: &str) -> Option<Db> {
    let here = |d: &str| [join(os, d, "Cookies"), join(os, d, "Network/Cookies")];
    let mut candidates: Vec<String> = here(dir).into();
    for n in env.list_dir(dir) {
        candidates.extend(here(&join(os, dir, &n)));
    }
    newest(env, candidates)
}

/// yt-dlp's two Safari cookie files, which only Full Disk Access makes
/// visible -- to MyTube as much as to yt-dlp.
fn safari_db(env: &dyn Env) -> Option<Db> {
    let h = env.home()?;
    newest(env, vec![
        join(Os::MacOs, &h, "Library/Cookies/Cookies.binarycookies"),
        join(Os::MacOs, &h, "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"),
    ])
}

const NOTE_WINDOWS_CHROMIUM: &str = "yt-dlp cannot read this browser's cookies on Windows \
    (app-bound encryption). Export a cookies file from it instead.";
const NOTE_MACOS_CHROMIUM: &str = "macOS will ask to allow Keychain access.";

/// Why a browser whose data is there is marked "no access". macOS 27 keeps
/// every other app out of the Application Support folders of Firefox,
/// Chrome, Brave and Edge (among others) -- the open fails with EPERM even
/// though the folder can be seen -- and Safari's cookies have always needed
/// Full Disk Access; [`MACOS_ACCESS_HINT`] says what to do about it.
fn blocked_note(os: Os) -> &'static str {
    match os {
        Os::MacOs => "macOS is not letting MyTube read it.",
        _ => "MyTube is not allowed to read its profile folder.",
    }
}

/// Said under the cookies list on every Mac: a browser macOS hides from
/// MyTube may not show up at all, and the fix is in System Settings.
///
/// The per-app switch macOS 27 adds under Files & Folders (MyTube →
/// Firefox.app) is no answer: seen on 27.0.1 to be off again at MyTube's next
/// launch, which is Apple's stated design for consent to another app's data --
/// "transient, process lifetime", every new pid asks again (DTS,
/// developer.apple.com/forums/thread/742147). Full Disk Access is the grant
/// that persists -- until an update, since TCC identifies an ad-hoc signed
/// app by the hash of the very build.
const MACOS_ACCESS_HINT: &str = "Browser missing, or marked “no access”? macOS lets MyTube \
    read another app's data only with Full Disk Access: System Settings → Privacy & Security → \
    Full Disk Access → MyTube, then restart MyTube. After updating MyTube, switch it off and on \
    again. A cookies file needs no permission at all.";

/// One browser as this machine has it.
struct Found {
    id: &'static str,
    label: &'static str,
    /// `None` for Safari, whose binary cookie format is not looked into.
    schema: Option<CookieDb>,
    db: Option<Db>,
    /// Its data is there, but the OS refuses MyTube it.
    blocked: bool,
    /// Whether yt-dlp can read this browser on this OS at all.
    readable: bool,
    note: Option<&'static str>,
    /// Whether `db` holds a YouTube sign-in; looked up once, by [`scan`].
    signed_in: Option<bool>,
}

/// Every browser whose data is on this machine, readable or not.
fn scan(os: Os, env: &dyn Env) -> Vec<Found> {
    let mut out = Vec::new();
    let db = firefox_db(os, env);
    let blocked = db.is_none() && firefox_roots(os, env).iter().any(|r| env.denied(r));
    if db.is_some() || blocked {
        out.push(Found {
            id: "firefox", label: "Firefox", schema: Some(CookieDb::Firefox), db, blocked,
            readable: true, note: None, signed_in: None,
        });
    }
    if os == Os::MacOs {
        // Without Full Disk Access neither file can be seen, so Safari being
        // installed at all is what puts it in the list -- as "no access".
        let db = safari_db(env);
        if db.is_some() || env.is_dir("/Applications/Safari.app") {
            out.push(Found {
                id: "safari", label: "Safari", schema: None, blocked: db.is_none(), db,
                readable: true, note: None, signed_in: None,
            });
        }
    }
    for (id, label) in CHROMIUM_BROWSERS {
        let Some(dir) = chromium_dir(os, env, id) else { continue };
        let db = chromium_db(os, env, &dir);
        let blocked = db.is_none() && env.denied(&dir);
        if db.is_none() && !blocked {
            continue;
        }
        let (readable, note) = match os {
            Os::Windows => (false, Some(NOTE_WINDOWS_CHROMIUM)),
            Os::MacOs => (true, Some(NOTE_MACOS_CHROMIUM)),
            Os::Linux => (true, None),
        };
        out.push(Found {
            id, label, schema: Some(CookieDb::Chromium), db, blocked, readable, note, signed_in: None,
        });
    }
    for f in &mut out {
        f.signed_in = match (&f.db, f.schema) {
            (Some(db), Some(schema)) if f.usable() => env.youtube_login(&db.path, schema),
            _ => None,
        };
    }
    // Alphabetical: the list is a choice, not a ranking.
    out.sort_by_key(|f| f.label);
    out
}

impl Found {
    /// Whether choosing it can work: yt-dlp reads it here and MyTube can see it.
    fn usable(&self) -> bool {
        self.readable && !self.blocked && self.db.is_some()
    }
}

/// What Automatic resolves to: of the browsers that can be used, one signed
/// in to YouTube, and between several -- or none -- the one used last, whose
/// cookie database was written most recently. No browser is preferred over
/// another for what it is; the only question is where your YouTube session is.
fn automatic(found: &[Found]) -> Option<&Found> {
    found
        .iter()
        .filter(|f| f.usable())
        .map(|f| (f, (f.signed_in == Some(true), f.db.as_ref().map_or(0, |d| d.modified))))
        .fold(None, |best: Option<(&Found, (bool, u64))>, cur| match best {
            Some(b) if b.1 >= cur.1 => Some(b),
            _ => Some(cur),
        })
        .map(|(f, _)| f)
}

pub(crate) fn scan_browsers(os: Os, env: &dyn Env) -> BrowserScan {
    let found = scan(os, env);
    let browsers = found
        .iter()
        .map(|f| BrowserOption {
            id: f.id.into(),
            label: f.label.into(),
            supported: f.readable && !f.blocked,
            blocked: f.blocked,
            signed_in: f.signed_in,
            note: if f.blocked { Some(blocked_note(os)) } else { f.note }.map(String::from),
        })
        .collect();
    BrowserScan {
        browsers,
        automatic: automatic(&found).map(|f| f.id.to_string()),
        access_hint: (os == Os::MacOs).then(|| MACOS_ACCESS_HINT.to_string()),
    }
}

// ---------------------------------------------------------------- cookies

/// The cookie source for a yt-dlp run that sends cookies: the cookies file
/// when one is set -- as the copy [`cookie_file::prepare`] makes of it -- else
/// `cookies_browser`, where `auto` is [`automatic`]'s pick and `""` is none.
/// An `Err` is a cookies file that cannot be used, which fails the run
/// rather than letting it go out signed out without a word.
pub fn resolve_cookies(s: &Settings) -> anyhow::Result<Cookies> {
    resolve_cookies_in(s, Os::current(), &RealEnv, &cookie_file::store_dir())
}

/// `auto` asks for a browser that is really there and readable, because a
/// `--cookies-from-browser` naming one that is not fails every download
/// rather than degrading to no cookies.
pub(crate) fn resolve_cookies_in(s: &Settings, os: Os, env: &dyn Env, store: &Path) -> anyhow::Result<Cookies> {
    let file = s.cookies_file.trim();
    if !file.is_empty() {
        return cookie_file::prepare(Path::new(file), store).map(Cookies::File);
    }
    Ok(match s.cookies_browser.trim() {
        "" => Cookies::None,
        COOKIES_AUTO => match automatic(&scan(os, env)) {
            Some(f) => Cookies::Browser(f.id.into()),
            None => Cookies::None,
        },
        spec => Cookies::Browser(spec.into()),
    })
}

// ------------------------------------------------------------------ tests

/// A fake machine: a set of files (directories are implied by them, plus any
/// named explicitly), a PATH, environment variables and a home.
#[cfg(test)]
pub(crate) mod fake {
    use super::{CookieDb, Env, Os};
    use std::collections::{BTreeSet, HashMap};

    pub struct FakeEnv {
        pub os: Os,
        pub files: BTreeSet<String>,
        pub dirs: BTreeSet<String>,
        pub path: HashMap<String, String>,
        pub vars: HashMap<String, String>,
        pub home: Option<String>,
        /// Modification times; a file not in here reads as 0.
        pub mtimes: HashMap<String, u64>,
        /// Cookie databases holding a YouTube sign-in.
        pub logins: BTreeSet<String>,
        /// Directories this process is refused: everything at or under one
        /// is invisible, the way macOS 27 hides a browser's data.
        pub refused: BTreeSet<String>,
    }

    impl FakeEnv {
        pub fn new(os: Os, home: &str) -> Self {
            FakeEnv {
                os,
                files: BTreeSet::new(),
                dirs: BTreeSet::new(),
                path: HashMap::new(),
                vars: HashMap::new(),
                home: Some(home.into()),
                mtimes: HashMap::new(),
                logins: BTreeSet::new(),
                refused: BTreeSet::new(),
            }
        }
        /// A file last written at `t`.
        pub fn file_at(mut self, p: &str, t: u64) -> Self {
            self.mtimes.insert(p.into(), t);
            self.file(p)
        }
        /// A cookie database holding a YouTube sign-in.
        pub fn login(mut self, p: &str) -> Self {
            self.logins.insert(p.into());
            self.file(p)
        }
        /// A directory this process may not read.
        pub fn refuse(mut self, p: &str) -> Self {
            self.refused.insert(p.into());
            self
        }
        fn hidden(&self, p: &str) -> bool {
            self.refused.iter().any(|d| p == d || p.starts_with(&format!("{d}{}", self.sep())))
        }
        pub fn file(mut self, p: &str) -> Self {
            self.files.insert(p.into());
            self
        }
        pub fn dir(mut self, p: &str) -> Self {
            self.dirs.insert(p.into());
            self
        }
        pub fn on_path(mut self, program: &str, at: &str) -> Self {
            self.path.insert(program.into(), at.into());
            self
        }
        pub fn var(mut self, k: &str, v: &str) -> Self {
            self.vars.insert(k.into(), v.into());
            self
        }
        fn sep(&self) -> char {
            if self.os == Os::Windows { '\\' } else { '/' }
        }
        fn all_dirs(&self) -> BTreeSet<String> {
            let sep = self.sep();
            let mut out = self.dirs.clone();
            for p in self.files.iter().chain(self.dirs.iter()) {
                let mut cur = p.as_str();
                while let Some(i) = cur.rfind(sep) {
                    cur = &cur[..i];
                    if cur.is_empty() {
                        break;
                    }
                    out.insert(cur.to_string());
                }
            }
            out
        }
    }

    impl Env for FakeEnv {
        fn is_file(&self, path: &str) -> bool {
            self.files.contains(path) && !self.hidden(path)
        }
        fn is_dir(&self, path: &str) -> bool {
            self.all_dirs().contains(path)
        }
        fn list_dir(&self, path: &str) -> Vec<String> {
            if self.hidden(path) {
                return Vec::new();
            }
            let prefix = format!("{path}{}", self.sep());
            let mut names: BTreeSet<String> = BTreeSet::new();
            for p in self.files.iter().chain(self.all_dirs().iter()) {
                if let Some(rest) = p.strip_prefix(&prefix) {
                    if !rest.is_empty() && !rest.contains(self.sep()) {
                        names.insert(rest.to_string());
                    }
                }
            }
            names.into_iter().collect()
        }
        fn which(&self, program: &str) -> Option<String> {
            self.path.get(program).cloned()
        }
        fn var(&self, name: &str) -> Option<String> {
            self.vars.get(name).cloned().filter(|v| !v.is_empty())
        }
        fn home(&self) -> Option<String> {
            self.home.clone()
        }
        fn modified(&self, path: &str) -> Option<u64> {
            self.is_file(path).then(|| self.mtimes.get(path).copied().unwrap_or(0))
        }
        fn denied(&self, path: &str) -> bool {
            self.hidden(path) && (self.files.contains(path) || self.all_dirs().contains(path))
        }
        fn youtube_login(&self, db: &str, _schema: CookieDb) -> Option<bool> {
            self.is_file(db).then(|| self.logins.contains(db))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fake::FakeEnv;
    use super::*;

    fn ids(ps: &[PlayerOption]) -> Vec<&str> {
        ps.iter().map(|p| p.id.as_str()).collect()
    }
    fn cmd<'a>(ps: &'a [PlayerOption], id: &str) -> &'a str {
        &ps.iter().find(|p| p.id == id).unwrap_or_else(|| panic!("no {id}")).command
    }
    fn bids(bs: &[BrowserOption]) -> Vec<&str> {
        bs.iter().map(|b| b.id.as_str()).collect()
    }

    const H: &str = "/home/u";
    const WH: &str = r"C:\Users\u";

    fn win() -> FakeEnv {
        FakeEnv::new(Os::Windows, WH)
            .var("ProgramFiles", r"C:\Program Files")
            .var("ProgramW6432", r"C:\Program Files")
            .var("ProgramFiles(x86)", r"C:\Program Files (x86)")
            .var("APPDATA", r"C:\Users\u\AppData\Roaming")
            .var("LOCALAPPDATA", r"C:\Users\u\AppData\Local")
    }

    // -- players

    #[test]
    fn with_nothing_installed_every_os_offers_the_system_default_alone() {
        for os in [Os::Linux, Os::MacOs, Os::Windows] {
            let ps = players_for(os, &FakeEnv::new(os, H));
            assert_eq!(ps.len(), 1, "{os:?}");
            assert_eq!(ps[0].id, "system");
            assert_eq!(ps[0].label, "System default");
            assert_eq!(ps[0].command, "");
        }
    }

    #[test]
    fn linux_path_players_come_in_preference_order_as_bare_names() {
        let env = FakeEnv::new(Os::Linux, H)
            .on_path("mplayer", "/usr/bin/mplayer")
            .on_path("vlc", "/usr/bin/vlc")
            .on_path("smplayer", "/usr/bin/smplayer")
            .on_path("mpv", "/usr/bin/mpv")
            .on_path("totem", "/usr/bin/totem")
            .on_path("haruna", "/usr/bin/haruna")
            .on_path("celluloid", "/usr/bin/celluloid");
        let ps = players_for(Os::Linux, &env);
        assert_eq!(
            ids(&ps),
            ["system", "mpv", "smplayer", "vlc", "celluloid", "haruna", "totem", "mplayer"]
        );
        assert_eq!(cmd(&ps, "smplayer"), "smplayer");
        assert_eq!(ps.iter().find(|p| p.id == "vlc").unwrap().label, "VLC");
    }

    #[test]
    fn linux_flatpak_exports_follow_the_native_players_from_either_installation() {
        let env = FakeEnv::new(Os::Linux, H)
            .on_path("mpv", "/usr/bin/mpv")
            .file("/var/lib/flatpak/exports/bin/io.mpv.Mpv")
            .file("/home/u/.local/share/flatpak/exports/bin/org.videolan.VLC")
            .file("/home/u/.local/share/flatpak/exports/bin/info.smplayer.SMPlayer")
            .file("/var/lib/flatpak/exports/bin/io.github.celluloid_player.Celluloid")
            .file("/var/lib/flatpak/exports/bin/org.kde.haruna");
        let ps = players_for(Os::Linux, &env);
        assert_eq!(
            ids(&ps),
            ["system", "mpv", "flatpak-mpv", "flatpak-vlc", "flatpak-smplayer", "flatpak-celluloid", "flatpak-haruna"]
        );
        assert_eq!(cmd(&ps, "flatpak-mpv"), "/var/lib/flatpak/exports/bin/io.mpv.Mpv");
        assert_eq!(cmd(&ps, "flatpak-vlc"), "/home/u/.local/share/flatpak/exports/bin/org.videolan.VLC");
        assert_eq!(ps[2].label, "mpv (Flatpak)");
    }

    #[test]
    fn a_user_flatpak_installation_honours_xdg_data_home() {
        let env = FakeEnv::new(Os::Linux, H)
            .var("XDG_DATA_HOME", "/data/u")
            .file("/data/u/flatpak/exports/bin/io.mpv.Mpv");
        assert_eq!(cmd(&players_for(Os::Linux, &env), "flatpak-mpv"), "/data/u/flatpak/exports/bin/io.mpv.Mpv");
    }

    #[test]
    fn macos_offers_app_bundles_through_open_a_then_a_homebrew_mpv() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u")
            .dir("/Applications/VLC.app")
            .dir("/Users/u/Applications/IINA.app")
            .dir("/Applications/mpv.app")
            .dir("/Applications/QuickTime Player.app")
            .file("/opt/homebrew/bin/mpv");
        let ps = players_for(Os::MacOs, &env);
        assert_eq!(ids(&ps), ["system", "iina", "vlc", "mpv-app", "mpv"]);
        assert_eq!(cmd(&ps, "iina"), "open -a IINA");
        assert_eq!(cmd(&ps, "vlc"), "open -a VLC");
        assert_eq!(cmd(&ps, "mpv-app"), "open -a mpv");
        assert_eq!(cmd(&ps, "mpv"), "/opt/homebrew/bin/mpv");
    }

    #[test]
    fn macos_finds_an_intel_homebrew_mpv_and_prefers_one_on_path() {
        let intel = FakeEnv::new(Os::MacOs, "/Users/u").file("/usr/local/bin/mpv");
        assert_eq!(cmd(&players_for(Os::MacOs, &intel), "mpv"), "/usr/local/bin/mpv");
        let path = FakeEnv::new(Os::MacOs, "/Users/u")
            .file("/usr/local/bin/mpv")
            .on_path("mpv", "/opt/local/bin/mpv");
        assert_eq!(cmd(&players_for(Os::MacOs, &path), "mpv"), "/opt/local/bin/mpv");
    }

    #[test]
    fn windows_finds_players_under_either_program_files_root_as_quoted_paths() {
        let env = win()
            .file(r"C:\Program Files\VideoLAN\VLC\vlc.exe")
            .file(r"C:\Users\u\scoop\apps\mpv\current\mpv.exe")
            .file(r"C:\Program Files\MPC-HC\mpc-hc64.exe")
            .file(r"C:\Program Files\MPC-BE x64\mpc-be64.exe")
            .file(r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini.exe")
            .file(r"C:\Program Files\SMPlayer\smplayer.exe");
        let ps = players_for(Os::Windows, &env);
        assert_eq!(ids(&ps), ["system", "vlc", "mpv", "mpc-hc", "mpc-be", "potplayer", "smplayer"]);
        assert_eq!(cmd(&ps, "vlc"), r#""C:\Program Files\VideoLAN\VLC\vlc.exe""#);
        assert_eq!(cmd(&ps, "mpv"), r#""C:\Users\u\scoop\apps\mpv\current\mpv.exe""#);
        assert_eq!(cmd(&ps, "mpc-be"), r#""C:\Program Files\MPC-BE x64\mpc-be64.exe""#);
        assert_eq!(cmd(&ps, "potplayer"), r#""C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini.exe""#);
    }

    #[test]
    fn windows_prefers_the_64_bit_build_and_the_current_mpc_be_folder() {
        let env = win()
            .file(r"C:\Program Files (x86)\VideoLAN\VLC\vlc.exe")
            .file(r"C:\Program Files\MPC-BE\mpc-be64.exe")
            .file(r"C:\Program Files (x86)\MPC-BE\mpc-be.exe")
            .file(r"C:\Program Files (x86)\K-Lite Codec Pack\MPC-HC64\mpc-hc64.exe")
            .file(r"C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe")
            .file(r"C:\Program Files (x86)\DAUM\PotPlayer\PotPlayerMini.exe");
        let ps = players_for(Os::Windows, &env);
        assert_eq!(cmd(&ps, "vlc"), r#""C:\Program Files (x86)\VideoLAN\VLC\vlc.exe""#);
        assert_eq!(cmd(&ps, "mpc-be"), r#""C:\Program Files\MPC-BE\mpc-be64.exe""#);
        assert_eq!(cmd(&ps, "mpc-hc"), r#""C:\Program Files (x86)\K-Lite Codec Pack\MPC-HC64\mpc-hc64.exe""#);
        assert_eq!(cmd(&ps, "potplayer"), r#""C:\Program Files\DAUM\PotPlayer\PotPlayerMini64.exe""#);
    }

    #[test]
    fn windows_mpv_comes_from_scoop_root_program_files_or_path() {
        let scoop = win().var("SCOOP", r"D:\scoop").file(r"D:\scoop\apps\mpv\current\mpv.exe");
        assert_eq!(cmd(&players_for(Os::Windows, &scoop), "mpv"), r#""D:\scoop\apps\mpv\current\mpv.exe""#);
        let pf = win().file(r"C:\Program Files\mpv\mpv.exe");
        assert_eq!(cmd(&players_for(Os::Windows, &pf), "mpv"), r#""C:\Program Files\mpv\mpv.exe""#);
        let path = win().on_path("mpv", r"C:\tools\mpv\mpv.exe");
        assert_eq!(cmd(&players_for(Os::Windows, &path), "mpv"), r#""C:\tools\mpv\mpv.exe""#);
    }

    // -- browsers

    fn scanned(os: Os, env: &FakeEnv) -> BrowserScan {
        scan_browsers(os, env)
    }
    fn browser<'a>(scan: &'a BrowserScan, id: &str) -> &'a BrowserOption {
        scan.browsers.iter().find(|b| b.id == id).unwrap_or_else(|| panic!("no {id}"))
    }

    #[test]
    fn linux_firefox_is_found_in_every_directory_yt_dlp_reads() {
        for db in [
            "/home/u/.config/mozilla/firefox/abc.default-release/cookies.sqlite",
            "/home/u/.mozilla/firefox/abc.default/cookies.sqlite",
            "/home/u/.var/app/org.mozilla.firefox/config/mozilla/firefox/x/cookies.sqlite",
            "/home/u/.var/app/org.mozilla.firefox/.mozilla/firefox/x/cookies.sqlite",
            "/home/u/snap/firefox/common/.mozilla/firefox/x/cookies.sqlite",
            "/home/u/.mozilla/firefox/Profiles/x/cookies.sqlite",
            "/home/u/.mozilla/firefox/cookies.sqlite",
        ] {
            let env = FakeEnv::new(Os::Linux, H).file(db);
            assert_eq!(bids(&scanned(Os::Linux, &env).browsers), ["firefox"], "{db}");
        }
    }

    #[test]
    fn a_firefox_directory_without_a_cookie_database_does_not_count() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/.mozilla/firefox/profiles.ini")
            .file("/home/u/.mozilla/firefox/.hidden/cookies.sqlite")
            .file("/home/u/.mozilla/firefox/a/b/cookies.sqlite");
        assert!(scanned(Os::Linux, &env).browsers.is_empty());
    }

    #[test]
    fn linux_firefox_honours_xdg_config_home() {
        let env = FakeEnv::new(Os::Linux, H)
            .var("XDG_CONFIG_HOME", "/cfg")
            .file("/cfg/mozilla/firefox/p/cookies.sqlite");
        assert_eq!(bids(&scanned(Os::Linux, &env).browsers), ["firefox"]);
        let stale = FakeEnv::new(Os::Linux, H)
            .var("XDG_CONFIG_HOME", "/cfg")
            .file("/home/u/.config/mozilla/firefox/p/cookies.sqlite");
        assert!(scanned(Os::Linux, &stale).browsers.is_empty());
    }

    #[test]
    fn browsers_are_listed_alphabetically_not_by_preference() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/.mozilla/firefox/p/cookies.sqlite")
            .file("/home/u/.config/vivaldi/Default/Cookies")
            .file("/home/u/.config/BraveSoftware/Brave-Browser/Default/Cookies");
        assert_eq!(bids(&scanned(Os::Linux, &env).browsers), ["brave", "firefox", "vivaldi"]);
    }

    #[test]
    fn linux_chromium_family_is_supported_without_a_note() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/.config/google-chrome/Default/Network/Cookies")
            .file("/home/u/.config/BraveSoftware/Brave-Browser/Profile 1/Cookies")
            .file("/home/u/.config/opera/Cookies")
            // A data dir with no cookie database anywhere is not a browser.
            .file("/home/u/.config/chromium/Local State");
        let scan = scanned(Os::Linux, &env);
        assert_eq!(bids(&scan.browsers), ["brave", "chrome", "opera"]);
        assert!(scan.browsers.iter().all(|b| b.supported && !b.blocked && b.note.is_none()));
        assert_eq!(scan.access_hint, None);
    }

    #[test]
    fn windows_chromium_is_unsupported_and_its_note_names_no_other_browser() {
        let env = win()
            .file(r"C:\Users\u\AppData\Local\Packages\Mozilla.Firefox_n80bbvh6b1yt2\LocalCache\Roaming\Mozilla\Firefox\Profiles\p.default\cookies.sqlite")
            .file(r"C:\Users\u\AppData\Local\Google\Chrome\User Data\Default\Network\Cookies")
            .file(r"C:\Users\u\AppData\Local\Microsoft\Edge\User Data\Default\Network\Cookies")
            .file(r"C:\Users\u\AppData\Roaming\Opera Software\Opera Stable\Network\Cookies")
            .file(r"C:\Users\u\AppData\Local\Naver\Naver Whale\User Data\Default\Cookies");
        let scan = scanned(Os::Windows, &env);
        assert_eq!(bids(&scan.browsers), ["chrome", "edge", "firefox", "opera", "whale"]);
        let firefox = browser(&scan, "firefox");
        assert!(firefox.supported && firefox.note.is_none());
        for b in scan.browsers.iter().filter(|b| b.id != "firefox") {
            assert!(!b.supported && !b.blocked, "{}", b.id);
            assert_eq!(b.signed_in, None, "{}", b.id);
            let note = b.note.as_deref().unwrap();
            assert!(note.contains("cookies file") && !note.contains("Firefox"), "{note}");
        }
        assert_eq!(scan.access_hint, None);
    }

    #[test]
    fn windows_firefox_classic_profiles_dir() {
        let env = win().file(r"C:\Users\u\AppData\Roaming\Mozilla\Firefox\Profiles\x.default-release\cookies.sqlite");
        assert_eq!(bids(&scanned(Os::Windows, &env).browsers), ["firefox"]);
    }

    #[test]
    fn macos_lists_what_it_can_read_with_a_keychain_note_on_chromium() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u")
            .file("/Users/u/Library/Application Support/Firefox/Profiles/p/cookies.sqlite")
            .file("/Users/u/Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies")
            .file("/Users/u/Library/Application Support/Google/Chrome/Default/Cookies")
            .file("/Users/u/Library/Application Support/Microsoft Edge/Default/Cookies")
            .file("/Users/u/Library/Application Support/Naver/Whale/Default/Cookies");
        let scan = scanned(Os::MacOs, &env);
        assert_eq!(bids(&scan.browsers), ["chrome", "edge", "firefox", "safari", "whale"]);
        assert!(scan.browsers.iter().all(|b| b.supported && !b.blocked));
        assert!(browser(&scan, "chrome").note.as_deref().unwrap().contains("Keychain"));
        // Safari's cookies are visible, so Full Disk Access is already given.
        assert_eq!(browser(&scan, "safari").note, None);
        assert_eq!(browser(&scan, "safari").signed_in, None);
        assert!(scan.access_hint.as_deref().unwrap().contains("Full Disk Access"));
    }

    /// macOS 27 refuses every other app the Application Support folders of
    /// Firefox, Chrome, Brave and Edge: the folder can be seen, its listing
    /// fails with EPERM. That used to read as "no Firefox here".
    #[test]
    fn macos_27_shows_a_refused_browser_as_no_access_rather_than_missing() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u")
            .refuse("/Users/u/Library/Application Support/Firefox")
            .login("/Users/u/Library/Application Support/Firefox/Profiles/p/cookies.sqlite")
            .refuse("/Users/u/Library/Application Support/Google/Chrome")
            .file("/Users/u/Library/Application Support/Google/Chrome/Default/Cookies")
            .file("/Users/u/Library/Application Support/Vivaldi/Default/Cookies");
        let scan = scanned(Os::MacOs, &env);
        assert_eq!(bids(&scan.browsers), ["chrome", "firefox", "vivaldi"]);
        for id in ["chrome", "firefox"] {
            let b = browser(&scan, id);
            assert!(b.blocked && !b.supported, "{id}");
            assert_eq!(b.signed_in, None, "{id}");
            assert_eq!(b.note.as_deref(), Some(blocked_note(Os::MacOs)), "{id}");
        }
        assert!(browser(&scan, "vivaldi").supported);
        // Automatic never names a browser it cannot read.
        assert_eq!(scan.automatic.as_deref(), Some("vivaldi"));
        assert!(scan.access_hint.as_deref().unwrap().contains("System Settings"));
    }

    #[test]
    fn macos_lists_safari_as_no_access_when_its_cookies_are_hidden_but_the_app_is_there() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u").dir("/Applications/Safari.app");
        let scan = scanned(Os::MacOs, &env);
        assert_eq!(bids(&scan.browsers), ["safari"]);
        assert!(scan.browsers[0].blocked && !scan.browsers[0].supported);
        assert_eq!(scan.automatic, None);
    }

    #[test]
    fn safari_is_never_offered_off_macos() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/Library/Cookies/Cookies.binarycookies")
            .dir("/Applications/Safari.app");
        assert!(scanned(Os::Linux, &env).browsers.is_empty());
    }

    #[test]
    fn a_refused_profile_folder_elsewhere_gets_a_plain_note() {
        let env = FakeEnv::new(Os::Linux, H)
            .refuse("/home/u/.config/google-chrome")
            .file("/home/u/.config/google-chrome/Default/Cookies");
        let scan = scanned(Os::Linux, &env);
        let chrome = browser(&scan, "chrome");
        assert!(chrome.blocked && !chrome.supported);
        assert_eq!(chrome.note.as_deref(), Some(blocked_note(Os::Linux)));
        assert_eq!(scan.access_hint, None);
    }

    #[test]
    fn the_youtube_sign_in_is_read_from_the_profile_yt_dlp_would_use() {
        // yt-dlp reads the newest profile; a sign-in in an older one is not
        // what it will send.
        let env = FakeEnv::new(Os::Linux, H)
            .login("/home/u/.mozilla/firefox/old/cookies.sqlite")
            .file_at("/home/u/.mozilla/firefox/new/cookies.sqlite", 200)
            .login("/home/u/.config/BraveSoftware/Brave-Browser/Default/Cookies");
        let scan = scanned(Os::Linux, &env);
        assert_eq!(browser(&scan, "firefox").signed_in, Some(false));
        assert_eq!(browser(&scan, "brave").signed_in, Some(true));
    }

    #[test]
    fn automatic_follows_the_youtube_sign_in_whichever_browser_has_it() {
        // Firefox was used last, but only Brave is signed in.
        let env = FakeEnv::new(Os::Linux, H)
            .file_at("/home/u/.mozilla/firefox/p/cookies.sqlite", 900)
            .login("/home/u/.config/BraveSoftware/Brave-Browser/Default/Cookies")
            .file_at("/home/u/.config/google-chrome/Default/Cookies", 500);
        assert_eq!(scanned(Os::Linux, &env).automatic.as_deref(), Some("brave"));

        // Signed in to two: the one used last.
        let env = env
            .login("/home/u/.mozilla/firefox/p/cookies.sqlite");
        assert_eq!(scanned(Os::Linux, &env).automatic.as_deref(), Some("firefox"));
    }

    #[test]
    fn automatic_falls_back_to_the_browser_used_last() {
        let env = FakeEnv::new(Os::Linux, H)
            .file_at("/home/u/.mozilla/firefox/p/cookies.sqlite", 100)
            .file_at("/home/u/.config/google-chrome/Default/Cookies", 300)
            .file_at("/home/u/.config/chromium/Default/Cookies", 200);
        assert_eq!(scanned(Os::Linux, &env).automatic.as_deref(), Some("chrome"));
    }

    #[test]
    fn automatic_skips_a_browser_yt_dlp_cannot_read_here() {
        let env = win()
            .file_at(r"C:\Users\u\AppData\Roaming\Mozilla\Firefox\Profiles\p\cookies.sqlite", 100)
            .login(r"C:\Users\u\AppData\Local\Google\Chrome\User Data\Default\Network\Cookies");
        assert_eq!(scanned(Os::Windows, &env).automatic.as_deref(), Some("firefox"));
        let chrome_only = win().login(r"C:\Users\u\AppData\Local\Google\Chrome\User Data\Default\Network\Cookies");
        assert_eq!(scanned(Os::Windows, &chrome_only).automatic, None);
    }

    #[test]
    fn sqlite_uris_survive_any_path() {
        assert_eq!(sqlite_uri("/home/u/a b#c?d%e/cookies.sqlite"),
                   "file:///home/u/a%20b%23c%3Fd%25e/cookies.sqlite?immutable=1");
        assert_eq!(sqlite_uri(r"C:\Users\u\Profile 1\Cookies"),
                   "file:///C:/Users/u/Profile%201/Cookies?immutable=1");
    }

    /// The real reader, against real databases in both schemas, under a path
    /// that needs escaping -- and without leaving a journal or lock file
    /// beside a browser's database.
    #[test]
    fn youtube_login_reads_both_schemas_and_writes_nothing_beside_them() {
        let tmp = tempfile::tempdir().unwrap();
        let make = |dir: &str, schema: CookieDb, rows: &[(&str, &str)]| -> String {
            let d = tmp.path().join(dir);
            std::fs::create_dir_all(&d).unwrap();
            let p = d.join(if schema == CookieDb::Firefox { "cookies.sqlite" } else { "Cookies" });
            let conn = rusqlite::Connection::open(&p).unwrap();
            let (table, host) = match schema {
                CookieDb::Firefox => ("moz_cookies", "host"),
                CookieDb::Chromium => ("cookies", "host_key"),
            };
            conn.execute_batch(&format!("CREATE TABLE {table} ({host} TEXT, name TEXT, value TEXT)")).unwrap();
            for (h, n) in rows {
                conn.execute(&format!("INSERT INTO {table} VALUES (?1, ?2, 'secret')"), [h, n]).unwrap();
            }
            p.to_string_lossy().into_owned()
        };
        let ff = make("Profile #1 %20", CookieDb::Firefox, &[(".youtube.com", "LOGIN_INFO")]);
        let cr = make("chrome", CookieDb::Chromium, &[("www.youtube.com", "__Secure-3PSID")]);
        let out = make("signed-out", CookieDb::Firefox, &[(".youtube.com", "VISITOR_INFO1_LIVE"), (".google.com", "SID")]);
        let near = make("lookalike", CookieDb::Chromium, &[(".notyoutube.com", "SAPISID")]);
        assert_eq!(youtube_login_in(&ff, CookieDb::Firefox), Some(true));
        assert_eq!(youtube_login_in(&cr, CookieDb::Chromium), Some(true));
        assert_eq!(youtube_login_in(&out, CookieDb::Firefox), Some(false));
        assert_eq!(youtube_login_in(&near, CookieDb::Chromium), Some(false));
        // The wrong schema, or no database at all, is "cannot tell".
        assert_eq!(youtube_login_in(&ff, CookieDb::Chromium), None);
        let junk = tmp.path().join("junk");
        std::fs::write(&junk, "not a database").unwrap();
        assert_eq!(youtube_login_in(&junk.to_string_lossy(), CookieDb::Firefox), None);

        let beside: Vec<_> = std::fs::read_dir(tmp.path().join("Profile #1 %20")).unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(beside, ["cookies.sqlite"]);
    }

    // -- cookies

    fn settings(browser: &str, file: &str) -> Settings {
        Settings { cookies_browser: browser.into(), cookies_file: file.into(), ..Settings::default() }
    }

    fn with_firefox() -> FakeEnv {
        FakeEnv::new(Os::Linux, H).file("/home/u/.mozilla/firefox/p/cookies.sqlite")
    }

    fn resolve(s: &Settings, os: Os, env: &FakeEnv) -> Cookies {
        resolve_cookies_in(s, os, env, Path::new("/nonexistent/store")).unwrap()
    }

    #[test]
    fn a_cookies_file_wins_over_any_browser_as_a_copy_yt_dlp_can_read() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("cookies.json");
        std::fs::write(&src, r#"[{"name": "SID", "value": "a", "domain": ".youtube.com"}]"#).unwrap();
        let store = tmp.path().join("store");
        let s = settings("chrome", &format!(" {} ", src.display()));
        let Cookies::File(copy) = resolve_cookies_in(&s, Os::Linux, &with_firefox(), &store).unwrap() else {
            panic!("not a file");
        };
        assert_eq!(copy.parent(), Some(store.as_path()));
        assert!(std::fs::read_to_string(copy).unwrap().starts_with("# Netscape HTTP Cookie File\n"));
    }

    #[test]
    fn a_cookies_file_that_cannot_be_used_is_an_error_never_a_signed_out_run() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("cookies.txt");
        std::fs::write(&src, "remember the milk").unwrap();
        let s = settings(COOKIES_AUTO, &src.to_string_lossy());
        let e = resolve_cookies_in(&s, Os::Linux, &with_firefox(), &tmp.path().join("store")).unwrap_err();
        assert!(e.to_string().starts_with("cookies.txt can't be used as a cookies file"), "{e}");
    }

    #[test]
    fn auto_is_whichever_browser_automatic_picks() {
        let s = settings(COOKIES_AUTO, "");
        assert_eq!(resolve(&s, Os::Linux, &with_firefox()), Cookies::Browser("firefox".into()));
        let brave = FakeEnv::new(Os::Linux, H).file("/home/u/.config/BraveSoftware/Brave-Browser/Default/Cookies");
        assert_eq!(resolve(&s, Os::Linux, &brave), Cookies::Browser("brave".into()));
        // Nothing usable: no cookie flag at all, never a browser that is not there.
        assert_eq!(resolve(&s, Os::Linux, &FakeEnv::new(Os::Linux, H)), Cookies::None);
        assert_eq!(resolve(&s, Os::Windows, &win()), Cookies::None);
        let refused = FakeEnv::new(Os::MacOs, "/Users/u")
            .refuse("/Users/u/Library/Application Support/Firefox")
            .file("/Users/u/Library/Application Support/Firefox/Profiles/p/cookies.sqlite");
        assert_eq!(resolve(&s, Os::MacOs, &refused), Cookies::None);
    }

    #[test]
    fn empty_means_no_cookies_even_with_a_browser_installed() {
        assert_eq!(resolve(&settings("", ""), Os::Linux, &with_firefox()), Cookies::None);
        assert_eq!(resolve(&settings("  ", "  "), Os::Linux, &with_firefox()), Cookies::None);
    }

    #[test]
    fn any_other_browser_spec_is_passed_verbatim() {
        let s = settings("chrome:Profile 1", "");
        assert_eq!(resolve(&s, Os::Linux, &FakeEnv::new(Os::Linux, H)), Cookies::Browser("chrome:Profile 1".into()));
    }

    #[test]
    fn the_default_settings_resolve_through_auto() {
        let s = Settings::default();
        assert_eq!(s.cookies_browser, COOKIES_AUTO);
        assert_eq!(resolve(&s, Os::Linux, &with_firefox()), Cookies::Browser("firefox".into()));
    }

    /// What this machine has. Run with
    /// `cargo test detect::tests::show_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn show_this_machine() {
        for p in detect_players() {
            println!("player  {:<18} {:<22} {}", p.id, p.label, p.command);
        }
        let scan = detect_browsers();
        for b in &scan.browsers {
            println!(
                "browser {:<10} {:<10} supported={} blocked={} signed_in={:?} note={:?}",
                b.id, b.label, b.supported, b.blocked, b.signed_in, b.note
            );
        }
        println!("automatic {:?}", scan.automatic);
        println!("cookies {:?}", resolve_cookies(&Settings::default()));
    }
}
