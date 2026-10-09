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
use std::collections::BTreeMap;

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
    /// Channels newly subscribed here -- inserted, or an ad-hoc uploader's row
    /// another machine subscribed to: the caller polls them.
    pub channels_added: Vec<String>,
    /// Videos newly inserted: the caller fetches their thumbnails.
    pub videos_added: Vec<String>,
    /// Whether any row was written at all. False means no refetch is needed,
    /// and applying the same snapshots twice must leave it false the second time.
    pub changed: bool,
}

/// Combines several machines' snapshots into one, field by field.
///
/// Every `(value, stamp)` pair is settled by last writer wins: the higher
/// stamp takes it, and a NULL stamp -- a value nobody ever set -- loses to any
/// stamp at all. A tie on the stamp goes to `true` over `false`, and for a
/// sibling key to a key over none and then to the greater key. A row's other
/// fields -- title, url, dates -- come whole from the copy with the newest
/// stamp on the row. `added_manually` is OR-ed, `added_at` takes the latest,
/// and a tombstone keeps its latest removal.
///
/// Every choice is a maximum over a total order of the fields it decides, so
/// the result is the same whatever order the snapshots arrive in, merging in
/// stages is merging all at once, and a snapshot merged with itself is itself.
/// That is the property sync stands on: two machines that have seen the same
/// slots hold the same library, however their pulls interleaved.
///
/// The result belongs to no one machine, so its `machine_name` is empty.
pub fn merge<'a, I>(snapshots: I) -> Snapshot
where
    I: IntoIterator<Item = &'a Snapshot>,
{
    let mut channels: BTreeMap<&str, Vec<&SyncChannel>> = BTreeMap::new();
    let mut videos: BTreeMap<&str, Vec<&SyncVideo>> = BTreeMap::new();
    let mut tombstones: BTreeMap<(TombstoneKind, &str), i64> = BTreeMap::new();
    for s in snapshots {
        for c in &s.channels {
            channels.entry(c.id.as_str()).or_default().push(c);
        }
        for v in &s.videos {
            videos.entry(v.id.as_str()).or_default().push(v);
        }
        for t in &s.tombstones {
            let at = tombstones.entry((t.kind, t.id.as_str())).or_insert(t.removed_at);
            *at = (*at).max(t.removed_at);
        }
    }
    Snapshot {
        format: SNAPSHOT_FORMAT,
        machine_name: String::new(),
        channels: channels.into_values().map(|copies| merge_channel(&copies)).collect(),
        videos: videos.into_values().map(|copies| merge_video(&copies)).collect(),
        tombstones: tombstones
            .into_iter()
            .map(|((kind, id), removed_at)| Tombstone { kind, id: id.to_string(), removed_at })
            .collect(),
    }
}

/// The newest of a row's stamps, which picks the copy its other fields come from.
fn newest(stamps: &[Option<i64>]) -> Option<i64> {
    stamps.iter().copied().max().flatten()
}

/// The copy that wins under `key`. Each key spans every field it hands back,
/// so two copies that tie are identical in those fields and which of them
/// `max_by_key` returns cannot matter.
fn winner<'a, T, K: Ord>(copies: &[&'a T], key: impl Fn(&T) -> K) -> &'a T {
    copies.iter().copied().max_by_key(|c| key(c)).expect("a merged row has at least one copy")
}

/// One channel's copies, merged. Also how `Db::apply_sync` folds a local row
/// into an incoming one, so the database and the merge can never disagree
/// about who won.
pub(crate) fn merge_channel(copies: &[&SyncChannel]) -> SyncChannel {
    let meta = winner(copies, |c| {
        (newest(&[c.subscribed_changed_at, c.member_changed_at]),
         c.title.clone(), c.handle.clone(), c.url.clone())
    });
    let sub = winner(copies, |c| (c.subscribed_changed_at, c.subscribed));
    let mem = winner(copies, |c| (c.member_changed_at, c.member));
    SyncChannel {
        id: meta.id.clone(),
        title: meta.title.clone(),
        handle: meta.handle.clone(),
        url: meta.url.clone(),
        subscribed: sub.subscribed,
        subscribed_changed_at: sub.subscribed_changed_at,
        member: mem.member,
        member_changed_at: mem.member_changed_at,
    }
}

