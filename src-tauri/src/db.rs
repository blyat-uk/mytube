use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;

use crate::models::*;

pub struct Db { conn: Mutex<Connection> }

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS channels (
  id             TEXT PRIMARY KEY,
  title          TEXT NOT NULL,
  handle         TEXT,
  url            TEXT NOT NULL,
  thumb_path     TEXT,
  subscribed     INTEGER NOT NULL DEFAULT 1,
  added_at       INTEGER NOT NULL,
  last_polled_at INTEGER
);
CREATE TABLE IF NOT EXISTS videos (
  id             TEXT PRIMARY KEY,
  channel_id     TEXT NOT NULL REFERENCES channels(id) ON DELETE CASCADE,
  title          TEXT NOT NULL,
  description    TEXT,
  thumb_url      TEXT,
  thumb_path     TEXT,
  published_at   INTEGER,
  sort_at        INTEGER,
  feed_rank      INTEGER NOT NULL DEFAULT 0,
  added_manually INTEGER NOT NULL DEFAULT 0,
  duration_secs  INTEGER,
  view_count     INTEGER,
  status         TEXT NOT NULL DEFAULT 'pending',
  watched        INTEGER NOT NULL DEFAULT 0,
  watched_at     INTEGER,
  download_state TEXT NOT NULL DEFAULT 'none',
  download_error TEXT,
  file_path      TEXT,
  first_seen_at  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_videos_feed    ON videos(status, sort_at DESC);
CREATE INDEX IF NOT EXISTS idx_videos_channel ON videos(channel_id, published_at DESC);
"#;

fn now() -> i64 { chrono::Utc::now().timestamp() }

/// Escapes LIKE wildcards so a literal `%` or `_` in a search box is literal.
fn like_pattern(term: &str) -> String {
    let escaped = term.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_");
    format!("%{escaped}%")
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> { Self::init(Connection::open_in_memory()?) }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        conn.pragma_update(None, "user_version", 1)?;
        Ok(Self { conn: Mutex::new(conn) })
    }

    // ---- channels ----

    pub fn upsert_channel(&self, c: &Channel) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO channels (id,title,handle,url,thumb_path,subscribed,added_at,last_polled_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET
               title=excluded.title,
               handle=COALESCE(excluded.handle, channels.handle),
               url=excluded.url,
               thumb_path=COALESCE(excluded.thumb_path, channels.thumb_path),
               -- subscribing is a one-way upgrade: adding an ad-hoc video from a
               -- channel you already follow must not silently unsubscribe you.
               subscribed=MAX(channels.subscribed, excluded.subscribed)",
            params![c.id, c.title, c.handle, c.url, c.thumb_path, c.subscribed as i64,
                    if c.added_at == 0 { now() } else { c.added_at }, c.last_polled_at],
        )?;
        Ok(())
    }

    pub fn list_channels(&self) -> Result<Vec<Channel>> {
        // No lock taken here: `channels_where` takes it, and this Mutex is not
        // reentrant — locking in both places deadlocks.
        self.channels_where("1=1")
    }

    /// Only these are polled and only these appear in the channel filter.
    pub fn list_subscribed_channels(&self) -> Result<Vec<Channel>> {
        self.channels_where("subscribed=1")
    }

    fn channels_where(&self, cond: &str) -> Result<Vec<Channel>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(&format!(
            "SELECT id,title,handle,url,thumb_path,subscribed,added_at,last_polled_at
             FROM channels WHERE {cond} ORDER BY title COLLATE NOCASE ASC"))?;
        let rows = st.query_map([], |r| Ok(Channel {
            id: r.get(0)?, title: r.get(1)?, handle: r.get(2)?, url: r.get(3)?,
            thumb_path: r.get(4)?, subscribed: r.get::<_, i64>(5)? != 0,
            added_at: r.get(6)?, last_polled_at: r.get(7)?,
        }))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_channel(&self, id: &str) -> Result<Option<Channel>> {
        Ok(self.list_channels()?.into_iter().find(|c| c.id == id))
    }

    pub fn remove_channel(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM channels WHERE id=?1", params![id])?;
        Ok(())
    }

    pub fn set_channel_polled(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE channels SET last_polled_at=?2 WHERE id=?1", params![id, now()])?;
        Ok(())
    }

    // ---- videos ----

    /// Returns true when the row was newly inserted.
    pub fn insert_video_if_new(&self, v: &NewVideo) -> Result<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT OR IGNORE INTO videos
             (id,channel_id,title,description,thumb_url,published_at,sort_at,feed_rank,
              added_manually,duration_secs,view_count,status,first_seen_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
            params![v.id, v.channel_id, v.title, v.description, v.thumb_url,
                    v.published_at, v.sort_at, v.feed_rank, v.added_manually as i64,
                    v.duration_secs, v.view_count, v.status.as_str(), now()],
        )?;
        Ok(n > 0)
    }

    /// Fills in metadata the flat-playlist pass resolved. Never clears a real
    /// published_at with NULL, so a later backfill cannot erase an RSS date.
    pub fn update_video_meta(&self, id: &str, duration: Option<i64>,
                             views: Option<i64>, status: VideoStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE videos SET
               duration_secs=COALESCE(?2, duration_secs),
               view_count=COALESCE(?3, view_count),
               status=?4
             WHERE id=?1",
            params![id, duration, views, status.as_str()])?;
        Ok(())
    }

    /// RSS supplies the real date for a row that was backfilled without one.
    /// `sort_at` only follows for rows that were NOT added manually — a manually
    /// added video must keep sorting by when the user added it.
    pub fn set_published_at(&self, id: &str, ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE videos SET published_at=?2,
                    sort_at = CASE WHEN added_manually=1 THEN sort_at ELSE ?2 END
             WHERE id=?1", params![id, ts])?;
        Ok(())
    }

    pub fn set_thumb_path(&self, id: &str, path: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET thumb_path=?2 WHERE id=?1", params![id, path])?;
        Ok(())
    }

    pub fn existing_video_ids(&self, ids: &[String]) -> Result<HashSet<String>> {
        if ids.is_empty() { return Ok(HashSet::new()); }
        let conn = self.conn.lock().unwrap();
        let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT id FROM videos WHERE id IN ({placeholders})");
        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(ids), |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
    }

    /// Videos still awaiting a duration/status, first seen at or after `since`.
    pub fn unresolved_video_ids(&self, channel_id: &str, since: i64) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id FROM videos
             WHERE channel_id=?1 AND status<>'ready' AND first_seen_at>=?2")?;
        let rows = st.query_map(params![channel_id, since], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn get_video(&self, id: &str) -> Result<Option<Video>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(&format!("{SELECT_VIDEO} WHERE v.id=?1"))?;
        Ok(st.query_row(params![id], map_video).optional()?)
    }

    pub fn list_videos(&self, f: &VideoFilter) -> Result<Vec<Video>> {
        let conn = self.conn.lock().unwrap();
        let mut sql = format!("{SELECT_VIDEO} WHERE v.status='ready'");
        let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(ch) = &f.channel_id {
            args.push(Box::new(ch.clone()));
            sql.push_str(&format!(" AND v.channel_id=?{}", args.len()));
        }
        if f.hide_watched { sql.push_str(" AND v.watched=0"); }
        if f.downloaded_only { sql.push_str(" AND v.download_state='done'"); }
        if let Some(term) = f.search.as_ref().filter(|s| !s.trim().is_empty()) {
            args.push(Box::new(like_pattern(term.trim())));
            let i = args.len();
            sql.push_str(&format!(
                " AND (v.title LIKE ?{i} ESCAPE '\\' OR c.title LIKE ?{i} ESCAPE '\\')"));
        }
        sql.push_str(match f.sort {
            SortOrder::Newest  => " ORDER BY v.sort_at DESC NULLS LAST, v.feed_rank ASC, v.id ASC",
            SortOrder::Oldest  => " ORDER BY v.sort_at ASC NULLS LAST, v.feed_rank DESC, v.id ASC",
            SortOrder::Channel => " ORDER BY c.title COLLATE NOCASE ASC, v.sort_at DESC NULLS LAST, v.feed_rank ASC, v.id ASC",
        });
        args.push(Box::new(f.limit));
        sql.push_str(&format!(" LIMIT ?{}", args.len()));
        args.push(Box::new(f.offset));
        sql.push_str(&format!(" OFFSET ?{}", args.len()));

        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), map_video)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn set_watched(&self, id: &str, watched: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET watched=?2, watched_at=?3 WHERE id=?1",
            params![id, watched as i64, if watched { Some(now()) } else { None }])?;
        Ok(())
    }

    pub fn set_download_state(&self, id: &str, state: DownloadState,
                              file_path: Option<&str>, error: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE videos SET download_state=?2,
                               file_path=COALESCE(?3, file_path),
                               download_error=?4
             WHERE id=?1",
            params![id, state.as_str(), file_path, error])?;
        Ok(())
    }

    pub fn clear_file_path(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET file_path=NULL, download_state='none', download_error=NULL WHERE id=?1",
                     params![id])?;
        Ok(())
    }

    /// A yt-dlp child never survives an app restart, so anything left mid-flight
    /// is reset to `none` and becomes clickable again.
    pub fn reset_stale_downloads(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET download_state='none'
                      WHERE download_state IN ('downloading','queued')", [])?;
        Ok(())
    }
}

