use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use crate::models::*;
use crate::siblings;

pub struct Db { conn: Mutex<Connection> }

const SCHEMA_V1: &str = r#"
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

/// Current schema version. Bump and add a step below when the schema changes.
const SCHEMA_VERSION: i64 = 5;

fn column_exists(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut st = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = st.query([])?;
    while let Some(r) = rows.next()? {
        if r.get::<_, String>(1)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Stepwise migration keyed off `user_version`.
///
/// A fresh database runs every step in order, so it converges on exactly the
/// same schema an upgraded one has. `CREATE TABLE IF NOT EXISTS` alone is NOT a
/// migration: on an existing database it silently does nothing, so any new
/// column has to arrive through its own ALTER step.
fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;

    if version < 1 {
        conn.execute_batch(SCHEMA_V1)?;
    }

    if version < 2 {
        if !column_exists(conn, "videos", "hidden")? {
            conn.execute(
                "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_videos_hidden ON videos(hidden, sort_at DESC);",
        )?;
    }

    if version < 3 {
        if !column_exists(conn, "videos", "downloaded_at")? {
            conn.execute("ALTER TABLE videos ADD COLUMN downloaded_at INTEGER", [])?;
            backfill_downloaded_at(conn)?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_videos_downloaded ON videos(downloaded_at DESC);",
        )?;
    }

    if version < 4 {
        if !column_exists(conn, "videos", "sibling_group")? {
            conn.execute("ALTER TABLE videos ADD COLUMN sibling_group TEXT", [])?;
        }
        conn.execute_batch(
            "CREATE INDEX IF NOT EXISTS idx_videos_sibling_group ON videos(sibling_group);",
        )?;
    }

    if version < 5 {
        if !column_exists(conn, "channels", "member")? {
            conn.execute(
                "ALTER TABLE channels ADD COLUMN member INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
    }

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

/// Seeds `downloaded_at` for files that were already on disk when the column
/// arrived, from each file's mtime. Without it every download predating the
/// migration falls back to release order -- precisely the ordering the column
/// exists to replace. Runs once, inside the ALTER step, so an ordinary open
/// never stats the library. A file that has since been moved or deleted keeps
/// NULL and sorts last, which is the same place an unknown date always lands.
fn backfill_downloaded_at(conn: &Connection) -> Result<()> {
    // Collected up front so the statement's borrow of `conn` ends before the
    // UPDATEs below need it.
    let rows: Vec<(String, String)> = {
        let mut st = conn.prepare(
            "SELECT id, file_path FROM videos
             WHERE download_state='done' AND file_path IS NOT NULL")?;
        let it = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        it.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, path) in rows {
        let Ok(meta) = std::fs::metadata(&path) else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        let Ok(age) = mtime.duration_since(std::time::UNIX_EPOCH) else { continue };
        conn.execute("UPDATE videos SET downloaded_at=?2 WHERE id=?1",
                     params![id, age.as_secs() as i64])?;
    }
    Ok(())
}

fn now() -> i64 { chrono::Utc::now().timestamp() }

/// Escapes LIKE wildcards so a literal `%` or `_` in a search box is literal.
fn like_pattern(term: &str) -> String {
    let escaped = term.replace('\\', r"\\").replace('%', r"\%").replace('_', r"\_");
    format!("%{escaped}%")
}

/// Every ready video on a channel as the grouping rule sees it, plus each row's
/// runtime.
///
/// Matching runs in Rust rather than in SQL because it is fuzzy, and because a
/// hand-marked group is a set of seed titles rather than a single stem (see
/// [`crate::siblings::Atoms`]); one channel's titles are few enough that
/// loading them to compare costs nothing. The runtimes ride along on the same
/// query because [`SortOrder::Length`] has to total a group up while all it
/// holds is ids, and the rows are already in hand.
fn channel_atoms(
    conn: &Connection,
    channel_id: &str,
) -> Result<(siblings::Atoms, HashMap<String, i64>)> {
    let mut st = conn.prepare(
        "SELECT id, title, duration_secs, sibling_group FROM videos
         WHERE channel_id=?1 AND status='ready'",
    )?;
    let rows = st.query_map(params![channel_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?,
        ))
    })?;

    let mut entries = Vec::new();
    let mut runtimes = HashMap::new();
    for row in rows {
        let (id, title, secs, group) = row?;
        // A row whose duration has not resolved yet contributes nothing to its
        // series rather than dropping the series out of the ordering.
        runtimes.insert(id.clone(), secs.unwrap_or(0));
        entries.push(siblings::Entry { id, title, group });
    }
    Ok((siblings::Atoms::new(entries), runtimes))
}

/// Ids of every ready video on the anchor's channel that belongs on the same
/// card -- the anchor itself included, since a series view that omits the video
/// you opened it from is disorienting.
fn sibling_ids(conn: &rusqlite::Connection, anchor_id: &str) -> Result<Vec<String>> {
    let channel_id: Option<String> = conn
        .query_row("SELECT channel_id FROM videos WHERE id=?1", params![anchor_id], |r| r.get(0))
        .optional()?;
    let Some(channel_id) = channel_id else { return Ok(Vec::new()) };
    let (atoms, _) = channel_atoms(conn, &channel_id)?;
    Ok(atoms.group_led_by(anchor_id).into_iter().map(String::from).collect())
}

/// The feed query, ordered but unpaged: `list_videos` bolts `LIMIT`/`OFFSET`
/// onto it, `list_video_groups` walks all of it. `select` decides how much of
/// each row comes back -- the grouping walk needs ids alone, and reading
/// twenty-two columns of a whole library only to drop them would be waste.
///
/// `Ok(None)` means the filter selects nothing at all, which is not the same as
/// a query that returns no rows: an anchor with no siblings has no `IN ()` that
/// SQLite would accept.
fn feed_query(
    conn: &Connection,
    f: &VideoFilter,
    select: &str,
) -> Result<Option<(String, Vec<Box<dyn rusqlite::ToSql>>)>> {
    let mut sql = format!("{select} WHERE v.status='ready'");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(anchor) = &f.sibling_of {
        // Showing a series means showing all of it, so none of the ordinary
        // filters apply here -- a part you have watched, hidden or not yet
        // downloaded is still a part.
        let ids = sibling_ids(conn, anchor)?;
        if ids.is_empty() { return Ok(None); }
        let holes = ids
            .into_iter()
            .map(|id| { args.push(Box::new(id)); format!("?{}", args.len()) })
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND v.id IN ({holes})"));
    } else {
        if let Some(ch) = &f.channel_id {
            args.push(Box::new(ch.clone()));
            sql.push_str(&format!(" AND v.channel_id=?{}", args.len()));
        }
        if !f.show_hidden { sql.push_str(" AND v.hidden=0"); }
        if f.hide_watched { sql.push_str(" AND v.watched=0"); }
        if f.downloaded_only { sql.push_str(" AND v.download_state='done'"); }
        if let Some(term) = f.search.as_ref().filter(|s| !s.trim().is_empty()) {
            args.push(Box::new(like_pattern(term.trim())));
            let i = args.len();
            sql.push_str(&format!(
                " AND (v.title LIKE ?{i} ESCAPE '\\' OR c.title LIKE ?{i} ESCAPE '\\')"));
        }
    }
    sql.push_str(order_by(f.sort));
    Ok(Some((sql, args)))
}

/// The one place the feed's orderings are written down. `list_video_groups`
/// reuses it to order a series' parts, so a group can never be sorted by a rule
/// the feed around it does not follow.
fn order_by(sort: SortOrder) -> &'static str {
    match sort {
        SortOrder::Newest  => " ORDER BY v.sort_at DESC NULLS LAST, v.feed_rank ASC, v.id ASC",
        SortOrder::Oldest  => " ORDER BY v.sort_at ASC NULLS LAST, v.feed_rank DESC, v.id ASC",
        SortOrder::Channel => " ORDER BY c.title COLLATE NOCASE ASC, v.sort_at DESC NULLS LAST, v.feed_rank ASC, v.id ASC",
        SortOrder::Downloaded => " ORDER BY v.downloaded_at DESC NULLS LAST, v.sort_at DESC NULLS LAST, v.id ASC",
        SortOrder::Length  => " ORDER BY v.duration_secs DESC NULLS LAST, v.sort_at DESC NULLS LAST, v.id ASC",
        // Group size is not a column: `list_video_groups` ranks the finished
        // groups itself. What SQL still owns here is the order the walk sees
        // rows in -- which decides each group's leader, its parts' order, and
        // how ties between equal-length series break.
        SortOrder::Parts => order_by(SortOrder::Newest),
    }
}

/// How many ids one hydration query may bind. Well under SQLite's parameter
/// ceiling, and only ever exceeded by a single series longer than the batch.
const HYDRATE_BATCH: usize = 200;

/// Fetches `ids` in the feed's order, recording each row and the position it
/// came back in. Positions are only ever compared within one batch, which is
/// why [`Db::list_video_groups`] never lets a group straddle two.
fn hydrate(
    conn: &Connection,
    ids: &[&String],
    sort: SortOrder,
    rows: &mut HashMap<String, Video>,
    rank: &mut HashMap<String, usize>,
) -> Result<()> {
    if ids.is_empty() { return Ok(()); }
    let holes = (1..=ids.len()).map(|i| format!("?{i}")).collect::<Vec<_>>().join(",");
    let sql = format!("{SELECT_VIDEO} WHERE v.id IN ({holes}){}", order_by(sort));
    let mut st = conn.prepare(&sql)?;
    let fetched = st.query_map(rusqlite::params_from_iter(ids.iter()), map_video)?;
    let base = rank.len();
    for (i, row) in fetched.enumerate() {
        let v = row?;
        rank.insert(v.id.clone(), base + i);
        rows.insert(v.id.clone(), v);
    }
    Ok(())
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
        migrate(&conn)?;
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
               subscribed=MAX(channels.subscribed, excluded.subscribed)
               -- `member` is deliberately absent: a new row takes the column
               -- default and an existing one keeps what it had. Every poll
               -- upserts its channel to catch a rename, and none of those
               -- carries any opinion about a membership. `Channel::member` is
               -- therefore read-only through this path -- see
               -- `set_channel_member`.",
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
            "SELECT id,title,handle,url,thumb_path,subscribed,added_at,last_polled_at,member
             FROM channels WHERE {cond} ORDER BY title COLLATE NOCASE ASC"))?;
        let rows = st.query_map([], |r| Ok(Channel {
            id: r.get(0)?, title: r.get(1)?, handle: r.get(2)?, url: r.get(3)?,
            thumb_path: r.get(4)?, subscribed: r.get::<_, i64>(5)? != 0,
            added_at: r.get(6)?, last_polled_at: r.get(7)?,
            member: r.get::<_, i64>(8)? != 0,
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

    /// Records whether you have joined this channel's membership, which is what
    /// lets a poll ingest its members-only uploads.
    pub fn set_channel_member(&self, id: &str, member: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE channels SET member=?2 WHERE id=?1", params![id, member as i64])?;
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

    /// Fills a date only where one is missing. An exact RSS timestamp must never
    /// be clobbered by the day-granular approximate date from a flat listing.
    pub fn set_published_at_if_missing(&self, id: &str, ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE videos SET published_at=?2,
                    sort_at = CASE WHEN added_manually=1 THEN sort_at
                                   ELSE COALESCE(sort_at, ?2) END
             WHERE id=?1 AND published_at IS NULL",
            params![id, ts],
        )?;
        Ok(())
    }

    /// The channel listing is the authority on a title -- RSS is not, however
    /// much it looks like it should be. A feed entry carries the title the
    /// video was *published* under and YouTube never revises it: `<updated>`
    /// moves, `<title>` does not. Verified on `O0jqUIRhiY0`, renamed a day
    /// after upload -- the feed, the watch page, its ld+json, oembed and
    /// `yt-dlp` on the watch URL all still served the original string, while
    /// `yt-dlp --flat-playlist` over the channel's /videos tab served the new
    /// one. A rename is therefore visible only by re-reading the listing,
    /// which `poll::refresh_from_listing` does on every poll.
    ///
    /// Batched because that listing is [`TITLE_REFRESH_LIMIT`] entries deep at
    /// its shallowest and `backfill_count` deep at its deepest: one implicit
    /// transaction per row would be one fsync per row, for a set where almost
    /// nothing has changed.
    ///
    /// All the guards live in the SQL rather than at the call site, so a poll
    /// can pass every entry it received: an id belonging to no row must be
    /// harmless (the listing sees uploads older than anything stored), an
    /// empty title from a mangled entry must not cost a card its name, and an
    /// unchanged title -- almost every entry of almost every poll -- must not
    /// rewrite the row.
    ///
    /// No `added_manually` exemption, unlike [`Db::set_published_at`]: a title
    /// carries no user intent to preserve.
    ///
    /// [`TITLE_REFRESH_LIMIT`]: crate::poll::TITLE_REFRESH_LIMIT
    pub fn set_titles(&self, titles: &[(String, String)]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare(
                "UPDATE videos SET title=?2 WHERE id=?1 AND ?2<>'' AND title<>?2")?;
            for (id, title) in titles {
                st.execute(params![id, title])?;
            }
        }
        tx.commit()?;
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
        let Some((mut sql, mut args)) = feed_query(&conn, f, SELECT_VIDEO)? else {
            return Ok(Vec::new());
        };
        args.push(Box::new(f.limit));
        sql.push_str(&format!(" LIMIT ?{}", args.len()));
        args.push(Box::new(f.offset));
        sql.push_str(&format!(" OFFSET ?{}", args.len()));

        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), map_video)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// The feed as cards, with each channel's multi-part uploads collapsed into
    /// one group. `limit` and `offset` count **groups**, not videos.
    ///
    /// Grouping cannot be done a page at a time: the parts of one series
    /// routinely sit pages apart in date order, so the walk has to see the whole
    /// filtered set before it can hand back the first card. Each group is formed
    /// by matching its leader against every ready video on that leader's channel
    /// -- the same match [`sibling_ids`] makes -- so a group's contents are by
    /// construction exactly what "Find siblings" on that leader returns.
    ///
    /// The filters therefore choose which groups appear, not what is inside
    /// them: a card says "7 parts" because there are seven, and the series stays
    /// on screen until every one of them has been filtered out. `sibling_of` is
    /// meaningless here and is ignored -- the series view is a flat list on
    /// purpose.
    pub fn list_video_groups(&self, f: &VideoFilter) -> Result<Vec<VideoGroup>> {
        let conn = self.conn.lock().unwrap();
        let seed_filter = VideoFilter { sibling_of: None, ..f.clone() };
        let Some((sql, args)) = feed_query(&conn, &seed_filter, SELECT_VIDEO_IDS)? else {
            return Ok(Vec::new());
        };

        let mut st = conn.prepare(&sql)?;
        let seeds: Vec<(String, String)> = st
            .query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Every ready video on each channel in play, bucketed once. Matching
        // against the unfiltered set is the whole point: the filterbar must not
        // be able to shorten a series, only to hide one.
        let mut index: HashMap<String, siblings::Atoms> = HashMap::new();
        let mut runtimes: HashMap<String, i64> = HashMap::new();
        for (_, channel) in &seeds {
            if index.contains_key(channel) { continue; }
            let (atoms, secs) = channel_atoms(&conn, channel)?;
            runtimes.extend(secs);
            index.insert(channel.clone(), atoms);
        }

        // Top-down: the first part to survive the filters leads its group, and
        // every part it claims is struck off so it cannot open a group of its
        // own further down the feed.
        let mut consumed: HashSet<String> = HashSet::new();
        let mut groups: Vec<Vec<String>> = Vec::new();
        for (id, channel) in &seeds {
            if consumed.contains(id) { continue; }
            let mut members: Vec<String> =
                index[channel].group_led_by(id).into_iter().map(String::from).collect();
            // A seed the atoms do not know is a row that stopped being `ready`
            // between the two queries; it still deserves its own card.
            if members.is_empty() { members.push(id.clone()); }
            for m in &members { consumed.insert(m.clone()); }
            groups.push(members);
        }

        // Two of the sort orders rank a card by something only the finished
        // group knows -- how many parts it holds, or how long they run in
        // total. Both have to be applied before the page is cut, or
        // `limit`/`offset` would page through the underlying order and then
        // reshuffle each page on its own. `sort_by_key` is stable, so a tie
        // keeps the order the feed's own sort gave it.
        //
        // `Length` is ranked here even though SQL already ordered the seeds by
        // duration: that ordering settled each group's leader and its parts'
        // order, but a series is worth the sum of its parts, which no single
        // row's `duration_secs` can express.
        match f.sort {
            SortOrder::Parts => {
                groups.sort_by_key(|members| std::cmp::Reverse(members.len()));
            }
            SortOrder::Length => {
                groups.sort_by_key(|members| {
                    let total: i64 = members.iter()
                        .map(|id| runtimes.get(id).copied().unwrap_or(0))
                        .sum();
                    std::cmp::Reverse(total)
                });
            }
            _ => {}
        }

        let page: Vec<Vec<String>> = groups
            .into_iter()
            .skip(f.offset.max(0) as usize)
            .take(f.limit.max(0) as usize)
            .collect();

        // Rows for the page, fetched in batches of whole groups: keeping a group
        // inside one ordered query is what makes its parts' order meaningful.
        let mut rows: HashMap<String, Video> = HashMap::new();
        let mut rank: HashMap<String, usize> = HashMap::new();
        let mut batch: Vec<&String> = Vec::new();
        for members in &page {
            if !batch.is_empty() && batch.len() + members.len() > HYDRATE_BATCH {
                hydrate(&conn, &batch, f.sort, &mut rows, &mut rank)?;
                batch.clear();
            }
            batch.extend(members.iter());
        }
        hydrate(&conn, &batch, f.sort, &mut rows, &mut rank)?;

        let mut out = Vec::with_capacity(page.len());
        for members in &page {
            let mut rest: Vec<&String> = members.iter().skip(1).collect();
            rest.sort_by_key(|id| rank.get(*id).copied().unwrap_or(usize::MAX));
            let mut videos: Vec<Video> = Vec::with_capacity(members.len());
            for id in std::iter::once(&members[0]).chain(rest) {
                if let Some(v) = rows.get(id) { videos.push(v.clone()); }
            }
            if videos.is_empty() { continue; }
            let stem = if videos.len() > 1 {
                let titles: Vec<&str> = videos.iter().map(|v| v.title.as_str()).collect();
                siblings::display_stem(&titles)
            } else {
                None
            };
            out.push(VideoGroup { videos, stem });
        }
        Ok(out)
    }

    /// Marks `ids` siblings by hand, and returns how many videos the finished
    /// group holds.
    ///
    /// The answer can exceed what was asked for, because the key is stamped on
    /// every row already sharing *any* key among the ids as well: dropping a
    /// card from one hand-built group onto another merges both wholesale rather
    /// than stranding half of one. The caller reports the number back, so what
    /// the user is told is what actually happened.
    ///
    /// Refused across channels. Nothing in the schema enforces that, but the
    /// grouping walk is per-channel and a card carries one channel's name, so a
    /// link spanning two would have nowhere to show.
    pub fn mark_siblings(&self, ids: &[String]) -> Result<usize> {
        let mut seen = HashSet::new();
        let ids: Vec<&String> = ids.iter().filter(|id| seen.insert(id.as_str())).collect();
        if ids.len() < 2 {
            bail!("Pick at least two videos to mark as siblings.");
        }

        let conn = self.conn.lock().unwrap();
        let mut channels: HashSet<String> = HashSet::new();
        let mut keys: Vec<String> = Vec::new();
        for id in &ids {
            let row: Option<(String, Option<String>)> = conn
                .query_row(
                    "SELECT channel_id, sibling_group FROM videos WHERE id=?1",
                    params![id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((channel, key)) = row else {
                bail!("That video is no longer in the library.");
            };
            channels.insert(channel);
            if let Some(k) = key { keys.push(k); }
        }
        if channels.len() > 1 {
            bail!("Siblings must come from the same channel.");
        }

        // Reusing a key a merge already has keeps one of the two groups' own
        // identity; a fresh one is only minted when neither side had a group.
        // A minted key cannot collide: it would take the same anchor keying two
        // different groups in one millisecond, and the anchor being in both is
        // exactly what would already have merged them.
        let key = keys.first().cloned().unwrap_or_else(|| {
            format!("{}-{}", chrono::Utc::now().timestamp_millis(), ids[0])
        });

        let mut args: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(key.clone())];
        let named = ids
            .iter()
            .map(|id| { args.push(Box::new((*id).clone())); format!("?{}", args.len()) })
            .collect::<Vec<_>>()
            .join(",");
        let merged = keys
            .iter()
            .map(|k| { args.push(Box::new(k.clone())); format!("?{}", args.len()) })
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "UPDATE videos SET sibling_group=?1
             WHERE id IN ({named}){}",
            if merged.is_empty() { String::new() } else { format!(" OR sibling_group IN ({merged})") },
        );
        conn.execute(&sql, rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())))?;

        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM videos WHERE sibling_group=?1", params![key], |r| r.get(0))?;
        Ok(n as usize)
    }

    /// Takes one video back out of its hand-built group.
    ///
    /// A key left holding a single member is cleared outright, since a group of
    /// one is not a group -- which is also what makes unlinking either half of
    /// a mistaken pair dissolve the whole thing. [`siblings::Atoms`] ignores
    /// singleton keys at read time regardless, so a video deleted out of a pair
    /// can never strand the other; this keeps the table itself honest.
    pub fn unlink_siblings(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let key: Option<Option<String>> = conn
            .query_row("SELECT sibling_group FROM videos WHERE id=?1", params![id], |r| r.get(0))
            .optional()?;
        let Some(Some(key)) = key else { return Ok(()) };

        conn.execute("UPDATE videos SET sibling_group=NULL WHERE id=?1", params![id])?;
        let left: i64 = conn.query_row(
            "SELECT COUNT(*) FROM videos WHERE sibling_group=?1", params![key], |r| r.get(0))?;
        if left < 2 {
            conn.execute("UPDATE videos SET sibling_group=NULL WHERE sibling_group=?1",
                         params![key])?;
        }
        Ok(())
    }

    pub fn set_watched(&self, id: &str, watched: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET watched=?2, watched_at=?3 WHERE id=?1",
            params![id, watched as i64, if watched { Some(now()) } else { None }])?;
        Ok(())
    }

    /// Also maintains `downloaded_at`, the Downloads tab's ordering key:
    ///
    /// - `Queued` stamps the queue time,
    /// - `Downloading` deliberately leaves it alone, so a running download keeps
    ///   its place in queue order instead of jumping ahead of what is still
    ///   waiting behind it,
    /// - `Done` and `Failed` overwrite it with the moment the attempt ended,
    /// - `None` -- a cancel -- clears it, since a row with nothing downloaded
    ///   has no place in that ordering at all.
    pub fn set_download_state(&self, id: &str, state: DownloadState,
                              file_path: Option<&str>, error: Option<&str>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE videos SET download_state=?2,
                               file_path=COALESCE(?3, file_path),
                               download_error=?4,
                               downloaded_at=CASE ?2
                                   WHEN 'none'        THEN NULL
                                   WHEN 'downloading' THEN downloaded_at
                                   ELSE ?5 END
             WHERE id=?1",
            params![id, state.as_str(), file_path, error, now()])?;
        Ok(())
    }

    pub fn clear_file_path(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET file_path=NULL, download_state='none', download_error=NULL,
                                        downloaded_at=NULL WHERE id=?1",
                     params![id])?;
        Ok(())
    }

    /// Tombstones a video so polling never surfaces it again. The row stays put,
    /// which is precisely what stops `INSERT OR IGNORE` from re-adding it.
    pub fn set_hidden(&self, id: &str, hidden: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET hidden=?2 WHERE id=?1", params![id, hidden as i64])?;
        Ok(())
    }

    /// Removes a row outright. Only safe for manually added videos: nothing polls
    /// them, so nothing can bring them back.
    pub fn delete_video(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("DELETE FROM videos WHERE id=?1", params![id])?;
        Ok(())
    }

    /// Drops an unsubscribed channel once it has no videos left, so ad-hoc
    /// uploaders do not accumulate as empty rows.
    pub fn prune_orphan_channel(&self, channel_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM channels
             WHERE id=?1 AND subscribed=0
               AND NOT EXISTS (SELECT 1 FROM videos WHERE channel_id=?1)",
            params![channel_id],
        )?;
        Ok(())
    }

    /// Video ids whose stored date is not a real upload date: either absent, or
    /// an `youtubetab:approximate_date` bucket, which lands on midnight UTC
    /// exactly (see `upload_date::is_bucketed`).
    ///
    /// Manually added rows keep whatever date they were given, so they are left
    /// out -- the same exemption `set_published_at` already makes for `sort_at`.
    pub fn video_ids_needing_real_date(&self, limit: usize) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id FROM videos
             WHERE added_manually = 0
               AND (published_at IS NULL OR published_at % 86400 = 0)
             ORDER BY feed_rank ASC
             LIMIT ?1",
        )?;
        let rows = st.query_map(params![limit as i64], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// A yt-dlp child never survives an app restart, so anything left mid-flight
    /// is reset to `none` and becomes clickable again.
    pub fn reset_stale_downloads(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET download_state='none', downloaded_at=NULL
                      WHERE download_state IN ('downloading','queued')", [])?;
        Ok(())
    }
}

