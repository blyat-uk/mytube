//! Finding a tool: a settings.json override, MyTube's managed copy, or one
//! already on the system -- in the order each tool wants them -- and asking it
//! its version.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::models::{ToolKind, ToolSource};
use crate::proc;

use super::sources::{self, Target};

/// A PyInstaller onefile build unpacks itself before it can print anything,
/// and on Windows the first run of a fresh exe also waits on Defender, so
/// this is generous. It only bounds a hung binary; a working one answers in
/// well under a second after the first run.
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// Where "the system" looks: `PATH`, then the directories a GUI launch
/// misses because the login shell never ran (Homebrew, `~/.local/bin`,
/// deno's own installer, WinGet and scoop shims).
#[derive(Debug, Clone, Default)]
pub struct Search {
    dirs: Vec<PathBuf>,
}

impl Search {
    pub fn system() -> Self {
        let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
            .map(|p| std::env::split_paths(&p).collect())
            .unwrap_or_default();
        let home = dirs::home_dir();
        let under_home = |rel: &str| home.as_ref().map(|h| h.join(rel));
        let extra: Vec<Option<PathBuf>> = match std::env::consts::OS {
            "macos" => vec![
                Some("/opt/homebrew/bin".into()),
                Some("/usr/local/bin".into()),
                under_home(".deno/bin"),
            ],
            "windows" => vec![
                dirs::data_local_dir().map(|d| d.join("Microsoft").join("WinGet").join("Links")),
                under_home("scoop/shims"),
                under_home(".deno/bin"),
            ],
            _ => vec![
                under_home(".local/bin"),
                under_home(".deno/bin"),
                Some("/usr/local/bin".into()),
            ],
        };
        for d in extra.into_iter().flatten() {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        }
        Search { dirs }
    }

    /// No system copies at all: the self-test, which must prove the managed
    /// sources, not whatever the CI image happens to ship.
    pub fn none() -> Self {
        Search { dirs: Vec::new() }
    }

    #[cfg(test)]
    pub fn only(dirs: Vec<PathBuf>) -> Self {
        Search { dirs }
    }

    /// Every executable called `name` in search order. `which` knows about
    /// the executable bit on unix and `PATHEXT` on Windows.
    fn candidates(&self, name: &str) -> Vec<PathBuf> {
        self.dirs
            .iter()
            .filter(|d| !d.as_os_str().is_empty())
            .filter_map(|d| which::which_in(name, Some(d), d).ok())
            .collect()
    }
}

/// A tool that resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub source: ToolSource,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Resolution {
    Found(Found),
    /// The settings.json override names a file that is not there. Never
    /// silently replaced by another copy: the person asked for that one.
    BadOverride(String),
    #[default]
    Missing,
}

impl Resolution {
    pub fn found(&self) -> Option<&Found> {
        match self {
            Resolution::Found(f) => Some(f),
            _ => None,
        }
    }
}

pub fn override_key(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Ytdlp => "ytdlp_path",
        ToolKind::Ffmpeg => "ffmpeg_path",
        ToolKind::Deno => "deno_path",
    }
}

/// The program `kind` is invoked as: the name looked up on PATH and the main
/// file a managed install puts in `bin/`.
pub fn program_name(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Ytdlp => "yt-dlp",
        ToolKind::Ffmpeg => "ffmpeg",
        ToolKind::Deno => "deno",
    }
}

/// The managed copy of `kind` in `bin`, if every file of it is there (ffmpeg
/// without its ffprobe is not an install: yt-dlp needs both).
pub fn managed(kind: ToolKind, bin: &Path, t: Target) -> Option<PathBuf> {
    let files = t.files(kind);
    if files.iter().all(|f| bin.join(f).is_file()) {
        Some(bin.join(&files[0]))
    } else {
        None
    }
}

/// Runs `cmd` to the end within `limit`, killing it if it overruns.
///
/// Built on `proc::output`, so an overrun kills the whole tree. The standalone
/// yt-dlp is a PyInstaller bootloader whose child is the real program, and a
/// `--version` that hangs -- a first run stuck behind Defender, a broken
/// unpack -- would otherwise leave that child running with nothing to reap
/// it: `kill_on_drop` reaches the bootloader alone.
///
/// Retries a spawn that fails with `ETXTBSY`. A binary written a moment ago
/// -- an install, or a test's fake script -- can be refused with "Text file
/// busy" when another thread forks in the window between that write and the
/// fork's own exec: the child briefly inherits the still-open write
/// descriptor. Seen as 2 failures in 40 runs of this module's tests before
/// this retry; the window is microseconds, so a few short waits clear it.
pub async fn output_of(
    cmd: &mut tokio::process::Command,
    limit: Duration,
) -> std::io::Result<std::process::Output> {
    let mut attempt = 0u64;
    loop {
        match tokio::time::timeout(limit, proc::output(cmd)).await {
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    format!("no answer within {}s", limit.as_secs()),
                ))
            }
            Ok(Err(e)) if text_file_busy(&e) && attempt < 8 => {
                attempt += 1;
                tokio::time::sleep(Duration::from_millis(25 * attempt)).await;
            }
            Ok(r) => return r,
        }
    }
}

