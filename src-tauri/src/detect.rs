//! What is installed on this machine that MyTube could use: video players and
//! browsers whose cookies yt-dlp can read.
//!
//! Every probe goes through [`Env`], and every list is built by a function that
//! takes the [`Os`] as a parameter, so the Windows and macOS lists are tested on
//! Linux against a fake filesystem. Paths are `String`s joined with the target
//! OS's separator for the same reason: a `PathBuf` built on Linux would put `/`
//! into a Windows path. Nothing here spawns a process.

use crate::config::{Settings, COOKIES_AUTO};
use crate::models::{BrowserOption, PlayerOption};
use crate::ytdlp::Cookies;
use std::path::{Path, PathBuf};

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

/// Browsers whose profile data exists, in yt-dlp's own paths.
pub fn detect_browsers() -> Vec<BrowserOption> {
    browsers_for(Os::current(), &RealEnv)
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

/// yt-dlp's `_firefox_cookie_dbs()`: `cookies.sqlite` at `<root>/`,
/// `<root>/*/` or `<root>/Profiles/*/`. A root that exists but holds no
/// cookie database is exactly what yt-dlp fails on ("could not find firefox
/// cookies database"), so the directory alone does not count. Python's glob
/// `*` skips dot-names, and so does this.
fn firefox_has_cookies(os: Os, env: &dyn Env) -> bool {
    let db = "cookies.sqlite";
    let in_children = |dir: &str| {
        env.list_dir(dir)
            .iter()
            .filter(|n| !n.starts_with('.'))
            .any(|n| env.is_file(&join(os, &join(os, dir, n), db)))
    };
    firefox_roots(os, env).iter().any(|root| {
        env.is_file(&join(os, root, db)) || in_children(root) || in_children(&join(os, root, "Profiles"))
    })
}

/// The Chromium family in the order the Settings view lists them:
/// (`--cookies-from-browser` name, label).
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

/// Whether a Chromium `Cookies` database sits where one lives: in the data
/// dir itself or its `Network/` (Opera keeps no profiles), or in a profile
/// dir (`Default`, `Profile 1`, …) or its `Network/`. yt-dlp walks the whole
/// tree for the newest `Cookies`; every real layout keeps it at one of these
/// depths, and a bounded look keeps detection from walking gigabytes of cache.
fn chromium_has_cookies(os: Os, env: &dyn Env, dir: &str) -> bool {
    let here = |d: &str| env.is_file(&join(os, d, "Cookies")) || env.is_file(&join(os, d, "Network/Cookies"));
    here(dir) || env.list_dir(dir).iter().any(|n| here(&join(os, dir, n)))
}

/// yt-dlp's two Safari cookie files. Without Full Disk Access the sandboxed
/// container is invisible to MyTube as much as to yt-dlp, which is exactly the
/// case the note is for, so Safari is also listed when the app itself is there.
fn safari_present(env: &dyn Env) -> bool {
    let files = env.home().map(|h| {
        [
            join(Os::MacOs, &h, "Library/Cookies/Cookies.binarycookies"),
            join(Os::MacOs, &h, "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies"),
        ]
    });
    files.is_some_and(|fs| fs.iter().any(|f| env.is_file(f))) || env.is_dir("/Applications/Safari.app")
}

const NOTE_WINDOWS_CHROMIUM: &str = "yt-dlp cannot read this browser's cookies on Windows \
    (app-bound encryption). Use Firefox, or export a cookies.txt file.";
const NOTE_MACOS_CHROMIUM: &str = "macOS will ask to allow Keychain access.";
const NOTE_SAFARI: &str = "Needs Full Disk Access for MyTube \
    (System Settings → Privacy & Security → Full Disk Access).";

pub(crate) fn browsers_for(os: Os, env: &dyn Env) -> Vec<BrowserOption> {
    let mut out = Vec::new();
    if firefox_has_cookies(os, env) {
        out.push(BrowserOption { id: "firefox".into(), label: "Firefox".into(), supported: true, note: None });
    }
    if os == Os::MacOs && safari_present(env) {
        out.push(BrowserOption {
            id: "safari".into(),
            label: "Safari".into(),
            supported: true,
            note: Some(NOTE_SAFARI.into()),
        });
    }
    for (id, label) in CHROMIUM_BROWSERS {
        let Some(dir) = chromium_dir(os, env, id) else { continue };
        if !chromium_has_cookies(os, env, &dir) {
            continue;
        }
        let (supported, note) = match os {
            Os::Windows => (false, Some(NOTE_WINDOWS_CHROMIUM)),
            Os::MacOs => (true, Some(NOTE_MACOS_CHROMIUM)),
            Os::Linux => (true, None),
        };
        out.push(BrowserOption {
            id: (*id).into(),
            label: (*label).into(),
            supported,
            note: note.map(String::from),
        });
    }
    out
}

// ---------------------------------------------------------------- cookies

/// The cookie source a yt-dlp run should use for these settings:
/// `cookies_file` wins; `auto` is Firefox when a Firefox profile exists, else
/// none; `""` is none; anything else is a browser spec verbatim.
pub fn resolve_cookies(s: &Settings) -> Cookies {
    resolve_cookies_for(s, Os::current(), &RealEnv)
}

/// `auto` asks for exactly what yt-dlp will look for — a Firefox cookie
/// database — because `--cookies-from-browser firefox` on a machine without
/// one fails every download rather than degrading to no cookies.
pub(crate) fn resolve_cookies_for(s: &Settings, os: Os, env: &dyn Env) -> Cookies {
    let file = s.cookies_file.trim();
    if !file.is_empty() {
        return Cookies::File(PathBuf::from(file));
    }
    match s.cookies_browser.trim() {
        "" => Cookies::None,
        COOKIES_AUTO if firefox_has_cookies(os, env) => Cookies::Browser("firefox".into()),
        COOKIES_AUTO => Cookies::None,
        spec => Cookies::Browser(spec.into()),
    }
}

// ------------------------------------------------------------------ tests

/// A fake machine: a set of files (directories are implied by them, plus any
/// named explicitly), a PATH, environment variables and a home.
#[cfg(test)]
pub(crate) mod fake {
    use super::{Env, Os};
    use std::collections::{BTreeSet, HashMap};

    pub struct FakeEnv {
        pub os: Os,
        pub files: BTreeSet<String>,
        pub dirs: BTreeSet<String>,
        pub path: HashMap<String, String>,
        pub vars: HashMap<String, String>,
        pub home: Option<String>,
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
            }
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
            self.files.contains(path)
        }
        fn is_dir(&self, path: &str) -> bool {
            self.all_dirs().contains(path)
        }
        fn list_dir(&self, path: &str) -> Vec<String> {
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
            assert_eq!(bids(&browsers_for(Os::Linux, &env)), ["firefox"], "{db}");
        }
    }

    #[test]
    fn a_firefox_directory_without_a_cookie_database_does_not_count() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/.mozilla/firefox/profiles.ini")
            .file("/home/u/.mozilla/firefox/.hidden/cookies.sqlite")
            .file("/home/u/.mozilla/firefox/a/b/cookies.sqlite");
        assert!(browsers_for(Os::Linux, &env).is_empty());
    }

    #[test]
    fn linux_firefox_honours_xdg_config_home() {
        let env = FakeEnv::new(Os::Linux, H)
            .var("XDG_CONFIG_HOME", "/cfg")
            .file("/cfg/mozilla/firefox/p/cookies.sqlite");
        assert_eq!(bids(&browsers_for(Os::Linux, &env)), ["firefox"]);
        let stale = FakeEnv::new(Os::Linux, H)
            .var("XDG_CONFIG_HOME", "/cfg")
            .file("/home/u/.config/mozilla/firefox/p/cookies.sqlite");
        assert!(browsers_for(Os::Linux, &stale).is_empty());
    }

    #[test]
    fn linux_chromium_family_is_supported_without_a_note() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/.config/google-chrome/Default/Network/Cookies")
            .file("/home/u/.config/BraveSoftware/Brave-Browser/Profile 1/Cookies")
            .file("/home/u/.config/opera/Cookies")
            // A data dir with no cookie database anywhere is not a browser.
            .file("/home/u/.config/chromium/Local State");
        let bs = browsers_for(Os::Linux, &env);
        assert_eq!(bids(&bs), ["chrome", "brave", "opera"]);
        assert!(bs.iter().all(|b| b.supported && b.note.is_none()));
    }

    #[test]
    fn windows_chromium_is_listed_as_unsupported_and_firefox_includes_the_store_build() {
        let env = win()
            .file(r"C:\Users\u\AppData\Local\Packages\Mozilla.Firefox_n80bbvh6b1yt2\LocalCache\Roaming\Mozilla\Firefox\Profiles\p.default\cookies.sqlite")
            .file(r"C:\Users\u\AppData\Local\Google\Chrome\User Data\Default\Network\Cookies")
            .file(r"C:\Users\u\AppData\Local\Microsoft\Edge\User Data\Default\Network\Cookies")
            .file(r"C:\Users\u\AppData\Roaming\Opera Software\Opera Stable\Network\Cookies")
            .file(r"C:\Users\u\AppData\Local\Naver\Naver Whale\User Data\Default\Cookies");
        let bs = browsers_for(Os::Windows, &env);
        assert_eq!(bids(&bs), ["firefox", "chrome", "edge", "opera", "whale"]);
        assert!(bs[0].supported && bs[0].note.is_none());
        for b in &bs[1..] {
            assert!(!b.supported, "{}", b.id);
            assert!(b.note.as_deref().unwrap().contains("Firefox"), "{}", b.id);
        }
    }

    #[test]
    fn windows_firefox_classic_profiles_dir() {
        let env = win().file(r"C:\Users\u\AppData\Roaming\Mozilla\Firefox\Profiles\x.default-release\cookies.sqlite");
        assert_eq!(bids(&browsers_for(Os::Windows, &env)), ["firefox"]);
    }

    #[test]
    fn macos_lists_safari_with_a_disk_access_note_and_chromium_with_a_keychain_note() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u")
            .file("/Users/u/Library/Application Support/Firefox/Profiles/p/cookies.sqlite")
            .file("/Users/u/Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies")
            .file("/Users/u/Library/Application Support/Google/Chrome/Default/Cookies")
            .file("/Users/u/Library/Application Support/Microsoft Edge/Default/Cookies")
            .file("/Users/u/Library/Application Support/Naver/Whale/Default/Cookies");
        let bs = browsers_for(Os::MacOs, &env);
        assert_eq!(bids(&bs), ["firefox", "safari", "chrome", "edge", "whale"]);
        assert!(bs.iter().all(|b| b.supported));
        assert!(bs[1].note.as_deref().unwrap().contains("Full Disk Access"));
        assert!(bs[2].note.as_deref().unwrap().contains("Keychain"));
    }

    #[test]
    fn macos_lists_safari_when_its_cookies_are_hidden_but_the_app_is_there() {
        let env = FakeEnv::new(Os::MacOs, "/Users/u").dir("/Applications/Safari.app");
        assert_eq!(bids(&browsers_for(Os::MacOs, &env)), ["safari"]);
    }

    #[test]
    fn safari_is_never_offered_off_macos() {
        let env = FakeEnv::new(Os::Linux, H)
            .file("/home/u/Library/Cookies/Cookies.binarycookies")
            .dir("/Applications/Safari.app");
        assert!(browsers_for(Os::Linux, &env).is_empty());
    }

    // -- cookies

    fn settings(browser: &str, file: &str) -> Settings {
        Settings { cookies_browser: browser.into(), cookies_file: file.into(), ..Settings::default() }
    }

    fn with_firefox() -> FakeEnv {
        FakeEnv::new(Os::Linux, H).file("/home/u/.mozilla/firefox/p/cookies.sqlite")
    }

    #[test]
    fn a_cookies_file_wins_over_any_browser() {
        let s = settings("chrome", " /home/u/cookies.txt ");
        assert_eq!(
            resolve_cookies_for(&s, Os::Linux, &with_firefox()),
            Cookies::File("/home/u/cookies.txt".into())
        );
    }

    #[test]
    fn auto_is_firefox_only_where_a_firefox_cookie_database_exists() {
        let s = settings(COOKIES_AUTO, "");
        assert_eq!(resolve_cookies_for(&s, Os::Linux, &with_firefox()), Cookies::Browser("firefox".into()));
        // No Firefox: no cookie flag at all, never `--cookies-from-browser firefox`.
        assert_eq!(resolve_cookies_for(&s, Os::Linux, &FakeEnv::new(Os::Linux, H)), Cookies::None);
        assert_eq!(resolve_cookies_for(&s, Os::Windows, &win()), Cookies::None);
    }

    #[test]
    fn empty_means_no_cookies_even_with_firefox_installed() {
        assert_eq!(resolve_cookies_for(&settings("", ""), Os::Linux, &with_firefox()), Cookies::None);
        assert_eq!(resolve_cookies_for(&settings("  ", "  "), Os::Linux, &with_firefox()), Cookies::None);
    }

    #[test]
    fn any_other_browser_spec_is_passed_verbatim() {
        let s = settings("chrome:Profile 1", "");
        assert_eq!(
            resolve_cookies_for(&s, Os::Linux, &FakeEnv::new(Os::Linux, H)),
            Cookies::Browser("chrome:Profile 1".into())
        );
    }

    #[test]
    fn the_default_settings_resolve_through_auto() {
        let s = Settings::default();
        assert_eq!(s.cookies_browser, COOKIES_AUTO);
        assert_eq!(resolve_cookies_for(&s, Os::Linux, &with_firefox()), Cookies::Browser("firefox".into()));
    }

    /// What this machine has. Run with
    /// `cargo test detect::tests::show_this_machine -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn show_this_machine() {
        for p in detect_players() {
            println!("player  {:<18} {:<22} {}", p.id, p.label, p.command);
        }
        for b in detect_browsers() {
            println!("browser {:<10} {:<10} supported={} note={:?}", b.id, b.label, b.supported, b.note);
        }
        println!("cookies {:?}", resolve_cookies(&Settings::default()));
    }
}
