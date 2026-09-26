//! Telling the user a newer MyTube exists. Notify only: nothing here downloads
//! or installs anything, because how to update depends on how MyTube was
//! installed (deb, rpm, AppImage, NSIS, dmg), and the release page is the one
//! destination that is right for all of them.
//!
//! `tauri-plugin-updater` was the obvious alternative and was rejected: it
//! needs a signing key and a `latest.json` per release, and it cannot update a
//! deb or rpm install at all. A frontend fetch was the other, and would only
//! run while the window is open -- while the tray, which is what carries the
//! news between toasts, lives here in Rust anyway.
//!
//! One GitHub request a day at most ([`due`]), driven by an hourly loop in
//! `lib.rs`. What the check learned is kept in `update-check.json` beside
//! `settings.json` rather than inside it: the Settings view saves the whole
//! object from a snapshot taken when it mounted, so anything written into
//! `settings.json` behind its back would be reverted by the next save.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

use crate::config::{self, Settings};
use crate::tray;

/// `/latest` already leaves out drafts and prereleases, so whatever it returns
/// is something a user should be told about.
const LATEST_URL: &str = "https://api.github.com/repos/blyat-uk/mytube/releases/latest";

/// The frontend listens for this and shows one toast with a Download action.
pub const EVENT: &str = "app://update-available";

const CHECK_EVERY_SECS: i64 = 24 * 60 * 60;

/// A published release. Both field names are single words, so this is the
/// same on both sides of the IPC boundary and in `update-check.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// `0.1.2`, never `v0.1.2`: it is compared against `CARGO_PKG_VERSION`.
    pub version: String,
    /// The release's page on GitHub, not an asset: which download is right
    /// depends on the install, and the page lists them all.
    pub url: String,
}

/// What survives between checks. Missing or unreadable means "never checked",
/// which costs at most one extra request.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Bookkeeping {
    #[serde(default)]
    pub checked_at: Option<i64>,
    /// The newest release GitHub reported, newer than this build or not. Kept
    /// so the tray item comes back at startup without waiting on the network.
    #[serde(default)]
    pub latest: Option<Release>,
    /// The version a toast has already been sent for. One toast per release,
    /// ever; the tray item is the standing reminder.
    #[serde(default)]
    pub notified_version: String,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
}

/// Reads GitHub's `releases/latest` response.
pub fn parse_latest(json: &str) -> Result<Release> {
    let r: ApiRelease =
        serde_json::from_str(json).context("GitHub's release response is not readable")?;
    let version = strip_v(r.tag_name.trim()).to_string();
    if version.is_empty() {
        bail!("GitHub's latest release has an empty tag");
    }
    Ok(Release { version, url: r.html_url })
}

fn strip_v(v: &str) -> &str {
    v.strip_prefix(['v', 'V']).unwrap_or(v)
}

fn parse_version(v: &str) -> Option<[u64; 3]> {
    let mut parts = strip_v(v.trim()).split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        *slot = parts.next()?.parse().ok()?;
    }
    parts.next().is_none().then_some(out)
}

/// Whether `latest` is a strictly higher `major.minor.patch` than `current`.
///
/// Anything that is not three plain numbers is never newer: a tag someone typed
/// by hand must not put a permanent "update available" in the tray of a build
/// that cannot tell whether it is behind.
pub fn is_newer(latest: &str, current: &str) -> bool {
    match (parse_version(latest), parse_version(current)) {
        (Some(l), Some(c)) => l > c,
        _ => false,
    }
}

/// Never checked, or last checked a day or more ago.
pub fn due(now: i64, checked_at: Option<i64>) -> bool {
    checked_at.is_none_or(|t| now - t >= CHECK_EVERY_SECS)
}

/// The known release, if it is newer than `current`: what the tray shows.
fn newer_release<'a>(book: &'a Bookkeeping, current: &str) -> Option<&'a Release> {
    book.latest.as_ref().filter(|r| is_newer(&r.version, current))
}

/// The release a toast should announce now, if any: newer than this build and
/// not already announced.
pub fn to_notify<'a>(book: &'a Bookkeeping, current: &str) -> Option<&'a Release> {
    newer_release(book, current).filter(|r| r.version != book.notified_version)
}

