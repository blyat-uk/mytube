//! The Recommendations' tables (v17) and every query over them. Machine-local:
//! no triggers, no stamps, never in the sync snapshot or the transfer archive.
//! See `docs/architecture/recommendations.md`.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

use super::Db;
use crate::models::{RecUpload, Recommendation};
use crate::recs::ChannelFacts;

pub(super) const SCHEMA_V17: &str = "
CREATE TABLE IF NOT EXISTS rec_scans (
  video_id   TEXT PRIMARY KEY,
  scanned_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS rec_links (
  channel_id        TEXT NOT NULL,
  video_id          TEXT NOT NULL,
  kind              TEXT NOT NULL,
  source_channel_id TEXT NOT NULL,
  seen_at           INTEGER NOT NULL,
  PRIMARY KEY (channel_id, video_id, kind)
);
CREATE INDEX IF NOT EXISTS rec_links_seen ON rec_links (kind, seen_at);
CREATE TABLE IF NOT EXISTS rec_channels (
  id          TEXT PRIMARY KEY,
  title       TEXT,
  handle      TEXT,
  avatar_url  TEXT,
  thumb_path  TEXT,
  subscribers INTEGER,
  video_count INTEGER,
  active      INTEGER NOT NULL DEFAULT 0,
  checked_at  INTEGER,
  tried_at    INTEGER,
  dismissed   INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS rec_uploads (
  channel_id   TEXT NOT NULL,
  video_id     TEXT NOT NULL,
  rank         INTEGER NOT NULL,
  title        TEXT NOT NULL,
  published_at INTEGER,
  thumb_path   TEXT,
  PRIMARY KEY (channel_id, video_id)
);
";

/// v18: up next's sightings dropped. `rec_links.kind` stays (part of the key),
/// only ever 'collab' from here on.
pub(super) const SCHEMA_V18: &str = "
DELETE FROM rec_links WHERE kind <> 'collab';
DROP INDEX IF EXISTS rec_links_seen;
";

const DAY: i64 = 86_400;

/// How far back a video's upload may lie behind its first sighting and still
/// count as new: keeps a fresh subscription's backfill out of the scan.
const NEW_WITHIN: i64 = 7 * DAY;

/// Re-check intervals: an active channel's strip goes stale in a day; a
/// dormant one is asked again in a month, in case it woke up.
const RECHECK_ACTIVE: i64 = DAY;
const RECHECK_INACTIVE: i64 = 30 * DAY;

/// How long a check that came to nothing -- a refusal, a bot check, an RSS
/// outage -- waits before the channel is asked again. What it last showed
/// stays listed meanwhile, and the rest of the queue goes ahead of it.
const RETRY: i64 = DAY;

/// The channels that could be suggested -- one collab credit is enough -- with
/// how many followed channels credited each; never one you follow, once
/// removed, or dismissed.
fn qualified() -> &'static str {
    "SELECT l.channel_id AS id, COUNT(DISTINCT l.source_channel_id) AS popularity
       FROM rec_links l
      WHERE NOT EXISTS (SELECT 1 FROM channels c WHERE c.id = l.channel_id AND c.subscribed = 1)
        AND NOT EXISTS (SELECT 1 FROM tombstones t WHERE t.kind = 'channel' AND t.id = l.channel_id)
        AND NOT EXISTS (SELECT 1 FROM rec_channels r WHERE r.id = l.channel_id AND r.dismissed = 1)
      GROUP BY l.channel_id"
}

/// Drops `id`'s strip, and its picture unless the channel has a row of its own
/// (a subscription, an uploader) that owns the same file. Returns the files.
fn drop_files(conn: &Connection, id: &str, with_picture: bool) -> Result<Vec<String>> {
    let mut files: Vec<String> = {
        let mut st = conn.prepare_cached(
            "SELECT thumb_path FROM rec_uploads WHERE channel_id = ?1 AND thumb_path IS NOT NULL")?;
        let rows = st.query_map([id], |r| r.get(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    conn.execute("DELETE FROM rec_uploads WHERE channel_id = ?1", [id])?;
    if with_picture {
        let picture: Option<String> = conn.query_row(
            "SELECT thumb_path FROM rec_channels
              WHERE id = ?1 AND NOT EXISTS (SELECT 1 FROM channels c WHERE c.id = ?1)",
            [id], |r| r.get(0)).optional()?.flatten();
        files.extend(picture);
        conn.execute("UPDATE rec_channels SET thumb_path = NULL WHERE id = ?1", [id])?;
    }
    Ok(files)
}

impl Db {
    /// The newest unscanned video of a followed channel, with its channel.
    /// Both clocks must be recent: a sync or an import keeps the other
    /// machine's first sighting, so a library arriving that way is history,
    /// not news.
    pub fn next_rec_scan(&self, now: i64) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            "SELECT v.id, v.channel_id FROM videos v JOIN channels c ON c.id = v.channel_id
              WHERE c.subscribed = 1 AND v.status = 'ready'
                AND v.first_seen_at >= ?2 - ?1
                AND COALESCE(v.published_at, v.first_seen_at) >= v.first_seen_at - ?1
                AND NOT EXISTS (SELECT 1 FROM rec_scans s WHERE s.video_id = v.id)
              ORDER BY COALESCE(v.published_at, v.first_seen_at) DESC, v.id
              LIMIT 1",
            [NEW_WITHIN, now], |r| Ok((r.get(0)?, r.get(1)?))).optional()?)
    }

    /// Marks `video` scanned and keeps the channels credited on it (`None`:
    /// the page could not be read, which is not asked again). Channels you
    /// follow are not kept: they could never be suggested.
    pub fn record_rec_scan(&self, video: &str, source: &str, collabs: Option<&[String]>, now: i64)
        -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("INSERT OR REPLACE INTO rec_scans (video_id, scanned_at) VALUES (?1, ?2)",
                   params![video, now])?;
        if let Some(collabs) = collabs {
            let mut st = tx.prepare_cached(
                "INSERT OR IGNORE INTO rec_links (channel_id, video_id, kind, source_channel_id, seen_at)
                 SELECT ?1, ?2, 'collab', ?3, ?4
                  WHERE NOT EXISTS (SELECT 1 FROM channels WHERE id = ?1 AND subscribed = 1)")?;
            for id in collabs {
                st.execute(params![id, video, source, now])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Forgets every strip that is no longer shown and every candidate that
    /// no longer qualifies (a dismissal is kept: it is the user's). Returns
    /// the files to delete.
    pub fn prune_recs(&self) -> Result<Vec<String>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let q = qualified();
        let stale: Vec<(String, bool)> = {
            let mut st = tx.prepare(&format!(
                "WITH q AS ({q})
                 SELECT r.id, r.dismissed = 0 AND r.id NOT IN (SELECT id FROM q) FROM rec_channels r
                  WHERE r.active = 0 OR r.dismissed = 1 OR r.id NOT IN (SELECT id FROM q)"))?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut files = Vec::new();
        for (id, gone) in stale {
            files.extend(drop_files(&tx, &id, gone)?);
            if gone {
                tx.execute("DELETE FROM rec_channels WHERE id = ?1", [&id])?;
            }
        }
        tx.commit()?;
        Ok(files)
    }

    /// The next candidate due a check: never checked first, then the longest
    /// since; an active one after a day, a dormant one after a month -- and
    /// none tried in the last [`RETRY`].
    pub fn next_rec_check(&self, now: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().unwrap();
        Ok(conn.query_row(
            &format!(
                "WITH q AS ({})
                 SELECT q.id FROM q LEFT JOIN rec_channels r ON r.id = q.id
                  WHERE (r.checked_at IS NULL
                         OR (r.active = 1 AND r.checked_at <= ?1 - {RECHECK_ACTIVE})
                         OR (r.active = 0 AND r.checked_at <= ?1 - {RECHECK_INACTIVE}))
                    AND (r.tried_at IS NULL OR r.tried_at <= ?1 - {RETRY})
                  ORDER BY r.checked_at IS NOT NULL, r.checked_at, q.popularity DESC, q.id
                  LIMIT 1",
                qualified()),
            [now], |r| r.get(0)).optional()?)
    }

    /// Keeps what a check found: the page's facts when it was read (`None`
    /// for a channel that is gone), whether it is alive, and its strip --
    /// newest first, at most four; a dormant channel keeps none. A file
    /// already fetched for an upload that stays is kept. Returns the files
    /// no longer needed.
    pub fn record_rec_check(&self, id: &str, facts: Option<&ChannelFacts>, uploads: &[RecUpload],
                            active: bool, now: i64) -> Result<Vec<String>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO rec_channels (id, title, handle, avatar_url, subscribers, video_count, active, checked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT (id) DO UPDATE SET
               title = COALESCE(excluded.title, title),
               handle = COALESCE(excluded.handle, handle),
               avatar_url = COALESCE(excluded.avatar_url, avatar_url),
               subscribers = COALESCE(excluded.subscribers, subscribers),
               video_count = COALESCE(excluded.video_count, video_count),
               active = excluded.active,
               checked_at = excluded.checked_at,
               tried_at = NULL",
            params![id, facts.map(|f| &f.title), facts.and_then(|f| f.handle.as_ref()),
                    facts.and_then(|f| f.avatar_url.as_ref()), facts.and_then(|f| f.subscribers),
                    facts.and_then(|f| f.video_count), active, now])?;
        let files = if !active {
            drop_files(&tx, id, true)?
        } else {
            let keep: Vec<&str> = uploads.iter().take(4).map(|u| u.id.as_str()).collect();
            let mut files = Vec::new();
            {
                let mut st = tx.prepare_cached(
                    "SELECT video_id, thumb_path FROM rec_uploads WHERE channel_id = ?1")?;
                let rows = st.query_map([id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?)))?;
                for row in rows {
                    let (video, path) = row?;
                    if !keep.contains(&video.as_str()) {
                        files.extend(path);
                        tx.execute("DELETE FROM rec_uploads WHERE channel_id = ?1 AND video_id = ?2",
                                   params![id, video])?;
                    }
                }
            }
            let mut st = tx.prepare_cached(
                "INSERT INTO rec_uploads (channel_id, video_id, rank, title, published_at)
                 VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT (channel_id, video_id) DO UPDATE SET
                   rank = excluded.rank, title = excluded.title, published_at = excluded.published_at")?;
            for (rank, u) in uploads.iter().take(4).enumerate() {
                st.execute(params![id, u.id, rank as i64, u.title, u.published_at])?;
            }
            files
        };
        tx.commit()?;
        Ok(files)
    }

    /// A check that came to nothing: asked again after [`RETRY`], and
    /// whatever it last showed stays as it was.
    pub fn note_rec_attempt(&self, id: &str, now: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("INSERT INTO rec_channels (id, tried_at) VALUES (?1, ?2)
                      ON CONFLICT (id) DO UPDATE SET tried_at = excluded.tried_at", params![id, now])?;
        Ok(())
    }

    pub fn set_rec_thumb(&self, video: &str, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE rec_uploads SET thumb_path = ?2 WHERE video_id = ?1", params![video, path])?;
        Ok(())
    }

    pub fn set_rec_avatar(&self, id: &str, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE rec_channels SET thumb_path = ?2 WHERE id = ?1", params![id, path])?;
        Ok(())
    }

    /// The suggestions: qualified, checked and alive, each with its strip.
    pub fn list_recommendations(&self) -> Result<Vec<Recommendation>> {
        let conn = self.conn.lock().unwrap();
        let mut recs: Vec<Recommendation> = {
            let mut st = conn.prepare(&format!(
                "WITH q AS ({})
                 SELECT r.id, COALESCE(r.title, r.id), r.handle, r.thumb_path, r.subscribers,
                        r.video_count, q.popularity
                   FROM q JOIN rec_channels r ON r.id = q.id
                  WHERE r.active = 1
                  ORDER BY r.video_count DESC NULLS LAST, r.title, r.id",
                qualified()))?;
            let rows = st.query_map([], |r| Ok(Recommendation {
                id: r.get(0)?, title: r.get(1)?, handle: r.get(2)?, thumb_path: r.get(3)?,
                subscribers: r.get(4)?, video_count: r.get(5)?, popularity: r.get(6)?,
                uploads: Vec::new(),
            }))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut st = conn.prepare_cached(
            "SELECT video_id, title, published_at, thumb_path FROM rec_uploads
              WHERE channel_id = ?1 ORDER BY rank LIMIT 4")?;
        for rec in &mut recs {
            let rows = st.query_map([&rec.id], |r| Ok(RecUpload {
                id: r.get(0)?, title: r.get(1)?, published_at: r.get(2)?, thumb_path: r.get(3)?,
            }))?;
            rec.uploads = rows.collect::<rusqlite::Result<_>>()?;
        }
        Ok(recs)
    }

    /// "Never suggest this again": kept for good, machine-local. Returns the
    /// strip's files and the picture's, to delete.
    pub fn dismiss_recommendation(&self, id: &str) -> Result<Vec<String>> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute("INSERT INTO rec_channels (id, dismissed) VALUES (?1, 1)
                    ON CONFLICT (id) DO UPDATE SET dismissed = 1", [id])?;
        let files = drop_files(&tx, id, true)?;
        tx.commit()?;
        Ok(files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Channel, NewVideo, VideoStatus};

    const DAY: i64 = 86_400;

    fn now() -> i64 {
        chrono::Utc::now().timestamp()
    }

    fn follow(d: &Db, id: &str) {
        d.upsert_channel(&Channel {
            id: id.into(), title: id.into(), handle: None, url: "u".into(),
            thumb_path: None, subscribed: true, member: false, added_at: 0,
            last_polled_at: None, terminated: false, auto_download: false,
        }).unwrap();
    }

    fn video(d: &Db, id: &str, channel: &str, published_at: i64) {
        d.insert_video_if_new(&NewVideo {
            id: id.into(), channel_id: channel.into(), title: id.into(), description: None,
            thumb_url: None, published_at: Some(published_at), sort_at: Some(published_at),
            feed_rank: 0, added_manually: false, duration_secs: Some(600), view_count: None,
            status: VideoStatus::Ready,
        }).unwrap();
    }

    fn collab(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    fn facts(id: &str, videos: i64) -> ChannelFacts {
        ChannelFacts {
            id: id.into(), title: format!("{id} title"), handle: Some(format!("@{id}")),
            avatar_url: None, subscribers: Some(1000), video_count: Some(videos),
        }
    }

    fn upload(id: &str, at: i64) -> RecUpload {
        RecUpload { id: id.into(), title: id.into(), published_at: Some(at), thumb_path: None }
    }

    /// Checks `id` as alive with one upload.
    fn alive(d: &Db, id: &str, videos: i64, t: i64) {
        d.record_rec_check(id, Some(&facts(id, videos)), &[upload(&format!("{id}-v"), t)], true, t)
            .unwrap();
    }

    fn listed(d: &Db) -> Vec<String> {
        d.list_recommendations().unwrap().into_iter().map(|r| r.id).collect()
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn v17_marks_existing_videos_scanned() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        video(&d, "old", "UCa", now());
        let conn = d.conn.into_inner().unwrap();
        conn.execute_batch(
            "DROP TABLE rec_scans; DROP TABLE rec_links; DROP TABLE rec_channels;
             DROP TABLE rec_uploads; PRAGMA user_version = 16;").unwrap();
        let d = Db::init(conn).unwrap();
        assert_eq!(d.next_rec_scan(now()).unwrap(), None, "the library before the upgrade is not scanned");
        video(&d, "new", "UCa", now());
        assert_eq!(d.next_rec_scan(now()).unwrap(), Some(("new".into(), "UCa".into())));
    }

    #[test]
    fn v18_forgets_up_next_sightings() {
        let d = Db::open_in_memory().unwrap();
        let t = now();
        for (v, src) in [("v1", "UCa"), ("v2", "UCb"), ("v3", "UCc")] {
            follow(&d, src);
            d.conn.lock().unwrap().execute(
                "INSERT INTO rec_links (channel_id, video_id, kind, source_channel_id, seen_at)
                 VALUES ('UCnews', ?1, 'next', ?2, ?3)", params![v, src, t]).unwrap();
        }
        d.record_rec_scan("v4", "UCa", Some(&collab(&["UCcollab"])), t).unwrap();
        alive(&d, "UCnews", 5, t);
        alive(&d, "UCcollab", 5, t);
        let conn = d.conn.into_inner().unwrap();
        conn.execute_batch("PRAGMA user_version = 17;").unwrap();
        let d = Db::init(conn).unwrap();
        assert_eq!(listed(&d), vec!["UCcollab"], "a sidebar beside three channels is no longer a lead");
    }

    #[test]
    fn new_feed_video_is_queued_but_backfill_is_not() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        video(&d, "backfill", "UCa", now() - 30 * DAY);
        assert_eq!(d.next_rec_scan(now()).unwrap(), None);
        video(&d, "fresh", "UCa", now() - DAY);
        assert_eq!(d.next_rec_scan(now()).unwrap(), Some(("fresh".into(), "UCa".into())));
        d.record_rec_scan("fresh", "UCa", None, now()).unwrap();
        assert_eq!(d.next_rec_scan(now()).unwrap(), None, "an unreadable page is not asked again");
    }

    #[test]
    fn synced_or_imported_history_is_not_scanned() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let month_ago = now() - 30 * DAY;
        video(&d, "synced", "UCa", month_ago);
        // A sync or an import keeps the other machine's first sighting.
        d.conn.lock().unwrap()
            .execute("UPDATE videos SET first_seen_at = ?1 WHERE id = 'synced'", [month_ago]).unwrap();
        assert_eq!(d.next_rec_scan(now()).unwrap(), None);
    }

    #[test]
    fn a_failed_check_waits_a_day_and_lets_the_others_through() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCone", "UCtwo"])), t).unwrap();
        let first = d.next_rec_check(t).unwrap().unwrap();
        d.note_rec_attempt(&first, t).unwrap();
        let second = d.next_rec_check(t).unwrap().unwrap();
        assert_ne!(first, second, "one stubborn page must not hold the queue");
        d.note_rec_attempt(&second, t).unwrap();
        assert_eq!(d.next_rec_check(t + DAY - 1).unwrap(), None);
        assert!(d.next_rec_check(t + DAY).unwrap().is_some());
    }

    #[test]
    fn a_failed_recheck_keeps_the_channel_listed() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCx"])), t).unwrap();
        alive(&d, "UCx", 5, t);
        assert_eq!(d.next_rec_check(t + DAY).unwrap().as_deref(), Some("UCx"));
        d.note_rec_attempt("UCx", t + DAY).unwrap();
        assert_eq!(listed(&d), vec!["UCx"]);
        assert_eq!(d.list_recommendations().unwrap()[0].uploads.len(), 1, "its strip stays");
        assert_eq!(d.next_rec_check(t + DAY).unwrap(), None);
        assert_eq!(d.next_rec_check(t + 2 * DAY).unwrap().as_deref(), Some("UCx"));
    }

    #[test]
    fn collab_qualifies_alone() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCx"])), t).unwrap();
        assert_eq!(d.next_rec_check(t).unwrap().as_deref(), Some("UCx"));
        alive(&d, "UCx", 50, t);
        assert_eq!(listed(&d), vec!["UCx"]);
    }

    #[test]
    fn followed_removed_and_dismissed_never_listed() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        follow(&d, "UCfollowed");
        follow(&d, "UCremoved");
        d.remove_channel("UCremoved").unwrap();
        let t = now();
        d.record_rec_scan("v1", "UCa",
            Some(&collab(&["UCfollowed", "UCremoved", "UCdismissed", "UCok"])), t).unwrap();
        for id in ["UCfollowed", "UCremoved", "UCdismissed", "UCok"] {
            alive(&d, id, 10, t);
        }
        d.dismiss_recommendation("UCdismissed").unwrap();
        d.record_rec_scan("v2", "UCa", Some(&collab(&["UCdismissed"])), t).unwrap();
        assert_eq!(listed(&d), vec!["UCok"]);
        assert_eq!(d.next_rec_check(t + 2 * DAY).unwrap().as_deref(), Some("UCok"),
            "nothing is checked that could never be shown");
    }

    #[test]
    fn inactive_and_unchecked_not_listed() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCdead", "UCnew", "UCgone"])), t).unwrap();
        d.record_rec_check("UCdead", Some(&facts("UCdead", 2)), &[], false, t).unwrap();
        d.record_rec_check("UCgone", None, &[], false, t).unwrap();
        assert!(listed(&d).is_empty());
        assert_eq!(d.next_rec_check(t).unwrap().as_deref(), Some("UCnew"));
    }

    #[test]
    fn active_rechecked_after_a_day_inactive_after_thirty() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCalive", "UCdead"])), t).unwrap();
        alive(&d, "UCalive", 5, t);
        d.record_rec_check("UCdead", Some(&facts("UCdead", 1)), &[], false, t).unwrap();
        assert_eq!(d.next_rec_check(t + DAY - 1).unwrap(), None);
        assert_eq!(d.next_rec_check(t + DAY).unwrap().as_deref(), Some("UCalive"));
        alive(&d, "UCalive", 5, t + DAY);
        assert_eq!(d.next_rec_check(t + 29 * DAY).unwrap().as_deref(), Some("UCalive"));
        alive(&d, "UCalive", 5, t + 30 * DAY);
        assert_eq!(d.next_rec_check(t + 30 * DAY).unwrap().as_deref(), Some("UCdead"));
    }

    #[test]
    fn popularity_counts_distinct_sources() {
        let d = Db::open_in_memory().unwrap();
        let t = now();
        for (v, src) in [("v1", "UCa"), ("v2", "UCa"), ("v3", "UCb")] {
            follow(&d, src);
            d.record_rec_scan(v, src, Some(&collab(&["UCx"])), t).unwrap();
        }
        alive(&d, "UCx", 40, t);
        let r = &d.list_recommendations().unwrap()[0];
        assert_eq!(r.popularity, 2);
        assert_eq!(r.video_count, Some(40));
        assert_eq!(r.handle.as_deref(), Some("@UCx"));
        assert_eq!(r.uploads.len(), 1);
    }

    #[test]
    fn record_check_returns_dropped_thumbs() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCx"])), t).unwrap();
        let four: Vec<RecUpload> = (1..=4).map(|i| upload(&format!("u{i}"), t - i)).collect();
        d.record_rec_check("UCx", Some(&facts("UCx", 9)), &four, true, t).unwrap();
        for u in &four {
            d.set_rec_thumb(&u.id, &format!("/thumbs/recs/{}.jpg", u.id)).unwrap();
        }
        let newer: Vec<RecUpload> =
            [upload("u0", t)].into_iter().chain(four[..3].iter().cloned()).collect();
        let dropped = d.record_rec_check("UCx", Some(&facts("UCx", 10)), &newer, true, t + DAY).unwrap();
        assert_eq!(dropped, vec!["/thumbs/recs/u4.jpg"]);
        let r = &d.list_recommendations().unwrap()[0];
        let ids: Vec<&str> = r.uploads.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["u0", "u1", "u2", "u3"]);
        assert_eq!(r.uploads[1].thumb_path.as_deref(), Some("/thumbs/recs/u1.jpg"), "kept files stay named");

        let gone = d.record_rec_check("UCx", Some(&facts("UCx", 10)), &[], false, t + 2 * DAY).unwrap();
        assert_eq!(sorted(gone), vec!["/thumbs/recs/u1.jpg", "/thumbs/recs/u2.jpg", "/thumbs/recs/u3.jpg"]);
    }

    #[test]
    fn dismissing_returns_its_files() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCx"])), t).unwrap();
        alive(&d, "UCx", 5, t);
        d.set_rec_thumb("UCx-v", "/thumbs/recs/UCx-v.jpg").unwrap();
        d.set_rec_avatar("UCx", "/thumbs/channels/UCx.jpg").unwrap();
        let files = d.dismiss_recommendation("UCx").unwrap();
        assert_eq!(sorted(files), vec!["/thumbs/channels/UCx.jpg", "/thumbs/recs/UCx-v.jpg"]);
        assert!(listed(&d).is_empty());
    }

    #[test]
    fn subscribing_leaves_the_list_and_prune_frees_its_strip() {
        let d = Db::open_in_memory().unwrap();
        follow(&d, "UCa");
        let t = now();
        d.record_rec_scan("v1", "UCa", Some(&collab(&["UCx"])), t).unwrap();
        alive(&d, "UCx", 5, t);
        d.set_rec_thumb("UCx-v", "/thumbs/recs/UCx-v.jpg").unwrap();
        d.set_rec_avatar("UCx", "/thumbs/channels/UCx.jpg").unwrap();
        follow(&d, "UCx");
        assert!(listed(&d).is_empty());
        assert_eq!(d.prune_recs().unwrap(), vec!["/thumbs/recs/UCx-v.jpg"],
            "the picture is the subscription's now");
    }
}