#[cfg(unix)]
fn text_file_busy(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(libc::ETXTBSY)
}

#[cfg(not(unix))]
fn text_file_busy(_: &std::io::Error) -> bool {
    false
}

/// Runs `path`'s version flag and reads the answer. `None` if it would not
/// run, timed out, or said something unrecognisable.
pub async fn version_of(kind: ToolKind, path: &Path) -> Option<String> {
    let flag = match kind {
        ToolKind::Ffmpeg => "-version",
        _ => "--version",
    };
    let out = output_of(proc::command(path).arg(flag), VERSION_TIMEOUT).await.ok()?;
    if !out.status.success() {
        return None;
    }
    sources::parse_version(kind, &String::from_utf8_lossy(&out.stdout))
}

/// Resolution order, per the spec:
///
/// - override non-empty -> that path, or an error naming the key;
/// - yt-dlp: managed -> system (the managed copy is the one kept current);
/// - ffmpeg, deno: system -> managed (a distro ffmpeg is as good as ours and
///   costs no download). A system deno older than 2.3.0 counts as absent.
pub async fn resolve(
    kind: ToolKind,
    override_path: &str,
    bin: &Path,
    t: Target,
    search: &Search,
) -> Resolution {
    let override_path = override_path.trim();
    if !override_path.is_empty() {
        let p = PathBuf::from(override_path);
        if !p.is_file() {
            return Resolution::BadOverride(format!(
                "{} in settings.json is {}, which does not exist",
                override_key(kind),
                p.display()
            ));
        }
        let version = version_of(kind, &p).await;
        return Resolution::Found(Found { path: p, source: ToolSource::Override, version });
    }

    let managed_found = || async {
        let p = managed(kind, bin, t)?;
        let version = version_of(kind, &p).await;
        Some(Found { path: p, source: ToolSource::Managed, version })
    };

    let system_found = || async {
        for p in search.candidates(program_name(kind)) {
            // Our own bin/ on PATH is still the managed copy, not a system one.
            if p.parent() == Some(bin) {
                continue;
            }
            let version = version_of(kind, &p).await;
            if kind == ToolKind::Deno && !version.as_deref().is_some_and(sources::deno_new_enough) {
                continue;
            }
            return Some(Found { path: p, source: ToolSource::System, version });
        }
        None
    };

    let found = match kind {
        ToolKind::Ytdlp => match managed_found().await {
            Some(f) => Some(f),
            None => system_found().await,
        },
        ToolKind::Ffmpeg | ToolKind::Deno => match system_found().await {
            Some(f) => Some(f),
            None => managed_found().await,
        },
    };
    found.map(Resolution::Found).unwrap_or(Resolution::Missing)
}