pub fn bookkeeping_path() -> PathBuf {
    config::config_dir().join("update-check.json")
}

fn load_from(path: &Path) -> Bookkeeping {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save_to(path: &Path, book: &Bookkeeping) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(book)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub async fn fetch_latest(http: &reqwest::Client) -> Result<Release> {
    let resp = http
        .get(LATEST_URL)
        // GitHub refuses a request without one.
        .header(
            reqwest::header::USER_AGENT,
            concat!("mytube/", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await?;
    let status = resp.status();
    if !status.is_success() {
        bail!("GitHub answered {status}");
    }
    parse_latest(&resp.text().await?)
}

fn show_in_tray(book: &Bookkeeping, current: &str) {
    tray::set_update(newer_release(book, current).map(|r| (r.version.clone(), r.url.clone())));
}

/// One round of the check, run hourly. Does no network at all unless a day has
/// passed since the last successful check.
pub async fn tick(app: &AppHandle, http: &reqwest::Client, s: &Settings) {
    if !s.check_app_updates {
        // Off means no request and no tray item -- including one a previous
        // check left behind.
        tray::set_update(None);
        return;
    }
    let current = env!("CARGO_PKG_VERSION");
    let path = bookkeeping_path();
    let mut book = load_from(&path);
    // Straight from the file, so the item survives a restart with no network;
    // and it disappears by itself once MyTube has been updated past it.
    show_in_tray(&book, current);

    let now = chrono::Utc::now().timestamp();
    if due(now, book.checked_at) {
        match fetch_latest(http).await {
            Ok(release) => {
                book.latest = Some(release);
                book.checked_at = Some(now);
                show_in_tray(&book, current);
                if let Err(err) = save_to(&path, &book) {
                    eprintln!("mytube: could not record the update check: {err:#}");
                }
            }
            // Offline, rate-limited, GitHub down: a journal line and nothing
            // else. `checked_at` stays put, so the next hourly tick tries again.
            Err(err) => eprintln!("mytube: update check failed: {err:#}"),
        }
    }

    if let Some(release) = to_notify(&book, current).cloned() {
        let _ = app.emit(EVENT, &release);
        book.notified_version = release.version;
        if let Err(err) = save_to(&path, &book) {
            eprintln!("mytube: could not record the update notice: {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Trimmed from a real `releases/latest` response. The nested `author`
    /// carries an `html_url` of its own, which must not be the one picked.
    const FIXTURE: &str = r#"{
        "url": "https://api.github.com/repos/blyat-uk/mytube/releases/397269164",
        "html_url": "https://github.com/blyat-uk/mytube/releases/tag/v0.1.1",
        "id": 397269164,
        "author": {
            "login": "github-actions[bot]",
            "html_url": "https://github.com/apps/github-actions",
            "type": "Bot"
        },
        "tag_name": "v0.1.1",
        "target_commitish": "main",
        "name": "MyTube 0.1.1",
        "draft": false,
        "prerelease": false,
        "published_at": "2026-09-25T18:02:11Z",
        "assets": []
    }"#;

    fn release(v: &str) -> Release {
        Release { version: v.into(), url: format!("https://example.test/v{v}") }
    }

    #[test]
    fn parse_latest_strips_the_v_and_takes_the_release_page() {
        assert_eq!(
            parse_latest(FIXTURE).unwrap(),
            Release {
                version: "0.1.1".into(),
                url: "https://github.com/blyat-uk/mytube/releases/tag/v0.1.1".into(),
            }
        );
    }

    #[test]
    fn parse_latest_accepts_a_tag_without_a_v() {
        let json = r#"{"tag_name":"0.2.0","html_url":"https://x.test/r"}"#;
        assert_eq!(parse_latest(json).unwrap().version, "0.2.0");
    }

    #[test]
    fn parse_latest_rejects_garbage_and_missing_fields() {
        assert!(parse_latest("not json").is_err());
        assert!(parse_latest(r#"{"message":"Not Found"}"#).is_err());
        assert!(parse_latest(r#"{"tag_name":"v","html_url":"https://x.test"}"#).is_err());
    }

    #[test]
    fn is_newer_compares_numerically() {
        assert!(!is_newer("0.1.1", "0.1.1"), "equal");
        assert!(!is_newer("0.1.0", "0.1.1"), "older");
        assert!(is_newer("0.1.2", "0.1.1"), "patch");
        assert!(is_newer("0.2.0", "0.1.9"), "minor");
        assert!(is_newer("1.0.0", "0.9.9"), "major");
        assert!(is_newer("0.1.10", "0.1.9"), "not a string compare");
        assert!(!is_newer("0.1.9", "0.1.10"));
    }

    #[test]
    fn is_newer_ignores_a_v_prefix_on_either_side() {
        assert!(is_newer("v0.1.2", "0.1.1"));
        assert!(is_newer("0.1.2", "v0.1.1"));
        assert!(!is_newer("v0.1.1", "0.1.1"));
    }

    #[test]
    fn a_malformed_version_is_never_newer() {
        for bad in ["", "latest", "1.2", "1.2.3.4", "1.2.x", "1.2.3-beta", "-1.0.0"] {
            assert!(!is_newer(bad, "0.0.1"), "{bad:?} counted as newer");
        }
        assert!(!is_newer("9.9.9", "garbage"), "an unreadable current version");
    }

    #[test]
    fn due_once_a_day() {
        let now = 1_800_000_000;
        assert!(due(now, None), "never checked");
        assert!(!due(now, Some(now)));
        assert!(!due(now, Some(now - CHECK_EVERY_SECS + 1)));
        assert!(due(now, Some(now - CHECK_EVERY_SECS)));
        // A clock that went backwards waits rather than hammering GitHub.
        assert!(!due(now, Some(now + 3600)));
    }

    #[test]
    fn a_newer_release_is_announced_once() {
        let mut book = Bookkeeping { latest: Some(release("0.1.2")), ..Default::default() };
        assert_eq!(to_notify(&book, "0.1.1"), Some(&release("0.1.2")));
        book.notified_version = "0.1.2".into();
        assert_eq!(to_notify(&book, "0.1.1"), None, "the same release toasted twice");
        // ...but the tray keeps showing it.
        assert_eq!(newer_release(&book, "0.1.1"), Some(&release("0.1.2")));
    }

    #[test]
    fn a_later_release_is_announced_even_after_an_earlier_one_was() {
        let book = Bookkeeping {
            latest: Some(release("0.1.3")),
            notified_version: "0.1.2".into(),
            ..Default::default()
        };
        assert_eq!(to_notify(&book, "0.1.1"), Some(&release("0.1.3")));
    }

    #[test]
    fn nothing_is_announced_or_shown_once_this_build_has_caught_up() {
        let book = Bookkeeping { latest: Some(release("0.1.2")), ..Default::default() };
        assert_eq!(to_notify(&book, "0.1.2"), None);
        assert_eq!(newer_release(&book, "0.1.2"), None);
        assert_eq!(to_notify(&book, "0.2.0"), None);
        assert_eq!(to_notify(&Bookkeeping::default(), "0.1.1"), None);
    }

    #[test]
    fn bookkeeping_round_trips_and_tolerates_a_missing_or_corrupt_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("update-check.json");
        assert_eq!(load_from(&path), Bookkeeping::default(), "missing file");

        let book = Bookkeeping {
            checked_at: Some(1_800_000_000),
            latest: Some(release("0.1.2")),
            notified_version: "0.1.2".into(),
        };
        save_to(&path, &book).unwrap();
        assert_eq!(load_from(&path), book);

        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(load_from(&path), Bookkeeping::default(), "corrupt file");
    }

    #[tokio::test]
    #[ignore = "hits the live GitHub API"]
    async fn fetch_latest_reads_the_real_release() {
        let http = reqwest::Client::new();
        let r = fetch_latest(&http).await.unwrap();
        assert!(parse_version(&r.version).is_some(), "unparseable {:?}", r.version);
        assert!(r.url.starts_with("https://github.com/blyat-uk/mytube/releases/"), "{}", r.url);
    }
}