const SELECT_VIDEO: &str = "
SELECT v.id, v.channel_id, c.title, v.title, v.description, v.thumb_url, v.thumb_path,
       v.published_at, v.sort_at, v.feed_rank, v.added_manually,
       v.duration_secs, v.view_count, v.status, v.hidden,
       v.watched, v.watched_at, v.download_state, v.download_error, v.file_path,
       v.downloaded_at, v.first_seen_at, v.sibling_group
FROM videos v JOIN channels c ON c.id = v.channel_id";

/// The same shape as `SELECT_VIDEO` -- the join stays, since `By channel`
/// orders on it -- but only the two columns the grouping walk reads.
const SELECT_VIDEO_IDS: &str =
    "SELECT v.id, v.channel_id FROM videos v JOIN channels c ON c.id = v.channel_id";

fn map_video(r: &Row) -> rusqlite::Result<Video> {
    Ok(Video {
        id: r.get(0)?, channel_id: r.get(1)?, channel_title: r.get(2)?, title: r.get(3)?,
        description: r.get(4)?, thumb_url: r.get(5)?, thumb_path: r.get(6)?,
        published_at: r.get(7)?, sort_at: r.get(8)?, feed_rank: r.get(9)?,
        added_manually: r.get::<_, i64>(10)? != 0,
        duration_secs: r.get(11)?, view_count: r.get(12)?,
        status: VideoStatus::parse(&r.get::<_, String>(13)?),
        hidden: r.get::<_, i64>(14)? != 0,
        watched: r.get::<_, i64>(15)? != 0, watched_at: r.get(16)?,
        download_state: DownloadState::parse(&r.get::<_, String>(17)?),
        download_error: r.get(18)?, file_path: r.get(19)?,
        downloaded_at: r.get(20)?, first_seen_at: r.get(21)?,
        sibling_group: r.get(22)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db { Db::open_in_memory().unwrap() }

    fn chan(id: &str, title: &str) -> Channel {
        Channel { id: id.into(), title: title.into(), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 0,
                  last_polled_at: None }
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

    /// Builds a channel of titled videos and returns the filter that asks for
    /// the siblings of one of them.
    fn titled(d: &Db, ch: &str, rows: &[(&str, &str)]) {
        d.upsert_channel(&chan(ch, ch)).unwrap();
        for (i, (id, title)) in rows.iter().enumerate() {
            let mut v = vid(id, ch, Some(100 + i as i64));
            v.title = (*title).into();
            d.insert_video_if_new(&v).unwrap();
        }
    }

    /// Like [`titled`], but each row carries a runtime too -- `None` for a
    /// video whose duration was never resolved.
    fn timed(d: &Db, ch: &str, rows: &[(&str, &str, Option<i64>)]) {
        d.upsert_channel(&chan(ch, ch)).unwrap();
        for (i, (id, title, secs)) in rows.iter().enumerate() {
            let mut v = vid(id, ch, Some(100 + i as i64));
            v.title = (*title).into();
            v.duration_secs = *secs;
            d.insert_video_if_new(&v).unwrap();
        }
    }

    fn siblings_of(d: &Db, id: &str) -> Vec<String> {
        let f = VideoFilter { sibling_of: Some(id.into()), ..Default::default() };
        d.list_videos(&f).unwrap().into_iter().map(|v| v.id).collect()
    }

    #[test]
    fn siblings_are_the_matching_titles_from_the_same_channel() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p7", "The Blackwood Tapes - Part 7"),
            ("other", "A Completely Different Story"),
        ]);
        let mut got = siblings_of(&d, "p7");
        got.sort();
        assert_eq!(got, vec!["p1", "p2", "p7"], "the anchor is part of its own series");
    }

    #[test]
    fn siblings_never_cross_a_channel_boundary() {
        let d = db();
        titled(&d, "UC1", &[("mine", "The Blackwood Tapes Part 1")]);
        titled(&d, "UC2", &[("theirs", "The Blackwood Tapes Part 2")]);
        assert_eq!(siblings_of(&d, "mine"), vec!["mine"]);
    }

    #[test]
    fn siblings_ignore_every_other_filter() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
            // Ignoring the filters must not mean ignoring the series.
            ("outsider", "An Unrelated Upload"),
        ]);
        d.set_watched("p1", true).unwrap();
        d.set_hidden("p2", true).unwrap();

        let f = VideoFilter {
            sibling_of: Some("p3".into()),
            // Every one of these would drop a part if it were honoured.
            hide_watched: true,
            downloaded_only: true,
            show_hidden: false,
            search: Some("nothing matches this".into()),
            channel_id: Some("UC-other".into()),
            ..Default::default()
        };
        let mut got: Vec<String> = d.list_videos(&f).unwrap().into_iter().map(|v| v.id).collect();
        got.sort();
        assert_eq!(got, vec!["p1", "p2", "p3"]);
    }

    #[test]
    fn siblings_still_follow_the_sort_order() {
        let d = db();
        titled(&d, "UC1", &[
            // Oldest of all, and no sibling: it leads the channel in this sort
            // order, so it appears here only if the filter is not applied.
            ("outsider", "An Unrelated Upload"),
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
        ]);
        let oldest = VideoFilter {
            sibling_of: Some("p3".into()), sort: SortOrder::Oldest, ..Default::default()
        };
        let ids: Vec<String> =
            d.list_videos(&oldest).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["p1", "p2", "p3"]);
    }

    #[test]
    fn an_unknown_anchor_yields_nothing() {
        let d = db();
        titled(&d, "UC1", &[("p1", "The Blackwood Tapes")]);
        assert!(siblings_of(&d, "does-not-exist").is_empty());
    }

    /// The grouped feed as `(leader, part count)` pairs, in the order the grid
    /// would lay them out.
    fn grouped(d: &Db, f: &VideoFilter) -> Vec<(String, usize)> {
        d.list_video_groups(f)
            .unwrap()
            .into_iter()
            .map(|g| (g.videos[0].id.clone(), g.videos.len()))
            .collect()
    }

    #[test]
    fn a_series_becomes_one_card_and_a_lone_video_stays_one() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("other", "A Completely Different Story"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
        ]);
        // Newest first, so the newest part leads its series -- and the unrelated
        // upload keeps the place its own date earned it.
        assert_eq!(
            grouped(&d, &VideoFilter::default()),
            vec![("p3".to_string(), 3), ("other".to_string(), 1)],
        );
    }

    #[test]
    fn a_group_carries_the_name_its_parts_share() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "The Blackwood Tapes - Part 2"),
        ]);
        let groups = d.list_video_groups(&VideoFilter::default()).unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].stem.as_deref(), Some("The Blackwood Tapes"));
    }

    #[test]
    fn a_lone_video_has_no_series_name() {
        let d = db();
        titled(&d, "UC1", &[("only", "A Completely Different Story")]);
        let groups = d.list_video_groups(&VideoFilter::default()).unwrap();
        assert_eq!(groups[0].stem, None);
    }

    #[test]
    fn filters_choose_which_groups_appear_not_what_is_in_them() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
            ("done", "An Entirely Watched Thing"),
        ]);
        for id in ["p1", "p2", "done"] {
            d.set_watched(id, true).unwrap();
        }
        let f = VideoFilter { hide_watched: true, ..Default::default() };
        // One part of the series survives the filter, and the card it leads
        // still holds all three -- while the lone watched video is gone.
        assert_eq!(grouped(&d, &f), vec![("p3".to_string(), 3)]);
    }

    #[test]
    fn a_series_vanishes_only_once_every_part_is_filtered_out() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
        ]);
        d.set_watched("p1", true).unwrap();
        d.set_watched("p2", true).unwrap();
        let f = VideoFilter { hide_watched: true, ..Default::default() };
        assert!(d.list_video_groups(&f).unwrap().is_empty());
    }

    #[test]
    fn the_leader_is_the_part_that_survived_the_filters() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
        ]);
        d.set_watched("p3", true).unwrap();
        let f = VideoFilter { hide_watched: true, ..Default::default() };
        let groups = d.list_video_groups(&f).unwrap();
        // Newest first would put p3 at the head of the series, but the card
        // wears the newest part you have *not* watched -- and opens on it, so
        // the card and the series view it leads to agree about the anchor.
        assert_eq!(groups[0].videos[0].id, "p2");
        assert_eq!(groups[0].videos.len(), 3);
    }

    #[test]
    fn a_group_holds_exactly_what_the_series_view_would_show() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
            ("other", "A Completely Different Story"),
        ]);
        let groups = d.list_video_groups(&VideoFilter::default()).unwrap();
        let mut in_card: Vec<String> = groups[0].videos.iter().map(|v| v.id.clone()).collect();
        let mut in_view = siblings_of(&d, &groups[0].videos[0].id);
        in_card.sort();
        in_view.sort();
        assert_eq!(in_card, in_view);
    }

    #[test]
    fn a_group_lists_its_parts_in_the_feeds_order() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
        ]);
        let ids = |f: &VideoFilter| -> Vec<String> {
            d.list_video_groups(f).unwrap()[0].videos.iter().map(|v| v.id.clone()).collect()
        };
        assert_eq!(ids(&VideoFilter::default()), vec!["p3", "p2", "p1"]);
        let oldest = VideoFilter { sort: SortOrder::Oldest, ..Default::default() };
        assert_eq!(ids(&oldest), vec!["p1", "p2", "p3"]);
    }

    /// A channel holding a three-part series, a two-part one that is newer, and
    /// a lone upload newer than both -- so date order and part-count order
    /// disagree about every card.
    fn mixed_series(d: &Db) {
        titled(d, "UC1", &[
            ("b1", "The Blackwood Tapes"),
            ("b2", "(2) The Blackwood Tapes"),
            ("b3", "(3) The Blackwood Tapes"),
            ("m1", "Midnight Diner"),
            ("m2", "Midnight Diner - Part 2"),
            ("lone", "A Completely Different Story"),
        ]);
    }

    #[test]
    fn parts_order_puts_the_longest_series_first() {
        let d = db();
        mixed_series(&d);
        // Newest first is the opposite order: the lone upload is the newest
        // thing on the channel and the longest series the oldest.
        assert_eq!(
            grouped(&d, &VideoFilter::default()),
            vec![("lone".to_string(), 1), ("m2".to_string(), 2), ("b3".to_string(), 3)],
        );
        let f = VideoFilter { sort: SortOrder::Parts, ..Default::default() };
        assert_eq!(
            grouped(&d, &f),
            vec![("b3".to_string(), 3), ("m2".to_string(), 2), ("lone".to_string(), 1)],
        );
    }

    #[test]
    fn series_of_equal_length_keep_their_date_order() {
        let d = db();
        titled(&d, "UC1", &[
            ("a1", "Alpha Chronicle"),
            ("a2", "Alpha Chronicle - Part 2"),
            ("z1", "Zephyr Diaries"),
            ("z2", "Zephyr Diaries - Part 2"),
        ]);
        let f = VideoFilter { sort: SortOrder::Parts, ..Default::default() };
        // The sort is stable, so a tie is broken by what the underlying feed
        // order already decided -- newest-led series first.
        assert_eq!(grouped(&d, &f), vec![("z2".to_string(), 2), ("a2".to_string(), 2)]);
    }

    #[test]
    fn parts_order_reorders_the_cards_not_what_is_inside_them() {
        let d = db();
        mixed_series(&d);
        let f = VideoFilter { sort: SortOrder::Parts, ..Default::default() };
        let groups = d.list_video_groups(&f).unwrap();
        let ids: Vec<String> = groups[0].videos.iter().map(|v| v.id.clone()).collect();
        // Only the cards are ranked by length; a series' own parts still run in
        // the feed's order, which for this sort is newest first.
        assert_eq!(ids, vec!["b3", "b2", "b1"]);
    }

    #[test]
    fn parts_order_still_pages_by_group() {
        let d = db();
        mixed_series(&d);
        let f = VideoFilter { sort: SortOrder::Parts, limit: 1, offset: 1, ..Default::default() };
        // Ranking happens before the page is cut, so page two is the second
        // longest series and not whatever date order would have put there.
        assert_eq!(grouped(&d, &f), vec![("m2".to_string(), 2)]);
    }

    #[test]
    fn the_flat_feed_can_run_longest_first() {
        let d = db();
        timed(&d, "UC1", &[
            ("short", "A Short Thing", Some(300)),
            ("long", "A Long Thing", Some(3000)),
            ("middling", "A Middling Thing", Some(1200)),
            ("unknown", "An Unresolved Thing", None),
        ]);
        let f = VideoFilter { sort: SortOrder::Length, ..Default::default() };
        let ids: Vec<String> = d.list_videos(&f).unwrap().into_iter().map(|v| v.id).collect();
        // A row whose duration never resolved cannot claim a place it has not
        // earned, so it sorts last rather than first.
        assert_eq!(ids, vec!["long", "middling", "short", "unknown"]);
    }

    #[test]
    fn a_series_is_ranked_by_its_parts_added_up() {
        let d = db();
        timed(&d, "UC1", &[
            ("b1", "The Blackwood Tapes", Some(1200)),
            ("b2", "(2) The Blackwood Tapes", Some(1200)),
            ("b3", "(3) The Blackwood Tapes", Some(1200)),
            ("solo", "One Very Long Film", Some(2700)),
        ]);
        let f = VideoFilter { sort: SortOrder::Length, ..Default::default() };
        // No single part comes near the lone film, but the series is an hour of
        // watching to its forty-five minutes -- and that is what the card ranks
        // on, since the card stands for the whole series.
        assert_eq!(grouped(&d, &f), vec![("b3".to_string(), 3), ("solo".to_string(), 1)]);
    }

    #[test]
    fn an_unresolved_part_adds_nothing_to_its_series_total() {
        let d = db();
        timed(&d, "UC1", &[
            ("a1", "Alpha Chronicle", Some(600)),
            ("a2", "Alpha Chronicle - Part 2", None),
            ("z1", "Zephyr Diaries", Some(400)),
            ("z2", "Zephyr Diaries - Part 2", Some(400)),
        ]);
        let f = VideoFilter { sort: SortOrder::Length, ..Default::default() };
        // 800 beats 600: the unknown part counts as nothing, not as something
        // large enough to carry its series to the top.
        assert_eq!(grouped(&d, &f), vec![("z2".to_string(), 2), ("a1".to_string(), 2)]);
    }

    #[test]
    fn length_order_reaches_inside_a_group_too() {
        let d = db();
        timed(&d, "UC1", &[
            ("p1", "The Blackwood Tapes", Some(600)),
            ("p2", "(2) The Blackwood Tapes", Some(1800)),
            ("p3", "(3) The Blackwood Tapes", Some(1200)),
        ]);
        let f = VideoFilter { sort: SortOrder::Length, ..Default::default() };
        let groups = d.list_video_groups(&f).unwrap();
        let ids: Vec<String> = groups[0].videos.iter().map(|v| v.id.clone()).collect();
        // The longest part leads the card and the rest follow it down, the same
        // rule the feed around the card is following.
        assert_eq!(ids, vec!["p2", "p3", "p1"]);
    }

    #[test]
    fn a_group_never_crosses_a_channel_boundary() {
        let d = db();
        titled(&d, "UC1", &[("mine", "The Blackwood Tapes Part 1")]);
        titled(&d, "UC2", &[("theirs", "The Blackwood Tapes Part 2")]);
        let got = grouped(&d, &VideoFilter::default());
        assert_eq!(got.len(), 2, "two channels, two lone cards: {got:?}");
        assert!(got.iter().all(|(_, parts)| *parts == 1));
    }

    #[test]
    fn paging_counts_groups_rather_than_videos() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
            ("solo", "A Completely Different Story"),
        ]);
        // Three of these four videos are one card, so a page of one group holds
        // three videos and the page after it holds the lone one.
        let first = VideoFilter { limit: 1, ..Default::default() };
        assert_eq!(grouped(&d, &first), vec![("solo".to_string(), 1)]);
        let second = VideoFilter { limit: 1, offset: 1, ..Default::default() };
        assert_eq!(grouped(&d, &second), vec![("p3".to_string(), 3)]);
    }

    /// Prints the grouped feed the real library would produce, so the cards can
    /// be judged against real titles rather than invented ones. Read-only in
    /// intent, but `Db::open` runs migrations, so it insists on a **copy**:
    ///
    ///     cp ~/.config/mytube/mytube.db /tmp/lib.db
    ///     MYTUBE_DB=/tmp/lib.db cargo test group_the_real_library -- --ignored --nocapture
    #[test]
    #[ignore = "reads a copy of the real library"]
    fn group_the_real_library() {
        let path = std::env::var("MYTUBE_DB")
            .expect("set MYTUBE_DB to a *copy* of the library; this opens it read-write");
        let d = Db::open(std::path::Path::new(&path)).expect("library opens");

        let f = VideoFilter { limit: 100_000, ..Default::default() };
        let started = std::time::Instant::now();
        let groups = d.list_video_groups(&f).unwrap();
        let elapsed = started.elapsed();

        let series: Vec<&VideoGroup> = groups.iter().filter(|g| g.videos.len() > 1).collect();
        for g in &series {
            println!(
                "\n[{}] {} parts  \u{2014}  {}",
                g.videos[0].channel_title,
                g.videos.len(),
                g.stem.as_deref().unwrap_or("<no shared name>"),
            );
            for v in &g.videos {
                println!("    {}", v.title);
            }
        }
        let videos: usize = groups.iter().map(|g| g.videos.len()).sum();
        println!(
            "\n=== {} cards for {videos} videos: {} series, {} lone \u{2014} built in {elapsed:?} ===",
            groups.len(),
            series.len(),
            groups.len() - series.len(),
        );
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

    /// Membership is what tells the poll whose members-only uploads it may
    /// ingest. Unlike `subscribed` it is not a one-way upgrade: a membership
    /// can be cancelled, so only `set_channel_member` may move it.
    #[test]
    fn a_membership_is_recorded_survives_a_poll_and_can_end() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        assert!(!d.list_channels().unwrap()[0].member, "nobody has joined anything yet");

        d.set_channel_member("UC1", true).unwrap();
        // Every poll upserts the channel to pick up a rename.
        d.upsert_channel(&chan("UC1", "One Renamed")).unwrap();
        let got = &d.list_channels().unwrap()[0];
        assert_eq!(got.title, "One Renamed");
        assert!(got.member, "an ordinary upsert must not cancel a membership");

        d.set_channel_member("UC1", false).unwrap();
        assert!(!d.list_channels().unwrap()[0].member);
    }

    /// v4 is what shipped before memberships existed, so it is the upgrade a
    /// real library actually takes.
    #[test]
    fn migrating_a_v4_database_adds_member_and_keeps_its_rows() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(
            "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN downloaded_at INTEGER;
             ALTER TABLE videos ADD COLUMN sibling_group TEXT;",
        ).unwrap();
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute(
            "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7)",
            [],
        ).unwrap();
        assert!(!column_exists(&conn, "channels", "member").unwrap());

        let db = Db::init(conn).unwrap();

        let c = &db.list_channels().unwrap()[0];
        assert_eq!(c.title, "One");
        assert_eq!(c.added_at, 7, "the row survived the migration");
        assert!(!c.member, "an existing channel has joined nothing");
        // And the feature works on the upgraded database.
        db.set_channel_member("UC1", true).unwrap();
        assert!(db.list_channels().unwrap()[0].member);
    }

    /// The failure mode that only shows up on a database that already has data:
    /// `CREATE TABLE IF NOT EXISTS` does nothing to an existing table, so a new
    /// column must arrive via its own ALTER step.
    #[test]
    fn migrating_a_v1_database_preserves_rows_and_adds_hidden() {
        let conn = Connection::open_in_memory().unwrap();
        // Build a genuine v1 database: original schema, stamped version 1.
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn.execute(
            "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO videos (id,channel_id,title,published_at,sort_at,status,first_seen_at)
             VALUES ('a','UC1','kept',100,100,'ready',1)",
            [],
        ).unwrap();
        assert!(!column_exists(&conn, "videos", "hidden").unwrap());

        let db = Db::init(conn).unwrap();

        let v = db.get_video("a").unwrap().expect("row survived the migration");
        assert_eq!(v.title, "kept");
        assert_eq!(v.published_at, Some(100));
        assert!(!v.hidden, "existing rows default to visible");
        assert_eq!(v.sibling_group, None, "existing rows carry no hand-built group");
        assert_eq!(db.list_channels().unwrap()[0].added_at, 7);
        assert_eq!(db.list_videos(&VideoFilter::default()).unwrap().len(), 1);
    }

    #[test]
    fn migrating_a_v3_database_adds_sibling_group_and_keeps_its_rows() {
        // The path a real library actually takes: v3 is what shipped before
        // marking siblings by hand existed.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(
            "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN downloaded_at INTEGER;",
        ).unwrap();
        conn.pragma_update(None, "user_version", 3).unwrap();
        conn.execute(
            "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO videos (id,channel_id,title,published_at,sort_at,status,first_seen_at,
                                 downloaded_at)
             VALUES ('a','UC1','kept',100,100,'ready',1,55)",
            [],
        ).unwrap();
        assert!(!column_exists(&conn, "videos", "sibling_group").unwrap());

        let db = Db::init(conn).unwrap();

        let v = db.get_video("a").unwrap().expect("row survived the migration");
        assert_eq!(v.title, "kept");
        assert_eq!(v.downloaded_at, Some(55), "the v3 column is untouched");
        assert_eq!(v.sibling_group, None, "nothing is marked until someone marks it");
        // And the feature works on the upgraded database.
        titled(&db, "UC2", &[("x", "Alpha"), ("y", "Bravo")]);
        assert_eq!(mark(&db, &["x", "y"]).unwrap(), 2);
    }

    #[test]
    fn migration_is_idempotent_and_a_fresh_db_matches_an_upgraded_one() {
        let conn = Connection::open_in_memory().unwrap();
        let db = Db::init(conn).unwrap();
        {
            let c = db.conn.lock().unwrap();
            assert!(column_exists(&c, "videos", "hidden").unwrap());
            assert!(column_exists(&c, "videos", "sibling_group").unwrap());
            assert!(column_exists(&c, "channels", "member").unwrap());
            let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, SCHEMA_VERSION);
            // Running it again must not error or duplicate anything.
            migrate(&c).unwrap();
        }
        assert_eq!(db.list_videos(&VideoFilter::default()).unwrap().len(), 0);
    }

    #[test]
    fn hidden_videos_leave_the_feed_and_come_back_with_show_hidden() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(2))).unwrap();
        d.insert_video_if_new(&vid("b", "UC1", Some(1))).unwrap();
        d.set_hidden("a", true).unwrap();

        let ids: Vec<String> = d.list_videos(&VideoFilter::default()).unwrap()
            .into_iter().map(|v| v.id).collect();
        assert_eq!(ids, vec!["b"]);

        let showing = VideoFilter { show_hidden: true, ..Default::default() };
        assert_eq!(d.list_videos(&showing).unwrap().len(), 2);

        d.set_hidden("a", false).unwrap();
        assert_eq!(d.list_videos(&VideoFilter::default()).unwrap().len(), 2);
    }

    /// The whole point of hiding rather than deleting: polling must not undo it.
    #[test]
    fn a_hidden_video_is_not_resurrected_by_a_later_poll() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(1))).unwrap();
        d.set_hidden("a", true).unwrap();
        // A later poll sees the same id again and tries to insert it.
        assert!(!d.insert_video_if_new(&vid("a", "UC1", Some(1))).unwrap());
        assert!(d.get_video("a").unwrap().unwrap().hidden, "still hidden after re-poll");
        assert!(d.list_videos(&VideoFilter::default()).unwrap().is_empty());
    }

    #[test]
    fn deleting_a_video_removes_it_and_prunes_an_orphan_adhoc_channel() {
        let d = db();
        let mut adhoc = chan("UC2", "Ad-hoc uploader");
        adhoc.subscribed = false;
        d.upsert_channel(&adhoc).unwrap();
        let mut v = vid("m", "UC2", Some(1));
        v.added_manually = true;
        d.insert_video_if_new(&v).unwrap();

        d.delete_video("m").unwrap();
        assert!(d.get_video("m").unwrap().is_none());
        d.prune_orphan_channel("UC2").unwrap();
        assert!(d.get_channel("UC2").unwrap().is_none(), "empty ad-hoc channel pruned");
    }

    #[test]
    fn pruning_never_removes_a_real_subscription() {
        let d = db();
        d.upsert_channel(&chan("UC1", "Subscribed")).unwrap();
        d.prune_orphan_channel("UC1").unwrap();
        assert!(d.get_channel("UC1").unwrap().is_some(), "subscribed channels are never pruned");
    }

    /// 2026-07-30 00:00:00 UTC -- in a real library 364 unrelated videos all
    /// carried this one `approximate_date` bucket.
    const BUCKET: i64 = 1_785_369_600;

    #[test]
    fn rows_without_a_real_upload_date_are_listed_for_repair() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();

        // A real date from RSS: carries a time of day, so it is left alone.
        d.insert_video_if_new(&vid("real", "UC1", Some(BUCKET + 43_200))).unwrap();
        // An approximate bucket: midnight exactly.
        d.insert_video_if_new(&vid("bucketed", "UC1", Some(BUCKET))).unwrap();
        // No date at all.
        let mut undated = vid("undated", "UC1", None);
        undated.sort_at = None;
        d.insert_video_if_new(&undated).unwrap();

        let mut got = d.video_ids_needing_real_date(100).unwrap();
        got.sort();
        assert_eq!(got, vec!["bucketed".to_string(), "undated".to_string()]);
    }

    #[test]
    fn a_manually_added_video_keeps_the_date_it_was_given() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        let mut manual = vid("manual", "UC1", Some(BUCKET));
        manual.added_manually = true;
        d.insert_video_if_new(&manual).unwrap();
        assert!(d.video_ids_needing_real_date(100).unwrap().is_empty());
    }

    #[test]
    fn the_repair_batch_is_bounded() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        for n in 0..5 {
            d.insert_video_if_new(&vid(&format!("v{n}"), "UC1", Some(BUCKET))).unwrap();
        }
        assert_eq!(d.video_ids_needing_real_date(3).unwrap().len(), 3);
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
    fn a_changed_listing_title_replaces_the_stored_one() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap();
        d.set_titles(&[("a".into(), "The Retitled Cut".into())]).unwrap();
        assert_eq!(d.get_video("a").unwrap().unwrap().title, "The Retitled Cut");
    }

    #[test]
    fn an_empty_title_never_blanks_a_row() {
        // A mangled listing entry must not cost a card its name.
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap();
        d.set_titles(&[("a".into(), String::new())]).unwrap();
        assert_eq!(d.get_video("a").unwrap().unwrap().title, "t-a");
    }

    #[test]
    fn retitling_only_touches_the_named_video() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap();
        d.insert_video_if_new(&vid("b", "UC1", Some(200))).unwrap();
        d.set_titles(&[("a".into(), "Only Mine".into())]).unwrap();
        assert_eq!(d.get_video("b").unwrap().unwrap().title, "t-b");
    }

    #[test]
    fn a_manually_added_video_is_retitled_too() {
        // Unlike `sort_at`, a title carries no user intent -- it is YouTube's
        // string either way, so `added_manually` earns no exemption here.
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        let mut manual = vid("m", "UC1", Some(10));
        manual.added_manually = true;
        d.insert_video_if_new(&manual).unwrap();
        d.set_titles(&[("m".into(), "Renamed By Its Uploader".into())]).unwrap();
        assert_eq!(d.get_video("m").unwrap().unwrap().title, "Renamed By Its Uploader");
    }

    #[test]
    fn a_batch_retitles_only_what_actually_changed() {
        // The listing hands back every entry it saw: mostly unchanged titles,
        // and ids for videos this library never stored (Shorts, or uploads
        // older than the rows we keep).
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap();
        d.insert_video_if_new(&vid("b", "UC1", Some(200))).unwrap();
        d.set_titles(&[
            ("a".into(), "The Retitled Cut".into()),
            ("b".into(), "t-b".into()),
            ("never-seen".into(), "Belongs To No Row".into()),
        ])
        .unwrap();
        assert_eq!(d.get_video("a").unwrap().unwrap().title, "The Retitled Cut");
        assert_eq!(d.get_video("b").unwrap().unwrap().title, "t-b");
        assert!(d.get_video("never-seen").unwrap().is_none());
    }

    #[test]
    fn an_empty_batch_is_not_an_error() {
        // Every poll of a channel yt-dlp could not list ends up here.
        let d = db();
        d.set_titles(&[]).unwrap();
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

    /// Backdates a fetch stamp. `now()` has one-second resolution, which is far
    /// too coarse to order two calls made in the same test.
    fn stamp(d: &Db, id: &str, at: i64) {
        d.conn.lock().unwrap()
            .execute("UPDATE videos SET downloaded_at=?2 WHERE id=?1", params![id, at])
            .unwrap();
    }

    /// The whole point of the Downloads tab's ordering: a video uploaded years
    /// ago but fetched this morning belongs above one uploaded last week and
    /// fetched a month back.
    #[test]
    fn downloads_order_by_when_they_were_fetched_not_when_released() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("old_upload", "UC1", Some(1_000))).unwrap();
        d.insert_video_if_new(&vid("new_upload", "UC1", Some(9_000))).unwrap();
        d.set_download_state("old_upload", DownloadState::Done, Some("/tmp/a.mkv"), None).unwrap();
        d.set_download_state("new_upload", DownloadState::Done, Some("/tmp/b.mkv"), None).unwrap();
        stamp(&d, "old_upload", 500);
        stamp(&d, "new_upload", 400);

        let by_release = VideoFilter {
            downloaded_only: true, sort: SortOrder::Newest, ..Default::default()
        };
        let ids: Vec<String> =
            d.list_videos(&by_release).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["new_upload", "old_upload"], "release order is left alone");

        let by_fetch = VideoFilter { sort: SortOrder::Downloaded, ..by_release };
        let ids: Vec<String> =
            d.list_videos(&by_fetch).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["old_upload", "new_upload"], "the most recent fetch comes first");
    }

    #[test]
    fn a_download_with_no_recorded_fetch_time_sorts_last() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("dated", "UC1", Some(1_000))).unwrap();
        d.insert_video_if_new(&vid("undated", "UC1", Some(9_000))).unwrap();
        d.set_download_state("dated", DownloadState::Done, Some("/tmp/a.mkv"), None).unwrap();
        d.set_download_state("undated", DownloadState::Done, Some("/tmp/b.mkv"), None).unwrap();
        d.conn.lock().unwrap()
            .execute("UPDATE videos SET downloaded_at=NULL WHERE id='undated'", []).unwrap();

        let f = VideoFilter {
            downloaded_only: true, sort: SortOrder::Downloaded, ..Default::default()
        };
        let ids: Vec<String> = d.list_videos(&f).unwrap().into_iter().map(|v| v.id).collect();
        assert_eq!(ids, ["dated", "undated"],
                   "the newer upload still sorts last while it has no fetch time");
    }

    #[test]
    fn a_running_download_keeps_its_queue_position() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(100))).unwrap();

        d.set_download_state("a", DownloadState::Queued, None, None).unwrap();
        stamp(&d, "a", 42);
        d.set_download_state("a", DownloadState::Downloading, None, None).unwrap();
        assert_eq!(d.get_video("a").unwrap().unwrap().downloaded_at, Some(42),
                   "starting a download must not push it past what is still waiting");

        d.set_download_state("a", DownloadState::Done, Some("/tmp/a.mkv"), None).unwrap();
        assert!(d.get_video("a").unwrap().unwrap().downloaded_at.unwrap() > 42,
                "finishing records when the file actually landed");
    }

    #[test]
    fn dropping_a_download_clears_its_fetch_time() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        for id in ["cancelled", "deleted", "stale"] {
            d.insert_video_if_new(&vid(id, "UC1", Some(100))).unwrap();
            d.set_download_state(id, DownloadState::Queued, None, None).unwrap();
            assert!(d.get_video(id).unwrap().unwrap().downloaded_at.is_some());
        }
        d.set_download_state("deleted", DownloadState::Done, Some("/tmp/d.mkv"), None).unwrap();

        d.set_download_state("cancelled", DownloadState::None, None, None).unwrap();
        d.clear_file_path("deleted").unwrap();
        d.reset_stale_downloads().unwrap();

        for id in ["cancelled", "deleted", "stale"] {
            assert_eq!(d.get_video(id).unwrap().unwrap().downloaded_at, None,
                       "{id} has no file, so it has no place in the fetch ordering");
        }
    }

    /// The v3 migration dates pre-existing downloads from their files. Without
    /// it an upgraded library opens with every completed download back in
    /// release order -- the ordering the column exists to replace.
    #[test]
    fn the_migration_dates_existing_downloads_from_their_files() {
        let dir = std::env::temp_dir().join("mytube-migration-backfill-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("kept.mkv");
        std::fs::write(&file, b"x").unwrap();

        // A database frozen at v2, before `downloaded_at` existed.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute("ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0", [])
            .unwrap();
        conn.pragma_update(None, "user_version", 2i64).unwrap();
        conn.execute(
            "INSERT INTO channels (id,title,url,added_at) VALUES ('UC1','One','u',0)", [])
            .unwrap();
        conn.execute(
            "INSERT INTO videos (id,channel_id,title,status,first_seen_at,download_state,file_path)
             VALUES ('kept','UC1','k','ready',0,'done',?1),
                    ('gone','UC1','g','ready',0,'done','/nonexistent/x.mkv')",
            params![file.to_str().unwrap()]).unwrap();

        migrate(&conn).unwrap();

        let mtime = std::fs::metadata(&file).unwrap().modified().unwrap()
            .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        let kept: Option<i64> = conn
            .query_row("SELECT downloaded_at FROM videos WHERE id='kept'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(kept, Some(mtime), "an existing file is dated from its mtime");

        let gone: Option<i64> = conn
            .query_row("SELECT downloaded_at FROM videos WHERE id='gone'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(gone, None, "a file that has since moved away stays undated");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    /* ---------------- marking siblings by hand ---------------- */

    fn mark(d: &Db, ids: &[&str]) -> Result<usize> {
        d.mark_siblings(&ids.iter().map(|s| (*s).to_string()).collect::<Vec<_>>())
    }

    fn key_of(d: &Db, id: &str) -> Option<String> {
        d.get_video(id).unwrap().unwrap().sibling_group
    }

    #[test]
    fn marking_joins_videos_no_matcher_could_have() {
        // The case the feature exists for: a follow-up renamed to chase the
        // algorithm shares nothing with the part before it.
        let d = db();
        titled(&d, "UC1", &[("p3", "The Blackwood Tapes Pt 3"), ("ec", "EVERYTHING CHANGED")]);
        assert_eq!(siblings_of(&d, "p3"), vec!["p3"], "unmarked, they are strangers");

        assert_eq!(mark(&d, &["p3", "ec"]).unwrap(), 2);
        let mut got = siblings_of(&d, "p3");
        got.sort();
        assert_eq!(got, vec!["ec", "p3"]);
        assert_eq!(key_of(&d, "p3"), key_of(&d, "ec"));
    }

    #[test]
    fn a_marked_title_then_seeds_the_matcher_on_its_own() {
        // The whole point of recording the new name: the *next* upload under it
        // needs no marking at all.
        let d = db();
        titled(&d, "UC1", &[
            ("p3", "The Blackwood Tapes Pt 3"),
            ("ec", "EVERYTHING CHANGED"),
            ("ec2", "EVERYTHING CHANGED Part 2"),
        ]);
        mark(&d, &["p3", "ec"]).unwrap();

        let mut got = siblings_of(&d, "p3");
        got.sort();
        assert_eq!(got, vec!["ec", "ec2", "p3"]);
    }

    #[test]
    fn marking_merges_two_hand_built_groups() {
        // Neither group may be stranded: a link into a group is a link into all
        // of it.
        let d = db();
        titled(&d, "UC1", &[
            ("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie"), ("e", "Echo"),
        ]);
        mark(&d, &["a", "b"]).unwrap();
        mark(&d, &["c", "e"]).unwrap();
        assert_ne!(key_of(&d, "a"), key_of(&d, "c"));

        assert_eq!(mark(&d, &["b", "c"]).unwrap(), 4, "all four end up together");
        let mut got = siblings_of(&d, "a");
        got.sort();
        assert_eq!(got, vec!["a", "b", "c", "e"]);
    }

    #[test]
    fn marking_is_refused_across_channels() {
        let d = db();
        titled(&d, "UC1", &[("a", "Alpha")]);
        titled(&d, "UC2", &[("b", "Bravo")]);
        let err = mark(&d, &["a", "b"]).unwrap_err().to_string();
        assert!(err.contains("same channel"), "{err}");
        assert_eq!(key_of(&d, "a"), None);
    }

    #[test]
    fn marking_needs_two_distinct_videos_that_exist() {
        let d = db();
        titled(&d, "UC1", &[("a", "Alpha")]);
        assert!(mark(&d, &["a"]).is_err());
        assert!(mark(&d, &["a", "a"]).is_err(), "the same card twice is one video");
        assert!(mark(&d, &["a", "gone"]).is_err());
        assert_eq!(key_of(&d, "a"), None, "a refused mark writes nothing");
    }

    #[test]
    fn unlinking_dissolves_a_pair_from_either_side() {
        // A group of one is not a group, so undoing a mis-drop takes one action
        // rather than two.
        let d = db();
        titled(&d, "UC1", &[("a", "Alpha"), ("b", "Bravo")]);
        mark(&d, &["a", "b"]).unwrap();

        d.unlink_siblings("b").unwrap();
        assert_eq!(key_of(&d, "a"), None, "the survivor is not left pinned to a group of one");
        assert_eq!(key_of(&d, "b"), None);
        assert_eq!(siblings_of(&d, "a"), vec!["a"]);
    }

    #[test]
    fn unlinking_leaves_a_larger_group_standing() {
        let d = db();
        titled(&d, "UC1", &[("a", "Alpha"), ("b", "Bravo"), ("c", "Charlie")]);
        mark(&d, &["a", "b", "c"]).unwrap();

        d.unlink_siblings("c").unwrap();
        assert_eq!(key_of(&d, "c"), None);
        let mut got = siblings_of(&d, "a");
        got.sort();
        assert_eq!(got, vec!["a", "b"]);
    }

    #[test]
    fn unlinking_an_unmarked_video_is_a_no_op() {
        let d = db();
        titled(&d, "UC1", &[("a", "Alpha")]);
        d.unlink_siblings("a").unwrap();
        d.unlink_siblings("gone").unwrap();
    }

    #[test]
    fn a_hand_marked_pair_is_one_card_in_the_grouped_feed() {
        let d = db();
        titled(&d, "UC1", &[
            ("p3", "The Blackwood Tapes Pt 3"),
            ("ec", "EVERYTHING CHANGED"),
            ("other", "Nothing To Do With It"),
        ]);
        assert_eq!(grouped(&d, &VideoFilter::default()).len(), 3, "three cards before marking");

        mark(&d, &["p3", "ec"]).unwrap();
        let cards = grouped(&d, &VideoFilter::default());
        assert_eq!(cards.len(), 2);
        // "other" is newest, so it leads; the pair follows behind its own leader.
        assert!(cards.contains(&("other".to_string(), 1)));
        assert!(cards.iter().any(|(_, n)| *n == 2), "{cards:?}");
    }

    #[test]
    fn a_marked_group_survives_a_filter_that_hides_part_of_it() {
        // The filters choose which cards appear, never what is inside one --
        // the same promise the title matcher already makes.
        let d = db();
        titled(&d, "UC1", &[("a", "Alpha"), ("b", "Bravo")]);
        mark(&d, &["a", "b"]).unwrap();
        d.set_watched("b", true).unwrap();

        let f = VideoFilter { hide_watched: true, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("a".to_string(), 2)], "the watched part stays inside");
    }

    #[test]
    fn find_siblings_still_returns_exactly_what_the_card_holds() {
        // The invariant CLAUDE.md records, now that a manual mark can widen a
        // group: opening a card must show precisely the card's own parts.
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "The Blackwood Tapes Part 2"),
            ("ec", "EVERYTHING CHANGED"),
            ("ec2", "EVERYTHING CHANGED Part 2"),
        ]);
        mark(&d, &["p2", "ec"]).unwrap();

        let cards = d.list_video_groups(&VideoFilter::default()).unwrap();
        for card in &cards {
            let mut inside: Vec<String> = card.videos.iter().map(|v| v.id.clone()).collect();
            let mut opened = siblings_of(&d, &card.videos[0].id);
            inside.sort();
            opened.sort();
            assert_eq!(inside, opened, "leader {}", card.videos[0].id);
        }
        assert_eq!(cards.len(), 1, "all four hang together off the marked pair");
    }

}
