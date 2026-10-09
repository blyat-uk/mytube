//! The plaintext of one machine's sync slot, and how several of them merge.
//!
//! snake_case on purpose: like the transfer manifest this is a file format
//! mirroring DB columns, and it never crosses the IPC boundary. Every list is
//! ordered by id so that the same library always serialises to the same bytes
//! -- the sync clock hashes it to tell whether anything changed.
//!
//! This file is the contract between `db.rs` (`Db::sync_snapshot`,
//! `Db::apply_sync`) and the rest of `sync/`.

use serde::{Deserialize, Serialize};

/// Bumped only for a change an older build would misread. A slot carrying a
/// newer format is skipped, and the status says another computer runs a newer
/// MyTube.
pub const SNAPSHOT_FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub format: u32,
    /// The sender's hostname, shown in the other machines' Settings.
    pub machine_name: String,
    pub channels: Vec<SyncChannel>,
    pub videos: Vec<SyncVideo>,
    pub tombstones: Vec<Tombstone>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncChannel {
    pub id: String,
    pub title: String,
    pub handle: Option<String>,
    pub url: String,
    pub subscribed: bool,
    pub subscribed_changed_at: Option<i64>,
    pub member: bool,
    pub member_changed_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncVideo {
    pub id: String,
    pub channel_id: String,
    pub title: String,
    pub published_at: Option<i64>,
    pub sort_at: Option<i64>,
    pub duration_secs: Option<i64>,
    pub added_manually: bool,
    /// The sender's `first_seen_at`. Compared against a video tombstone, and
    /// stored as `first_seen_at` when the row is inserted -- never the local
    /// clock, or a stale copy would out-date a removal.
    pub added_at: i64,
    pub watched: bool,
    pub watched_at: Option<i64>,
    pub watched_changed_at: Option<i64>,
    pub hidden: bool,
    pub hidden_changed_at: Option<i64>,
    pub sibling_group: Option<String>,
    pub sibling_changed_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TombstoneKind {
    Channel,
    Video,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tombstone {
    pub kind: TombstoneKind,
    pub id: String,
    pub removed_at: i64,
}

/// What `Db::apply_sync` did, for the caller's follow-up work.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ApplyReport {
    /// Subscribed channels newly inserted: the caller polls them.
    pub channels_added: Vec<String>,
    /// Videos newly inserted: the caller fetches their thumbnails.
    pub videos_added: Vec<String>,
    /// Whether any row was written at all. False means no refetch is needed,
    /// and applying the same snapshots twice must leave it false the second time.
    pub changed: bool,
}