/// One video's copies, merged; see [`merge_channel`].
///
/// `watched_at` rides in the watched pair's key rather than beside it: it is
/// part of what a watch says, and two copies that agree on the stamp and the
/// flag but not the instant must still settle the same way in any order.
/// Without stamps -- a manual add nobody has touched since -- the later add
/// names the row.
pub(crate) fn merge_video(copies: &[&SyncVideo]) -> SyncVideo {
    let meta = winner(copies, |v| {
        (newest(&[v.watched_changed_at, v.hidden_changed_at, v.sibling_changed_at]),
         v.added_at, v.title.clone(), v.channel_id.clone(),
         v.published_at, v.sort_at, v.duration_secs)
    });
    let watched = winner(copies, |v| (v.watched_changed_at, v.watched, v.watched_at));
    let hidden = winner(copies, |v| (v.hidden_changed_at, v.hidden));
    let sibling = winner(copies, |v| (v.sibling_changed_at, v.sibling_group.clone()));
    SyncVideo {
        id: meta.id.clone(),
        channel_id: meta.channel_id.clone(),
        title: meta.title.clone(),
        published_at: meta.published_at,
        sort_at: meta.sort_at,
        duration_secs: meta.duration_secs,
        added_manually: copies.iter().any(|v| v.added_manually),
        added_at: copies.iter().map(|v| v.added_at).max().unwrap_or(meta.added_at),
        watched: watched.watched,
        watched_at: watched.watched_at,
        watched_changed_at: watched.watched_changed_at,
        hidden: hidden.hidden,
        hidden_changed_at: hidden.hidden_changed_at,
        sibling_group: sibling.sibling_group.clone(),
        sibling_changed_at: sibling.sibling_changed_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chan(id: &str) -> SyncChannel {
        SyncChannel { id: id.into(), title: format!("T-{id}"), handle: None,
                      url: format!("https://www.youtube.com/channel/{id}"),
                      subscribed: true, subscribed_changed_at: Some(100),
                      member: false, member_changed_at: None }
    }

    fn vid(id: &str) -> SyncVideo {
        SyncVideo { id: id.into(), channel_id: "UC1".into(), title: format!("t-{id}"),
                    published_at: Some(50), sort_at: Some(50), duration_secs: Some(600),
                    added_manually: false, added_at: 60,
                    watched: false, watched_at: None, watched_changed_at: None,
                    hidden: false, hidden_changed_at: None,
                    sibling_group: None, sibling_changed_at: None }
    }

    fn snap(name: &str, channels: Vec<SyncChannel>, videos: Vec<SyncVideo>,
            tombstones: Vec<Tombstone>) -> Snapshot {
        Snapshot { format: SNAPSHOT_FORMAT, machine_name: name.into(),
                   channels, videos, tombstones }
    }

    fn videos(v: Vec<SyncVideo>) -> Snapshot { snap("m", vec![], v, vec![]) }

    fn channels(c: Vec<SyncChannel>) -> Snapshot { snap("m", c, vec![], vec![]) }

    fn tomb(kind: TombstoneKind, id: &str, removed_at: i64) -> Tombstone {
        Tombstone { kind, id: id.into(), removed_at }
    }

    /// The one video both snapshots carry, merged.
    fn one(a: SyncVideo, b: SyncVideo) -> SyncVideo {
        let m = merge(&[videos(vec![a]), videos(vec![b])]);
        assert_eq!(m.videos.len(), 1);
        m.videos.into_iter().next().unwrap()
    }

    #[test]
    fn the_later_stamp_wins_even_when_it_clears_the_flag() {
        let watched = SyncVideo { watched: true, watched_at: Some(10), watched_changed_at: Some(10),
                                  ..vid("a") };
        let unwatched = SyncVideo { watched_changed_at: Some(20), ..vid("a") };
        let m = one(watched.clone(), unwatched.clone());
        assert!(!m.watched, "an un-watch is a change like any other");
        assert_eq!((m.watched_at, m.watched_changed_at), (None, Some(20)));
        assert_eq!(one(unwatched, watched), m, "whichever way round");
    }

    #[test]
    fn a_missing_stamp_loses_to_any_stamp() {
        // A NULL stamp is a value nobody ever set, however it reads.
        let unstamped = SyncVideo { hidden: true, ..vid("a") };
        let stamped = SyncVideo { hidden_changed_at: Some(1), ..vid("a") };
        let m = one(unstamped, stamped);
        assert!(!m.hidden);
        assert_eq!(m.hidden_changed_at, Some(1));
    }

    #[test]
    fn on_equal_stamps_true_beats_false() {
        let on = SyncVideo { watched: true, watched_at: Some(5), watched_changed_at: Some(5),
                             hidden: true, hidden_changed_at: Some(7), ..vid("a") };
        let off = SyncVideo { watched_changed_at: Some(5), hidden_changed_at: Some(7), ..vid("a") };
        let m = one(off.clone(), on.clone());
        assert!(m.watched && m.hidden);
        assert_eq!(one(on, off), m);

        let joined = SyncChannel { member: true, member_changed_at: Some(9), ..chan("UC1") };
        let left = SyncChannel { subscribed: false, member_changed_at: Some(9), ..chan("UC1") };
        for [x, y] in [[joined.clone(), left.clone()], [left, joined]] {
            let m = merge(&[channels(vec![x]), channels(vec![y])]);
            let c = &m.channels[0];
            assert!(c.subscribed && c.member, "{c:?}");
        }
    }

    #[test]
    fn watched_at_travels_with_the_watched_pair() {
        // The same stamp and the same flag, but two different first-watch
        // instants: the pair is settled as a whole, never field by field, and
        // the same way whichever snapshot comes first.
        let early = SyncVideo { watched: true, watched_at: Some(3), watched_changed_at: Some(9),
                                ..vid("a") };
        let late = SyncVideo { watched_at: Some(4), ..early.clone() };
        assert_eq!(one(early.clone(), late.clone()).watched_at, Some(4));
        assert_eq!(one(late, early.clone()).watched_at, Some(4));

        let newer_unwatch = SyncVideo { watched_changed_at: Some(10), ..vid("a") };
        assert_eq!(one(early, newer_unwatch).watched_at, None, "not left behind by an un-watch");
    }

    #[test]
    fn a_sibling_key_beats_none_and_the_greater_key_wins_a_tie() {
        let none = SyncVideo { sibling_changed_at: Some(5), ..vid("a") };
        let k1 = SyncVideo { sibling_group: Some("1-a".into()), sibling_changed_at: Some(5),
                             ..vid("a") };
        let k2 = SyncVideo { sibling_group: Some("2-b".into()), ..k1.clone() };
        assert_eq!(one(none, k1.clone()).sibling_group.as_deref(), Some("1-a"));
        assert_eq!(one(k2.clone(), k1.clone()).sibling_group.as_deref(), Some("2-b"));
        assert_eq!(one(k1.clone(), k2).sibling_group.as_deref(), Some("2-b"));

        let unlinked_later = SyncVideo { sibling_changed_at: Some(6), ..vid("a") };
        assert_eq!(one(k1, unlinked_later).sibling_group, None, "an unlink is a change too");
    }

    #[test]
    fn added_manually_is_ored_and_added_at_is_the_latest() {
        let manual = SyncVideo { added_manually: true, added_at: 10, ..vid("a") };
        let polled = SyncVideo { added_at: 30, ..vid("a") };
        let m = one(manual, polled);
        assert!(m.added_manually);
        assert_eq!(m.added_at, 30);
    }

    #[test]
    fn title_and_dates_come_from_the_copy_with_the_newest_stamp() {
        let stale = SyncVideo { title: "old name".into(), published_at: Some(1), sort_at: Some(1),
                                duration_secs: Some(1), watched_changed_at: Some(10),
                                ..vid("a") };
        let fresh = SyncVideo { title: "new name".into(), published_at: Some(2), sort_at: Some(3),
                                duration_secs: Some(4), hidden_changed_at: Some(20),
                                ..vid("a") };
        let m = one(stale.clone(), fresh.clone());
        assert_eq!((m.title.as_str(), m.published_at, m.sort_at, m.duration_secs),
                   ("new name", Some(2), Some(3), Some(4)));
        assert_eq!(one(fresh, stale), m);

        let old = SyncChannel { title: "Old".into(), subscribed_changed_at: Some(1), ..chan("UC1") };
        let new = SyncChannel { title: "New".into(), url: "u2".into(), handle: Some("@n".into()),
                                subscribed_changed_at: None,
                                member: true, member_changed_at: Some(2), ..chan("UC1") };
        let m = merge(&[channels(vec![old]), channels(vec![new])]);
        let c = &m.channels[0];
        assert_eq!((c.title.as_str(), c.url.as_str(), c.handle.as_deref()), ("New", "u2", Some("@n")));
        assert_eq!(c.subscribed_changed_at, Some(1), "the pair still settles on its own");
    }

    #[test]
    fn tombstones_keep_the_latest_removal_per_kind_and_id() {
        let a = snap("a", vec![], vec![], vec![
            tomb(TombstoneKind::Video, "x", 5),
            tomb(TombstoneKind::Channel, "UC9", 30),
        ]);
        let b = snap("b", vec![], vec![], vec![
            tomb(TombstoneKind::Channel, "UC9", 20),
            tomb(TombstoneKind::Video, "UC9", 1),
            tomb(TombstoneKind::Channel, "UC1", 2),
        ]);
        assert_eq!(merge(&[a, b]).tombstones, vec![
            tomb(TombstoneKind::Channel, "UC1", 2),
            tomb(TombstoneKind::Channel, "UC9", 30),
            tomb(TombstoneKind::Video, "UC9", 1),
            tomb(TombstoneKind::Video, "x", 5),
        ]);
    }

    #[test]
    fn every_list_comes_out_ordered_by_id() {
        let a = snap("a", vec![chan("UC3"), chan("UC1")], vec![vid("c"), vid("a")], vec![]);
        let b = snap("b", vec![chan("UC2")], vec![vid("b")], vec![]);
        let m = merge(&[a, b]);
        let channel_ids: Vec<&str> = m.channels.iter().map(|c| c.id.as_str()).collect();
        let video_ids: Vec<&str> = m.videos.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(channel_ids, ["UC1", "UC2", "UC3"]);
        assert_eq!(video_ids, ["a", "b", "c"]);
        assert_eq!(m.format, SNAPSHOT_FORMAT);
    }

    /// Three machines that disagree about nearly everything, ties included.
    fn quarrel() -> [Snapshot; 3] {
        let a = snap(
            "a",
            vec![SyncChannel { title: "A".into(), member: true, member_changed_at: Some(7),
                               ..chan("UC1") }],
            vec![SyncVideo { watched: true, watched_at: Some(4), watched_changed_at: Some(9),
                             sibling_group: Some("k-a".into()), sibling_changed_at: Some(3),
                             title: "from a".into(), ..vid("v") },
                 SyncVideo { added_manually: true, added_at: 70, ..vid("m") }],
            vec![tomb(TombstoneKind::Video, "gone", 3)],
        );
        let b = snap(
            "b",
            vec![SyncChannel { title: "B".into(), subscribed_changed_at: Some(200), ..chan("UC1") }],
            vec![SyncVideo { watched: true, watched_at: Some(6), watched_changed_at: Some(9),
                             hidden: true, hidden_changed_at: Some(11),
                             sibling_group: Some("k-b".into()), sibling_changed_at: Some(3),
                             title: "from b".into(), ..vid("v") }],
            vec![tomb(TombstoneKind::Video, "gone", 8)],
        );
        let c = snap(
            "c",
            vec![SyncChannel { title: "C".into(), subscribed: false, ..chan("UC1") }],
            vec![SyncVideo { hidden_changed_at: Some(9), title: "from c".into(), ..vid("v") },
                 SyncVideo { title: "polled".into(), added_at: 40, ..vid("m") }],
            vec![tomb(TombstoneKind::Channel, "gone", 1)],
        );
        [a, b, c]
    }

    #[test]
    fn the_result_does_not_depend_on_the_order_of_the_snapshots() {
        let [a, b, c] = quarrel();
        let first = merge(&[a.clone(), b.clone(), c.clone()]);
        for order in [[&a, &c, &b], [&b, &a, &c], [&b, &c, &a], [&c, &a, &b], [&c, &b, &a]] {
            assert_eq!(merge(order), first);
        }
        // Merging in stages is merging all at once...
        assert_eq!(merge(&[merge(&[a.clone(), b.clone()]), c.clone()]), first);
        assert_eq!(merge(&[a.clone(), merge(&[c, b])]), first);
        // ...and a snapshot merged with itself is unchanged.
        assert_eq!(merge([&merge([&a]), &a]), merge([&a]));
    }

    #[test]
    fn the_quarrel_settles_the_way_the_rules_say() {
        let m = merge(&quarrel());
        let v = m.videos.iter().find(|v| v.id == "v").unwrap();
        assert!(v.watched && v.hidden);
        assert_eq!(v.watched_at, Some(6), "the tie on stamp and flag goes to the later instant");
        assert_eq!(v.sibling_group.as_deref(), Some("k-b"));
        assert_eq!(v.title, "from b", "b holds the newest stamp on the row");
        let manual = m.videos.iter().find(|v| v.id == "m").unwrap();
        assert!(manual.added_manually);
        assert_eq!(manual.added_at, 70);
        assert_eq!(manual.title, "t-m", "neither copy is stamped, so the later add names it");
        let c = &m.channels[0];
        assert!(c.subscribed && c.member);
        assert_eq!(c.subscribed_changed_at, Some(200));
        assert_eq!(c.title, "B");
        assert_eq!(m.tombstones, vec![tomb(TombstoneKind::Channel, "gone", 1),
                                      tomb(TombstoneKind::Video, "gone", 8)]);
    }
}
