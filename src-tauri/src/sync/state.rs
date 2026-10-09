//! `sync.json`: everything sync remembers, beside `settings.json` and never
//! inside it.
//!
//! Not in `settings.json` for two reasons. The code is a secret, and that file
//! is exported, hand-edited and pasted into bug reports. And the Settings view
//! saves the whole object from a snapshot taken when it mounted, so anything
//! written there behind its back -- every exchange updates `have` -- would be
//! reverted by the next save (the same reason `update-check.json` exists).
//!
//! The file's existence *is* the on/off switch: absent means sync is off, and
//! Turn off deletes it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config;

/// One other computer in the group, as last seen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Peer {
    /// Its hostname, read from inside its last snapshot -- the server never
    /// sees it.
    pub name: String,
    /// When the server last stored a slot from it, Unix seconds.
    #[serde(default)]
    pub written_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncState {
    /// The sync code as shown, `MYT1-…`. Everything else derives from it.
    pub code: String,
    /// This machine's slot in the group.
    pub machine_id: String,
    /// The server answered `no_group`: nothing wrote to the group for 90 days
    /// and it was deleted. Nothing is sent until sync is turned on again.
    #[serde(default)]
    pub expired: bool,
    /// The newest rev already read from each other machine's slot, so the
    /// server sends only what changed. Pruned to the machines still present.
    #[serde(default)]
    pub have: BTreeMap<String, u64>,
    /// Machine id → who it is, for the Settings list.
    #[serde(default)]
    pub peers: BTreeMap<String, Peer>,
    /// SHA-256 hex of the snapshot JSON last pushed. A snapshot hashing
    /// differently is a change waiting to go out.
    #[serde(default)]
    pub pushed_hash: Option<String>,
    #[serde(default)]
    pub last_sync_at: Option<i64>,
    /// What the last attempt went wrong with, in words for the status line.
    #[serde(default)]
    pub last_error: Option<String>,
}

impl SyncState {
    /// A fresh identity in a group: what Turn on and Join start from.
    pub fn new(code: String, machine_id: String) -> Self {
        Self {
            code,
            machine_id,
            expired: false,
            have: BTreeMap::new(),
            peers: BTreeMap::new(),
            pushed_hash: None,
            last_sync_at: None,
            last_error: None,
        }
    }
}

pub fn path() -> PathBuf {
    config::config_dir().join("sync.json")
}

/// `None` when sync is off. An unreadable file is an error rather than "off":
/// silently treating it as off would let the next Turn on write over a code
/// the user may still want to read out of it.
pub fn load_from(path: &Path) -> Result<Option<SyncState>> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e).with_context(|| format!("could not read {}", path.display())),
    };
    serde_json::from_str(&raw)
        .map(Some)
        .with_context(|| format!("{} is not readable", path.display()))
}

/// Writes the whole file atomically, readable by this user only.
///
/// Atomic because the clock rewrites it after every exchange, and a crash
/// half-way through must leave the old file rather than a truncated one -- that
/// would cost the code. 0600 because the code is the group: anyone who can read
/// it can read and write every synced library in it.
pub fn save_to(path: &Path, state: &SyncState) -> Result<()> {
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    // A name of its own per write: a leftover from a crash must not be
    // reopened, since `mode` applies only to a file being created and that
    // one's permissions are whatever they were.
    let tmp = dir.join(format!(
        ".{}.{}.{}.partial",
        path.file_name().unwrap_or_default().to_string_lossy(),
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let bytes = serde_json::to_vec_pretty(state)?;
    let result = (|| {
        let mut f = opts.open(&tmp)?;
        f.write_all(&bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}

/// Turn off. Already gone is fine: the result is the same.
pub fn remove_at(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e).with_context(|| format!("could not remove {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_state() -> SyncState {
        let mut s = SyncState::new("MYT1-00041-06105-0R3GG-28A1C-60T3GF".into(), "m".repeat(22));
        s.have.insert("other".into(), 7);
        s.peers.insert("other".into(), Peer { name: "laptop".into(), written_at: Some(1_700_000_000) });
        s.pushed_hash = Some("ab".repeat(32));
        s.last_sync_at = Some(1_700_000_100);
        s.last_error = Some("Can't reach the sync server. MyTube will keep trying.".into());
        s
    }

    #[test]
    fn absent_means_off() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_from(&dir.path().join("sync.json")).unwrap(), None);
    }

    #[test]
    fn a_saved_state_reads_back_whole() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested").join("sync.json");
        save_to(&p, &a_state()).unwrap();
        assert_eq!(load_from(&p).unwrap(), Some(a_state()));
    }

    #[test]
    fn a_minimal_file_fills_in_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sync.json");
        std::fs::write(&p, r#"{"code":"MYT1-X","machine_id":"m"}"#).unwrap();
        let s = load_from(&p).unwrap().unwrap();
        assert_eq!(s, SyncState::new("MYT1-X".into(), "m".into()));
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_off() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sync.json");
        std::fs::write(&p, "{\"code\":").unwrap();
        assert!(load_from(&p).is_err());
    }

    #[test]
    fn saving_replaces_the_file_and_leaves_no_partial_behind() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sync.json");
        save_to(&p, &SyncState::new("first".into(), "m".into())).unwrap();
        save_to(&p, &a_state()).unwrap();
        assert_eq!(load_from(&p).unwrap(), Some(a_state()));
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["sync.json".to_string()]);
    }

    #[test]
    fn a_failed_write_leaves_no_partial_behind() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sync.json");
        save_to(&p, &a_state()).unwrap();
        // A directory where the file should be renamed to: the write itself
        // succeeds, the rename cannot, and nothing else in the folder moves.
        let blocked = dir.path().join("blocked");
        std::fs::create_dir_all(blocked.join("child")).unwrap();
        assert!(save_to(&blocked, &a_state()).is_err());
        assert_eq!(load_from(&p).unwrap(), Some(a_state()));
        assert!(blocked.join("child").is_dir());
        let partials = std::fs::read_dir(dir.path())
            .unwrap()
            .filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().ends_with(".partial"))
            .count();
        assert_eq!(partials, 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_file_is_readable_by_this_user_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sync.json");
        // A pre-existing world-readable file must not lend its mode to the new one.
        std::fs::write(&p, "{}").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        save_to(&p, &a_state()).unwrap();
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{mode:o}");
    }

    #[test]
    fn removing_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sync.json");
        save_to(&p, &a_state()).unwrap();
        remove_at(&p).unwrap();
        assert_eq!(load_from(&p).unwrap(), None);
        remove_at(&p).unwrap();
    }
}