const SELECT_VIDEO: &str = "
SELECT v.id, v.channel_id, c.title, v.title, v.description, v.thumb_url, v.thumb_path,
       v.published_at, v.sort_at, v.feed_rank, v.added_manually,
       v.duration_secs, v.view_count, v.status,
       v.watched, v.watched_at, v.download_state, v.download_error, v.file_path,
       v.first_seen_at
FROM videos v JOIN channels c ON c.id = v.channel_id";

fn map_video(r: &Row) -> rusqlite::Result<Video> {
    Ok(Video {
        id: r.get(0)?, channel_id: r.get(1)?, channel_title: r.get(2)?, title: r.get(3)?,
        description: r.get(4)?, thumb_url: r.get(5)?, thumb_path: r.get(6)?,
        published_at: r.get(7)?, sort_at: r.get(8)?, feed_rank: r.get(9)?,
        added_manually: r.get::<_, i64>(10)? != 0,
        duration_secs: r.get(11)?, view_count: r.get(12)?,
        status: VideoStatus::parse(&r.get::<_, String>(13)?),
        watched: r.get::<_, i64>(14)? != 0, watched_at: r.get(15)?,
        download_state: DownloadState::parse(&r.get::<_, String>(16)?),
        download_error: r.get(17)?, file_path: r.get(18)?, first_seen_at: r.get(19)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn db() -> Db { Db::open_in_memory().unwrap() }

    fn chan(id: &str, title: &str) -> Channel {
        Channel { id: id.into(), title: title.into(), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, added_at: 0, last_polled_at: None }
    }

    fn vid(id: &str, ch: &str, published: Option<i64>) -> NewVideo {
        NewVideo { id: id.into(), channel_id: ch.into(), title: format!("t-{id}"),
                   description: None, thumb_url: None, published_at: published,
                   sort_at: published, feed_rank: 0, added_manually: false,
                   duration_secs: Some(600), view_count: Some(1),
                   status: VideoStatus::Ready }
    }

    #[test]
    fn migrations_apply_to_an_empty_database() {
        let d = db();
        assert_eq!(d.list_channels().unwrap().len(), 0);
    }

    #[test]
    fn insert_video_is_idempotent() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        assert!(d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap());
        assert!(!d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap());
        assert_eq!(d.list_videos(&VideoFilter::default()).unwrap().len(), 1);
    }

    #[test]
    fn only_ready_videos_appear_in_the_feed() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        for (id, st) in [("a", VideoStatus::Ready), ("b", VideoStatus::Upcoming),
                         ("c", VideoStatus::Live), ("d", VideoStatus::Pending)] {
            let mut v = vid(id, "UC1", Some(1));
            v.status = st;
            d.insert_video_if_new(&v).unwrap();
        }
        let got = d.list_videos(&VideoFilter::default()).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, "a");
    }

    #[test]
    fn null_published_dates_sort_last() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("old", "UC1", Some(50))).unwrap();
        d.insert_video_if_new(&vid("new", "UC1", Some(500))).unwrap();
        let mut backfilled = vid("backfill", "UC1", None);
        backfilled.sort_at = None;
        backfilled.feed_rank = 3;
        d.insert_video_if_new(&backfilled).unwrap();
        let ids: Vec<String> = d.list_videos(&VideoFilter::default()).unwrap()
            .into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["new", "old", "backfill"]);
    }

    #[test]
    fn filters_combine_correctly() {
        let d = db();
        d.upsert_channel(&chan("UC1", "Alpha")).unwrap();
        d.upsert_channel(&chan("UC2", "Beta")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(3))).unwrap();
        d.insert_video_if_new(&vid("b", "UC2", Some(2))).unwrap();
        d.insert_video_if_new(&vid("c", "UC1", Some(1))).unwrap();
        d.set_watched("a", true).unwrap();
        d.set_download_state("b", DownloadState::Done, Some("/tmp/b.mkv"), None).unwrap();

        let by_channel = VideoFilter { channel_id: Some("UC1".into()), ..Default::default() };
        assert_eq!(d.list_videos(&by_channel).unwrap().len(), 2);

        let unwatched = VideoFilter { hide_watched: true, ..Default::default() };
        let ids: Vec<String> = d.list_videos(&unwatched).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["b", "c"]);

        let downloaded = VideoFilter { downloaded_only: true, ..Default::default() };
        assert_eq!(d.list_videos(&downloaded).unwrap()[0].id, "b");
    }

    #[test]
    fn search_matches_title_or_channel_and_escapes_wildcards() {
        let d = db();
        d.upsert_channel(&chan("UC1", "Alpha")).unwrap();
        let mut v = vid("a", "UC1", Some(1));
        v.title = "100% cotton".into();
        d.insert_video_if_new(&v).unwrap();
        d.insert_video_if_new(&vid("b", "UC1", Some(2))).unwrap();

        let f = |s: &str| VideoFilter { search: Some(s.into()), ..Default::default() };
        assert_eq!(d.list_videos(&f("cotton")).unwrap().len(), 1);
        assert_eq!(d.list_videos(&f("alpha")).unwrap().len(), 2, "channel name should match");
        // A literal % must not behave as a wildcard.
        assert_eq!(d.list_videos(&f("%")).unwrap().len(), 1);
    }

    #[test]
    fn watched_toggle_sets_and_clears_timestamp() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(1))).unwrap();
        d.set_watched("a", true).unwrap();
        let v = d.get_video("a").unwrap().unwrap();
        assert!(v.watched && v.watched_at.is_some());
        d.set_watched("a", false).unwrap();
        let v = d.get_video("a").unwrap().unwrap();
        assert!(!v.watched && v.watched_at.is_none());
    }

    #[test]
    fn removing_a_channel_cascades_to_its_videos() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(1))).unwrap();
        d.remove_channel("UC1").unwrap();
        assert_eq!(d.list_videos(&VideoFilter::default()).unwrap().len(), 0);
    }

    #[test]
    fn upsert_channel_updates_title_without_losing_added_at() {
        let d = db();
        let mut c = chan("UC1", "Old"); c.added_at = 42;
        d.upsert_channel(&c).unwrap();
        let mut c2 = chan("UC1", "New"); c2.added_at = 999;
        d.upsert_channel(&c2).unwrap();
        let got = &d.list_channels().unwrap()[0];
        assert_eq!(got.title, "New");
        assert_eq!(got.added_at, 42, "added_at must not be overwritten");
    }

    #[test]
    fn stale_downloads_reset_on_startup() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(1))).unwrap();
        d.insert_video_if_new(&vid("b", "UC1", Some(2))).unwrap();
        d.set_download_state("a", DownloadState::Downloading, None, None).unwrap();
        d.set_download_state("b", DownloadState::Done, Some("/tmp/b.mkv"), None).unwrap();
        d.reset_stale_downloads().unwrap();
        assert_eq!(d.get_video("a").unwrap().unwrap().download_state, DownloadState::None);
        assert_eq!(d.get_video("b").unwrap().unwrap().download_state, DownloadState::Done);
    }

    #[test]
    fn a_manually_added_video_sorts_by_when_it_was_added_not_when_uploaded() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("recent", "UC1", Some(9_000))).unwrap();
        // Uploaded long ago, but added just now.
        let mut manual = vid("manual", "UC1", Some(10));
        manual.sort_at = Some(10_000);
        manual.added_manually = true;
        d.insert_video_if_new(&manual).unwrap();

        let ids: Vec<String> = d.list_videos(&VideoFilter::default()).unwrap()
            .into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["manual", "recent"], "manual add sorts to the top");
        let v = d.get_video("manual").unwrap().unwrap();
        assert_eq!(v.published_at, Some(10), "card still shows the true upload date");
        assert!(v.added_manually);
    }

    #[test]
    fn rss_dates_never_reorder_a_manually_added_video() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        let mut manual = vid("m", "UC1", Some(10));
        manual.sort_at = Some(10_000);
        manual.added_manually = true;
        d.insert_video_if_new(&manual).unwrap();
        d.set_published_at("m", 42).unwrap();
        let v = d.get_video("m").unwrap().unwrap();
        assert_eq!(v.published_at, Some(42));
        assert_eq!(v.sort_at, Some(10_000), "sort position must be preserved");
    }

    #[test]
    fn only_subscribed_channels_are_listed_for_polling() {
        let d = db();
        d.upsert_channel(&chan("UC1", "Subscribed")).unwrap();
        let mut adhoc = chan("UC2", "Ad-hoc uploader");
        adhoc.subscribed = false;
        d.upsert_channel(&adhoc).unwrap();
        assert_eq!(d.list_channels().unwrap().len(), 2);
        let subbed = d.list_subscribed_channels().unwrap();
        assert_eq!(subbed.len(), 1);
        assert_eq!(subbed[0].id, "UC1");
    }

    #[test]
    fn subscribing_is_a_one_way_upgrade() {
        let d = db();
        d.upsert_channel(&chan("UC1", "Real subscription")).unwrap();
        let mut adhoc = chan("UC1", "Real subscription");
        adhoc.subscribed = false;
        d.upsert_channel(&adhoc).unwrap();
        assert!(d.get_channel("UC1").unwrap().unwrap().subscribed,
                "adding an ad-hoc video must not unsubscribe an existing subscription");
    }

    #[test]
    fn unresolved_ids_exclude_ready_and_respect_the_age_cutoff() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        let mut pending = vid("p", "UC1", Some(1)); pending.status = VideoStatus::Pending;
        d.insert_video_if_new(&pending).unwrap();
        d.insert_video_if_new(&vid("r", "UC1", Some(1))).unwrap();
        let ids = d.unresolved_video_ids("UC1", 0).unwrap();
        assert_eq!(ids, vec!["p".to_string()]);
        // Cutoff in the future excludes everything.
        assert!(d.unresolved_video_ids("UC1", i64::MAX).unwrap().is_empty());
    }
}