/// Test helpers shared with `mod.rs`'s tests: fake tools as shell scripts.
#[cfg(all(test, unix))]
pub mod fake {
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};

    /// An executable at `dir/name` that prints `stdout` and exits 0.
    pub fn tool(dir: &Path, name: &str, stdout: &str) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\nprintf '%s\\n' '{stdout}'\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    /// Fakes of all three tools, answering their version flags.
    pub fn all(dir: &Path, tag: &str) {
        tool(dir, "yt-dlp", tag);
        tool(dir, "ffmpeg", &format!("ffmpeg version {tag} Copyright"));
        tool(dir, "ffprobe", &format!("ffprobe version {tag} Copyright"));
        tool(dir, "deno", "deno 2.9.7 (stable, release, x86_64-unknown-linux-gnu)");
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn host() -> Target {
        Target { os: "linux", arch: "x86_64" }
    }

    #[tokio::test]
    async fn ytdlp_prefers_override_then_managed_then_system() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let sys = tmp.path().join("sys");
        let own = tmp.path().join("own");
        fake::tool(&sys, "yt-dlp", "2020.01.01");
        let search = Search::only(vec![sys.clone()]);

        let r = resolve(ToolKind::Ytdlp, "", &bin, host(), &search).await;
        let f = r.found().unwrap();
        assert_eq!((f.source, f.path.clone(), f.version.as_deref()), (ToolSource::System, sys.join("yt-dlp"), Some("2020.01.01")));

        fake::tool(&bin, "yt-dlp", "2026.09.16.232951");
        let r = resolve(ToolKind::Ytdlp, "", &bin, host(), &search).await;
        let f = r.found().unwrap();
        assert_eq!((f.source, f.version.as_deref()), (ToolSource::Managed, Some("2026.09.16.232951")));

        let mine = fake::tool(&own, "my-ytdlp", "1999.1.1");
        let r = resolve(ToolKind::Ytdlp, mine.to_str().unwrap(), &bin, host(), &search).await;
        let f = r.found().unwrap();
        assert_eq!((f.source, f.path.clone()), (ToolSource::Override, mine));
    }

    #[tokio::test]
    async fn ffmpeg_and_deno_prefer_the_system_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let sys = tmp.path().join("sys");
        fake::all(&bin, "managed");
        let search = Search::only(vec![sys.clone()]);

        for kind in [ToolKind::Ffmpeg, ToolKind::Deno] {
            let r = resolve(kind, "", &bin, host(), &search).await;
            assert_eq!(r.found().unwrap().source, ToolSource::Managed, "{kind:?} with no system copy");
        }

        fake::tool(&sys, "ffmpeg", "ffmpeg version n9.0.1 Copyright");
        fake::tool(&sys, "deno", "deno 2.5.0 (stable)");
        let r = resolve(ToolKind::Ffmpeg, "", &bin, host(), &search).await;
        let f = r.found().unwrap();
        assert_eq!((f.source, f.version.as_deref()), (ToolSource::System, Some("n9.0.1")));
        let r = resolve(ToolKind::Deno, "", &bin, host(), &search).await;
        let f = r.found().unwrap();
        assert_eq!((f.source, f.version.as_deref()), (ToolSource::System, Some("2.5.0")));
    }

    #[tokio::test]
    async fn a_system_deno_older_than_2_3_counts_as_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("bin");
        let old = tmp.path().join("old");
        let newer = tmp.path().join("newer");
        fake::tool(&old, "deno", "deno 2.2.12 (stable, release, x86_64-unknown-linux-gnu)");

        let search = Search::only(vec![old.clone()]);
        assert_eq!(resolve(ToolKind::Deno, "", &bin, host(), &search).await, Resolution::Missing);

        // A good one later on the path is still found past the old one.
        fake::tool(&newer, "deno", "deno 2.3.0 (stable)");
        let search = Search::only(vec![old, newer.clone()]);
        let r = resolve(ToolKind::Deno, "", &bin, host(), &search).await;
        assert_eq!(r.found().unwrap().path, newer.join("deno"));

        // And with none good enough, the managed one wins.
        let search = Search::only(vec![tmp.path().join("old")]);
        fake::tool(&bin, "deno", "deno 2.9.7 (stable)");
        let r = resolve(ToolKind::Deno, "", &bin, host(), &search).await;
        assert_eq!(r.found().unwrap().source, ToolSource::Managed);
    }

    #[tokio::test]
    async fn a_missing_override_is_an_error_naming_the_key() {
        let tmp = tempfile::tempdir().unwrap();
        let sys = tmp.path().join("sys");
        fake::all(&sys, "x");
        let search = Search::only(vec![sys]);
        let r = resolve(ToolKind::Ffmpeg, "/nowhere/ffmpeg", tmp.path(), host(), &search).await;
        match r {
            Resolution::BadOverride(msg) => {
                assert!(msg.contains("ffmpeg_path"), "{msg}");
                assert!(msg.contains("/nowhere/ffmpeg"), "{msg}");
            }
            other => panic!("expected BadOverride, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_managed_ffmpeg_without_ffprobe_is_not_an_install() {
        let tmp = tempfile::tempdir().unwrap();
        fake::tool(tmp.path(), "ffmpeg", "ffmpeg version x Copyright");
        assert_eq!(managed(ToolKind::Ffmpeg, tmp.path(), host()), None);
        fake::tool(tmp.path(), "ffprobe", "ffprobe version x Copyright");
        assert_eq!(managed(ToolKind::Ffmpeg, tmp.path(), host()), Some(tmp.path().join("ffmpeg")));
    }

    #[tokio::test]
    async fn a_version_check_that_hangs_is_killed_with_everything_it_started() {
        // The shape of PyInstaller's bootloader: the process we spawn starts
        // the one that does the work. A timeout must take both.
        let secs = format!("308.{}", std::process::id());
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("yt-dlp");
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#"printf '%s\n' "$1" > "$2" && chmod 755 "$2""#)
            .arg("sh")
            .arg(format!("#!/bin/sh\nsleep {secs} & sleep {secs}"))
            .arg(&p)
            .status()
            .unwrap();
        assert!(ok.success());
        let alive = || {
            !std::process::Command::new("pgrep").arg("-f").arg(&secs).output().unwrap().stdout.is_empty()
        };
        let err = output_of(proc::command(&p).arg("--version"), Duration::from_millis(500))
            .await
            .expect_err("a 300 s sleep cannot answer in 500 ms");
        assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
        let mut gone = false;
        for _ in 0..80 {
            if !alive() {
                gone = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let _ = std::process::Command::new("pkill").arg("-f").arg(&secs).status();
        assert!(gone, "a child of the version check outlived its timeout");
    }

    #[tokio::test]
    async fn a_binary_that_will_not_run_has_no_version() {
        let tmp = tempfile::tempdir().unwrap();
        let p = fake::tool(tmp.path(), "yt-dlp", "x");
        std::fs::write(&p, "#!/bin/sh\nexit 3\n").unwrap();
        assert_eq!(version_of(ToolKind::Ytdlp, &p).await, None);
        assert_eq!(version_of(ToolKind::Ytdlp, &tmp.path().join("absent")).await, None);
    }
}
