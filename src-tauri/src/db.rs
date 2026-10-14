use anyhow::{bail, Result};
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection, OptionalExtension, Row};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Mutex;

use crate::models::*;
use crate::quality::Quality;
use crate::search;
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
const SCHEMA_VERSION: i64 = 12;

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

    // How deep each channel's back catalogue has been read. NULL on every row
    // that predates the column: nobody recorded how far those were backfilled,
    // so the next poll reads each one to the current `backfill_count` once.
    // That costs a listing per channel and a watch page per video it did not
    // have -- nothing for a video it did -- and it is the only way a raised
    // setting could reach channels already subscribed before this existed.
    if version < 6 {
        if !column_exists(conn, "channels", "backfill_depth")? {
            conn.execute("ALTER TABLE channels ADD COLUMN backfill_depth INTEGER", [])?;
        }
    }

    // When YouTube was seen to have terminated the channel's account. NULL,
    // which is every row, is a live channel. See `Db::set_channel_terminated`.
    if version < 7 {
        if !column_exists(conn, "channels", "terminated_at")? {
            conn.execute("ALTER TABLE channels ADD COLUMN terminated_at INTEGER", [])?;
        }
    }

    // Auto-download. `auto_download_since` is NULL (off) or the instant it was
    // switched on, which is what tells a new upload from a backlog the user
    // did not ask for. `auto_queued` marks a row the sweep has already queued
    // once, so a failure or a deleted file never brings it back on its own.
    if version < 8 {
        if !column_exists(conn, "channels", "auto_download_since")? {
            conn.execute("ALTER TABLE channels ADD COLUMN auto_download_since INTEGER", [])?;
        }
        if !column_exists(conn, "videos", "auto_queued")? {
            conn.execute(
                "ALTER TABLE videos ADD COLUMN auto_queued INTEGER NOT NULL DEFAULT 0",
                [],
            )?;
        }
    }

    // A one-off quality for one video's download, as `Quality` JSON; NULL,
    // which is nearly every row, means "whatever Settings says when the job
    // starts". It lives while the row is queued, downloading, failed or done
    // -- so a Retry repeats a custom download exactly and resumes its part
    // files -- and is cleared whenever the row goes back to `none`.
    if version < 9 {
        if !column_exists(conn, "videos", "download_quality")? {
            conn.execute("ALTER TABLE videos ADD COLUMN download_quality TEXT", [])?;
        }
    }

    // Cloud sync. Each stamp is when its value last changed (unix seconds),
    // which is what lets two machines settle a disagreement by last writer
    // wins; NULL is a value nobody ever set. A tombstone is when a
    // subscription or a manual video was removed, so a removal can travel
    // and a stale copy elsewhere cannot bring it back. See `sync::snapshot`.
    if version < 10 {
        for (table, column) in [
            ("videos", "watched_changed_at"),
            ("videos", "hidden_changed_at"),
            ("videos", "sibling_changed_at"),
            ("channels", "subscribed_changed_at"),
            ("channels", "member_changed_at"),
        ] {
            if !column_exists(conn, table, column)? {
                conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} INTEGER"), [])?;
            }
        }
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS tombstones (
               kind       TEXT NOT NULL,
               id         TEXT NOT NULL,
               removed_at INTEGER NOT NULL,
               PRIMARY KEY (kind, id)
             );",
        )?;
        seed_sync_stamps(conn)?;
        conn.execute_batch(SYNC_TRIGGERS)?;
    }

    // A series hidden as a whole. `hidden` says "this video is out of the
    // feed"; `series_hidden` says "and so is every part that joins its card
    // later" -- the one bit that tells hiding a bad episode from hiding the
    // show. A series has no row of its own (a card is recomputed on every
    // fetch, led by whichever part comes first), so the mark lives on its
    // parts, and a new part that joins the card of a marked one is hidden and
    // marked in turn, which is what lets it follow a series whose titles
    // drift. Stamped by trigger like the v10 columns, so it syncs. Nothing to
    // seed: every existing row is 0.
    if version < 11 {
        for column in ["series_hidden INTEGER NOT NULL DEFAULT 0", "series_hidden_changed_at INTEGER"] {
            let name = column.split_whitespace().next().unwrap();
            if !column_exists(conn, "videos", name)? {
                conn.execute(&format!("ALTER TABLE videos ADD COLUMN {column}"), [])?;
            }
        }
        conn.execute_batch(SERIES_HIDDEN_TRIGGERS)?;
    }

    // v12: every name a video has carried besides its current one, so a
    // clickbait rename cannot break the series its old name matched -- see
    // `VIDEO_TITLES`. Nothing to seed: no former name was ever kept before.
    if version < 12 {
        conn.execute_batch(VIDEO_TITLES)?;
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

/// Stamps what a library already held when sync arrived, and only that.
///
/// A default value -- unwatched, visible, unmarked, not a member -- stays
/// unstamped: every machine has it without anyone having chosen it, and a
/// stamp would let it overrule a real choice made elsewhere. A set value must
/// be stamped or it could never travel. Nobody recorded when most of these
/// happened, so they are dated by the upgrade, which only has to beat a NULL;
/// a watch has its own instant and a subscription its `added_at`, and both are
/// truer than that.
fn seed_sync_stamps(conn: &Connection) -> Result<()> {
    let now = now();
    for sql in [
        "UPDATE videos SET watched_changed_at = COALESCE(watched_at, ?1)
          WHERE watched <> 0 AND watched_changed_at IS NULL",
        "UPDATE videos SET hidden_changed_at = ?1
          WHERE hidden <> 0 AND hidden_changed_at IS NULL",
        "UPDATE videos SET sibling_changed_at = ?1
          WHERE sibling_group IS NOT NULL AND sibling_changed_at IS NULL",
        "UPDATE channels SET member_changed_at = ?1
          WHERE member <> 0 AND member_changed_at IS NULL",
    ] {
        conn.execute(sql, params![now])?;
    }
    conn.execute(
        "UPDATE channels SET subscribed_changed_at = added_at
          WHERE subscribed <> 0 AND subscribed_changed_at IS NULL",
        [],
    )?;
    Ok(())
}

/// Sync's stamps and tombstones, kept by the database itself so that every
/// write path keeps them -- the UI's commands, a transfer import, the CLI, the
/// Python scripts in `scripts/` -- without one mutator having to remember to.
/// Built-in SQL functions only: the scripts open this file without what
/// `register_search` adds, and SQLite refuses a schema it cannot compile.
///
/// - **A changed value is stamped now**, unless the same write moved the stamp
///   itself. That exception is `Db::apply_sync`, which copies another
///   machine's stamp verbatim and must never have it replaced by this clock.
/// - **An insert carrying a set value is stamped** -- a watch by its own
///   instant when it has one -- so an import's rows travel like any other.
/// - **Deleting a subscription or a manual video leaves a tombstone.** A
///   tombstone covers every copy stamped at or before it. When the row deleted
///   is one an existing tombstone already covers, which is how an apply
///   deletes (the remote tombstone landed first), the tombstone keeps its
///   time. When the row outlived it -- subscribed again, added again by hand --
///   this is a new removal and the tombstone moves on to now: left where it
///   was, it would not cover the comeback, and the second removal would never
///   reach a machine that holds it. An ad-hoc uploader leaves none, since it
///   was never a subscription, and neither does a polled video, which is the
///   poll's to bring back.
const SYNC_TRIGGERS: &str = "
CREATE TRIGGER IF NOT EXISTS sync_watched_changed AFTER UPDATE OF watched ON videos
WHEN NEW.watched IS NOT OLD.watched AND NEW.watched_changed_at IS OLD.watched_changed_at
BEGIN
  UPDATE videos SET watched_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_hidden_changed AFTER UPDATE OF hidden ON videos
WHEN NEW.hidden IS NOT OLD.hidden AND NEW.hidden_changed_at IS OLD.hidden_changed_at
BEGIN
  UPDATE videos SET hidden_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_sibling_changed AFTER UPDATE OF sibling_group ON videos
WHEN NEW.sibling_group IS NOT OLD.sibling_group
 AND NEW.sibling_changed_at IS OLD.sibling_changed_at
BEGIN
  UPDATE videos SET sibling_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_subscribed_changed AFTER UPDATE OF subscribed ON channels
WHEN NEW.subscribed IS NOT OLD.subscribed
 AND NEW.subscribed_changed_at IS OLD.subscribed_changed_at
BEGIN
  UPDATE channels SET subscribed_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_member_changed AFTER UPDATE OF member ON channels
WHEN NEW.member IS NOT OLD.member AND NEW.member_changed_at IS OLD.member_changed_at
BEGIN
  UPDATE channels SET member_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;

CREATE TRIGGER IF NOT EXISTS sync_watched_inserted AFTER INSERT ON videos
WHEN NEW.watched <> 0 AND NEW.watched_changed_at IS NULL
BEGIN
  UPDATE videos
     SET watched_changed_at = COALESCE(NEW.watched_at, CAST(strftime('%s','now') AS INTEGER))
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_hidden_inserted AFTER INSERT ON videos
WHEN NEW.hidden <> 0 AND NEW.hidden_changed_at IS NULL
BEGIN
  UPDATE videos SET hidden_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_sibling_inserted AFTER INSERT ON videos
WHEN NEW.sibling_group IS NOT NULL AND NEW.sibling_changed_at IS NULL
BEGIN
  UPDATE videos SET sibling_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_subscribed_inserted AFTER INSERT ON channels
WHEN NEW.subscribed <> 0 AND NEW.subscribed_changed_at IS NULL
BEGIN
  UPDATE channels SET subscribed_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_member_inserted AFTER INSERT ON channels
WHEN NEW.member <> 0 AND NEW.member_changed_at IS NULL
BEGIN
  UPDATE channels SET member_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;

CREATE TRIGGER IF NOT EXISTS sync_channel_removed AFTER DELETE ON channels
WHEN OLD.subscribed <> 0
BEGIN
  INSERT INTO tombstones (kind, id, removed_at)
  VALUES ('channel', OLD.id, CAST(strftime('%s','now') AS INTEGER))
  ON CONFLICT (kind, id) DO UPDATE
     SET removed_at = MAX(tombstones.removed_at, excluded.removed_at)
   WHERE tombstones.removed_at < COALESCE(OLD.subscribed_changed_at, 0);
END;
CREATE TRIGGER IF NOT EXISTS sync_video_removed AFTER DELETE ON videos
WHEN OLD.added_manually <> 0
BEGIN
  INSERT INTO tombstones (kind, id, removed_at)
  VALUES ('video', OLD.id, CAST(strftime('%s','now') AS INTEGER))
  ON CONFLICT (kind, id) DO UPDATE
     SET removed_at = MAX(tombstones.removed_at, excluded.removed_at)
   WHERE tombstones.removed_at < OLD.first_seen_at;
END;
";

/// The v11 stamp, kept exactly like the v10 ones in `SYNC_TRIGGERS`: a changed
/// value is stamped now unless the same write moved the stamp (an apply), and
/// an insert carrying a set value is stamped. Its own constant because the v10
/// step runs before the column exists.
const SERIES_HIDDEN_TRIGGERS: &str = "
CREATE TRIGGER IF NOT EXISTS sync_series_hidden_changed AFTER UPDATE OF series_hidden ON videos
WHEN NEW.series_hidden IS NOT OLD.series_hidden
 AND NEW.series_hidden_changed_at IS OLD.series_hidden_changed_at
BEGIN
  UPDATE videos SET series_hidden_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
CREATE TRIGGER IF NOT EXISTS sync_series_hidden_inserted AFTER INSERT ON videos
WHEN NEW.series_hidden <> 0 AND NEW.series_hidden_changed_at IS NULL
BEGIN
  UPDATE videos SET series_hidden_changed_at = CAST(strftime('%s','now') AS INTEGER)
   WHERE rowid = NEW.rowid;
END;
";

/// The names a video carried before its current one (v12).
///
/// Series matching runs on titles alone and is recomputed on every fetch, so
/// a YouTuber retitling a part to chase the algorithm used to drop it off a
/// card it had sat on for weeks. A video never forgets a name instead: the
/// matcher takes every name here as well as the current title, so the old
/// name keeps the bond and the new one widens it (see [`siblings::Atoms`]).
///
/// A trigger rather than the mutators, like the sync stamps: every path that
/// retitles a row -- the poll's `set_titles`, a transfer Replace, `scripts/`
/// -- records the old name without knowing this table exists. The current
/// title is never also a former one; a title flipping back (YouTube's title
/// A/B test serves different variants to different reads) moves between the
/// two rather than piling up. An empty title is a mangled entry, not a name.
/// The rows go with their video (`ON DELETE CASCADE`; `foreign_keys` is ON).
const VIDEO_TITLES: &str = "
CREATE TABLE IF NOT EXISTS video_titles (
  video_id TEXT NOT NULL REFERENCES videos(id) ON DELETE CASCADE,
  title    TEXT NOT NULL,
  PRIMARY KEY (video_id, title)
) WITHOUT ROWID;
CREATE TRIGGER IF NOT EXISTS video_title_kept AFTER UPDATE OF title ON videos
WHEN NEW.title IS NOT OLD.title
BEGIN
  INSERT OR IGNORE INTO video_titles (video_id, title)
    SELECT OLD.id, OLD.title WHERE OLD.title <> '';
  DELETE FROM video_titles WHERE video_id = NEW.id AND title = NEW.title;
END;
";

fn now() -> i64 { chrono::Utc::now().timestamp() }

/// `search_match(query, title, channel)`: the feed's search box, as a
/// [`search::Query`]. A function rather than LIKE because the box takes a small
/// language of its own -- regex, exclusion, phrases -- and it stays inside the
/// SQL so that `LIMIT`/`OFFSET` and the grouping walk still count only the rows
/// it keeps. The parse is held as SQLite auxiliary data on the first argument,
/// which is a bound parameter, so it runs once per statement rather than once
/// per row.
///
/// It exists only on connections this process opened. Nothing persistent -- a
/// view, an index, a trigger -- may call it: the scripts in `scripts/` open the
/// same file without it, and SQLite would refuse them the schema.
fn register_search(conn: &Connection) -> rusqlite::Result<()> {
    type BoxError = Box<dyn std::error::Error + Send + Sync>;
    conn.create_scalar_function(
        "search_match",
        3,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let query = ctx.get_or_create_aux(0, |v| -> std::result::Result<_, BoxError> {
                Ok(search::Query::parse(v.as_str()?))
            })?;
            let title = ctx.get_raw(1).as_str().unwrap_or("");
            let channel = ctx.get_raw(2).as_str().unwrap_or("");
            Ok(query.matches(title, channel))
        },
    )
}

/// Every ready video on a channel as the grouping rule sees it, plus what each
/// row contributes to its card as a whole.
///
/// Matching runs in Rust rather than in SQL because it is fuzzy, and because a
/// hand-marked group is a set of seed titles rather than a single stem (see
/// [`crate::siblings::Atoms`]); one channel's titles are few enough that
/// loading them to compare costs nothing. The runtimes ride along on the same
/// query because [`SortOrder::Length`] has to total a group up while all it
/// holds is ids, and the rows are already in hand. `watched` rides along for the
/// same reason: "Continue watching" asks it of every part of a group, hidden and
/// filtered-out parts included.
///
/// Only the reading happens here. Building [`siblings::Atoms`] from the entries
/// -- a regex pass over every title, then a match per candidate pair -- is left
/// to the caller, which does it after the connection lock is released: that
/// work is the bulk of a grouped fetch, and nothing else can touch the database
/// while the lock is held.
fn channel_rows(
    conn: &Connection,
    channel_id: &str,
) -> Result<(Vec<siblings::Entry>, HashMap<String, PartFacts>)> {
    let mut former = former_titles(conn, channel_id)?;
    let mut st = conn.prepare_cached(
        "SELECT id, title, duration_secs, sibling_group, watched FROM videos
         WHERE channel_id=?1 AND status='ready'",
    )?;
    let rows = st.query_map(params![channel_id], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
            r.get::<_, Option<String>>(3)?,
            r.get::<_, bool>(4)?,
        ))
    })?;

    let mut entries = Vec::new();
    let mut facts = HashMap::new();
    for row in rows {
        let (id, title, secs, group, watched) = row?;
        // A row whose duration has not resolved yet contributes nothing to its
        // series rather than dropping the series out of the ordering.
        facts.insert(id.clone(), PartFacts { secs: secs.unwrap_or(0), watched });
        let former_titles = former.remove(&id).unwrap_or_default();
        entries.push(siblings::Entry { id, title, former_titles, group });
    }
    Ok((entries, facts))
}

/// Every name each video on a channel carried before its current title
/// (`video_titles`), by video id. Both readers of a channel for the grouping
/// rule -- [`channel_rows`] and [`Db::series_entries`] -- take their names from
/// here, so a card and a series hide can never disagree about what a video was
/// once called.
///
/// One query per channel rather than one per video: nearly every video has no
/// former name at all, and is simply absent from the map. A channel's rows
/// that the caller does not read (one not `ready`, for [`channel_rows`]) cost
/// nothing but a map entry nobody asks for. Each video's names come back in
/// title order -- the table keeps no other -- so the same rows always make the
/// same entries.
fn former_titles(conn: &Connection, channel_id: &str) -> Result<HashMap<String, Vec<String>>> {
    let mut st = conn.prepare_cached(
        "SELECT t.video_id, t.title FROM video_titles t
           JOIN videos v ON v.id = t.video_id
          WHERE v.channel_id = ?1
          ORDER BY t.video_id, t.title",
    )?;
    let rows = st.query_map(params![channel_id], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    })?;
    let mut out: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let (id, title) = row?;
        out.entry(id).or_default().push(title);
    }
    Ok(out)
}

/// What the grouped walk needs to know about one part once its card is built,
/// beyond the id: its share of the card's runtime, and whether it was watched.
#[derive(Clone, Copy)]
struct PartFacts {
    secs: i64,
    watched: bool,
}

/// The feed query, ordered but unpaged: `list_videos` bolts `LIMIT`/`OFFSET`
/// onto it, `list_video_groups` walks all of it. `select` decides how much of
/// each row comes back -- the grouping walk needs ids alone, and reading
/// twenty-two columns of a whole library only to drop them would be waste.
///
/// `series` is what [`Db::sibling_ids`] returned for `f.sibling_of`, worked out
/// by the caller before it took the lock this query runs under: the matching is
/// the expensive half, and the lock is not reentrant anyway. It is ignored when
/// `sibling_of` is `None`.
///
/// `None` means the filter selects nothing at all, which is not the same as a
/// query that returns no rows: an anchor with no siblings has no `IN ()` that
/// SQLite would accept.
fn feed_query(
    f: &VideoFilter,
    select: &str,
    series: Vec<String>,
) -> Option<(String, Vec<Box<dyn rusqlite::ToSql>>)> {
    let mut sql = format!("{select} WHERE v.status='ready'");
    let mut args: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if f.sibling_of.is_some() {
        // Showing a series means showing all of it, so none of the ordinary
        // filters apply here -- a part you have watched, hidden or not yet
        // downloaded is still a part.
        if series.is_empty() { return None; }
        let holes = series
            .into_iter()
            .map(|id| { args.push(Box::new(id)); format!("?{}", args.len()) })
            .collect::<Vec<_>>()
            .join(",");
        sql.push_str(&format!(" AND v.id IN ({holes})"));
    } else {
        if !f.channel_ids.is_empty() {
            let holes = f.channel_ids
                .iter()
                .map(|id| { args.push(Box::new(id.clone())); format!("?{}", args.len()) })
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND v.channel_id IN ({holes})"));
        }
        if !f.show_hidden { sql.push_str(" AND v.hidden=0"); }
        if f.hide_watched { sql.push_str(" AND v.watched=0"); }
        if f.downloaded_only { sql.push_str(" AND v.download_state='done'"); }
        if let Some(term) = f.search.as_ref().filter(|s| !s.trim().is_empty()) {
            args.push(Box::new(term.clone()));
            sql.push_str(&format!(" AND search_match(?{}, v.title, c.title)", args.len()));
        }
    }
    sql.push_str(order_by(f.sort));
    Some((sql, args))
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

const INSERT_VIDEO: &str = "INSERT OR IGNORE INTO videos
     (id,channel_id,title,description,thumb_url,published_at,sort_at,feed_rank,
      added_manually,duration_secs,view_count,status,first_seen_at)
     VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)";

const UPDATE_META: &str = "UPDATE videos SET
       duration_secs=COALESCE(?2, duration_secs),
       view_count=COALESCE(?3, view_count),
       status=?4
     WHERE id=?1";

const SET_PUBLISHED: &str = "UPDATE videos SET published_at=?2,
            sort_at = CASE WHEN added_manually=1 THEN sort_at ELSE ?2 END
     WHERE id=?1";

/// Shared by `Db::prune_orphan_channel` and a sync apply that deleted the last
/// manual video an uploader had, so the two can never disagree about which
/// rows count as orphans.
const PRUNE_ORPHAN_CHANNEL: &str = "DELETE FROM channels
     WHERE id=?1 AND subscribed=0
       AND NOT EXISTS (SELECT 1 FROM videos WHERE channel_id=?1)";

const SET_PUBLISHED_IF_MISSING: &str = "UPDATE videos SET published_at=?2,
            sort_at = CASE WHEN added_manually=1 THEN sort_at
                           ELSE COALESCE(sort_at, ?2) END
     WHERE id=?1 AND published_at IS NULL";

/// What a channel listing resolved for a row still awaiting it -- the input to
/// [`Db::resolve_listed`].
pub struct ResolvedMeta {
    pub id: String,
    pub duration_secs: Option<i64>,
    pub view_count: Option<i64>,
    pub status: VideoStatus,
    /// The listing's date, written only where the row has none.
    pub published_at: Option<i64>,
}

/// The body of [`Db::insert_video_if_new`] and its batch form: `INSERT OR
/// IGNORE` each row, counting the ones that were new. Runs on whatever it is
/// handed, so the batch form can hand it a transaction.
fn insert_all(conn: &Connection, videos: &[NewVideo]) -> Result<usize> {
    let mut st = conn.prepare_cached(INSERT_VIDEO)?;
    let mut added = 0;
    for v in videos {
        let n = st.execute(params![v.id, v.channel_id, v.title, v.description, v.thumb_url,
                                   v.published_at, v.sort_at, v.feed_rank,
                                   v.added_manually as i64, v.duration_secs, v.view_count,
                                   v.status.as_str(), now()])?;
        if n > 0 { added += 1; }
    }
    Ok(added)
}

/// [`SET_PUBLISHED`] for each pair, skipping a row that fails rather than
/// failing the lot; returns how many ran. For an ordinary statement error
/// SQLite rolls back that statement alone, not the transaction around it; the
/// kind that does take the transaction with it (a full disk, an I/O error)
/// then fails the commit, which is reported.
fn set_published_all(conn: &Connection, dates: &[(String, i64)]) -> Result<usize> {
    let mut st = conn.prepare_cached(SET_PUBLISHED)?;
    Ok(dates.iter().filter(|(id, ts)| st.execute(params![id, ts]).is_ok()).count())
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> { Self::init(Connection::open_in_memory()?) }

    fn init(conn: Connection) -> Result<Self> {
        // This process holds one connection, but the Python scripts in
        // `scripts/` open the same file; a moment's wait for their lock beats
        // failing a poll's write with SQLITE_BUSY on the spot. First, so the
        // journal-mode switch below -- the statement most likely to meet their
        // lock -- waits too.
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        // NORMAL is the setting WAL was designed around: a commit no longer
        // waits on an fsync of its own, only a checkpoint does. What it gives up
        // is the last few commits on a power cut -- never the database itself.
        // Most of those rows the next poll writes again; not all of them: a
        // watched or hidden flag, a sibling mark, or a finished download's
        // `done` (which `reset_stale_downloads` then turns to `none`, leaving
        // the file on disk untracked) made in those last moments is lost.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        register_search(&conn)?;
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
               subscribed=MAX(channels.subscribed, excluded.subscribed),
               -- An upsert means the channel was just seen alive (a feed that
               -- answered) or added again by hand, which is also the only way
               -- to retry one marked terminated.
               terminated_at=NULL
               -- `member` is deliberately absent: a new row takes the column
               -- default and an existing one keeps what it had. Every poll
               -- upserts its channel to catch a rename, and none of those
               -- carries any opinion about a membership. `Channel::member` is
               -- therefore read-only through this path -- see
               -- `set_channel_member`. `auto_download_since` is absent for
               -- the same reason: only `set_channel_auto_download` moves it.",
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
            "SELECT id,title,handle,url,thumb_path,subscribed,added_at,last_polled_at,member,
                    terminated_at IS NOT NULL, auto_download_since IS NOT NULL
             FROM channels WHERE {cond} ORDER BY title COLLATE NOCASE ASC"))?;
        let rows = st.query_map([], |r| Ok(Channel {
            id: r.get(0)?, title: r.get(1)?, handle: r.get(2)?, url: r.get(3)?,
            thumb_path: r.get(4)?, subscribed: r.get::<_, i64>(5)? != 0,
            added_at: r.get(6)?, last_polled_at: r.get(7)?,
            member: r.get::<_, i64>(8)? != 0,
            terminated: r.get::<_, i64>(9)? != 0,
            auto_download: r.get::<_, i64>(10)? != 0,
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

    /// Switches auto-download on (stamping now as the instant new uploads are
    /// counted from) or off. Switching on again restamps: whatever arrived while
    /// it was off is backlog, which the caller offers separately.
    pub fn set_channel_auto_download(&self, id: &str, on: bool) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE channels SET auto_download_since=?2 WHERE id=?1",
                     params![id, if on { Some(now()) } else { None }])?;
        Ok(())
    }

    /// Videos the post-poll sweep should queue: new uploads, on channels with
    /// auto-download on, never queued automatically before.
    ///
    /// "New" takes both clocks. `first_seen_at` after the switch keeps out
    /// what was already in the library; `published_at` near or after it keeps
    /// out what a deepened listing or a members-only join inserts now but which
    /// aired long ago. The day of slack absorbs a listing-ingested row still
    /// wearing its midnight-UTC bucket (see `upload_date::is_bucketed`), which
    /// can sit up to a day before the real instant.
    pub fn auto_download_candidates(&self) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT v.id FROM videos v JOIN channels c ON c.id = v.channel_id
              WHERE c.auto_download_since IS NOT NULL AND c.subscribed=1
                AND c.terminated_at IS NULL
                AND v.status='ready' AND v.hidden=0 AND v.watched=0 AND v.auto_queued=0
                AND v.download_state='none'
                AND v.first_seen_at >= c.auto_download_since
                AND COALESCE(v.published_at, v.first_seen_at) >= c.auto_download_since - 86400
              ORDER BY COALESCE(v.published_at, v.first_seen_at) ASC, v.id ASC")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// What "everything so far as well" would queue for one channel: rows
    /// already in the library with nothing downloaded, queued or failed. A
    /// failure is left out because retrying it is the user's call, one video
    /// at a time. `auto_queued` is not consulted: this is an explicit ask.
    pub fn auto_download_backlog(&self, channel_id: &str, include_watched: bool,
                                 include_hidden: bool) -> Result<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id FROM videos
              WHERE channel_id=?1 AND status='ready' AND download_state='none'
                AND (?2 OR watched=0) AND (?3 OR hidden=0)
              ORDER BY COALESCE(published_at, first_seen_at) ASC, id ASC")?;
        let rows = st.query_map(params![channel_id, include_watched, include_hidden],
                                |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Flags rows as queued by auto-download, so the sweep never takes them
    /// again. One transaction: a backlog can be hundreds of rows.
    pub fn mark_auto_queued(&self, ids: &[String]) -> Result<()> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut st = tx.prepare("UPDATE videos SET auto_queued=1 WHERE id=?1")?;
            for id in ids { st.execute(params![id])?; }
        }
        tx.commit()?;
        Ok(())
    }

    /// Marks a channel whose account YouTube has terminated. Its feed 404s and
    /// its listing refuses outright, so polling it only ever produces the same
    /// error; the poll skips it from here on. Only `upsert_channel` clears it.
    pub fn set_channel_terminated(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE channels SET terminated_at=?2 WHERE id=?1 AND terminated_at IS NULL",
                     params![id, now()])?;
        Ok(())
    }

    /// Records that a channel's listing has been read `depth` entries deep.
    ///
    /// Only ever raised: lowering `backfill_count` removes nothing, so the
    /// catalogue is still as deep as it ever was, and raising the setting again
    /// to anything short of that must not read the listing a second time.
    pub fn set_backfill_depth(&self, id: &str, depth: u32) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "UPDATE channels SET backfill_depth=MAX(COALESCE(backfill_depth, 0), ?2) WHERE id=?1",
            params![id, depth],
        )?;
        Ok(())
    }

    /// Subscribed channels backfilled less than `depth` deep -- the ones a poll
    /// has to deepen for a raised `backfill_count` to reach them. NULL is a row
    /// whose depth was never recorded, which counts as shallower than anything.
    pub fn channels_shallower_than(&self, depth: u32) -> Result<HashSet<String>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            "SELECT id FROM channels
             WHERE subscribed=1 AND COALESCE(backfill_depth, 0) < ?1")?;
        let rows = st.query_map(params![depth], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
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
        Ok(insert_all(&conn, std::slice::from_ref(v))? > 0)
    }

    /// [`Db::insert_video_if_new`] for a whole batch, returning how many rows
    /// were actually new.
    ///
    /// One lock and one transaction for the lot, for the same reason as
    /// [`Db::set_titles`]: a backfill writes hundreds of rows, and one implicit
    /// transaction each was one commit each, with the connection locked and
    /// released around every one -- so the UI's reads queued behind a poll's
    /// writes one row at a time. `first_seen_at` is still stamped per row, so
    /// it reads exactly as the one-at-a-time path would have left it.
    pub fn insert_videos_if_new(&self, videos: &[NewVideo]) -> Result<usize> {
        if videos.is_empty() { return Ok(0); }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let added = insert_all(&tx, videos)?;
        tx.commit()?;
        Ok(added)
    }

    /// Fills in metadata the flat-playlist pass resolved. Never clears a real
    /// published_at with NULL, so a later backfill cannot erase an RSS date.
    pub fn update_video_meta(&self, id: &str, duration: Option<i64>,
                             views: Option<i64>, status: VideoStatus) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(UPDATE_META, params![id, duration, views, status.as_str()])?;
        Ok(())
    }

    /// [`Db::update_video_meta`] and then, where the listing carried a date,
    /// [`Db::set_published_at_if_missing`], for every row in one transaction.
    pub fn resolve_listed(&self, rows: &[ResolvedMeta]) -> Result<()> {
        if rows.is_empty() { return Ok(()); }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        {
            let mut meta = tx.prepare(UPDATE_META)?;
            let mut date = tx.prepare(SET_PUBLISHED_IF_MISSING)?;
            for r in rows {
                meta.execute(params![r.id, r.duration_secs, r.view_count, r.status.as_str()])?;
                if let Some(ts) = r.published_at {
                    date.execute(params![r.id, ts])?;
                }
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// RSS supplies the real date for a row that was backfilled without one.
    /// `sort_at` only follows for rows that were NOT added manually — a manually
    /// added video must keep sorting by when the user added it.
    pub fn set_published_at(&self, id: &str, ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(SET_PUBLISHED, params![id, ts])?;
        Ok(())
    }

    /// [`Db::set_published_at`] for a batch, in one transaction. Returns how
    /// many of the updates ran without error.
    ///
    /// A row that fails is skipped rather than failing the batch, which is what
    /// both callers did with the single-row method: `repair_dates` counts the
    /// successes, and a feed's date refresh ignored its errors outright.
    pub fn set_published_at_many(&self, dates: &[(String, i64)]) -> Result<usize> {
        if dates.is_empty() { return Ok(0); }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let ok = set_published_all(&tx, dates)?;
        tx.commit()?;
        Ok(ok)
    }

    /// Everything one RSS feed writes, in one transaction: the exact instant
    /// for each row already stored (`dates`, via [`Db::set_published_at`] and
    /// its per-row error tolerance), the title each of those rows was published
    /// under (`titles`, see below), then the new rows (via
    /// [`Db::insert_videos_if_new`]). Returns how many rows were new.
    ///
    /// A feed's title is never written to `videos.title`: it is frozen at
    /// publish time (see [`Db::set_titles`]), so it is the one source that
    /// cannot know a rename. That same freezing is what makes it worth
    /// keeping. Where it differs from the stored title it is a name the video
    /// carried and no longer does, and it goes into `video_titles` beside the
    /// ones the rename trigger records. The guards live in the SQL, as in
    /// `set_titles`, so a poll can hand over every entry: an empty title, the
    /// title the row carries now, and an id belonging to no row all record
    /// nothing. A name that fails to write is skipped like a date -- the feed
    /// carries it again next poll, and it is not worth the new rows.
    pub fn apply_feed(
        &self,
        dates: &[(String, i64)],
        titles: &[(String, String)],
        fresh: &[NewVideo],
    ) -> Result<usize> {
        if dates.is_empty() && titles.is_empty() && fresh.is_empty() { return Ok(0); }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        set_published_all(&tx, dates)?;
        {
            let mut keep = tx.prepare_cached(
                "INSERT OR IGNORE INTO video_titles (video_id, title)
                 SELECT id, ?2 FROM videos WHERE id=?1 AND ?2<>'' AND title<>?2")?;
            for (id, title) in titles {
                // Per row, like `set_published_all`: an ordinary statement
                // error rolls back that statement alone, and one that takes
                // the transaction with it fails the commit below.
                let _ = keep.execute(params![id, title]);
            }
        }
        let added = insert_all(&tx, fresh)?;
        tx.commit()?;
        Ok(added)
    }

    /// Fills a date only where one is missing. An exact RSS timestamp must never
    /// be clobbered by the day-granular approximate date from a flat listing.
    pub fn set_published_at_if_missing(&self, id: &str, ts: i64) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(SET_PUBLISHED_IF_MISSING, params![id, ts])?;
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

    /// Ids of every ready video on the anchor's channel that belongs on the same
    /// card -- the anchor itself included, since a series view that omits the
    /// video you opened it from is disorienting.
    ///
    /// Takes the lock only to read the channel's rows and matches after letting
    /// go of it, so it must never be called with the lock held -- it is not
    /// reentrant.
    fn sibling_ids(&self, anchor_id: &str) -> Result<Vec<String>> {
        let entries = {
            let conn = self.conn.lock().unwrap();
            let channel_id: Option<String> = conn
                .query_row("SELECT channel_id FROM videos WHERE id=?1", params![anchor_id],
                           |r| r.get(0))
                .optional()?;
            let Some(channel_id) = channel_id else { return Ok(Vec::new()) };
            channel_rows(&conn, &channel_id)?.0
        };
        let atoms = siblings::Atoms::new(entries);
        Ok(atoms.group_led_by(anchor_id).into_iter().map(String::from).collect())
    }

    pub fn list_videos(&self, f: &VideoFilter) -> Result<Vec<Video>> {
        // Before the lock, not under it: see `sibling_ids`.
        let series = match &f.sibling_of {
            Some(anchor) => self.sibling_ids(anchor)?,
            None => Vec::new(),
        };
        let conn = self.conn.lock().unwrap();
        let Some((mut sql, mut args)) = feed_query(f, SELECT_VIDEO, series) else {
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
    /// -- the same match [`Db::sibling_ids`] makes -- so a group's contents are by
    /// construction exactly what "Find siblings" on that leader returns.
    ///
    /// The filters therefore choose which groups appear, not what is inside
    /// them: a card says "7 parts" because there are seven, and the series stays
    /// on screen until every one of them has been filtered out. `sibling_of` is
    /// meaningless here and is ignored -- the series view is a flat list on
    /// purpose.
    ///
    /// The lock is held only for the two rounds of queries -- the seeds and
    /// each channel's rows, then the page's hydration -- and released for the
    /// matching in between, which is where the time goes: a regex pass over
    /// every title in the library and a comparison per candidate pair. Held
    /// throughout, it stalled every other command and the poll's writes for the
    /// whole walk. A write landing in that gap can leave one fetch grouping by
    /// rows a moment stale, which the next refetch corrects; a row deleted in it
    /// is simply absent from the hydrated page.
    pub fn list_video_groups(&self, f: &VideoFilter) -> Result<Vec<VideoGroup>> {
        let seed_filter = VideoFilter { sibling_of: None, ..f.clone() };
        let Some((sql, args)) = feed_query(&seed_filter, SELECT_VIDEO_IDS, Vec::new()) else {
            return Ok(Vec::new());
        };

        // Every ready video on each channel in play, read once. Matching
        // against the unfiltered set is the whole point: the filterbar must not
        // be able to shorten a series, only to hide one.
        let (seeds, rows_by_channel) = {
            let conn = self.conn.lock().unwrap();
            let mut st = conn.prepare(&sql)?;
            let seeds: Vec<(String, String)> = st
                .query_map(rusqlite::params_from_iter(args.iter().map(|b| b.as_ref())), |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let mut rows_by_channel = HashMap::new();
            for (_, channel) in &seeds {
                if rows_by_channel.contains_key(channel) { continue; }
                rows_by_channel.insert(channel.clone(), channel_rows(&conn, channel)?);
            }
            (seeds, rows_by_channel)
        };

        // Bucketed with the lock released. The atoms take the entries whole,
        // so the former names -- which name a card as well as match it -- are
        // set aside first, for the few rows that have any.
        let mut index: HashMap<String, siblings::Atoms> = HashMap::new();
        let mut parts: HashMap<String, PartFacts> = HashMap::new();
        let mut former: HashMap<String, Vec<String>> = HashMap::new();
        for (channel, (entries, facts)) in rows_by_channel {
            parts.extend(facts);
            former.extend(
                entries.iter()
                    .filter(|e| !e.former_titles.is_empty())
                    .map(|e| (e.id.clone(), e.former_titles.clone())),
            );
            index.insert(channel, siblings::Atoms::new(entries));
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
        // After the walk, not in it: a lone video must still strike itself off
        // and claim nothing, and only the finished walk knows who is alone.
        if f.groups_only {
            groups.retain(|members| members.len() > 1);
        }
        // "Continue watching" is the very test the card uses for its amber
        // in-progress look -- some part watched, some part not -- so the chip and
        // the colour can never disagree. It asks it of the whole group, not the
        // parts that survived the filters, which is what lets it combine with
        // Hide watched: the series you are midway through, each card opening on
        // its next unwatched part. A lone video is never both, so it drops out.
        // Before the cut, like `groups_only`, so every page comes back full.
        if f.in_progress {
            let watched = |id: &String| parts.get(id).is_some_and(|p| p.watched);
            groups.retain(|members| {
                members.iter().any(watched) && !members.iter().all(watched)
            });
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
                        .map(|id| parts.get(id).map_or(0, |p| p.secs))
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
        let conn = self.conn.lock().unwrap();
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
        // Naming each group is another regex pass; nothing below reads the
        // database.
        drop(conn);

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
                // Each part's names, its current title first. That one comes
                // from the hydrated row, which is fresher than the walk's read
                // when a rename lands in between -- so a name it now carries
                // again is kept out of the former ones rather than counted twice.
                let names: Vec<Vec<&str>> = videos
                    .iter()
                    .map(|v| {
                        let was = former.get(&v.id).into_iter().flatten().map(String::as_str);
                        std::iter::once(v.title.as_str())
                            .chain(was.filter(|t| *t != v.title))
                            .collect()
                    })
                    .collect();
                siblings::series_stem(&names)
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

        // Linking a video to a series hidden as a whole puts it on that card,
        // so it goes the way of the series, as an undecided part would at the
        // next sweep (see `hide_series_arrivals`) -- and here even one that
        // was decided before, since the user has just put it there by hand.
        // Only unmarked rows are touched: a part brought back on its own keeps
        // its mark and stays back.
        conn.execute(
            "UPDATE videos SET hidden=1, series_hidden=1
             WHERE sibling_group=?1 AND series_hidden=0
               AND EXISTS (SELECT 1 FROM videos WHERE sibling_group=?1 AND series_hidden=1)",
            params![key])?;

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

    /// Every row on a channel as the grouping rule sees it, whatever its
    /// `status` -- unlike [`channel_rows`], which reads only what the feed can
    /// show. Hiding a series has to reach the parts not on screen yet too: an
    /// upcoming premiere is the next part, and the day it goes live it must
    /// not walk back into the feed. Extra entries can only add members to a
    /// card, never take one away, so the visible card is always inside what
    /// this groups.
    fn series_entries(conn: &Connection, channel_id: &str) -> Result<Vec<siblings::Entry>> {
        let mut former = former_titles(conn, channel_id)?;
        let mut st = conn.prepare_cached(
            "SELECT id, title, sibling_group FROM videos WHERE channel_id=?1")?;
        let rows = st.query_map(params![channel_id], |r| {
            let id: String = r.get(0)?;
            let former_titles = former.remove(&id).unwrap_or_default();
            Ok(siblings::Entry { id, title: r.get(1)?, former_titles, group: r.get(2)? })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Hides (or brings back) the series `leader_id` leads, as a whole, and
    /// returns the ids of the parts whose flags changed.
    ///
    /// A series has no row of its own -- a card is recomputed on every fetch --
    /// so the decision is written onto its parts: `hidden` takes each out of
    /// the feed, and `series_hidden` records that the *show* was hidden rather
    /// than one bad episode, which is what [`Db::hide_series_arrivals`] follows
    /// to the parts that join the card later.
    ///
    /// Bringing it back clears both, and stamps `series_hidden_changed_at` on
    /// every part -- even one whose mark was already clear, which the trigger
    /// alone would leave unstamped. The stamp is what makes a row *decided*:
    /// the sweep only ever marks rows nobody has decided about, so a part on
    /// the card brought back can never be swept back out by a mark elsewhere
    /// on the channel. It also lets this choice win a sync against an older
    /// mark another machine still holds.
    ///
    /// The lock is released for the matching, as in [`Db::sibling_ids`]. An
    /// unknown leader changes nothing.
    pub fn set_series_hidden(&self, leader_id: &str, hidden: bool) -> Result<Vec<String>> {
        let entries = {
            let conn = self.conn.lock().unwrap();
            let channel_id: Option<String> = conn
                .query_row("SELECT channel_id FROM videos WHERE id=?1", params![leader_id],
                           |r| r.get(0))
                .optional()?;
            let Some(channel_id) = channel_id else { return Ok(Vec::new()) };
            Self::series_entries(&conn, &channel_id)?
        };
        let parts: Vec<String> = siblings::Atoms::new(entries)
            .group_led_by(leader_id)
            .into_iter()
            .map(String::from)
            .collect();

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut changed = Vec::new();
        {
            // Hiding writes only rows not already there, so a part already
            // hidden and marked keeps its stamps and is not reported.
            let mut flags = tx.prepare(if hidden {
                "UPDATE videos SET hidden=1, series_hidden=1
                 WHERE id=?1 AND (hidden=0 OR series_hidden=0)"
            } else {
                "UPDATE videos SET hidden=0, series_hidden=0
                 WHERE id=?1 AND (hidden<>0 OR series_hidden<>0)"
            })?;
            // A stamp-only write, so the v11 trigger -- which fires on writes
            // naming `series_hidden` -- stays out of it.
            let mut decided = tx.prepare(
                "UPDATE videos SET series_hidden_changed_at=?2 WHERE id=?1")?;
            let at = now();
            for id in parts {
                if flags.execute(params![id])? > 0 { changed.push(id.clone()); }
                if !hidden { decided.execute(params![id, at])?; }
            }
        }
        tx.commit()?;
        Ok(changed)
    }

    /// Hides every part of a series hidden as a whole that nobody has decided
    /// about yet, marks it so the part after it follows in turn, and returns
    /// the ids.
    ///
    /// Run after everything that can bring such a part in: a poll (before its
    /// auto-download sweep, which must never take a part of a series the user
    /// hid, and before its toast, which must not announce one), a membership
    /// join, hiding a series, and -- where a mark itself arrives -- a sync or
    /// an import.
    ///
    /// A row is a candidate while its `series_hidden_changed_at` is NULL: the
    /// mark was never set or cleared on it, by anyone, here or on a machine it
    /// synced from. That one rule covers every way a part turns up -- polled
    /// new, deepened from the back catalogue, a premiere going live under its
    /// real title, an old upload retitled into the series, or a part this
    /// machine already held when a mark arrived from elsewhere -- and it is
    /// what keeps the sweep from ever overruling a person: Un-hide series
    /// stamps every part it brings back, and un-hiding one part keeps that
    /// part's mark, so neither can be swept back out. A video added by hand is
    /// never a candidate either -- pasting it into the Add box was asking to
    /// see it.
    ///
    /// A candidate is hidden when the card it would lead holds a marked part.
    /// Each one hidden counts as marked for the rest, repeated until nothing
    /// changes: two new parts in one poll, where the second matches only the
    /// first, must not depend on which the loop reached first. That chaining
    /// is also what follows a series whose titles drift -- p7 that matches
    /// only p6, which matches only what was hidden by hand.
    ///
    /// Only channels holding a mark are read, and each candidate's card is
    /// worked out once, with the lock released as in [`Db::sibling_ids`]. A
    /// decision landing in that gap is respected by the write, which takes
    /// only rows still undecided; that is also why only the rows this call
    /// actually marked are returned, so two callers racing over one part never
    /// both report it.
    pub fn hide_series_arrivals(&self) -> Result<Vec<String>> {
        // Per channel holding a mark: every row on it, which are marked, and
        // which are still undecided.
        let channels: Vec<(Vec<siblings::Entry>, HashSet<String>, Vec<String>)> = {
            let conn = self.conn.lock().unwrap();
            let ids: Vec<String> = {
                let mut st = conn.prepare(
                    "SELECT DISTINCT channel_id FROM videos WHERE series_hidden=1")?;
                let rows = st.query_map([], |r| r.get(0))?;
                rows.collect::<rusqlite::Result<Vec<_>>>()?
            };
            let mut out = Vec::new();
            for channel_id in ids {
                let mut st = conn.prepare_cached(
                    "SELECT id, series_hidden FROM videos
                     WHERE channel_id=?1
                       AND (series_hidden=1
                            OR (series_hidden_changed_at IS NULL AND added_manually=0))")?;
                let rows = st.query_map(params![channel_id], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?))
                })?;
                let (mut marked, mut undecided) = (HashSet::new(), Vec::new());
                for row in rows {
                    let (id, is_marked) = row?;
                    if is_marked { marked.insert(id); } else { undecided.push(id); }
                }
                if undecided.is_empty() { continue; }
                out.push((Self::series_entries(&conn, &channel_id)?, marked, undecided));
            }
            out
        };

        let mut hide: Vec<String> = Vec::new();
        for (entries, already, undecided) in channels {
            let atoms = siblings::Atoms::new(entries);
            let mut marked: HashSet<&str> = already.iter().map(String::as_str).collect();
            // Each candidate's card once, up front: which rows are marked
            // changes from pass to pass, but the cards themselves do not. A
            // card of one holds nothing but the candidate, which is unmarked.
            let mut pending: Vec<(&str, Vec<&str>)> = undecided
                .iter()
                .map(|id| (id.as_str(), atoms.group_led_by(id)))
                .filter(|(_, card)| card.len() > 1)
                .collect();
            loop {
                let before = pending.len();
                pending.retain(|(id, card)| {
                    if !card.iter().any(|m| marked.contains(m)) {
                        return true;
                    }
                    marked.insert(id);
                    hide.push(id.to_string());
                    false
                });
                if pending.len() == before { break; }
            }
        }
        if hide.is_empty() { return Ok(hide); }

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut changed = Vec::new();
        {
            let mut st = tx.prepare(
                "UPDATE videos SET hidden=1, series_hidden=1
                 WHERE id=?1 AND series_hidden=0 AND series_hidden_changed_at IS NULL")?;
            for id in hide {
                if st.execute(params![id])? > 0 { changed.push(id); }
            }
        }
        tx.commit()?;
        Ok(changed)
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
    ///
    /// `None` also clears `download_quality`: a custom quality belongs to one
    /// download, and the next plain Download of the row follows Settings.
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
                                   ELSE ?5 END,
                               download_quality=CASE ?2
                                   WHEN 'none' THEN NULL
                                   ELSE download_quality END
             WHERE id=?1",
            params![id, state.as_str(), file_path, error, now()])?;
        Ok(())
    }

    /// Which of `ids` a "Download all" queues, in the order to queue them:
    /// oldest upload first, so a series downloads from part 1, and the queue
    /// (`queue::Line`) starts them in exactly that order.
    ///
    /// Only what a Download button would offer: ready, and with nothing
    /// downloaded, queued or running -- a failure is retried, as its own Retry
    /// would. Watched and hidden rows are not filtered out: the ids are the
    /// parts the caller chose. An undated row goes after the dated ones;
    /// `sort_at` and then the id settle ties, so the order is the same every
    /// time. Unknown ids, and the same id twice, cost nothing.
    pub fn download_order(&self, ids: &[String]) -> Result<Vec<String>> {
        if ids.is_empty() { return Ok(Vec::new()); }
        let conn = self.conn.lock().unwrap();
        let placeholders = std::iter::repeat("?").take(ids.len()).collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id FROM videos
             WHERE id IN ({placeholders})
               AND status='ready' AND download_state IN ('none','failed')
             ORDER BY published_at ASC NULLS LAST, sort_at ASC NULLS LAST, id ASC");
        let mut st = conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(ids), |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn clear_file_path(&self, id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET file_path=NULL, download_state='none', download_error=NULL,
                                        downloaded_at=NULL, download_quality=NULL WHERE id=?1",
                     params![id])?;
        Ok(())
    }

    /// Stores (or with `None`, clears) the quality one video's next download
    /// is fetched at, overriding Settings. See the v9 migration step.
    pub fn set_download_quality(&self, id: &str, q: Option<&Quality>) -> Result<()> {
        let json = q.map(serde_json::to_string).transpose()?;
        let conn = self.conn.lock().unwrap();
        conn.execute("UPDATE videos SET download_quality=?2 WHERE id=?1", params![id, json])?;
        Ok(())
    }

    /// The video's own quality, if it carries one. JSON that no longer parses
    /// -- a later build's shape, a hand edit -- reads as none, so the download
    /// falls back to Settings rather than failing; what does parse is
    /// sanitised on the way out, since it is about to become argv.
    pub fn download_quality(&self, id: &str) -> Result<Option<Quality>> {
        let conn = self.conn.lock().unwrap();
        let raw: Option<String> = conn
            .query_row("SELECT download_quality FROM videos WHERE id=?1", params![id],
                       |r| r.get(0))
            .optional()?
            .flatten();
        Ok(raw.and_then(|j| serde_json::from_str::<Quality>(&j).ok()).map(Quality::sanitized))
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
        conn.execute(PRUNE_ORPHAN_CHANNEL, params![channel_id])?;
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
        conn.execute("UPDATE videos SET download_state='none', downloaded_at=NULL,
                                        download_quality=NULL
                      WHERE download_state IN ('downloading','queued')", [])?;
        Ok(())
    }
}

const SELECT_VIDEO: &str = "
SELECT v.id, v.channel_id, c.title, v.title, v.description, v.thumb_url, v.thumb_path,
       v.published_at, v.sort_at, v.feed_rank, v.added_manually,
       v.duration_secs, v.view_count, v.status, v.hidden,
       v.watched, v.watched_at, v.download_state, v.download_error, v.file_path,
       v.downloaded_at, v.first_seen_at, v.sibling_group, v.series_hidden
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
        series_hidden: r.get::<_, i64>(23)? != 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Db { Db::open_in_memory().unwrap() }

    mod download_quality {
        use super::*;

        fn one(d: &Db) {
            d.upsert_channel(&chan("UC1", "One")).unwrap();
            d.insert_video_if_new(&vid("a", "UC1", Some(1))).unwrap();
        }

        fn custom() -> Quality {
            Quality { mode: "audio".into(), audio_format: "mp3".into(), format: "251".into(),
                      ..Quality::default() }
        }

        #[test]
        fn a_stored_quality_reads_back_and_none_is_the_default() {
            let d = db();
            one(&d);
            assert_eq!(d.download_quality("a").unwrap(), None);
            assert_eq!(d.download_quality("no such row").unwrap(), None);
            d.set_download_quality("a", Some(&custom())).unwrap();
            assert_eq!(d.download_quality("a").unwrap(), Some(custom()));
            d.set_download_quality("a", None).unwrap();
            assert_eq!(d.download_quality("a").unwrap(), None);
        }

        #[test]
        fn it_lives_while_queued_downloading_failed_or_done() {
            let d = db();
            one(&d);
            d.set_download_quality("a", Some(&custom())).unwrap();
            for s in [DownloadState::Queued, DownloadState::Downloading, DownloadState::Failed,
                      DownloadState::Done] {
                d.set_download_state("a", s, Some("/x.mp3"), None).unwrap();
                assert_eq!(d.download_quality("a").unwrap(), Some(custom()), "{s:?}");
            }
        }

        #[test]
        fn it_is_cleared_whenever_the_row_returns_to_none() {
            let d = db();
            one(&d);

            d.set_download_quality("a", Some(&custom())).unwrap();
            d.set_download_state("a", DownloadState::None, None, None).unwrap();
            assert_eq!(d.download_quality("a").unwrap(), None, "a cancel");

            d.set_download_quality("a", Some(&custom())).unwrap();
            d.set_download_state("a", DownloadState::Done, Some("/x.mp3"), None).unwrap();
            d.clear_file_path("a").unwrap();
            assert_eq!(d.download_quality("a").unwrap(), None, "a deleted file");

            d.set_download_quality("a", Some(&custom())).unwrap();
            d.set_download_state("a", DownloadState::Downloading, None, None).unwrap();
            d.reset_stale_downloads().unwrap();
            assert_eq!(d.download_quality("a").unwrap(), None, "a restart mid-download");
        }

        #[test]
        fn a_restart_leaves_a_failed_download_its_quality() {
            let d = db();
            one(&d);
            d.set_download_quality("a", Some(&custom())).unwrap();
            d.set_download_state("a", DownloadState::Failed, None, Some("boom")).unwrap();
            d.reset_stale_downloads().unwrap();
            assert_eq!(d.download_quality("a").unwrap(), Some(custom()), "Retry repeats it");
        }

        #[test]
        fn unreadable_json_reads_as_none_and_odd_values_are_sanitised() {
            let d = db();
            one(&d);
            {
                let c = d.conn.lock().unwrap();
                c.execute("UPDATE videos SET download_quality='{not json' WHERE id='a'", []).unwrap();
            }
            assert_eq!(d.download_quality("a").unwrap(), None);
            {
                let c = d.conn.lock().unwrap();
                c.execute(r#"UPDATE videos SET download_quality='{"vcodec":"hevc"}' WHERE id='a'"#,
                          []).unwrap();
            }
            assert_eq!(d.download_quality("a").unwrap(), Some(Quality::default()));
        }

        #[test]
        fn migrating_a_v8_database_adds_the_column_empty() {
            let conn = Connection::open_in_memory().unwrap();
            conn.execute_batch(SCHEMA_V1).unwrap();
            conn.execute_batch(
                "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE videos ADD COLUMN downloaded_at INTEGER;
                 ALTER TABLE videos ADD COLUMN sibling_group TEXT;
                 ALTER TABLE channels ADD COLUMN member INTEGER NOT NULL DEFAULT 0;
                 ALTER TABLE channels ADD COLUMN backfill_depth INTEGER;
                 ALTER TABLE channels ADD COLUMN terminated_at INTEGER;
                 ALTER TABLE channels ADD COLUMN auto_download_since INTEGER;
                 ALTER TABLE videos ADD COLUMN auto_queued INTEGER NOT NULL DEFAULT 0;",
            ).unwrap();
            conn.pragma_update(None, "user_version", 8).unwrap();
            conn.execute(
                "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7)",
                [],
            ).unwrap();
            conn.execute(
                "INSERT INTO videos (id,channel_id,title,published_at,sort_at,status,first_seen_at,
                                     download_state,file_path)
                 VALUES ('a','UC1','kept',100,100,'ready',1,'done','/x.mkv')",
                [],
            ).unwrap();

            let db = Db::init(conn).unwrap();
            {
                let c = db.conn.lock().unwrap();
                assert!(column_exists(&c, "videos", "download_quality").unwrap());
                let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
                assert_eq!(v, SCHEMA_VERSION);
            }
            assert_eq!(db.download_quality("a").unwrap(), None);
            assert_eq!(db.get_video("a").unwrap().unwrap().file_path.as_deref(), Some("/x.mkv"));
        }
    }

    mod download_order {
        use super::*;

        /// A ready row on UC1 with these two dates.
        fn at(d: &Db, id: &str, published: Option<i64>, sort: Option<i64>) {
            d.insert_video_if_new(&NewVideo { published_at: published, sort_at: sort,
                                              ..vid(id, "UC1", None) })
                .unwrap();
        }

        fn ids(v: &[&str]) -> Vec<String> {
            v.iter().map(|s| s.to_string()).collect()
        }

        fn one_channel() -> Db {
            let d = db();
            d.upsert_channel(&chan("UC1", "One")).unwrap();
            d
        }

        #[test]
        fn oldest_upload_first_then_sort_at_then_id_and_undated_last() {
            let d = one_channel();
            at(&d, "p3", Some(300), Some(300));
            at(&d, "undated", None, Some(50));
            at(&d, "p1", Some(100), Some(100));
            // One upload instant -- a midnight bucket -- the earlier sort_at first,
            at(&d, "p2b", Some(200), Some(260));
            at(&d, "p2a", Some(200), Some(250));
            // and with both the same, the id.
            at(&d, "p4y", Some(400), Some(400));
            at(&d, "p4x", Some(400), Some(400));
            at(&d, "nothing", None, None);
            let asked = ids(&["nothing", "p4y", "p3", "undated", "p2b", "p1", "p4x", "p2a"]);
            assert_eq!(d.download_order(&asked).unwrap(),
                       ids(&["p1", "p2a", "p2b", "p3", "p4x", "p4y", "undated", "nothing"]));
        }

        #[test]
        fn only_what_a_download_button_would_offer_and_a_failure_is_retried() {
            let d = one_channel();
            for (i, id) in ["none", "failed", "queued", "downloading", "done", "watched", "hidden"]
                .into_iter().enumerate() {
                at(&d, id, Some(i as i64 * 10), Some(i as i64 * 10));
            }
            d.insert_video_if_new(&NewVideo { status: VideoStatus::Upcoming,
                                              ..vid("upcoming", "UC1", Some(5)) })
                .unwrap();
            d.set_download_state("failed", DownloadState::Failed, None, Some("boom")).unwrap();
            d.set_download_state("queued", DownloadState::Queued, None, None).unwrap();
            d.set_download_state("downloading", DownloadState::Downloading, None, None).unwrap();
            d.set_download_state("done", DownloadState::Done, Some("/x.mkv"), None).unwrap();
            d.set_watched("watched", true).unwrap();
            d.set_hidden("hidden", true).unwrap();

            let asked = ids(&["hidden", "upcoming", "done", "downloading", "queued", "failed",
                              "none", "watched", "no such row", "none"]);
            assert_eq!(d.download_order(&asked).unwrap(),
                       ids(&["none", "failed", "watched", "hidden"]),
                       "the same id twice is queued once; an unknown one is no error");
        }

        #[test]
        fn only_the_ids_asked_for_and_none_for_none() {
            let d = one_channel();
            at(&d, "a", Some(1), Some(1));
            at(&d, "b", Some(2), Some(2));
            assert_eq!(d.download_order(&ids(&["b"])).unwrap(), ids(&["b"]));
            assert!(d.download_order(&[]).unwrap().is_empty());
        }
    }

    fn chan(id: &str, title: &str) -> Channel {
        Channel { id: id.into(), title: title.into(), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 0,
                  last_polled_at: None, terminated: false, auto_download: false }
    }

    fn vid(id: &str, ch: &str, published: Option<i64>) -> NewVideo {
        NewVideo { id: id.into(), channel_id: ch.into(), title: format!("t-{id}"),
                   description: None, thumb_url: None, published_at: published,
                   sort_at: published, feed_rank: 0, added_manually: false,
                   duration_secs: Some(600), view_count: Some(1),
                   status: VideoStatus::Ready }
    }

    #[test]
    fn a_raised_backfill_count_reaches_channels_read_shallower() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.upsert_channel(&chan("UC2", "Two")).unwrap();
        // Never recorded, as every channel predating the column is: shallower
        // than anything, so the first poll reads it to the setting.
        assert_eq!(d.channels_shallower_than(30).unwrap().len(), 2);

        d.set_backfill_depth("UC1", 30).unwrap();
        d.set_backfill_depth("UC2", 30).unwrap();
        assert!(d.channels_shallower_than(30).unwrap().is_empty(), "read once, then left alone");
        assert_eq!(d.channels_shallower_than(300).unwrap().len(), 2, "raised: both deepen");

        d.set_backfill_depth("UC1", 300).unwrap();
        let left: Vec<_> = d.channels_shallower_than(300).unwrap().into_iter().collect();
        assert_eq!(left, vec!["UC2".to_string()]);
    }

    #[test]
    fn lowering_the_backfill_count_never_lowers_the_recorded_depth() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.set_backfill_depth("UC1", 300).unwrap();
        // 300 -> 50 -> 200 must not read the listing again: it is already 300 deep.
        d.set_backfill_depth("UC1", 50).unwrap();
        assert!(d.channels_shallower_than(200).unwrap().is_empty());
        // And a poll's upsert, which carries no depth, keeps it.
        d.upsert_channel(&chan("UC1", "Renamed")).unwrap();
        assert!(d.channels_shallower_than(300).unwrap().is_empty());
    }

    #[test]
    fn an_unsubscribed_uploader_is_never_deepened() {
        let d = db();
        let mut c = chan("UC1", "Ad hoc");
        c.subscribed = false;
        d.upsert_channel(&c).unwrap();
        assert!(d.channels_shallower_than(300).unwrap().is_empty());
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
            channel_ids: vec!["UC-other".into()],
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
    fn groups_only_drops_every_lone_video() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("other", "A Completely Different Story"),
            ("p2", "(2) The Blackwood Tapes"),
        ]);
        let f = VideoFilter { groups_only: true, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("p2".to_string(), 2)]);
    }

    #[test]
    fn groups_only_counts_the_whole_series_not_the_parts_left_after_filters() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
        ]);
        d.set_watched("p1", true).unwrap();
        // One part survives "Unwatched", but the card it leads is still a series.
        let f = VideoFilter { hide_watched: true, groups_only: true, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("p2".to_string(), 2)]);
    }

    #[test]
    fn groups_only_is_applied_before_the_page_is_cut() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("lone1", "A Completely Different Story"),
            ("lone2", "Yet Another Unrelated Upload"),
        ]);
        // Newest first puts both lone videos ahead of the series. Filtering
        // after the cut would return an empty first page and hide the series.
        let f = VideoFilter { groups_only: true, limit: 1, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("p2".to_string(), 2)]);
    }

    #[test]
    fn in_progress_keeps_only_series_part_watched() {
        let d = db();
        titled(&d, "UC1", &[
            ("a1", "Alpha Chronicle"),
            ("a2", "Alpha Chronicle - Part 2"),
            ("z1", "Zephyr Diaries"),
            ("z2", "Zephyr Diaries - Part 2"),
            ("m1", "Midway Journal"),
            ("m2", "Midway Journal - Part 2"),
            ("lone", "A Completely Different Story"),
        ]);
        // Alpha finished, Zephyr unstarted, Midway half watched; the lone video
        // is watched too, and one part cannot be both watched and not.
        for id in ["a1", "a2", "m1", "lone"] {
            d.set_watched(id, true).unwrap();
        }
        let f = VideoFilter { in_progress: true, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("m2".to_string(), 2)]);
    }

    #[test]
    fn in_progress_counts_the_whole_series_not_the_parts_left_after_filters() {
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "The Blackwood Tapes"),
            ("p2", "(2) The Blackwood Tapes"),
            ("p3", "(3) The Blackwood Tapes"),
        ]);
        d.set_watched("p1", true).unwrap();
        d.set_watched("p3", true).unwrap();
        // Only p2 survives "Unwatched", so what is left of the series is all
        // unwatched -- but the series itself is midway, and it opens on p2.
        let f = VideoFilter { hide_watched: true, in_progress: true, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("p2".to_string(), 3)]);
    }

    #[test]
    fn in_progress_is_applied_before_the_page_is_cut() {
        let d = db();
        titled(&d, "UC1", &[
            ("m1", "Midway Journal"),
            ("m2", "Midway Journal - Part 2"),
            ("z1", "Zephyr Diaries"),
            ("z2", "Zephyr Diaries - Part 2"),
            ("lone", "A Completely Different Story"),
        ]);
        d.set_watched("m1", true).unwrap();
        // Newest first puts the lone video and the unstarted series ahead of
        // the one in progress. Filtering after the cut would return an empty
        // first page.
        let f = VideoFilter { in_progress: true, limit: 1, ..Default::default() };
        assert_eq!(grouped(&d, &f), vec![("m2".to_string(), 2)]);
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

        let by_channel = VideoFilter { channel_ids: vec!["UC1".into()], ..Default::default() };
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
    fn search_takes_regex_and_exclusions_in_sql() {
        let d = db();
        d.upsert_channel(&chan("UC1", "Alpha")).unwrap();
        d.upsert_channel(&chan("UC2", "Beta")).unwrap();
        for (id, ch, title, at) in [
            ("a", "UC1", "Part one of the tapes", 4),
            ("b", "UC1", "Part two of the tapes", 3),
            ("c", "UC1", "Part three, a finale", 2),
            ("d", "UC2", "Part one, elsewhere", 1),
        ] {
            let mut v = vid(id, ch, Some(at));
            v.title = title.into();
            d.insert_video_if_new(&v).unwrap();
        }
        let ids = |s: &str| -> Vec<String> {
            let f = VideoFilter { search: Some(s.into()), ..Default::default() };
            d.list_videos(&f).unwrap().into_iter().map(|v| v.id).collect()
        };
        assert_eq!(ids("part.*tapes"), ["a", "b"]);
        assert_eq!(ids("(one|three)"), ["a", "c", "d"]);
        assert_eq!(ids("part -(two|finale)"), ["a", "d"]);
        // An exclusion reaches the channel name as a match does.
        assert_eq!(ids("one -beta"), ["a"]);

        // The grouping walk shares the WHERE clause, so it keeps the same rows.
        let grouped = VideoFilter { search: Some("-alpha".into()), ..Default::default() };
        let groups = d.list_video_groups(&grouped).unwrap();
        let ids: Vec<&str> = groups.iter().flat_map(|g| g.videos.iter().map(|v| v.id.as_str())).collect();
        assert_eq!(ids, ["d"]);
    }

    #[test]
    fn a_channel_filter_keeps_any_of_its_channels_and_nothing_else() {
        let d = db();
        for (id, title) in [("UC1", "Alpha"), ("UC2", "Beta"), ("UC3", "Gamma")] {
            d.upsert_channel(&chan(id, title)).unwrap();
        }
        d.insert_video_if_new(&vid("a", "UC1", Some(4))).unwrap();
        d.insert_video_if_new(&vid("b", "UC2", Some(3))).unwrap();
        d.insert_video_if_new(&vid("c", "UC3", Some(2))).unwrap();
        d.insert_video_if_new(&vid("d", "UC1", Some(1))).unwrap();
        d.set_watched("d", true).unwrap();

        let ids = |f: &VideoFilter| -> Vec<String> {
            d.list_videos(f).unwrap().into_iter().map(|v| v.id).collect()
        };
        let two = VideoFilter { channel_ids: vec!["UC1".into(), "UC3".into()], ..Default::default() };
        assert_eq!(ids(&two), ["a", "c", "d"]);
        // An empty list is no filter at all, not a filter that matches nothing.
        assert_eq!(ids(&VideoFilter::default()), ["a", "b", "c", "d"]);
        // The channels are one filter among the others, ANDed with them.
        let unwatched = VideoFilter { hide_watched: true, ..two.clone() };
        assert_eq!(ids(&unwatched), ["a", "c"]);
        // A channel that is gone narrows to nothing rather than erroring.
        let gone = VideoFilter { channel_ids: vec!["UC-gone".into()], ..Default::default() };
        assert!(ids(&gone).is_empty());

        // The grouping walk shares the WHERE clause, so it keeps the same rows.
        let groups = d.list_video_groups(&two).unwrap();
        let grouped: Vec<&str> =
            groups.iter().flat_map(|g| g.videos.iter().map(|v| v.id.as_str())).collect();
        assert_eq!(grouped, ["a", "c", "d"]);
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

    /// A terminated account stays in the list (its videos are still yours) but
    /// is marked, and the mark survives until the channel is seen alive again.
    #[test]
    fn a_terminated_channel_is_marked_until_it_is_upserted_again() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        assert!(!d.list_channels().unwrap()[0].terminated);

        d.set_channel_terminated("UC1").unwrap();
        let got = &d.list_subscribed_channels().unwrap()[0];
        assert!(got.terminated && got.subscribed, "still listed, now marked");

        d.upsert_channel(&chan("UC1", "One")).unwrap();
        assert!(!d.list_channels().unwrap()[0].terminated, "re-adding it retries it");
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
    fn a_batch_insert_counts_only_the_rows_that_were_new() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("old", "UC1", Some(1))).unwrap();
        let rows = [vid("old", "UC1", Some(2)), vid("a", "UC1", Some(3)), vid("b", "UC1", None)];
        assert_eq!(d.insert_videos_if_new(&rows).unwrap(), 2);
        assert_eq!(d.insert_videos_if_new(&rows).unwrap(), 0, "a second pass adds nothing");
        assert_eq!(d.get_video("old").unwrap().unwrap().published_at, Some(1),
                   "an existing row is left as it was");
        assert!(d.get_video("a").unwrap().unwrap().first_seen_at > 0);
        assert_eq!(d.insert_videos_if_new(&[]).unwrap(), 0);
    }

    #[test]
    fn a_feed_redates_what_it_knows_and_inserts_what_it_does_not_in_one_go() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("known", "UC1", Some(BUCKET))).unwrap();
        let mut manual = vid("manual", "UC1", Some(10));
        manual.sort_at = Some(10_000);
        manual.added_manually = true;
        d.insert_video_if_new(&manual).unwrap();

        let dates = vec![("known".to_string(), 1234), ("manual".to_string(), 42),
                         ("gone".to_string(), 7)];
        let added = d.apply_feed(&dates, &[], &[vid("new", "UC1", Some(99))]).unwrap();
        assert_eq!(added, 1);

        let known = d.get_video("known").unwrap().unwrap();
        assert_eq!((known.published_at, known.sort_at), (Some(1234), Some(1234)));
        let manual = d.get_video("manual").unwrap().unwrap();
        assert_eq!((manual.published_at, manual.sort_at), (Some(42), Some(10_000)),
                   "the added_manually exemption still holds");
        assert!(d.get_video("new").unwrap().is_some());
    }

    #[test]
    fn a_batched_date_repair_counts_every_row_it_ran() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.insert_video_if_new(&vid("a", "UC1", Some(BUCKET))).unwrap();
        let n = d.set_published_at_many(&[("a".into(), 5), ("absent".into(), 6)]).unwrap();
        // An id no row carries still "ran", exactly as the per-row call did.
        assert_eq!(n, 2);
        assert_eq!(d.get_video("a").unwrap().unwrap().published_at, Some(5));
        assert!(d.video_ids_needing_real_date(10).unwrap().is_empty());
    }

    #[test]
    fn resolving_listed_rows_fills_meta_but_never_clobbers_a_date() {
        let d = db();
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        let mut dated = vid("dated", "UC1", Some(777));
        dated.status = VideoStatus::Pending;
        dated.duration_secs = None;
        d.insert_video_if_new(&dated).unwrap();
        let mut undated = vid("undated", "UC1", None);
        undated.status = VideoStatus::Pending;
        d.insert_video_if_new(&undated).unwrap();

        let meta = |id: &str| ResolvedMeta {
            id: id.into(), duration_secs: Some(321), view_count: None,
            status: VideoStatus::Ready, published_at: Some(BUCKET),
        };
        d.resolve_listed(&[meta("dated"), meta("undated")]).unwrap();

        let a = d.get_video("dated").unwrap().unwrap();
        assert_eq!((a.duration_secs, a.status, a.published_at),
                   (Some(321), VideoStatus::Ready, Some(777)));
        assert_eq!(a.view_count, Some(1), "a missing count never blanks the stored one");
        let b = d.get_video("undated").unwrap().unwrap();
        assert_eq!((b.published_at, b.sort_at), (Some(BUCKET), Some(BUCKET)));
    }

    #[test]
    fn a_retitle_regroups_on_the_very_next_fetch() {
        // Normalised titles are remembered between walks; the memo is keyed on
        // the title itself, so a rename must read as a different title at once.
        // A rename no longer splits a card -- the old name is kept and still
        // matches (see `former_titles_tests`) -- so the new name shows it was
        // read by the card it brings in.
        let d = db();
        titled(&d, "UC1", &[
            ("p1", "Retitle Regroup Saga Part 1"),
            ("p2", "Retitle Regroup Saga Part 2"),
            ("q1", "Something Else Entirely Part 1"),
        ]);
        let f = VideoFilter::default();
        assert_eq!(grouped(&d, &f), vec![("q1".to_string(), 1), ("p2".to_string(), 2)]);
        assert_eq!(siblings_of(&d, "p1").len(), 2);

        d.set_titles(&[("p1".into(), "Something Else Entirely Part 2".into())]).unwrap();
        assert_eq!(grouped(&d, &f), vec![("q1".to_string(), 3)],
                   "the new name joins q1, the old one keeps p2");
        assert_eq!(siblings_of(&d, "q1").len(), 3);
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
        // The grouping invariant, which matters more now that a manual mark can widen a
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

// ---- config transfer (export / import) ----

/// How many ids one batched `DELETE ... WHERE id IN (...)` may bind.
///
/// A Replace import can be asked to drop a whole library at once -- thousands
/// of rows in real use -- and one statement carrying a placeholder per id would
/// walk straight into SQLite's parameter ceiling. Chunking costs one extra
/// round trip per 500 rows and has no ceiling at all.
const IMPORT_ID_BATCH: usize = 500;

fn delete_ids_in_batches(conn: &Connection, table: &str, ids: &[&str]) -> Result<usize> {
    let mut removed = 0;
    for chunk in ids.chunks(IMPORT_ID_BATCH) {
        let holes = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("DELETE FROM {table} WHERE id IN ({holes})");
        removed += conn.execute(&sql, rusqlite::params_from_iter(chunk.iter()))?;
    }
    Ok(removed)
}

fn count_videos_of_channels(conn: &Connection, channel_ids: &[&str]) -> Result<usize> {
    let mut total = 0i64;
    for chunk in channel_ids.chunks(IMPORT_ID_BATCH) {
        let holes = std::iter::repeat("?").take(chunk.len()).collect::<Vec<_>>().join(",");
        let sql = format!("SELECT COUNT(*) FROM videos WHERE channel_id IN ({holes})");
        total += conn.query_row(&sql, rusqlite::params_from_iter(chunk.iter()), |r| r.get::<_, i64>(0))?;
    }
    Ok(total as usize)
}

/// A channel row written exactly as the archive carries it: every insert, and
/// every update under [`ImportMode::Replace`].
const PUT_CHANNEL: &str = "
INSERT INTO channels (id,title,handle,url,thumb_path,subscribed,member,added_at,last_polled_at)
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)
ON CONFLICT(id) DO UPDATE SET
  title=excluded.title, handle=excluded.handle, url=excluded.url,
  thumb_path=excluded.thumb_path, subscribed=excluded.subscribed,
  member=excluded.member, added_at=excluded.added_at,
  last_polled_at=excluded.last_polled_at";

/// What a Merge does to a channel that is already here: the two flags are
/// lifted and nothing else is. The local `title`, `handle` and `url` come from
/// a poll that has run since the archive was written, and `added_at` /
/// `last_polled_at` are this machine's own history, not the exporter's.
///
/// `member` is a one-way upgrade here, which is a deliberate departure from
/// [`Db::upsert_channel`] -- that path omits the column entirely, because a
/// poll's upsert carries no opinion about a membership and one can end. An
/// import is not a poll: exporting a config in which you are a member is a
/// statement of intent. The asymmetry of getting it wrong settles it. A stale
/// `true` costs you some locked videos you cannot download -- exactly the state
/// a lapsed membership already produces, and visible on the card. A missed
/// `true` silently switches off the members-only ingest, and you find out
/// months later that the uploads you paid for were never in the library.
///
/// The WHERE guard is what makes re-importing the same archive report an
/// honest zero: without it every row would come back "updated" forever.
const MERGE_CHANNEL: &str = "
UPDATE channels
   SET subscribed = MAX(subscribed, ?2),
       member     = MAX(member, ?3)
 WHERE id = ?1 AND (subscribed < ?2 OR member < ?3)";

/// A video row written exactly as the archive carries it.
///
/// `download_quality` is not in an archive; a row this lands on `none` loses
/// its local one, keeping the rule that only a queued, running, failed or
/// finished download carries a custom quality.
const PUT_VIDEO: &str = "
INSERT INTO videos
  (id,channel_id,title,description,thumb_url,thumb_path,published_at,sort_at,feed_rank,
   added_manually,duration_secs,view_count,status,hidden,watched,watched_at,
   download_state,download_error,file_path,downloaded_at,first_seen_at,sibling_group,
   series_hidden)
VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23)
ON CONFLICT(id) DO UPDATE SET
  channel_id=excluded.channel_id, title=excluded.title, description=excluded.description,
  thumb_url=excluded.thumb_url, thumb_path=excluded.thumb_path,
  published_at=excluded.published_at, sort_at=excluded.sort_at,
  feed_rank=excluded.feed_rank, added_manually=excluded.added_manually,
  duration_secs=excluded.duration_secs, view_count=excluded.view_count,
  status=excluded.status, hidden=excluded.hidden, watched=excluded.watched,
  watched_at=excluded.watched_at, download_state=excluded.download_state,
  download_error=excluded.download_error, file_path=excluded.file_path,
  downloaded_at=excluded.downloaded_at, first_seen_at=excluded.first_seen_at,
  sibling_group=excluded.sibling_group, series_hidden=excluded.series_hidden,
  download_quality=CASE excluded.download_state WHEN 'none' THEN NULL
                        ELSE download_quality END";

/// What a Merge lifts onto a video that is already here: user-owned state only.
///
/// `title`, `description`, `thumb_url`, `published_at`, `sort_at`, `feed_rank`,
/// `duration_secs`, `view_count`, `status`, `first_seen_at` and
/// `download_error` are all deliberately absent. The local poll has re-read
/// every one of them since the archive was written, so the archive is the
/// staler source; only the columns that record what *you* did have no other
/// authority.
///
/// **A local `done` is never downgraded.** Only an incoming `done` moves the
/// three download columns, and it moves them together -- state, path and
/// timestamp describe one file and splitting them would leave a row claiming a
/// download that is not there. Anything else incoming (`none` from a machine
/// that never fetched it, or a `failed`) leaves all three alone: a file on this
/// disk is a fact, and an archive written elsewhere cannot contradict it.
///
/// `watched_at` takes the earliest of the two, since the question it answers is
/// when you first watched the thing, and that is not a per-machine fact.
///
/// `series_hidden` is lifted like `hidden` and apart from it: an archive part
/// un-hidden on its own still carries its series' mark, and a part hidden on
/// its own carries none, so neither flag can stand in for the other.
///
/// The WHERE guard repeats every rule above as a predicate so that `changes()`
/// counts rows that actually moved. It is what makes a second run of the same
/// archive report zero updates instead of rewriting the whole library.
const MERGE_VIDEO: &str = "
UPDATE videos SET
  watched        = MAX(watched, ?2),
  hidden         = MAX(hidden, ?3),
  added_manually = MAX(added_manually, ?4),
  watched_at     = CASE WHEN ?5 IS NULL        THEN watched_at
                        WHEN watched_at IS NULL THEN ?5
                        ELSE MIN(watched_at, ?5) END,
  sibling_group  = COALESCE(sibling_group, ?6),
  thumb_path     = COALESCE(thumb_path, ?7),
  download_state = CASE WHEN ?8 = 'done' THEN ?8  ELSE download_state END,
  file_path      = CASE WHEN ?8 = 'done' THEN ?9  ELSE file_path      END,
  downloaded_at  = CASE WHEN ?8 = 'done' THEN ?10 ELSE downloaded_at  END,
  series_hidden  = MAX(series_hidden, ?11)
WHERE id = ?1
  AND ( watched < ?2
     OR hidden < ?3
     OR series_hidden < ?11
     OR added_manually < ?4
     OR (?5 IS NOT NULL AND (watched_at IS NULL OR watched_at > ?5))
     OR (sibling_group IS NULL AND ?6 IS NOT NULL)
     OR (thumb_path IS NULL AND ?7 IS NOT NULL)
     OR (?8 = 'done' AND (download_state <> 'done'
                          OR file_path IS NOT ?9
                          OR downloaded_at IS NOT ?10)) )";

impl Db {
    /// Every channel, subscribed or not.
    ///
    /// The unsubscribed ad-hoc uploaders are not optional baggage: a manually
    /// added video's parent row has to travel with it, or `videos.channel_id`'s
    /// foreign key rejects the video on the importing machine.
    pub fn export_channels(&self) -> Result<Vec<Channel>> {
        // No lock taken here, for the same reason `list_channels` takes none:
        // `channels_where` takes it, and this Mutex is not reentrant. Going
        // through it also means an export can never drift from the projection
        // the rest of the app reads.
        self.channels_where("1=1")
    }

    /// Every video row, in a stable order so two libraries holding the same
    /// rows export byte-identical lists.
    pub fn export_videos(&self) -> Result<Vec<Video>> {
        let conn = self.conn.lock().unwrap();
        let mut st = conn.prepare(
            &format!("{SELECT_VIDEO} ORDER BY v.first_seen_at ASC, v.id ASC"))?;
        let rows = st.query_map([], map_video)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// `(channel_count, video_count)`, for the export dialog's size estimate.
    pub fn transfer_estimate(&self) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        let channels: i64 = conn.query_row("SELECT COUNT(*) FROM channels", [], |r| r.get(0))?;
        let videos: i64 = conn.query_row("SELECT COUNT(*) FROM videos", [], |r| r.get(0))?;
        Ok((channels as usize, videos as usize))
    }

    /// How much of this machine's library an archive does not mention at all,
    /// so a Replace can say exactly what it would remove before it removes it.
    /// Returns `(channels, videos)`; the videos are the ones hanging off those
    /// channels, which a Replace takes with them through the cascade.
    ///
    /// Counted against every id the archive holds, ticked or not, for the same
    /// reason [`Db::apply_import`] deletes against that set: leaving a channel
    /// unticked withholds it from the import, it does not condemn the local
    /// copy. So this number never moves as the user works down the checklist,
    /// which is what makes it safe to show beside it.
    pub fn absent_from(&self, archive_channel_ids: &[String]) -> Result<(usize, usize)> {
        let conn = self.conn.lock().unwrap();
        let in_archive: HashSet<&str> =
            archive_channel_ids.iter().map(|s| s.as_str()).collect();
        let absent: Vec<String> = {
            let mut st = conn.prepare("SELECT id FROM channels")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
                .into_iter()
                .filter(|id| !in_archive.contains(id.as_str()))
                .collect()
        };
        let refs: Vec<&str> = absent.iter().map(|s| s.as_str()).collect();
        Ok((absent.len(), count_videos_of_channels(&conn, &refs)?))
    }

    /// Applies a whole archive in one transaction, so a failure halfway leaves
    /// the library exactly as it was rather than half-imported.
    ///
    /// `archive_channel_ids` is every channel id the *archive* carries, ticked
    /// or not, which is a different and wider set than `channels`. Only a
    /// Replace reads it, to tell a channel the archive never mentioned from one
    /// the user merely left unticked; under `Merge` nothing is deleted at all,
    /// so it is unused and `&[]` is a fair thing to pass.
    ///
    /// `channels` and `videos` are already the subset the user ticked, and the
    /// incoming rows' `file_path` / `download_state` / `downloaded_at` /
    /// `thumb_path` have already been resolved for *this* machine by the
    /// caller. Nothing here second-guesses them, and nothing here touches the
    /// filesystem at all.
    ///
    /// The report's `downloads_relinked`, `thumbs_written`, `settings_applied`
    /// and `download_dir_kept` are left at their defaults: they are the
    /// caller's to fill in and merge.
    pub fn apply_import(
        &self,
        channels: &[Channel],
        videos: &[Video],
        mode: ImportMode,
        archive_channel_ids: &[String],
    ) -> Result<ImportReport> {
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut report = ImportReport::default();

        // What is here before anything is written. Read once: asking per row
        // whether it exists would be one SELECT per video on a library of
        // thousands, and inside the transaction nothing else can move these
        // sets underneath us. They are what tells an insert from an update --
        // an UPSERT's `changes()` reports 1 either way.
        let mut local_channels: HashSet<String> = {
            let mut st = tx.prepare("SELECT id FROM channels")?;
            let rows = st.query_map([], |r| r.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<HashSet<_>>>()?
        };
        let mut local_videos: HashMap<String, String> = {
            let mut st = tx.prepare("SELECT id, channel_id FROM videos")?;
            let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
            rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
        };

        let keep_channels: HashSet<&str> = channels.iter().map(|c| c.id.as_str()).collect();
        let keep_videos: HashSet<&str> = videos.iter().map(|v| v.id.as_str()).collect();

        if mode == ImportMode::Replace {
            // Replace deletes database ROWS and nothing else. Not a downloaded
            // video, not a cached thumbnail -- the import dialog promises that
            // in so many words, and the asymmetry is total: a deleted row comes
            // back on the next poll or the next import, while a deleted
            // recording is simply gone. Nothing in this branch, or anywhere
            // else in this method, touches the filesystem.
            //
            // And only a channel the archive does not mention *at all* goes.
            // The checklist is an inclusion control: unticking a channel says
            // "don't import this one", never "destroy my copy of it". A channel
            // that is in the archive but outside the picked subset is therefore
            // skipped whole -- not imported, not deleted, not modified -- and
            // its videos are not swept either, which is why `orphans` below is
            // scoped to `keep_channels` and this is scoped to the archive.
            let in_archive: HashSet<&str> =
                archive_channel_ids.iter().map(|s| s.as_str()).collect();
            let drop_channels: Vec<String> = local_channels
                .iter()
                .filter(|id| !in_archive.contains(id.as_str()))
                .cloned()
                .collect();
            let dropped: HashSet<&str> = drop_channels.iter().map(|s| s.as_str()).collect();

            // Counted before the DELETE: `videos.channel_id` cascades, and
            // SQLite's `changes()` never reports a cascaded row, so after the
            // fact there is nothing left to count.
            let cascaded: Vec<String> = local_videos
                .iter()
                .filter(|(_, ch)| dropped.contains(ch.as_str()))
                .map(|(id, _)| id.clone())
                .collect();
            // A kept channel's library becomes exactly what the archive holds.
            let orphans: Vec<String> = local_videos
                .iter()
                .filter(|(id, ch)| {
                    keep_channels.contains(ch.as_str()) && !keep_videos.contains(id.as_str())
                })
                .map(|(id, _)| id.clone())
                .collect();

            let refs: Vec<&str> = drop_channels.iter().map(|s| s.as_str()).collect();
            report.channels_removed = delete_ids_in_batches(&tx, "channels", &refs)?;
            let refs: Vec<&str> = orphans.iter().map(|s| s.as_str()).collect();
            report.videos_removed =
                cascaded.len() + delete_ids_in_batches(&tx, "videos", &refs)?;

            for id in &drop_channels { local_channels.remove(id); }
            for id in cascaded.iter().chain(orphans.iter()) { local_videos.remove(id); }
        }

        // Channels first, always: a video whose channel arrives in the same
        // archive needs its parent row in place before `videos.channel_id`'s
        // foreign key will accept it.
        for c in channels {
            let existed = local_channels.contains(&c.id);
            if existed && mode == ImportMode::Merge {
                report.channels_updated += tx.execute(
                    MERGE_CHANNEL,
                    params![c.id, c.subscribed as i64, c.member as i64],
                )?;
            } else {
                tx.execute(
                    PUT_CHANNEL,
                    params![c.id, c.title, c.handle, c.url, c.thumb_path,
                            c.subscribed as i64, c.member as i64, c.added_at,
                            c.last_polled_at],
                )?;
                if existed { report.channels_updated += 1 } else { report.channels_added += 1 }
            }
            local_channels.insert(c.id.clone());
        }

        for v in videos {
            let existed = local_videos.contains_key(&v.id);
            if existed && mode == ImportMode::Merge {
                report.videos_updated += tx.execute(
                    MERGE_VIDEO,
                    params![v.id, v.watched as i64, v.hidden as i64, v.added_manually as i64,
                            v.watched_at, v.sibling_group, v.thumb_path,
                            v.download_state.as_str(), v.file_path, v.downloaded_at,
                            v.series_hidden as i64],
                )?;
            } else {
                tx.execute(
                    PUT_VIDEO,
                    params![v.id, v.channel_id, v.title, v.description, v.thumb_url,
                            v.thumb_path, v.published_at, v.sort_at, v.feed_rank,
                            v.added_manually as i64, v.duration_secs, v.view_count,
                            v.status.as_str(), v.hidden as i64, v.watched as i64,
                            v.watched_at, v.download_state.as_str(), v.download_error,
                            v.file_path, v.downloaded_at, v.first_seen_at, v.sibling_group,
                            v.series_hidden as i64],
                )?;
                if existed { report.videos_updated += 1 } else { report.videos_added += 1 }
            }
            local_videos.insert(v.id.clone(), v.channel_id.clone());
        }

        tx.commit()?;
        Ok(report)
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;

    fn db() -> Db { Db::open_in_memory().unwrap() }

    fn chan(id: &str, title: &str) -> Channel {
        Channel { id: id.into(), title: title.into(), handle: Some(format!("@{id}")),
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 1_000,
                  last_polled_at: Some(2_000), terminated: false, auto_download: false }
    }

    fn video(id: &str, ch: &str, ch_title: &str) -> Video {
        Video { id: id.into(), channel_id: ch.into(), channel_title: ch_title.into(),
                title: format!("t-{id}"), description: None, thumb_url: None,
                thumb_path: None, published_at: Some(1_000), sort_at: Some(1_000),
                feed_rank: 0, added_manually: false, duration_secs: Some(600),
                view_count: Some(1), status: VideoStatus::Ready, hidden: false,
                watched: false, watched_at: None, download_state: DownloadState::None,
                download_error: None, file_path: None, downloaded_at: None,
                first_seen_at: 500, sibling_group: None, series_hidden: false }
    }

    /// Every user-owned column set to something distinguishable, so a round
    /// trip that drops one is a failed assertion rather than a coincidence.
    fn rich(id: &str, ch: &str, ch_title: &str) -> Video {
        let mut v = video(id, ch, ch_title);
        v.watched = true;
        v.watched_at = Some(9_999);
        v.hidden = true;
        v.series_hidden = true;
        v.added_manually = true;
        v.sibling_group = Some("k-1".into());
        v.thumb_path = Some(format!("/thumbs/{id}.jpg"));
        v.download_state = DownloadState::Done;
        v.file_path = Some(format!("/videos/{id}.mkv"));
        v.downloaded_at = Some(4_242);
        v.first_seen_at = 42;
        v
    }

    fn seed(d: &Db, channels: &[Channel], videos: &[Video]) -> ImportReport {
        d.apply_import(channels, videos, ImportMode::Merge, &[]).unwrap()
    }

    /// Every channel id an archive carries, which is what a Replace measures
    /// itself against. Deliberately spelled out separately from the ticked
    /// subset handed to `apply_import`: the two are different sets, and
    /// conflating them is exactly the bug these tests exist to keep out.
    fn archive(ids: &[&str]) -> Vec<String> { ids.iter().map(|s| (*s).to_string()).collect() }

    #[test]
    fn export_carries_the_ad_hoc_uploader_a_manual_video_hangs_from() {
        let d = db();
        let mut adhoc = chan("UC2", "Ad hoc");
        adhoc.subscribed = false;
        d.upsert_channel(&chan("UC1", "One")).unwrap();
        d.upsert_channel(&adhoc).unwrap();

        let ids: Vec<String> = d.export_channels().unwrap().into_iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), 2, "both, not just the subscription");
        assert!(ids.contains(&"UC2".to_string()));
        assert_eq!(d.list_subscribed_channels().unwrap().len(), 1, "for contrast");
    }

    #[test]
    fn the_estimate_counts_both_tables() {
        let d = db();
        seed(&d, &[chan("UC1", "One")], &[video("a", "UC1", "One"), video("b", "UC1", "One")]);
        assert_eq!(d.transfer_estimate().unwrap(), (1, 2));
    }

    #[test]
    fn an_archive_into_an_empty_library_reproduces_every_user_owned_column() {
        let src = db();
        let mut c = chan("UC1", "One");
        c.member = true;
        c.thumb_path = Some("/thumbs/UC1.jpg".into());
        seed(&src, &[c], &[rich("a", "UC1", "One")]);

        let dst = db();
        let report = dst
            .apply_import(
                &src.export_channels().unwrap(),
                &src.export_videos().unwrap(),
                ImportMode::Merge,
                &[],
            )
            .unwrap();
        assert_eq!((report.channels_added, report.videos_added), (1, 1));

        assert_eq!(dst.export_channels().unwrap(), src.export_channels().unwrap());
        assert_eq!(dst.export_videos().unwrap(), src.export_videos().unwrap());

        let ch = dst.get_channel("UC1").unwrap().unwrap();
        assert!(ch.subscribed && ch.member);
        let v = dst.get_video("a").unwrap().unwrap();
        assert!(v.watched && v.hidden && v.series_hidden && v.added_manually);
        assert_eq!(v.watched_at, Some(9_999));
        assert_eq!(v.sibling_group.as_deref(), Some("k-1"));
        assert_eq!(v.download_state, DownloadState::Done);
        assert_eq!(v.file_path.as_deref(), Some("/videos/a.mkv"));
        assert_eq!(v.downloaded_at, Some(4_242));
        assert_eq!(v.first_seen_at, 42);
    }

    #[test]
    fn merging_lifts_watched_and_hidden_but_leaves_the_local_title_and_date_alone() {
        let d = db();
        seed(&d, &[chan("UC1", "One")], &[video("a", "UC1", "One")]);

        let mut incoming = video("a", "UC1", "One");
        incoming.title = "the title the archive was written under".into();
        incoming.published_at = Some(1);
        incoming.sort_at = Some(1);
        incoming.duration_secs = Some(3);
        incoming.watched = true;
        incoming.watched_at = Some(77);
        incoming.hidden = true;
        let report = d
            .apply_import(&[chan("UC1", "One")], &[incoming], ImportMode::Merge, &[])
            .unwrap();

        assert_eq!(report.videos_added, 0);
        assert_eq!(report.videos_updated, 1);
        let v = d.get_video("a").unwrap().unwrap();
        assert_eq!(v.title, "t-a", "the local poll is the fresher source");
        assert_eq!(v.published_at, Some(1_000));
        assert_eq!(v.duration_secs, Some(600));
        assert!(v.watched && v.hidden);
        assert_eq!(v.watched_at, Some(77));
    }

    #[test]
    fn merging_never_un_watches_a_video() {
        let d = db();
        let mut local = video("a", "UC1", "One");
        local.watched = true;
        local.watched_at = Some(500);
        seed(&d, &[chan("UC1", "One")], &[local]);

        let report = d
            .apply_import(
                &[chan("UC1", "One")],
                &[video("a", "UC1", "One")],
                ImportMode::Merge,
                &[],
            )
            .unwrap();

        let v = d.get_video("a").unwrap().unwrap();
        assert!(v.watched, "an unwatched archive row cannot clear a local watch");
        assert_eq!(v.watched_at, Some(500));
        assert_eq!(report.videos_updated, 0, "nothing moved, so nothing is reported");
    }

    #[test]
    fn merging_never_downgrades_a_finished_download() {
        let d = db();
        let mut done = video("a", "UC1", "One");
        done.download_state = DownloadState::Done;
        done.file_path = Some("/videos/a.mkv".into());
        done.downloaded_at = Some(800);
        seed(&d, &[chan("UC1", "One")], &[done, video("b", "UC1", "One")]);

        // 'a' is finished here and absent there; 'b' is the other way round.
        let mut incoming_b = video("b", "UC1", "One");
        incoming_b.download_state = DownloadState::Done;
        incoming_b.file_path = Some("/videos/b.mkv".into());
        incoming_b.downloaded_at = Some(900);
        d.apply_import(
            &[chan("UC1", "One")],
            &[video("a", "UC1", "One"), incoming_b],
            ImportMode::Merge,
            &[],
        )
        .unwrap();

        let a = d.get_video("a").unwrap().unwrap();
        assert_eq!(a.download_state, DownloadState::Done);
        assert_eq!(a.file_path.as_deref(), Some("/videos/a.mkv"));
        assert_eq!(a.downloaded_at, Some(800));

        let b = d.get_video("b").unwrap().unwrap();
        assert_eq!(b.download_state, DownloadState::Done, "an incoming done does land");
        assert_eq!(b.file_path.as_deref(), Some("/videos/b.mkv"));
        assert_eq!(b.downloaded_at, Some(900));
    }

    #[test]
    fn merging_joins_a_membership_and_never_ends_one() {
        let d = db();
        let joined = Channel { member: true, ..chan("UC2", "Two") };
        seed(&d, &[chan("UC1", "One"), joined], &[]);

        let report = d
            .apply_import(
                &[Channel { member: true, ..chan("UC1", "One") }, chan("UC2", "Two")],
                &[],
                ImportMode::Merge,
                &[],
            )
            .unwrap();

        assert!(d.get_channel("UC1").unwrap().unwrap().member, "false -> true");
        assert!(d.get_channel("UC2").unwrap().unwrap().member, "true survives an incoming false");
        assert_eq!(report.channels_updated, 1, "only UC1 actually moved");
    }

    #[test]
    fn merging_the_same_archive_twice_changes_nothing_the_second_time() {
        let src = db();
        let c = Channel { member: true, ..chan("UC1", "One") };
        seed(&src, &[c], &[rich("a", "UC1", "One"), video("b", "UC1", "One")]);
        let archive_c = src.export_channels().unwrap();
        let archive_v = src.export_videos().unwrap();

        let dst = db();
        let first = dst.apply_import(&archive_c, &archive_v, ImportMode::Merge, &[]).unwrap();
        assert_eq!((first.channels_added, first.videos_added), (1, 2));
        let channels = dst.export_channels().unwrap();
        let videos = dst.export_videos().unwrap();

        let second = dst.apply_import(&archive_c, &archive_v, ImportMode::Merge, &[]).unwrap();
        assert_eq!(second, ImportReport::default(), "every count zero");
        assert_eq!(dst.export_channels().unwrap(), channels);
        assert_eq!(dst.export_videos().unwrap(), videos);
    }

    #[test]
    fn replacing_removes_a_channel_the_archive_never_had_and_its_videos_with_it() {
        let d = db();
        seed(
            &d,
            &[chan("UC1", "One"), chan("UC2", "Two")],
            &[video("a", "UC1", "One"), video("b", "UC1", "One"), video("c", "UC2", "Two")],
        );

        // UC2 is nowhere in the archive at all, which is the only thing that
        // makes it removable.
        let archive_ids = archive(&["UC1"]);
        let report = d
            .apply_import(
                &[chan("UC1", "One")],
                &[video("a", "UC1", "One"), video("b", "UC1", "One")],
                ImportMode::Replace,
                &archive_ids,
            )
            .unwrap();

        assert_eq!(report.channels_removed, 1);
        assert_eq!(report.videos_removed, 1, "the cascade is counted before it happens");
        assert!(d.get_channel("UC2").unwrap().is_none());
        assert!(d.get_video("c").unwrap().is_none());
        assert!(d.get_video("a").unwrap().is_some());
        assert_eq!(d.transfer_estimate().unwrap(), (1, 2));
    }

    #[test]
    fn replacing_removes_a_kept_channels_video_the_archive_lacks() {
        let d = db();
        seed(&d, &[chan("UC1", "One")], &[video("a", "UC1", "One"), video("b", "UC1", "One")]);

        let archive_ids = archive(&["UC1"]);
        let report = d
            .apply_import(
                &[chan("UC1", "One")],
                &[video("a", "UC1", "One")],
                ImportMode::Replace,
                &archive_ids,
            )
            .unwrap();

        assert_eq!(report.channels_removed, 0);
        assert_eq!(report.videos_removed, 1);
        assert_eq!(report.videos_updated, 1);
        assert!(d.get_video("b").unwrap().is_none());
        assert!(d.get_video("a").unwrap().is_some());
    }

    #[test]
    fn replacing_never_removes_a_channel_the_user_merely_left_unticked() {
        let d = db();
        seed(
            &d,
            &[chan("UC1", "One"), chan("UC2", "Two"), chan("UC3", "Three")],
            &[video("a", "UC1", "One"), video("b", "UC2", "Two"), video("c", "UC3", "Three")],
        );

        // The archive holds UC1 and UC2; the user ticked only UC1. UC3 is the
        // one the archive never heard of.
        let archive_ids = archive(&["UC1", "UC2"]);
        let report = d
            .apply_import(
                &[chan("UC1", "One")],
                &[video("a", "UC1", "One")],
                ImportMode::Replace,
                &archive_ids,
            )
            .unwrap();

        assert!(d.get_channel("UC2").unwrap().is_some(), "unticked is withheld, not condemned");
        assert!(d.get_video("b").unwrap().is_some(), "and its videos stay with it");
        assert_eq!(d.get_channel("UC2").unwrap().unwrap().title, "Two", "nor is it rewritten");
        assert!(d.get_channel("UC3").unwrap().is_none(), "absent from the archive, so it goes");
        assert!(d.get_video("c").unwrap().is_none());
        assert_eq!((report.channels_removed, report.videos_removed), (1, 1));
    }

    #[test]
    fn absent_from_counts_the_channels_and_videos_an_archive_never_mentions() {
        let d = db();
        seed(
            &d,
            &[chan("UC1", "One"), chan("UC2", "Two")],
            &[video("a", "UC1", "One"), video("b", "UC1", "One"), video("c", "UC2", "Two")],
        );
        assert_eq!(d.absent_from(&archive(&["UC1"])).unwrap(), (1, 1));
        assert_eq!(d.absent_from(&archive(&["UC2"])).unwrap(), (1, 2));
    }

    #[test]
    fn an_empty_archive_leaves_the_whole_library_absent() {
        let d = db();
        seed(
            &d,
            &[chan("UC1", "One"), chan("UC2", "Two")],
            &[video("a", "UC1", "One"), video("b", "UC1", "One"), video("c", "UC2", "Two")],
        );
        assert_eq!(d.absent_from(&[]).unwrap(), (2, 3));
    }

    #[test]
    fn an_archive_naming_more_than_is_here_leaves_nothing_absent() {
        let d = db();
        seed(&d, &[chan("UC1", "One")], &[video("a", "UC1", "One")]);
        assert_eq!(d.absent_from(&archive(&["UC1", "UC2", "UC3"])).unwrap(), (0, 0));
    }

    #[test]
    fn replacing_overwrites_the_metadata_a_merge_would_have_kept() {
        let d = db();
        let mut local = video("a", "UC1", "One");
        local.watched = true;
        local.watched_at = Some(500);
        seed(&d, &[chan("UC1", "One")], &[local]);

        let mut incoming = video("a", "UC1", "One");
        incoming.title = "the archive's title".into();
        incoming.published_at = Some(1);
        incoming.duration_secs = Some(3);
        let archive_ids = archive(&["UC1"]);
        let report = d
            .apply_import(&[chan("UC1", "Renamed")], &[incoming], ImportMode::Replace, &archive_ids)
            .unwrap();

        assert_eq!((report.channels_updated, report.videos_updated), (1, 1));
        let v = d.get_video("a").unwrap().unwrap();
        assert_eq!(v.title, "the archive's title");
        assert_eq!(v.published_at, Some(1));
        assert_eq!(v.duration_secs, Some(3));
        assert!(!v.watched, "incoming wins outright, unlike a merge");
        assert_eq!(v.watched_at, None);
        assert_eq!(d.get_channel("UC1").unwrap().unwrap().title, "Renamed");
    }

    #[test]
    fn a_video_arrives_with_the_channel_it_hangs_from_in_the_same_archive() {
        let d = db();
        let report = d
            .apply_import(
                &[chan("UC9", "Nine")],
                &[video("x", "UC9", "Nine")],
                ImportMode::Merge,
                &[],
            )
            .unwrap();

        assert_eq!((report.channels_added, report.videos_added), (1, 1));
        assert_eq!(d.get_video("x").unwrap().unwrap().channel_title, "Nine");
    }

    #[test]
    fn a_failed_row_rolls_the_whole_archive_back() {
        let d = db();
        seed(&d, &[chan("UC1", "One")], &[video("a", "UC1", "One")]);

        // The second video names a channel neither the archive nor the library
        // carries, so the foreign key refuses it and takes the first one down
        // with it.
        let err = d.apply_import(
            &[chan("UC1", "One")],
            &[video("b", "UC1", "One"), video("c", "UC404", "Gone")],
            ImportMode::Merge,
            &[],
        );
        assert!(err.is_err());
        assert!(d.get_video("b").unwrap().is_none(), "half an import is no import");
        assert_eq!(d.transfer_estimate().unwrap(), (1, 1));
    }

    #[test]
    fn merging_lifts_a_series_mark_and_never_lowers_one() {
        let d = db();
        let marked = Video { series_hidden: true, ..video("b", "UC1", "One") };
        seed(&d, &[chan("UC1", "One")], &[video("a", "UC1", "One"), marked]);

        // The archive has it the other way round.
        let report = d
            .apply_import(
                &[chan("UC1", "One")],
                &[Video { series_hidden: true, ..video("a", "UC1", "One") }, video("b", "UC1", "One")],
                ImportMode::Merge,
                &[],
            )
            .unwrap();

        let (a, b) = (d.get_video("a").unwrap().unwrap(), d.get_video("b").unwrap().unwrap());
        assert!(a.series_hidden, "false -> true");
        assert!(b.series_hidden, "true survives an incoming false");
        assert!(!a.hidden, "the mark is lifted on its own, never the hide with it");
        assert_eq!(report.videos_updated, 1, "only 'a' actually moved");
    }

    #[test]
    fn replacing_applies_a_series_mark_verbatim_either_way() {
        let d = db();
        let marked = Video { series_hidden: true, ..video("a", "UC1", "One") };
        seed(&d, &[chan("UC1", "One")], &[marked, video("b", "UC1", "One")]);

        d.apply_import(
            &[chan("UC1", "One")],
            &[video("a", "UC1", "One"), Video { series_hidden: true, ..video("b", "UC1", "One") }],
            ImportMode::Replace,
            &archive(&["UC1"]),
        )
        .unwrap();

        assert!(!d.get_video("a").unwrap().unwrap().series_hidden, "incoming wins outright");
        assert!(d.get_video("b").unwrap().unwrap().series_hidden);
    }
}

#[cfg(test)]
mod auto_download_tests {
    use super::*;

    fn db() -> Db { Db::open_in_memory().unwrap() }

    fn chan(id: &str) -> Channel {
        Channel { id: id.into(), title: format!("T-{id}"), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 0,
                  last_polled_at: None, terminated: false, auto_download: false }
    }

    /// The switch-on instant, rewritten to something fixed so a test can place
    /// rows either side of it without racing the clock.
    const SINCE: i64 = 1_000_000;

    fn switch_on(d: &Db, ch: &str) {
        d.set_channel_auto_download(ch, true).unwrap();
        let conn = d.conn.lock().unwrap();
        conn.execute("UPDATE channels SET auto_download_since=?2 WHERE id=?1",
                     params![ch, SINCE]).unwrap();
    }

    /// A ready, unwatched, visible, undownloaded row, first seen and published
    /// at the given instants -- the two clocks the candidate rule turns on.
    fn row(d: &Db, id: &str, ch: &str, first_seen: i64, published: Option<i64>) {
        let conn = d.conn.lock().unwrap();
        conn.execute(
            "INSERT INTO videos (id,channel_id,title,published_at,sort_at,status,first_seen_at)
             VALUES (?1,?2,?3,?4,?4,'ready',?5)",
            params![id, ch, format!("t-{id}"), published, first_seen]).unwrap();
    }

    fn set(d: &Db, id: &str, assignment: &str) {
        let conn = d.conn.lock().unwrap();
        conn.execute(&format!("UPDATE videos SET {assignment} WHERE id=?1"), params![id]).unwrap();
    }

    fn sorted(mut v: Vec<String>) -> Vec<String> { v.sort(); v }

    #[test]
    fn migrating_a_v7_database_adds_both_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(
            "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN downloaded_at INTEGER;
             ALTER TABLE videos ADD COLUMN sibling_group TEXT;
             ALTER TABLE channels ADD COLUMN member INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE channels ADD COLUMN backfill_depth INTEGER;
             ALTER TABLE channels ADD COLUMN terminated_at INTEGER;",
        ).unwrap();
        conn.pragma_update(None, "user_version", 7).unwrap();
        conn.execute(
            "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7)",
            [],
        ).unwrap();
        conn.execute(
            "INSERT INTO videos (id,channel_id,title,published_at,sort_at,status,first_seen_at)
             VALUES ('a','UC1','kept',100,100,'ready',1)",
            [],
        ).unwrap();

        let db = Db::init(conn).unwrap();
        {
            let c = db.conn.lock().unwrap();
            assert!(column_exists(&c, "channels", "auto_download_since").unwrap());
            assert!(column_exists(&c, "videos", "auto_queued").unwrap());
            let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, SCHEMA_VERSION);
        }
        assert!(!db.list_channels().unwrap()[0].auto_download, "nothing is switched on by an upgrade");
        assert!(db.auto_download_candidates().unwrap().is_empty());
    }

    #[test]
    fn switching_on_and_off_is_read_back_on_the_channel() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        d.set_channel_auto_download("UC1", true).unwrap();
        assert!(d.get_channel("UC1").unwrap().unwrap().auto_download);
        d.set_channel_auto_download("UC1", false).unwrap();
        assert!(!d.get_channel("UC1").unwrap().unwrap().auto_download);
    }

    #[test]
    fn a_new_upload_first_seen_after_switch_on_is_a_candidate() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        switch_on(&d, "UC1");
        row(&d, "new", "UC1", SINCE + 60, Some(SINCE + 30));
        row(&d, "old", "UC1", SINCE - 60, Some(SINCE - 90));
        assert_eq!(d.auto_download_candidates().unwrap(), vec!["new".to_string()]);
    }

    #[test]
    fn candidates_skip_every_row_and_channel_the_rule_excludes() {
        let d = db();
        for ch in ["UC1", "UNSUB", "TERM", "OFF"] {
            let mut c = chan(ch);
            c.subscribed = ch != "UNSUB";
            d.upsert_channel(&c).unwrap();
        }
        for ch in ["UC1", "UNSUB", "TERM"] { switch_on(&d, ch); }
        d.set_channel_terminated("TERM").unwrap();

        let fresh = (SINCE + 60, Some(SINCE + 30));
        row(&d, "ok", "UC1", fresh.0, fresh.1);
        for id in ["hidden", "watched", "queued_before", "pending", "done", "failed", "downloading"] {
            row(&d, id, "UC1", fresh.0, fresh.1);
        }
        set(&d, "hidden", "hidden=1");
        set(&d, "watched", "watched=1");
        set(&d, "queued_before", "auto_queued=1");
        set(&d, "pending", "status='pending'");
        set(&d, "done", "download_state='done'");
        set(&d, "failed", "download_state='failed'");
        set(&d, "downloading", "download_state='downloading'");
        row(&d, "unsub", "UNSUB", fresh.0, fresh.1);
        row(&d, "term", "TERM", fresh.0, fresh.1);
        row(&d, "off", "OFF", fresh.0, fresh.1);

        assert_eq!(d.auto_download_candidates().unwrap(), vec!["ok".to_string()]);
    }

    /// A raised `backfill_count` or a members-only join inserts rows now that
    /// aired long ago. First seen after the switch, but not new uploads.
    #[test]
    fn a_deepened_row_first_seen_now_but_published_long_ago_is_not_a_candidate() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        switch_on(&d, "UC1");
        row(&d, "deep", "UC1", SINCE + 60, Some(SINCE - 30 * 86_400));
        assert!(d.auto_download_candidates().unwrap().is_empty());
    }

    /// A listing-ingested upload wears a midnight-UTC bucket until its real
    /// date arrives, which can land up to a day before the real instant.
    #[test]
    fn a_midnight_bucket_within_a_day_of_the_switch_is_still_a_candidate() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        switch_on(&d, "UC1");
        let bucket = SINCE - SINCE % 86_400; // midnight on the switch-on day
        row(&d, "bucketed", "UC1", SINCE + 60, Some(bucket));
        row(&d, "undated", "UC1", SINCE + 60, None);
        assert_eq!(sorted(d.auto_download_candidates().unwrap()),
                   vec!["bucketed".to_string(), "undated".to_string()]);
    }

    #[test]
    fn marking_rows_auto_queued_takes_them_out_of_the_candidates_for_good() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        switch_on(&d, "UC1");
        row(&d, "a", "UC1", SINCE + 60, Some(SINCE + 30));
        row(&d, "b", "UC1", SINCE + 60, Some(SINCE + 30));
        d.mark_auto_queued(&["a".to_string()]).unwrap();
        assert_eq!(d.auto_download_candidates().unwrap(), vec!["b".to_string()]);

        // A failed download, or a deleted file, lands back on 'none'; the flag
        // is what keeps the sweep from queueing it again.
        d.mark_auto_queued(&["b".to_string()]).unwrap();
        d.set_download_state("b", DownloadState::Failed, None, Some("boom")).unwrap();
        d.set_download_state("b", DownloadState::None, None, None).unwrap();
        assert!(d.auto_download_candidates().unwrap().is_empty());
    }

    #[test]
    fn the_backlog_honours_its_ticks_and_skips_what_is_not_there_to_download() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        d.upsert_channel(&chan("UC2")).unwrap();
        for id in ["plain", "watched", "hidden", "both", "done", "failed", "pending", "queued"] {
            row(&d, id, "UC1", 10, Some(10));
        }
        row(&d, "other", "UC2", 10, Some(10));
        set(&d, "watched", "watched=1");
        set(&d, "hidden", "hidden=1");
        set(&d, "both", "watched=1, hidden=1");
        set(&d, "done", "download_state='done'");
        set(&d, "failed", "download_state='failed'");
        set(&d, "pending", "status='pending'");
        // Already auto-queued once: the backlog is an explicit ask, so it counts.
        set(&d, "queued", "auto_queued=1");

        let s = |w, h| sorted(d.auto_download_backlog("UC1", w, h).unwrap());
        assert_eq!(s(false, false), vec!["plain", "queued"]);
        assert_eq!(s(true, false), vec!["plain", "queued", "watched"]);
        assert_eq!(s(false, true), vec!["hidden", "plain", "queued"]);
        assert_eq!(s(true, true), vec!["both", "hidden", "plain", "queued", "watched"]);
    }

    #[test]
    fn an_upsert_does_not_switch_auto_download_off() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        switch_on(&d, "UC1");
        d.upsert_channel(&Channel { title: "Renamed".into(), ..chan("UC1") }).unwrap();
        let c = d.get_channel("UC1").unwrap().unwrap();
        assert_eq!(c.title, "Renamed");
        assert!(c.auto_download);
        row(&d, "a", "UC1", SINCE + 60, Some(SINCE + 30));
        assert_eq!(d.auto_download_candidates().unwrap(), vec!["a".to_string()],
                   "and the switch-on instant is kept, not restamped");
    }

    fn archived_video(id: &str, ch: &str) -> Video {
        Video { id: id.into(), channel_id: ch.into(), channel_title: String::new(),
                title: format!("t-{id}"), description: None, thumb_url: None,
                thumb_path: None, published_at: Some(SINCE + 30), sort_at: Some(SINCE + 30),
                feed_rank: 0, added_manually: false, duration_secs: Some(600),
                view_count: Some(1), status: VideoStatus::Ready, hidden: false,
                watched: false, watched_at: None, download_state: DownloadState::None,
                download_error: None, file_path: None, downloaded_at: None,
                first_seen_at: SINCE + 60, sibling_group: None, series_hidden: false }
    }

    #[test]
    fn an_import_leaves_auto_download_and_auto_queued_alone_on_existing_rows() {
        for mode in [ImportMode::Merge, ImportMode::Replace] {
            let d = db();
            d.upsert_channel(&chan("UC1")).unwrap();
            switch_on(&d, "UC1");
            row(&d, "a", "UC1", SINCE + 60, Some(SINCE + 30));
            row(&d, "b", "UC1", SINCE + 60, Some(SINCE + 30));
            d.mark_auto_queued(&["a".to_string()]).unwrap();

            // An archive claiming the opposite, from a machine that never
            // switched anything on.
            let incoming = Channel { member: true, ..chan("UC1") };
            d.apply_import(&[incoming], &[archived_video("a", "UC1"), archived_video("b", "UC1")],
                           mode, &["UC1".to_string()]).unwrap();

            assert!(d.get_channel("UC1").unwrap().unwrap().auto_download, "{mode:?}");
            assert_eq!(d.auto_download_candidates().unwrap(), vec!["b".to_string()],
                       "{mode:?}: the switch-on instant and the auto_queued flag both survived");
        }
    }

    #[test]
    fn an_imported_channel_new_here_arrives_switched_off() {
        let d = db();
        let incoming = Channel { auto_download: true, ..chan("UC1") };
        d.apply_import(&[incoming], &[], ImportMode::Merge, &[]).unwrap();
        assert!(!d.get_channel("UC1").unwrap().unwrap().auto_download);
    }
}

// ---- sync ----
//
// The two halves of `sync::snapshot`'s contract. See
// docs/superpowers/specs/2026-10-09-cloud-sync-design.md, section 3.

use crate::sync::snapshot::{
    self, ApplyReport, Snapshot, SyncChannel, SyncVideo, Tombstone, TombstoneKind, SNAPSHOT_FORMAT,
};

/// The rows a snapshot carries: every manual add, and every row whose watched,
/// hidden, sibling or hidden-series state anyone ever changed. A stamp outlives
/// the change it records -- a watch taken back keeps its stamp -- which is how
/// an un-watch travels at all.
const SYNC_TOUCHED: &str = "(added_manually <> 0 OR watched_changed_at IS NOT NULL
     OR hidden_changed_at IS NOT NULL OR sibling_changed_at IS NOT NULL
     OR series_hidden_changed_at IS NOT NULL)";

const SYNC_VIDEO_COLUMNS: &str = "id, channel_id, title, published_at, sort_at, duration_secs,
     added_manually, first_seen_at, watched, watched_at, watched_changed_at,
     hidden, hidden_changed_at, sibling_group, sibling_changed_at,
     series_hidden, series_hidden_changed_at";

const SYNC_CHANNEL_COLUMNS: &str =
    "id, title, handle, url, subscribed, subscribed_changed_at, member, member_changed_at";

fn map_sync_video(r: &Row) -> rusqlite::Result<SyncVideo> {
    Ok(SyncVideo {
        id: r.get(0)?, channel_id: r.get(1)?, title: r.get(2)?,
        published_at: r.get(3)?, sort_at: r.get(4)?, duration_secs: r.get(5)?,
        added_manually: r.get(6)?, added_at: r.get(7)?,
        watched: r.get(8)?, watched_at: r.get(9)?, watched_changed_at: r.get(10)?,
        hidden: r.get(11)?, hidden_changed_at: r.get(12)?,
        sibling_group: r.get(13)?, sibling_changed_at: r.get(14)?,
        series_hidden: r.get(15)?, series_hidden_changed_at: r.get(16)?,
    })
}

fn map_sync_channel(r: &Row) -> rusqlite::Result<SyncChannel> {
    Ok(SyncChannel {
        id: r.get(0)?, title: r.get(1)?, handle: r.get(2)?, url: r.get(3)?,
        subscribed: r.get(4)?, subscribed_changed_at: r.get(5)?,
        member: r.get(6)?, member_changed_at: r.get(7)?,
    })
}

fn kind_name(kind: TombstoneKind) -> &'static str {
    match kind {
        TombstoneKind::Channel => "channel",
        TombstoneKind::Video => "video",
    }
}

fn read_tombstones(conn: &Connection, kind: TombstoneKind) -> Result<HashMap<String, i64>> {
    let mut st = conn.prepare("SELECT id, removed_at FROM tombstones WHERE kind=?1")?;
    let rows = st.query_map(params![kind_name(kind)], |r| Ok((r.get(0)?, r.get(1)?)))?;
    Ok(rows.collect::<rusqlite::Result<HashMap<_, _>>>()?)
}

/// Whether a tombstone dated `removed_at` covers a copy stamped `stamp`. At or
/// before counts: a removal in the same second as the change it removes is
/// still the later of the two, and the delete triggers draw the line in the
/// same place. A NULL stamp is older than any removal.
fn covered(removed_at: Option<i64>, stamp: Option<i64>) -> bool {
    removed_at.is_some_and(|t| t >= stamp.unwrap_or(0))
}

/// A copy of a channel whose subscription a tombstone has removed. Only a
/// subscription can be: an ad-hoc uploader -- including one added back by hand
/// after its subscription went -- is not what any channel tombstone records.
fn removed_subscription(c: &SyncChannel, tombstones: &HashMap<String, i64>) -> bool {
    c.subscribed && covered(tombstones.get(&c.id).copied(), c.subscribed_changed_at)
}

/// Everything about a channel that sync owns.
fn channel_state(c: &SyncChannel) -> (bool, Option<i64>, bool, Option<i64>) {
    (c.subscribed, c.subscribed_changed_at, c.member, c.member_changed_at)
}

/// Everything about a video that sync owns. `added_at` is `first_seen_at`.
#[allow(clippy::type_complexity)]
fn video_state(v: &SyncVideo)
    -> (bool, Option<i64>, Option<i64>, bool, Option<i64>, Option<&str>, Option<i64>, bool, i64,
        bool, Option<i64>) {
    (v.watched, v.watched_at, v.watched_changed_at, v.hidden, v.hidden_changed_at,
     v.sibling_group.as_deref(), v.sibling_changed_at, v.added_manually, v.added_at,
     v.series_hidden, v.series_hidden_changed_at)
}

/// `removed_at` only ever moves forward, so a second apply of the same
/// snapshots writes nothing -- the WHERE is what keeps `changes()` honest.
const UPSERT_TOMBSTONE: &str = "
INSERT INTO tombstones (kind, id, removed_at) VALUES (?1, ?2, ?3)
ON CONFLICT (kind, id) DO UPDATE SET removed_at = excluded.removed_at
 WHERE excluded.removed_at > tombstones.removed_at";

/// The manual uploads of a channel about to go, tombstoned as of its removal --
/// or of their own add, for one added after it by a machine that had not yet
/// heard. Either way the time comes from synced data: left to the delete
/// trigger, it would be this machine's clock, and two machines would date one
/// removal apart.
const TOMBSTONE_MANUAL_UPLOADS: &str = "
INSERT INTO tombstones (kind, id, removed_at)
SELECT 'video', id, MAX(?2, first_seen_at) FROM videos
 WHERE channel_id = ?1 AND added_manually <> 0
ON CONFLICT (kind, id) DO UPDATE SET removed_at = excluded.removed_at
 WHERE excluded.removed_at > tombstones.removed_at";

/// `added_at` is this machine's own history and nothing sync carries, so a
/// channel arriving now was added now.
const INSERT_SYNC_CHANNEL: &str = "
INSERT INTO channels (id, title, handle, url, subscribed, subscribed_changed_at,
                      member, member_changed_at, added_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)";

const SET_SYNC_CHANNEL: &str = "
UPDATE channels SET subscribed = ?2, subscribed_changed_at = ?3,
                    member = ?4, member_changed_at = ?5
 WHERE id = ?1";

/// Run after [`SET_SYNC_CHANNEL`]. A pair that wins a tie on the stamp keeps
/// the stamp the row already had, and the triggers cannot tell that from a
/// local edit that left the stamp alone, so they restamp it with this clock.
/// Putting the copied stamp back undoes exactly that. A NULL winner (only ever
/// a malformed snapshot) is left to the trigger, whose stamp is then the truth.
const RESTAMP_SYNC_CHANNEL: &str = "
UPDATE channels SET subscribed_changed_at = COALESCE(?2, subscribed_changed_at),
                    member_changed_at     = COALESCE(?3, member_changed_at)
 WHERE id = ?1";

const INSERT_SYNC_VIDEO: &str = "
INSERT INTO videos (id, channel_id, title, thumb_url, published_at, sort_at, feed_rank,
                    added_manually, duration_secs, status, first_seen_at,
                    watched, watched_at, watched_changed_at, hidden, hidden_changed_at,
                    sibling_group, sibling_changed_at, series_hidden, series_hidden_changed_at)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7, ?8, 'ready', ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16,
        ?17, ?18)";

const SET_SYNC_VIDEO: &str = "
UPDATE videos SET watched = ?2, watched_at = ?3, watched_changed_at = ?4,
                  hidden = ?5, hidden_changed_at = ?6,
                  sibling_group = ?7, sibling_changed_at = ?8,
                  added_manually = ?9, first_seen_at = ?10,
                  series_hidden = ?11, series_hidden_changed_at = ?12
 WHERE id = ?1";

/// [`RESTAMP_SYNC_CHANNEL`], for a video.
const RESTAMP_SYNC_VIDEO: &str = "
UPDATE videos SET watched_changed_at = COALESCE(?2, watched_changed_at),
                  hidden_changed_at  = COALESCE(?3, hidden_changed_at),
                  sibling_changed_at = COALESCE(?4, sibling_changed_at),
                  series_hidden_changed_at = COALESCE(?5, series_hidden_changed_at)
 WHERE id = ?1";

fn insert_sync_channel(conn: &Connection, c: &SyncChannel) -> Result<()> {
    conn.execute(
        INSERT_SYNC_CHANNEL,
        params![c.id, c.title, c.handle, c.url, c.subscribed as i64, c.subscribed_changed_at,
                c.member as i64, c.member_changed_at, now()],
    )?;
    Ok(())
}

fn insert_sync_video(conn: &Connection, v: &SyncVideo) -> Result<()> {
    conn.execute(
        INSERT_SYNC_VIDEO,
        params![v.id, v.channel_id, v.title, crate::ytdlp::thumb_url_for(&v.id),
                v.published_at, v.sort_at.or(v.published_at), v.added_manually as i64,
                v.duration_secs, v.added_at, v.watched as i64, v.watched_at,
                v.watched_changed_at, v.hidden as i64, v.hidden_changed_at,
                v.sibling_group, v.sibling_changed_at, v.series_hidden as i64,
                v.series_hidden_changed_at],
    )?;
    Ok(())
}

impl Db {
    /// This machine's slim snapshot: subscribed channels, touched videos and
    /// every tombstone, each list ordered by id so the same library always
    /// serialises to the same bytes -- the sync clock hashes them to tell
    /// whether there is anything to push.
    ///
    /// A channel rides along when it is a subscription, when its membership
    /// was ever changed, or when a video in the snapshot hangs from it: an
    /// ad-hoc uploader travels only as the parent its manual video needs.
    pub fn sync_snapshot(&self, machine_name: &str) -> Result<Snapshot> {
        let mut conn = self.conn.lock().unwrap();
        // One read transaction, so the three lists describe one instant even
        // with a script writing to the same file beside us.
        let tx = conn.transaction()?;
        let videos = {
            let mut st = tx.prepare(&format!(
                "SELECT {SYNC_VIDEO_COLUMNS} FROM videos WHERE {SYNC_TOUCHED} ORDER BY id"))?;
            let rows = st.query_map([], map_sync_video)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let channels = {
            let mut st = tx.prepare(&format!(
                "SELECT {SYNC_CHANNEL_COLUMNS} FROM channels
                  WHERE subscribed <> 0 OR member_changed_at IS NOT NULL
                     OR id IN (SELECT channel_id FROM videos WHERE {SYNC_TOUCHED})
                  ORDER BY id"))?;
            let rows = st.query_map([], map_sync_channel)?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let tombstones = {
            // `kind` sorts as text, which is the order `TombstoneKind` derives.
            let mut st = tx.prepare("SELECT kind, id, removed_at FROM tombstones ORDER BY kind, id")?;
            let rows = st.query_map([], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?))
            })?;
            let mut out = Vec::new();
            for row in rows {
                let (kind, id, removed_at) = row?;
                let kind = match kind.as_str() {
                    "channel" => TombstoneKind::Channel,
                    "video" => TombstoneKind::Video,
                    // Nothing this build writes, so nothing it can speak for.
                    _ => continue,
                };
                out.push(Tombstone { kind, id, removed_at });
            }
            out
        };
        tx.commit()?;
        Ok(Snapshot {
            format: SNAPSHOT_FORMAT,
            machine_name: machine_name.to_string(),
            channels,
            videos,
            tombstones,
        })
    }

    /// Merges the other machines' snapshots into the library, by per-field
    /// last writer wins, in one transaction -- one lock, like `apply_import`,
    /// and a failure halfway leaves the library as it was.
    ///
    /// **It never stamps with this machine's clock.** Every stamp and every
    /// tombstone it writes is copied verbatim from what arrived, which is what
    /// makes it idempotent (the same snapshots a second time write nothing and
    /// report `changed == false`) and convergent (machines that have seen the
    /// same snapshots hold the same library, whatever order they saw them in).
    /// The local library is folded into the comparison as one more copy, by
    /// the very functions `snapshot::merge` uses.
    ///
    /// Only sync's own columns are written on a row already here: a title,
    /// date, status or download, and a channel's handle, url, polling and
    /// auto-download, are this machine's poll's and this machine's user's.
    /// `first_seen_at` is the one date that moves, and only forward, to the
    /// latest copy's: it is what a video tombstone is measured against, so a
    /// copy added again after a delete must not be taken for the one deleted;
    /// and with it in step, two libraries' snapshots converge byte for byte.
    pub fn apply_sync(&self, snapshots: &[Snapshot]) -> Result<ApplyReport> {
        // The caller skips and reports a slot from a newer build; skipping it
        // here too means one that slips through is never half-understood.
        let merged = snapshot::merge(snapshots.iter().filter(|s| s.format <= SNAPSHOT_FORMAT));

        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction()?;
        let mut report = ApplyReport::default();
        let mut writes = 0usize;

        // 1. Tombstones, before anything is deleted. A delete trigger keeps a
        //    tombstone that already covers the row it removes, so landing the
        //    remote ones first is what keeps their times instead of this clock's.
        {
            let mut st = tx.prepare(UPSERT_TOMBSTONE)?;
            for t in &merged.tombstones {
                writes += st.execute(params![kind_name(t.kind), t.id, t.removed_at])?;
            }
        }
        let channel_tombstones = read_tombstones(&tx, TombstoneKind::Channel)?;

        // 2. Channels. A removed subscription is skipped whole: the local row,
        //    if it is one, goes in step 3. Otherwise the local row takes the
        //    winning pairs, and a channel missing here arrives only as a
        //    subscription -- an uploader waits for a video that needs it.
        {
            let mut read = tx.prepare(&format!(
                "SELECT {SYNC_CHANNEL_COLUMNS} FROM channels WHERE id=?1"))?;
            for incoming in &merged.channels {
                let local = read.query_row(params![incoming.id], map_sync_channel).optional()?;
                let joined = match &local {
                    Some(l) => snapshot::merge_channel(&[l, incoming]),
                    None => incoming.clone(),
                };
                if removed_subscription(&joined, &channel_tombstones) {
                    continue;
                }
                match local {
                    Some(l) if channel_state(&l) != channel_state(&joined) => {
                        tx.execute(SET_SYNC_CHANNEL, params![
                            joined.id, joined.subscribed as i64, joined.subscribed_changed_at,
                            joined.member as i64, joined.member_changed_at])?;
                        tx.execute(RESTAMP_SYNC_CHANNEL, params![
                            joined.id, joined.subscribed_changed_at, joined.member_changed_at])?;
                        writes += 1;
                        // An uploader here that another machine subscribed to
                        // wants polling as much as a channel that just arrived.
                        if joined.subscribed && !l.subscribed {
                            report.channels_added.push(joined.id.clone());
                        }
                    }
                    Some(_) => {}
                    None if joined.subscribed => {
                        insert_sync_channel(&tx, &joined)?;
                        writes += 1;
                        report.channels_added.push(joined.id.clone());
                    }
                    None => {}
                }
            }
        }

        // 3. Local subscriptions a tombstone covers go, and the cascade takes
        //    their uploads -- manual ones too, exactly as a local removal would.
        let doomed: Vec<(String, i64)> = {
            let mut st = tx.prepare(
                "SELECT c.id, t.removed_at FROM channels c
                   JOIN tombstones t ON t.kind = 'channel' AND t.id = c.id
                  WHERE c.subscribed <> 0
                    AND COALESCE(c.subscribed_changed_at, 0) <= t.removed_at")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, removed_at) in &doomed {
            tx.execute(TOMBSTONE_MANUAL_UPLOADS, params![id, removed_at])?;
            writes += tx.execute("DELETE FROM channels WHERE id=?1", params![id])?;
        }

        // 4. Videos.
        let video_tombstones = read_tombstones(&tx, TombstoneKind::Video)?;
        let incoming_channels: HashMap<&str, &SyncChannel> =
            merged.channels.iter().map(|c| (c.id.as_str(), c)).collect();
        // What is here once the removals are done: id -> subscribed.
        let mut here: HashMap<String, bool> = {
            let mut st = tx.prepare("SELECT id, subscribed FROM channels")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<HashMap<_, _>>>()?
        };
        {
            let mut read = tx.prepare(&format!(
                "SELECT {SYNC_VIDEO_COLUMNS} FROM videos WHERE id=?1"))?;
            for incoming in &merged.videos {
                if let Some(local) = read.query_row(params![incoming.id], map_sync_video).optional()? {
                    let joined = snapshot::merge_video(&[&local, incoming]);
                    if video_state(&local) != video_state(&joined) {
                        tx.execute(SET_SYNC_VIDEO, params![
                            joined.id, joined.watched as i64, joined.watched_at,
                            joined.watched_changed_at, joined.hidden as i64,
                            joined.hidden_changed_at, joined.sibling_group,
                            joined.sibling_changed_at, joined.added_manually as i64,
                            joined.added_at, joined.series_hidden as i64,
                            joined.series_hidden_changed_at])?;
                        tx.execute(RESTAMP_SYNC_VIDEO, params![
                            joined.id, joined.watched_changed_at, joined.hidden_changed_at,
                            joined.sibling_changed_at, joined.series_hidden_changed_at])?;
                        writes += 1;
                    }
                    continue;
                }

                // Missing here. An upload of a removed subscription stays
                // gone -- unless this library subscribes to it again, more
                // recently than the removal -- and so does a video whose own
                // tombstone covers this copy.
                let parent = incoming_channels.get(incoming.channel_id.as_str()).copied();
                let subscribed_here = here.get(&incoming.channel_id).copied();
                if parent.is_some_and(|c| removed_subscription(c, &channel_tombstones))
                    && subscribed_here != Some(true)
                {
                    continue;
                }
                if covered(video_tombstones.get(&incoming.id).copied(), Some(incoming.added_at)) {
                    continue;
                }
                if subscribed_here.is_none() {
                    // An uploader with no subscription behind it, inserted only
                    // now that a video needs it, so the foreign key holds.
                    let Some(parent) = parent else { continue };
                    insert_sync_channel(&tx, parent)?;
                    here.insert(parent.id.clone(), parent.subscribed);
                    writes += 1;
                }
                insert_sync_video(&tx, incoming)?;
                writes += 1;
                report.videos_added.push(incoming.id.clone());
            }
        }

        // 5. Local manual videos a tombstone covers go, measured against the
        //    `first_seen_at` step 4 may just have moved on; then any uploader
        //    left with nothing, by the same rule a local delete prunes by.
        let condemned: Vec<(String, String)> = {
            let mut st = tx.prepare(
                "SELECT v.id, v.channel_id FROM videos v
                   JOIN tombstones t ON t.kind = 'video' AND t.id = v.id
                  WHERE v.added_manually <> 0 AND v.first_seen_at <= t.removed_at")?;
            let rows = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut uploaders: Vec<&str> = Vec::new();
        for (id, channel) in &condemned {
            writes += tx.execute("DELETE FROM videos WHERE id=?1", params![id])?;
            uploaders.push(channel);
        }
        uploaders.sort_unstable();
        uploaders.dedup();
        for channel in uploaders {
            writes += tx.execute(PRUNE_ORPHAN_CHANNEL, params![channel])?;
        }

        tx.commit()?;
        report.changed = writes > 0;
        Ok(report)
    }
}

#[cfg(test)]
mod series_tests {
    use super::*;
    use std::ops::RangeInclusive;

    const CH: &str = "UC1";

    fn chan(id: &str) -> Channel {
        Channel { id: id.into(), title: format!("T-{id}"), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 0,
                  last_polled_at: None, terminated: false, auto_download: false }
    }

    fn row(id: &str, title: &str) -> NewVideo {
        NewVideo { id: id.into(), channel_id: CH.into(), title: title.into(),
                   description: None, thumb_url: None, published_at: Some(1_000),
                   sort_at: Some(1_000), feed_rank: 0, added_manually: false,
                   duration_secs: Some(600), view_count: Some(1), status: VideoStatus::Ready }
    }

    fn db() -> Db {
        let d = Db::open_in_memory().unwrap();
        d.upsert_channel(&chan(CH)).unwrap();
        d
    }

    fn add(d: &Db, v: NewVideo) { assert!(d.insert_video_if_new(&v).unwrap()); }

    /// The Blackwood Tapes, parts 1 and 2, and an unrelated upload beside them.
    fn tapes(d: &Db) {
        add(d, row("p1", "The Blackwood Tapes Pt 1"));
        add(d, row("p2", "The Blackwood Tapes Pt 2"));
        add(d, row("other", "Cooking With Gas"));
    }

    fn sql(d: &Db, s: &str) { d.conn.lock().unwrap().execute_batch(s).unwrap(); }

    /// `(hidden, series_hidden)`.
    fn flags(d: &Db, id: &str) -> (bool, bool) {
        let v = d.get_video(id).unwrap().unwrap();
        (v.hidden, v.series_hidden)
    }

    /// `(hidden_changed_at, series_hidden_changed_at)`.
    fn stamps(d: &Db, id: &str) -> (Option<i64>, Option<i64>) {
        d.conn.lock().unwrap().query_row(
            "SELECT hidden_changed_at, series_hidden_changed_at FROM videos WHERE id=?1",
            params![id], |r| Ok((r.get(0)?, r.get(1)?))).unwrap()
    }

    fn sorted(mut ids: Vec<String>) -> Vec<String> { ids.sort(); ids }

    fn around<T>(f: impl FnOnce() -> T) -> (T, RangeInclusive<i64>) {
        let before = now();
        let out = f();
        (out, before..=now())
    }

    /// The premise of the drift tests, checked rather than assumed: "The Long
    /// Road Home" reads as a part of "The Long Road Part 6", but not of "The
    /// Long Road" -- neither of those two carries a part marker, and without
    /// one only an identical stem matches. So p6 sits on both cards, p5's and
    /// p7's, and p5 and p7 never share one.
    fn the_long_road_drifts() {
        use crate::siblings::{is_sibling, normalize};
        let (p5, p6, p7) = (normalize("The Long Road"), normalize("The Long Road Part 6"),
                            normalize("The Long Road Home"));
        assert!(is_sibling(&p5, &p6) && is_sibling(&p6, &p7));
        assert!(!is_sibling(&p5, &p7));
    }

    // ---- hiding and un-hiding a series ----

    #[test]
    fn hiding_a_series_marks_every_part_including_one_not_ready_yet() {
        let d = db();
        tapes(&d);
        // An upcoming premiere: never on screen, but it is the next part, and
        // the day it goes live it must not walk back into the feed.
        add(&d, NewVideo { status: VideoStatus::Upcoming, ..row("p3", "The Blackwood Tapes Pt 3") });

        let changed = d.set_series_hidden("p1", true).unwrap();
        assert_eq!(sorted(changed), vec!["p1", "p2", "p3"]);
        for id in ["p1", "p2", "p3"] {
            assert_eq!(flags(&d, id), (true, true), "{id}");
        }
        assert_eq!(flags(&d, "other"), (false, false), "not a part, not touched");
    }

    #[test]
    fn hiding_again_changes_nothing_and_says_so() {
        let d = db();
        tapes(&d);
        d.set_series_hidden("p1", true).unwrap();
        assert!(d.set_series_hidden("p2", true).unwrap().is_empty(),
                "led from another part, the same series is already hidden");
    }

    #[test]
    fn a_part_hidden_on_its_own_is_marked_when_its_series_is_hidden() {
        let d = db();
        tapes(&d);
        d.set_hidden("p2", true).unwrap();
        let changed = d.set_series_hidden("p1", true).unwrap();
        assert_eq!(sorted(changed), vec!["p1", "p2"], "p2 was hidden but not yet marked");
        assert_eq!(flags(&d, "p2"), (true, true));
    }

    #[test]
    fn unhiding_a_series_clears_both_flags_on_every_part() {
        let d = db();
        tapes(&d);
        add(&d, NewVideo { status: VideoStatus::Upcoming, ..row("p3", "The Blackwood Tapes Pt 3") });
        d.set_series_hidden("p1", true).unwrap();

        let changed = d.set_series_hidden("p2", false).unwrap();
        assert_eq!(sorted(changed), vec!["p1", "p2", "p3"]);
        for id in ["p1", "p2", "p3"] {
            assert_eq!(flags(&d, id), (false, false), "{id}");
        }

        // And with the mark gone, a new part is just a new part.
        add(&d, row("p4", "The Blackwood Tapes Pt 4"));
        assert!(d.hide_series_arrivals().unwrap().is_empty());
        assert_eq!(flags(&d, "p4"), (false, false));
    }

    #[test]
    fn an_unknown_leader_changes_nothing() {
        let d = db();
        tapes(&d);
        assert!(d.set_series_hidden("no such video", true).unwrap().is_empty());
        assert_eq!(flags(&d, "p1"), (false, false));
    }

    #[test]
    fn hiding_and_unhiding_a_series_stamp_both_columns() {
        let d = db();
        tapes(&d);
        let (_, at) = around(|| d.set_series_hidden("p1", true).unwrap());
        for id in ["p1", "p2"] {
            let (hidden, series) = stamps(&d, id);
            assert!(hidden.is_some_and(|s| at.contains(&s)), "{id} hidden stamp");
            assert!(series.is_some_and(|s| at.contains(&s)), "{id} series stamp");
        }

        // Pinned into the past, so the un-hide can be seen to move them on.
        sql(&d, "UPDATE videos SET hidden_changed_at=5, series_hidden_changed_at=5
                 WHERE id IN ('p1','p2')");
        let (_, at) = around(|| d.set_series_hidden("p1", false).unwrap());
        let (hidden, series) = stamps(&d, "p2");
        assert!(hidden.is_some_and(|s| at.contains(&s)));
        assert!(series.is_some_and(|s| at.contains(&s)));
        assert_eq!(stamps(&d, "other"), (None, None), "an untouched row stays unstamped");
    }

    #[test]
    fn unhiding_a_series_decides_even_a_part_that_was_never_marked() {
        let d = db();
        tapes(&d);
        d.set_series_hidden("p1", true).unwrap();
        // On the card but never marked: added by hand, so the sweep let it be.
        add(&d, NewVideo { added_manually: true, ..row("p3", "The Blackwood Tapes Pt 3") });
        assert_eq!(stamps(&d, "p3"), (None, None));

        let (changed, at) = around(|| d.set_series_hidden("p1", false).unwrap());
        assert_eq!(sorted(changed), vec!["p1", "p2"], "p3's flags did not change");
        assert!(stamps(&d, "p3").1.is_some_and(|s| at.contains(&s)),
                "but bringing the series back was a decision about it too");
        assert_eq!(flags(&d, "p3"), (false, false));
    }

    // ---- the sweep ----

    #[test]
    fn a_part_arriving_after_the_hide_is_hidden_and_marked() {
        let d = db();
        tapes(&d);
        d.set_series_hidden("p1", true).unwrap();

        add(&d, row("p3", "The Blackwood Tapes Pt 3"));
        add(&d, row("stranger", "A Quiet Walk"));
        let (hid, at) = around(|| d.hide_series_arrivals().unwrap());
        assert_eq!(hid, vec!["p3"]);
        assert_eq!(flags(&d, "p3"), (true, true), "marked too, so the part after it follows");
        assert_eq!(flags(&d, "stranger"), (false, false));
        let (hidden, series) = stamps(&d, "p3");
        assert!(hidden.is_some_and(|s| at.contains(&s)), "a hide like any other: it syncs");
        assert!(series.is_some_and(|s| at.contains(&s)));
        assert_eq!(stamps(&d, "stranger"), (None, None), "still undecided, still unstamped");

        assert!(d.hide_series_arrivals().unwrap().is_empty(), "nothing left to do twice");
    }

    #[test]
    fn a_part_outside_the_hidden_card_whose_own_card_holds_a_marked_part_is_swept() {
        the_long_road_drifts();
        let d = db();
        add(&d, row("p5", "The Long Road"));
        add(&d, row("p6", "The Long Road Part 6"));
        add(&d, row("p7", "The Long Road Home"));

        // p5's card is p5 and p6; p7 is not on it, and the hide leaves it be.
        assert_eq!(sorted(d.set_series_hidden("p5", true).unwrap()), vec!["p5", "p6"]);
        assert_eq!(flags(&d, "p7"), (false, false));

        // But p7's own card is p7 and p6, and p6 is marked now.
        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p7"]);
        assert_eq!(flags(&d, "p7"), (true, true));
    }

    #[test]
    fn a_drifting_series_is_followed_one_part_at_a_time() {
        the_long_road_drifts();
        let d = db();
        add(&d, row("p5", "The Long Road"));
        d.set_series_hidden("p5", true).unwrap();

        add(&d, row("p6", "The Long Road Part 6"));
        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p6"]);

        // p7 shares nothing with what was hidden by hand. It is hidden because
        // p6 arrived marked, which is the whole reason an arrival is marked
        // rather than only hidden.
        add(&d, row("p7", "The Long Road Home"));
        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p7"]);
        assert_eq!(flags(&d, "p7"), (true, true));
    }

    #[test]
    fn two_arrivals_in_one_sweep_where_the_second_matches_only_the_first() {
        the_long_road_drifts();
        let d = db();
        add(&d, row("p5", "The Long Road"));
        d.set_series_hidden("p5", true).unwrap();

        // p7 first, so a single pass in insertion order would reach it before
        // p6 was marked and leave it out.
        add(&d, row("p7", "The Long Road Home"));
        add(&d, row("p6", "The Long Road Part 6"));
        assert_eq!(sorted(d.hide_series_arrivals().unwrap()), vec!["p6", "p7"]);
    }

    #[test]
    fn a_part_brought_back_with_its_series_is_never_swept_back_out() {
        the_long_road_drifts();
        let d = db();
        add(&d, row("p5", "The Long Road"));
        add(&d, row("p6", "The Long Road Part 6"));
        add(&d, row("p7", "The Long Road Home"));
        d.set_series_hidden("p5", true).unwrap();

        // Brought back from p7's card, which holds p7 and p6 but not p5: p5
        // stays hidden and marked, on a card of its own.
        assert_eq!(d.set_series_hidden("p7", false).unwrap(), vec!["p6"]);
        assert_eq!(flags(&d, "p5"), (true, true));

        // p6 still matches the marked p5, and p7 matches p6. Neither may come
        // back out: the user has just said so.
        assert!(d.hide_series_arrivals().unwrap().is_empty());
        assert_eq!(flags(&d, "p6"), (false, false));
        assert_eq!(flags(&d, "p7"), (false, false));

        // A new part reads as p5's (the same stem) and as p7's (a numbered
        // title sharing p7's opening), so it is swept -- and lands on p7's
        // card marked. p7's mark was never set, only its stamp: that is all
        // that keeps it from following p8 out.
        add(&d, row("p8", "The Long Road Part 8"));
        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p8"]);
        assert_eq!(flags(&d, "p7"), (false, false));
        assert_eq!(flags(&d, "p6"), (false, false));
    }

    #[test]
    fn a_part_already_here_when_a_mark_arrives_from_elsewhere_is_swept() {
        let d = db();
        tapes(&d);
        // Another machine hid the series and only its p1 travelled -- the way
        // a sync apply writes it, stamp and all, so the trigger stays out.
        sql(&d, "UPDATE videos SET hidden=1, hidden_changed_at=50,
                                   series_hidden=1, series_hidden_changed_at=50
                 WHERE id='p1'");

        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p2"]);
        assert_eq!(flags(&d, "p2"), (true, true));
        assert_eq!(flags(&d, "other"), (false, false));
    }

    #[test]
    fn an_old_upload_retitled_into_a_hidden_series_is_swept() {
        let d = db();
        tapes(&d);
        // A premiere listed under a placeholder when the series was hidden, so
        // it matched nothing and was left unmarked.
        add(&d, NewVideo { status: VideoStatus::Upcoming, ..row("p3", "Coming Soon") });
        d.set_series_hidden("p1", true).unwrap();
        assert_eq!(flags(&d, "p3"), (false, false));

        d.set_titles(&[("p3".into(), "The Blackwood Tapes Pt 3".into())]).unwrap();
        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p3"]);
        assert_eq!(flags(&d, "p3"), (true, true));
    }

    #[test]
    fn a_video_added_by_hand_is_never_swept() {
        let d = db();
        tapes(&d);
        d.set_series_hidden("p1", true).unwrap();

        add(&d, NewVideo { added_manually: true, ..row("p3", "The Blackwood Tapes Pt 3") });
        assert!(d.hide_series_arrivals().unwrap().is_empty(),
                "pasting it into the Add box was asking to see it");
        assert_eq!(flags(&d, "p3"), (false, false));
    }

    #[test]
    fn hiding_one_part_on_its_own_leaves_nothing_for_the_sweep_to_follow() {
        let d = db();
        tapes(&d);
        // A bad episode, not the show.
        d.set_hidden("p1", true).unwrap();

        add(&d, row("p3", "The Blackwood Tapes Pt 3"));
        assert!(d.hide_series_arrivals().unwrap().is_empty());
        assert_eq!(flags(&d, "p2"), (false, false));
        assert_eq!(flags(&d, "p3"), (false, false));
    }

    #[test]
    fn unhiding_one_part_keeps_its_mark_and_arrivals_still_hide() {
        let d = db();
        tapes(&d);
        d.set_series_hidden("p1", true).unwrap();
        d.set_hidden("p1", false).unwrap();
        assert_eq!(flags(&d, "p1"), (false, true), "one episode back, the show still hidden");

        add(&d, row("p3", "The Blackwood Tapes Pt 3"));
        assert_eq!(d.hide_series_arrivals().unwrap(), vec!["p3"]);
        assert_eq!(flags(&d, "p1"), (false, true), "and the episode brought back stays back");
    }

    #[test]
    fn a_part_is_matched_only_against_its_own_channel() {
        let d = db();
        tapes(&d);
        d.set_series_hidden("p1", true).unwrap();
        d.upsert_channel(&chan("UC2")).unwrap();

        // The very same title, uploaded by someone else.
        add(&d, NewVideo { channel_id: "UC2".into(), ..row("copy", "The Blackwood Tapes Pt 3") });
        assert!(d.hide_series_arrivals().unwrap().is_empty());
        assert_eq!(flags(&d, "copy"), (false, false));
    }

    #[test]
    fn nothing_is_swept_on_a_channel_with_no_hidden_series() {
        let d = db();
        tapes(&d);
        add(&d, row("p3", "The Blackwood Tapes Pt 3"));
        assert!(d.hide_series_arrivals().unwrap().is_empty());
        assert_eq!(flags(&d, "p3"), (false, false));
    }

    // ---- marking by hand ----

    #[test]
    fn marking_a_video_into_a_hidden_series_hides_it() {
        let d = db();
        tapes(&d);
        add(&d, row("ec", "EVERYTHING CHANGED"));
        d.set_series_hidden("p1", true).unwrap();

        // The renamed follow-up, linked by hand.
        assert_eq!(d.mark_siblings(&["p2".into(), "ec".into()]).unwrap(), 2,
                   "the group's size, as before");
        assert_eq!(flags(&d, "ec"), (true, true));
        assert_eq!(flags(&d, "other"), (false, false));
    }

    #[test]
    fn marking_hides_even_a_video_decided_before_since_the_user_just_linked_it() {
        let d = db();
        tapes(&d);
        add(&d, row("ec", "EVERYTHING CHANGED"));
        // Its own one-part "series" brought back once: decided, so no sweep
        // would ever touch it.
        d.set_series_hidden("ec", true).unwrap();
        d.set_series_hidden("ec", false).unwrap();
        d.set_series_hidden("p1", true).unwrap();

        d.mark_siblings(&["p2".into(), "ec".into()]).unwrap();
        assert_eq!(flags(&d, "ec"), (true, true));
    }

    #[test]
    fn marking_visible_videos_together_hides_nothing() {
        let d = db();
        tapes(&d);
        add(&d, row("ec", "EVERYTHING CHANGED"));
        d.mark_siblings(&["p2".into(), "ec".into()]).unwrap();
        assert_eq!(flags(&d, "ec"), (false, false));
        assert_eq!(flags(&d, "p2"), (false, false));
    }

    #[test]
    fn marking_never_re_hides_a_part_brought_back_on_its_own() {
        let d = db();
        tapes(&d);
        add(&d, row("ec", "EVERYTHING CHANGED"));
        d.set_series_hidden("p1", true).unwrap();
        d.set_hidden("p2", false).unwrap();

        d.mark_siblings(&["p2".into(), "ec".into()]).unwrap();
        assert_eq!(flags(&d, "ec"), (true, true), "the newcomer joins a hidden series");
        assert_eq!(flags(&d, "p2"), (false, true), "the episode brought back stays back");
    }
}

#[cfg(test)]
mod sync_tests {
    use super::*;
    use std::ops::RangeInclusive;

    fn db() -> Db { Db::open_in_memory().unwrap() }

    fn chan(id: &str) -> Channel {
        Channel { id: id.into(), title: format!("T-{id}"), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 0,
                  last_polled_at: None, terminated: false, auto_download: false }
    }

    fn adhoc(id: &str) -> Channel { Channel { subscribed: false, ..chan(id) } }

    fn polled(id: &str, ch: &str) -> NewVideo {
        NewVideo { id: id.into(), channel_id: ch.into(), title: format!("t-{id}"),
                   description: None, thumb_url: None, published_at: Some(1_000),
                   sort_at: Some(1_000), feed_rank: 0, added_manually: false,
                   duration_secs: Some(600), view_count: Some(1), status: VideoStatus::Ready }
    }

    fn manual(id: &str, ch: &str) -> NewVideo { NewVideo { added_manually: true, ..polled(id, ch) } }

    /// A video row the way a transfer archive carries it.
    fn archived(id: &str, ch: &str) -> Video {
        Video { id: id.into(), channel_id: ch.into(), channel_title: format!("T-{ch}"),
                title: format!("t-{id}"), description: None, thumb_url: None,
                thumb_path: None, published_at: Some(1_000), sort_at: Some(1_000),
                feed_rank: 0, added_manually: false, duration_secs: Some(600),
                view_count: Some(1), status: VideoStatus::Ready, hidden: false,
                watched: false, watched_at: None, download_state: DownloadState::None,
                download_error: None, file_path: None, downloaded_at: None,
                first_seen_at: 500, sibling_group: None, series_hidden: false }
    }

    /// Raw SQL, for what only a migration, the clock or another machine
    /// produces. Tests run inside one second, and last writer wins needs its
    /// stamps apart, so every edit below is made through the public method and
    /// then pinned to a chosen instant here.
    fn sql(d: &Db, s: &str) { d.conn.lock().unwrap().execute_batch(s).unwrap(); }

    fn pin(d: &Db, table: &str, id: &str, assignments: &str) {
        sql(d, &format!("UPDATE {table} SET {assignments} WHERE id='{id}'"));
    }

    fn video_stamps(d: &Db, id: &str) -> [Option<i64>; 3] {
        d.conn.lock().unwrap().query_row(
            "SELECT watched_changed_at, hidden_changed_at, sibling_changed_at FROM videos WHERE id=?1",
            params![id], |r| Ok([r.get(0)?, r.get(1)?, r.get(2)?])).unwrap()
    }

    fn channel_stamps(d: &Db, id: &str) -> [Option<i64>; 2] {
        d.conn.lock().unwrap().query_row(
            "SELECT subscribed_changed_at, member_changed_at FROM channels WHERE id=?1",
            params![id], |r| Ok([r.get(0)?, r.get(1)?])).unwrap()
    }

    fn tombstone(d: &Db, kind: &str, id: &str) -> Option<i64> {
        d.conn.lock().unwrap()
            .query_row("SELECT removed_at FROM tombstones WHERE kind=?1 AND id=?2",
                       params![kind, id], |r| r.get(0))
            .optional().unwrap()
    }

    fn trigger_names(d: &Db) -> Vec<String> {
        let c = d.conn.lock().unwrap();
        let mut st = c.prepare("SELECT name FROM sqlite_master WHERE type='trigger' ORDER BY name").unwrap();
        let rows = st.query_map([], |r| r.get(0)).unwrap();
        rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    /// Runs `f`, returning what it returned and the clock either side of it, so
    /// a stamp it wrote can be checked against "now" without racing the second.
    fn around<T>(f: impl FnOnce() -> T) -> (T, RangeInclusive<i64>) {
        let before = now();
        let out = f();
        (out, before..=now())
    }

    fn stamped_now(at: &RangeInclusive<i64>, stamp: Option<i64>) -> bool {
        stamp.is_some_and(|s| at.contains(&s))
    }

    // ---- the v11 migration ----

    fn series_stamp(d: &Db, id: &str) -> (bool, Option<i64>) {
        d.conn.lock().unwrap().query_row(
            "SELECT series_hidden, series_hidden_changed_at FROM videos WHERE id=?1",
            params![id], |r| Ok((r.get::<_, i64>(0)? != 0, r.get(1)?))).unwrap()
    }

    #[test]
    fn upgrading_adds_series_hidden_unset_and_unstamped() {
        let conn = v9();
        conn.execute_batch(
            "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7);
             INSERT INTO videos (id,channel_id,title,status,first_seen_at,hidden)
               VALUES ('a','UC1','t','ready',1,1);",
        ).unwrap();
        let d = Db::init(conn).unwrap();
        assert_eq!(series_stamp(&d, "a"), (false, None), "a hidden video is not a hidden series");
        let names = trigger_names(&d);
        assert!(names.contains(&"sync_series_hidden_changed".to_string()));
        assert!(names.contains(&"sync_series_hidden_inserted".to_string()));
    }

    #[test]
    fn marking_a_series_hidden_is_stamped_but_an_apply_keeps_its_own_stamp() {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        d.insert_video_if_new(&polled("a", "UC1")).unwrap();
        d.insert_video_if_new(&polled("b", "UC1")).unwrap();

        let ((), at) = around(|| sql(&d, "UPDATE videos SET series_hidden=1 WHERE id='a'"));
        let (on, stamp) = series_stamp(&d, "a");
        assert!(on && stamped_now(&at, stamp), "a local change is stamped now");

        // How `apply_sync` writes: the value and another machine's stamp together.
        sql(&d, "UPDATE videos SET series_hidden=1, series_hidden_changed_at=42 WHERE id='b'");
        assert_eq!(series_stamp(&d, "b"), (true, Some(42)));
    }

    // ---- the v10 migration ----

    /// A database exactly as v9 left it: every ALTER up to and including the
    /// download-quality column, and nothing of sync's.
    fn v9() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(
            "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN downloaded_at INTEGER;
             ALTER TABLE videos ADD COLUMN sibling_group TEXT;
             ALTER TABLE channels ADD COLUMN member INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE channels ADD COLUMN backfill_depth INTEGER;
             ALTER TABLE channels ADD COLUMN terminated_at INTEGER;
             ALTER TABLE channels ADD COLUMN auto_download_since INTEGER;
             ALTER TABLE videos ADD COLUMN auto_queued INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN download_quality TEXT;",
        ).unwrap();
        conn.pragma_update(None, "user_version", 9).unwrap();
        conn
    }

    #[test]
    fn migrating_a_v9_database_stamps_only_what_was_ever_set() {
        let conn = v9();
        conn.execute_batch(
            "INSERT INTO channels (id,title,url,subscribed,added_at,member) VALUES
               ('UC1','One','u',1,7,1), ('UC2','Uploader','u',0,8,0), ('UC3','Three','u',1,9,0);
             INSERT INTO videos (id,channel_id,title,status,first_seen_at,
                                 watched,watched_at,hidden,sibling_group) VALUES
               ('w', 'UC1','t','ready',1, 1,500, 0,NULL),
               ('w0','UC1','t','ready',1, 1,NULL,0,NULL),
               ('h', 'UC1','t','ready',1, 0,NULL,1,NULL),
               ('s', 'UC1','t','ready',1, 0,NULL,0,'k'),
               ('u', 'UC1','t','ready',1, 0,NULL,0,NULL);",
        ).unwrap();

        let (d, at) = around(|| Db::init(conn).unwrap());

        assert_eq!(video_stamps(&d, "w"), [Some(500), None, None], "dated by the watch itself");
        assert!(stamped_now(&at, video_stamps(&d, "w0")[0]), "no instant recorded: the upgrade's");
        assert!(stamped_now(&at, video_stamps(&d, "h")[1]));
        assert!(stamped_now(&at, video_stamps(&d, "s")[2]));
        assert_eq!(video_stamps(&d, "u"), [None; 3], "an untouched row stays untouched");

        let [sub, member] = channel_stamps(&d, "UC1");
        assert_eq!(sub, Some(7), "a subscription dates from when it was added");
        assert!(stamped_now(&at, member));
        assert_eq!(channel_stamps(&d, "UC2"), [None, None], "an uploader is no subscription");
        assert_eq!(channel_stamps(&d, "UC3"), [Some(9), None]);

        {
            let c = d.conn.lock().unwrap();
            let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, SCHEMA_VERSION);
            let n: i64 = c.query_row("SELECT COUNT(*) FROM tombstones", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "nothing has been removed yet");
        }
        assert_eq!(trigger_names(&d), trigger_names(&db()), "upgraded and fresh carry the same triggers");
        assert!(!trigger_names(&d).is_empty());
        // And the rows themselves came through untouched.
        let w = d.get_video("w").unwrap().unwrap();
        assert!(w.watched && w.watched_at == Some(500));
    }

    // ---- the triggers ----

    fn one_video(id: &str) -> Db {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        d.insert_video_if_new(&polled(id, "UC1")).unwrap();
        d
    }

    #[test]
    fn watching_and_unwatching_are_both_stamped() {
        let d = one_video("a");
        assert_eq!(video_stamps(&d, "a"), [None; 3], "a polled row is untouched");

        let (_, at) = around(|| d.set_watched("a", true).unwrap());
        assert!(stamped_now(&at, video_stamps(&d, "a")[0]));

        pin(&d, "videos", "a", "watched_changed_at=1");
        d.set_watched("a", true).unwrap();
        assert_eq!(video_stamps(&d, "a")[0], Some(1), "no change, no stamp");

        let (_, at) = around(|| d.set_watched("a", false).unwrap());
        assert!(stamped_now(&at, video_stamps(&d, "a")[0]), "an un-watch is a change too");
    }

    #[test]
    fn hiding_and_unhiding_are_both_stamped() {
        let d = one_video("a");
        let (_, at) = around(|| d.set_hidden("a", true).unwrap());
        assert!(stamped_now(&at, video_stamps(&d, "a")[1]));
        pin(&d, "videos", "a", "hidden_changed_at=1");
        let (_, at) = around(|| d.set_hidden("a", false).unwrap());
        assert!(stamped_now(&at, video_stamps(&d, "a")[1]));
        assert_eq!(video_stamps(&d, "a")[0], None, "the other clocks are not touched");
    }

    #[test]
    fn marking_stamps_every_row_whose_key_moved_and_no_other() {
        let d = one_video("a");
        d.insert_video_if_new(&polled("b", "UC1")).unwrap();
        d.insert_video_if_new(&polled("c", "UC1")).unwrap();
        let (_, at) = around(|| d.mark_siblings(&["a".into(), "b".into()]).unwrap());
        assert!(stamped_now(&at, video_stamps(&d, "a")[2]));
        assert!(stamped_now(&at, video_stamps(&d, "b")[2]));
        assert_eq!(video_stamps(&d, "c")[2], None);

        sql(&d, "UPDATE videos SET sibling_changed_at=1 WHERE sibling_group IS NOT NULL");
        let (_, at) = around(|| d.mark_siblings(&["b".into(), "c".into()]).unwrap());
        assert!(stamped_now(&at, video_stamps(&d, "c")[2]), "c joined b's group");
        assert_eq!(video_stamps(&d, "a")[2], Some(1), "a's key never moved");
        assert_eq!(video_stamps(&d, "b")[2], Some(1));
    }

    #[test]
    fn unlinking_stamps_the_row_and_the_partner_it_strands() {
        let d = one_video("a");
        d.insert_video_if_new(&polled("b", "UC1")).unwrap();
        d.mark_siblings(&["a".into(), "b".into()]).unwrap();
        sql(&d, "UPDATE videos SET sibling_changed_at=1");
        let (_, at) = around(|| d.unlink_siblings("a").unwrap());
        for id in ["a", "b"] {
            assert_eq!(d.get_video(id).unwrap().unwrap().sibling_group, None);
            assert!(stamped_now(&at, video_stamps(&d, id)[2]), "{id}");
        }
    }

    #[test]
    fn a_membership_is_stamped_when_joined_and_when_left() {
        let d = one_video("a");
        assert_eq!(channel_stamps(&d, "UC1")[1], None);
        let (_, at) = around(|| d.set_channel_member("UC1", true).unwrap());
        assert!(stamped_now(&at, channel_stamps(&d, "UC1")[1]));
        pin(&d, "channels", "UC1", "member_changed_at=1");
        d.upsert_channel(&chan("UC1")).unwrap();
        assert_eq!(channel_stamps(&d, "UC1")[1], Some(1), "a poll's upsert says nothing about it");
        let (_, at) = around(|| d.set_channel_member("UC1", false).unwrap());
        assert!(stamped_now(&at, channel_stamps(&d, "UC1")[1]));
    }

    #[test]
    fn subscribing_is_stamped_and_a_polls_upsert_is_not() {
        let d = db();
        let (_, at) = around(|| d.upsert_channel(&chan("UC1")).unwrap());
        assert!(stamped_now(&at, channel_stamps(&d, "UC1")[0]), "a fresh subscription");
        pin(&d, "channels", "UC1", "subscribed_changed_at=1");
        d.upsert_channel(&chan("UC1")).unwrap();
        assert_eq!(channel_stamps(&d, "UC1")[0], Some(1), "every poll upserts to catch a rename");

        d.upsert_channel(&adhoc("UC2")).unwrap();
        assert_eq!(channel_stamps(&d, "UC2"), [None, None], "an uploader is no subscription");
        let (_, at) = around(|| d.upsert_channel(&chan("UC2")).unwrap());
        assert!(stamped_now(&at, channel_stamps(&d, "UC2")[0]), "subscribing to an uploader already here");
    }

    #[test]
    fn removing_a_subscription_tombstones_it_and_the_manual_videos_it_takes() {
        let d = one_video("p");
        d.insert_video_if_new(&manual("m", "UC1")).unwrap();
        let (_, at) = around(|| d.remove_channel("UC1").unwrap());
        assert!(stamped_now(&at, tombstone(&d, "channel", "UC1")));
        assert!(stamped_now(&at, tombstone(&d, "video", "m")), "the cascade counts as a delete");
        assert_eq!(tombstone(&d, "video", "p"), None, "a polled video is the poll's to bring back");
    }

    #[test]
    fn deleting_a_manual_video_tombstones_it_and_pruning_its_uploader_does_not() {
        let d = one_video("p");
        d.upsert_channel(&adhoc("UC2")).unwrap();
        d.insert_video_if_new(&manual("m", "UC2")).unwrap();
        let (_, at) = around(|| {
            d.delete_video("m").unwrap();
            d.prune_orphan_channel("UC2").unwrap();
        });
        assert!(stamped_now(&at, tombstone(&d, "video", "m")));
        assert_eq!(tombstone(&d, "channel", "UC2"), None, "an uploader was never a subscription");
        d.delete_video("p").unwrap();
        assert_eq!(tombstone(&d, "video", "p"), None);
    }

    #[test]
    fn a_second_removal_after_a_comeback_moves_the_tombstone_on() {
        // Removed once at 50, then subscribed again at 60. Were the tombstone
        // kept at 50, the second removal would never reach a machine holding
        // the comeback: 50 does not cover 60.
        let d = one_video("p");
        sql(&d, "INSERT INTO tombstones (kind,id,removed_at) VALUES ('channel','UC1',50);
                 UPDATE channels SET subscribed_changed_at=60");
        let (_, at) = around(|| d.remove_channel("UC1").unwrap());
        assert!(stamped_now(&at, tombstone(&d, "channel", "UC1")));

        // The same for a manual video deleted, added again and deleted again.
        d.upsert_channel(&adhoc("UC2")).unwrap();
        d.insert_video_if_new(&manual("m", "UC2")).unwrap();
        sql(&d, "INSERT INTO tombstones (kind,id,removed_at) VALUES ('video','m',50);
                 UPDATE videos SET first_seen_at=60 WHERE id='m'");
        let (_, at) = around(|| d.delete_video("m").unwrap());
        assert!(stamped_now(&at, tombstone(&d, "video", "m")));
    }

    #[test]
    fn a_delete_a_tombstone_already_covers_keeps_the_tombstones_time() {
        // Which is how an apply deletes: the remote tombstone lands first, and
        // the row it covers then goes without this machine's clock entering.
        let d = one_video("p");
        d.upsert_channel(&adhoc("UC2")).unwrap();
        d.insert_video_if_new(&manual("m", "UC2")).unwrap();
        sql(&d, "INSERT INTO tombstones (kind,id,removed_at) VALUES ('channel','UC1',50),('video','m',50);
                 UPDATE channels SET subscribed_changed_at=50 WHERE id='UC1';
                 UPDATE videos SET first_seen_at=40 WHERE id='m'");
        d.remove_channel("UC1").unwrap();
        d.delete_video("m").unwrap();
        assert_eq!(tombstone(&d, "channel", "UC1"), Some(50), "a tie is covered");
        assert_eq!(tombstone(&d, "video", "m"), Some(50));
    }

    #[test]
    fn a_write_that_brings_its_own_stamp_is_not_restamped() {
        let d = one_video("a");
        pin(&d, "videos", "a",
            "watched=1, watched_at=3, watched_changed_at=42, hidden=1, hidden_changed_at=43,
             sibling_group='k', sibling_changed_at=44");
        assert_eq!(video_stamps(&d, "a"), [Some(42), Some(43), Some(44)]);
        pin(&d, "channels", "UC1",
            "subscribed=0, subscribed_changed_at=45, member=1, member_changed_at=46");
        assert_eq!(channel_stamps(&d, "UC1"), [Some(45), Some(46)]);
        sql(&d, "INSERT INTO videos (id,channel_id,title,status,first_seen_at,watched,watched_changed_at)
                 VALUES ('b','UC1','t','ready',1,1,7)");
        assert_eq!(video_stamps(&d, "b"), [Some(7), None, None]);
    }

    #[test]
    fn an_import_that_lifts_or_brings_user_state_is_stamped() {
        let d = one_video("a");
        let a = Video { watched: true, watched_at: Some(77), ..archived("a", "UC1") };
        let b = Video { watched: true, watched_at: Some(88), hidden: true,
                        sibling_group: Some("k".into()), ..archived("b", "UC9") };
        let joined = Channel { member: true, ..chan("UC9") };
        let (_, at) = around(|| {
            d.apply_import(&[chan("UC1"), joined], &[a, b], ImportMode::Merge, &[]).unwrap()
        });

        let [w, h, s] = video_stamps(&d, "a");
        assert!(stamped_now(&at, w), "a merge lifting watched");
        assert_eq!((h, s), (None, None));
        let [w, h, s] = video_stamps(&d, "b");
        assert_eq!(w, Some(88), "a watch arriving whole is dated by its own instant");
        assert!(stamped_now(&at, h) && stamped_now(&at, s));
        let [sub, member] = channel_stamps(&d, "UC9");
        assert!(stamped_now(&at, sub) && stamped_now(&at, member));
    }

    // ---- the snapshot ----

    /// One library holding something of every kind: an untouched upload, a
    /// watch, a watch taken back, a hide, a marked pair, a manual add from an
    /// ad-hoc uploader, a joined membership and a removed subscription.
    fn touched() -> Db {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        d.upsert_channel(&chan("UC3")).unwrap();
        d.upsert_channel(&adhoc("UC2")).unwrap();
        d.upsert_channel(&adhoc("UC5")).unwrap();
        d.upsert_channel(&chan("UC4")).unwrap();
        d.remove_channel("UC4").unwrap();
        // Inserted out of order on purpose: the snapshot orders by id.
        for id in ["w", "u", "uw", "s2", "s1", "h"] {
            d.insert_video_if_new(&polled(id, "UC1")).unwrap();
        }
        d.insert_video_if_new(&manual("m", "UC2")).unwrap();
        d.set_watched("w", true).unwrap();
        d.set_watched("uw", true).unwrap();
        d.set_watched("uw", false).unwrap();
        d.set_hidden("h", true).unwrap();
        d.mark_siblings(&["s1".into(), "s2".into()]).unwrap();
        d.set_channel_member("UC1", true).unwrap();
        d
    }

    #[test]
    fn the_snapshot_carries_what_was_touched_and_nothing_else() {
        let d = touched();
        let s = d.sync_snapshot("desk").unwrap();
        assert_eq!(s.format, SNAPSHOT_FORMAT);
        assert_eq!(s.machine_name, "desk");

        let ids: Vec<&str> = s.videos.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["h", "m", "s1", "s2", "uw", "w"], "never the untouched 'u'");
        let ids: Vec<&str> = s.channels.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, ["UC1", "UC2", "UC3"],
                   "both subscriptions, and the uploader only because 'm' needs it");
        assert_eq!(s.tombstones.len(), 1);
        assert_eq!((s.tombstones[0].kind, s.tombstones[0].id.as_str()),
                   (TombstoneKind::Channel, "UC4"));

        let m = s.videos.iter().find(|v| v.id == "m").unwrap();
        assert!(m.added_manually);
        assert_eq!(m.channel_id, "UC2");
        assert_eq!(m.added_at, d.get_video("m").unwrap().unwrap().first_seen_at);
        let uw = s.videos.iter().find(|v| v.id == "uw").unwrap();
        assert!(!uw.watched && uw.watched_at.is_none() && uw.watched_changed_at.is_some(),
                "the stamp left behind by an un-watch is what carries it");
        let s1 = s.videos.iter().find(|v| v.id == "s1").unwrap();
        assert_eq!(s1.sibling_group, d.get_video("s1").unwrap().unwrap().sibling_group);
        assert_eq!(s1.title, "t-s1");
        assert_eq!((s1.published_at, s1.sort_at, s1.duration_secs), (Some(1_000), Some(1_000), Some(600)));

        let uc1 = &s.channels[0];
        assert!(uc1.subscribed && uc1.member && uc1.member_changed_at.is_some());
        let uc2 = &s.channels[1];
        assert!(!uc2.subscribed && uc2.subscribed_changed_at.is_none());
        assert_eq!(uc2.url, "https://www.youtube.com/channel/UC2");
    }

    #[test]
    fn an_unsubscribed_channel_rides_along_for_a_membership_too() {
        let d = touched();
        pin(&d, "channels", "UC5", "member=1");
        let s = d.sync_snapshot("desk").unwrap();
        assert!(s.channels.iter().any(|c| c.id == "UC5"));
    }

    #[test]
    fn the_same_library_always_snapshots_to_the_same_bytes() {
        let d = touched();
        let once = serde_json::to_string(&d.sync_snapshot("desk").unwrap()).unwrap();
        let again = serde_json::to_string(&d.sync_snapshot("desk").unwrap()).unwrap();
        assert_eq!(once, again);

        // The same rows written in the opposite order, every clock pinned alike.
        let build = |ids: &[&str]| {
            let d = db();
            d.upsert_channel(&chan("UC1")).unwrap();
            for id in ids {
                d.insert_video_if_new(&polled(id, "UC1")).unwrap();
                d.set_hidden(id, true).unwrap();
            }
            sql(&d, "UPDATE videos SET first_seen_at=5, hidden_changed_at=6;
                     UPDATE channels SET subscribed_changed_at=7");
            serde_json::to_string(&d.sync_snapshot("desk").unwrap()).unwrap()
        };
        assert_eq!(build(&["a", "b", "c"]), build(&["c", "b", "a"]));
    }

    // ---- apply ----

    /// One machine's library as a poll leaves it: a subscription and three of
    /// its uploads, with every clock pinned at 100.
    fn library() -> Db {
        let d = db();
        d.upsert_channel(&chan("UC1")).unwrap();
        for id in ["a", "b", "c"] {
            d.insert_video_if_new(&polled(id, "UC1")).unwrap();
        }
        sql(&d, "UPDATE channels SET subscribed_changed_at=100; UPDATE videos SET first_seen_at=100");
        d
    }

    fn sync(from: &Db, to: &Db) -> ApplyReport {
        to.apply_sync(&[from.sync_snapshot("from").unwrap()]).unwrap()
    }

    /// A snapshot with the sender's name taken off, so two machines' can be compared.
    fn anon(d: &Db) -> Snapshot {
        Snapshot { machine_name: String::new(), ..d.sync_snapshot("x").unwrap() }
    }

    #[test]
    fn a_watch_travels_and_so_does_taking_it_back() {
        let (a, b) = (library(), library());
        a.set_watched("a", true).unwrap();
        pin(&a, "videos", "a", "watched_at=150, watched_changed_at=150");
        assert!(sync(&a, &b).changed);
        let v = b.get_video("a").unwrap().unwrap();
        assert!(v.watched);
        assert_eq!(v.watched_at, Some(150));
        assert_eq!(video_stamps(&b, "a")[0], Some(150), "copied, never restamped here");

        b.set_watched("a", false).unwrap();
        pin(&b, "videos", "a", "watched_changed_at=200");
        assert!(sync(&b, &a).changed);
        let v = a.get_video("a").unwrap().unwrap();
        assert!(!v.watched);
        assert_eq!(v.watched_at, None);
        assert_eq!(video_stamps(&a, "a")[0], Some(200));
    }

    #[test]
    fn a_hide_travels_and_so_does_taking_it_back() {
        let (a, b) = (library(), library());
        a.set_hidden("b", true).unwrap();
        pin(&a, "videos", "b", "hidden_changed_at=150");
        sync(&a, &b);
        assert!(b.get_video("b").unwrap().unwrap().hidden);

        b.set_hidden("b", false).unwrap();
        pin(&b, "videos", "b", "hidden_changed_at=200");
        sync(&b, &a);
        assert!(!a.get_video("b").unwrap().unwrap().hidden);
        assert_eq!(video_stamps(&a, "b")[1], Some(200));
    }

    #[test]
    fn a_mark_travels_and_so_does_an_unlink() {
        let (a, b) = (library(), library());
        a.mark_siblings(&["a".into(), "b".into()]).unwrap();
        sql(&a, "UPDATE videos SET sibling_changed_at=150 WHERE sibling_group IS NOT NULL");
        sync(&a, &b);
        let key = a.get_video("a").unwrap().unwrap().sibling_group;
        assert!(key.is_some());
        assert_eq!(b.get_video("a").unwrap().unwrap().sibling_group, key);
        assert_eq!(b.get_video("b").unwrap().unwrap().sibling_group, key);

        b.unlink_siblings("a").unwrap();
        sql(&b, "UPDATE videos SET sibling_changed_at=200 WHERE id IN ('a','b')");
        sync(&b, &a);
        assert_eq!(a.get_video("a").unwrap().unwrap().sibling_group, None);
        assert_eq!(a.get_video("b").unwrap().unwrap().sibling_group, None, "the partner it stranded");
    }

    #[test]
    fn a_membership_travels_and_so_does_its_end() {
        let (a, b) = (library(), library());
        a.set_channel_member("UC1", true).unwrap();
        pin(&a, "channels", "UC1", "member_changed_at=150");
        sync(&a, &b);
        assert!(b.get_channel("UC1").unwrap().unwrap().member);

        b.set_channel_member("UC1", false).unwrap();
        pin(&b, "channels", "UC1", "member_changed_at=200");
        sync(&b, &a);
        assert!(!a.get_channel("UC1").unwrap().unwrap().member);
        assert_eq!(channel_stamps(&a, "UC1")[1], Some(200));
    }

    #[test]
    fn a_new_subscription_arrives_and_is_handed_back_for_polling() {
        let (a, b) = (library(), library());
        a.upsert_channel(&Channel { handle: Some("@three".into()), ..chan("UC3") }).unwrap();
        pin(&a, "channels", "UC3", "subscribed_changed_at=150");
        let r = sync(&a, &b);
        assert_eq!(r.channels_added, ["UC3"]);
        assert!(r.videos_added.is_empty());
        let c = b.get_channel("UC3").unwrap().unwrap();
        assert!(c.subscribed);
        assert_eq!((c.title.as_str(), c.handle.as_deref()), ("T-UC3", Some("@three")));
        assert_eq!(channel_stamps(&b, "UC3")[0], Some(150));
    }

    #[test]
    fn a_channel_removal_travels_and_takes_its_videos() {
        let (a, b) = (library(), library());
        b.set_watched("a", true).unwrap();
        pin(&b, "videos", "a", "watched_at=150, watched_changed_at=150");
        let stale = b.sync_snapshot("b").unwrap();
        a.remove_channel("UC1").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");

        assert!(sync(&a, &b).changed);
        assert!(b.get_channel("UC1").unwrap().is_none());
        assert!(b.get_video("a").unwrap().is_none());
        assert_eq!(tombstone(&b, "channel", "UC1"), Some(300), "the remover's time, not this clock's");

        // B's copy from before it heard is not enough to bring any of it back.
        let r = a.apply_sync(&[stale]).unwrap();
        assert_eq!(r, ApplyReport::default());
        assert!(a.get_channel("UC1").unwrap().is_none());
        assert!(a.get_video("a").unwrap().is_none());
    }

    #[test]
    fn a_resubscribe_after_a_removal_beats_the_older_tombstone() {
        let (a, b) = (library(), library());
        a.remove_channel("UC1").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");
        sync(&a, &b);
        assert!(b.get_channel("UC1").unwrap().is_none());

        b.upsert_channel(&chan("UC1")).unwrap();
        pin(&b, "channels", "UC1", "subscribed_changed_at=400");
        let r = sync(&b, &a);
        assert_eq!(r.channels_added, ["UC1"]);
        assert!(a.get_channel("UC1").unwrap().unwrap().subscribed);
        assert_eq!(channel_stamps(&a, "UC1")[0], Some(400));
        assert_eq!(tombstone(&a, "channel", "UC1"), Some(300), "kept, and no longer covering it");
        assert!(!sync(&a, &b).changed);
    }

    #[test]
    fn a_manual_delete_travels_and_prunes_its_uploader() {
        let (a, b) = (library(), library());
        a.upsert_channel(&adhoc("UC2")).unwrap();
        a.insert_video_if_new(&manual("m", "UC2")).unwrap();
        pin(&a, "videos", "m", "first_seen_at=120");
        sync(&a, &b);
        assert!(b.get_video("m").unwrap().is_some());

        a.delete_video("m").unwrap();
        a.prune_orphan_channel("UC2").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");
        assert!(sync(&a, &b).changed);
        assert!(b.get_video("m").unwrap().is_none());
        assert!(b.get_channel("UC2").unwrap().is_none(), "an uploader left with nothing goes too");
        assert!(b.get_channel("UC1").unwrap().is_some(), "a subscription is never pruned");
        assert_eq!(tombstone(&b, "video", "m"), Some(300));
    }

    #[test]
    fn a_stale_copy_never_brings_a_deleted_manual_video_back() {
        let (a, b) = (library(), library());
        a.upsert_channel(&adhoc("UC2")).unwrap();
        a.insert_video_if_new(&manual("m", "UC2")).unwrap();
        pin(&a, "videos", "m", "first_seen_at=120");
        sync(&a, &b);
        let stale = b.sync_snapshot("b").unwrap();

        a.delete_video("m").unwrap();
        a.prune_orphan_channel("UC2").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");
        let r = a.apply_sync(&[stale]).unwrap();
        assert_eq!(r, ApplyReport::default());
        assert!(a.get_video("m").unwrap().is_none());
        assert!(a.get_channel("UC2").unwrap().is_none(), "nor the uploader it would have needed");
    }

    #[test]
    fn a_manual_video_added_again_outlives_its_older_tombstone() {
        let (a, b) = (library(), library());
        a.upsert_channel(&adhoc("UC2")).unwrap();
        a.insert_video_if_new(&manual("m", "UC2")).unwrap();
        pin(&a, "videos", "m", "first_seen_at=120");
        sync(&a, &b);

        // Deleted at 300 and added again at 400; B still holds the first copy.
        a.delete_video("m").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");
        a.insert_video_if_new(&manual("m", "UC2")).unwrap();
        pin(&a, "videos", "m", "first_seen_at=400");
        sync(&a, &b);
        assert!(b.get_video("m").unwrap().is_some(), "the comeback is newer than the removal");
        assert_eq!(b.get_video("m").unwrap().unwrap().first_seen_at, 400);
        assert_eq!(tombstone(&b, "video", "m"), Some(300));

        let c = library();
        assert_eq!(sync(&a, &c).videos_added, ["m"], "and a machine that never had it gets it");
    }

    #[test]
    fn an_uploader_arrives_with_the_manual_video_that_needs_it() {
        let (a, b) = (library(), library());
        a.upsert_channel(&adhoc("UC2")).unwrap();
        a.insert_video_if_new(&manual("m", "UC2")).unwrap();
        pin(&a, "videos", "m", "first_seen_at=120, sort_at=130");
        let r = sync(&a, &b);
        assert_eq!(r.videos_added, ["m"]);
        assert!(r.channels_added.is_empty(), "an uploader is not a subscription to poll");

        let c = b.get_channel("UC2").unwrap().unwrap();
        assert!(!c.subscribed);
        assert_eq!(c.title, "T-UC2");
        let v = b.get_video("m").unwrap().unwrap();
        assert!(v.added_manually);
        assert_eq!(v.status, VideoStatus::Ready);
        assert_eq!(v.first_seen_at, 120, "the sender's clock, not this one's");
        assert_eq!((v.published_at, v.sort_at), (Some(1_000), Some(130)));
        assert_eq!(v.thumb_url.as_deref(), Some(crate::ytdlp::thumb_url_for("m").as_str()));
        assert_eq!((v.title.as_str(), v.duration_secs, v.feed_rank), ("t-m", Some(600), 0));
        assert_eq!(b.list_videos(&VideoFilter::default()).unwrap().len(), 4, "and it is in the feed");
    }

    #[test]
    fn a_missing_sort_date_falls_back_to_the_upload_date() {
        let (a, b) = (library(), library());
        a.set_hidden("a", true).unwrap();
        let mut s = a.sync_snapshot("a").unwrap();
        s.videos[0].id = "fresh".into();
        s.videos[0].sort_at = None;
        s.videos[0].published_at = Some(777);
        b.apply_sync(&[s]).unwrap();
        assert_eq!(b.get_video("fresh").unwrap().unwrap().sort_at, Some(777));
    }

    #[test]
    fn an_uploader_added_back_by_hand_is_not_taken_for_the_removed_subscription() {
        // A removed UC1 at 300 and then added one of its uploads by hand at
        // 400, which brings UC1 back as an ad-hoc uploader: no subscription
        // for the old tombstone to cover.
        let (a, b) = (library(), library());
        a.remove_channel("UC1").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");
        a.upsert_channel(&adhoc("UC1")).unwrap();
        a.insert_video_if_new(&manual("m", "UC1")).unwrap();
        pin(&a, "videos", "m", "first_seen_at=400");

        let r = sync(&a, &b);
        assert_eq!(r.videos_added, ["m"]);
        assert!(!b.get_channel("UC1").unwrap().unwrap().subscribed, "back as an uploader");
        assert!(b.get_video("a").unwrap().is_none(), "the subscription's uploads went with it");
        assert!(!sync(&b, &a).changed);
        assert_eq!(anon(&a), anon(&b));
    }

    #[test]
    fn manual_videos_going_with_a_removed_channel_are_tombstoned_at_the_removal() {
        // B holds a manual upload of UC1 that A never had. Its tombstone has to
        // come from the removal it went with, never from B's clock, or B's
        // snapshot would move for no reason and two machines would date it apart.
        let (a, b) = (library(), library());
        b.insert_video_if_new(&manual("m", "UC1")).unwrap();
        pin(&b, "videos", "m", "first_seen_at=120");
        a.remove_channel("UC1").unwrap();
        sql(&a, "UPDATE tombstones SET removed_at=300");
        sync(&a, &b);
        assert!(b.get_video("m").unwrap().is_none());
        assert_eq!(tombstone(&b, "video", "m"), Some(300));
        sync(&b, &a);
        assert_eq!(anon(&a), anon(&b));
    }

    #[test]
    fn a_tie_on_the_stamp_is_settled_without_restamping() {
        let (a, b) = (library(), library());
        a.set_hidden("a", true).unwrap();
        pin(&a, "videos", "a", "hidden_changed_at=500");
        a.set_channel_member("UC1", true).unwrap();
        pin(&a, "channels", "UC1", "member_changed_at=600");
        b.set_hidden("a", true).unwrap();
        b.set_hidden("a", false).unwrap();
        pin(&b, "videos", "a", "hidden_changed_at=500");
        b.set_channel_member("UC1", true).unwrap();
        b.set_channel_member("UC1", false).unwrap();
        pin(&b, "channels", "UC1", "member_changed_at=600");

        sync(&a, &b);
        assert!(b.get_video("a").unwrap().unwrap().hidden, "true wins the tie");
        assert_eq!(video_stamps(&b, "a")[1], Some(500), "with the stamp it tied on");
        assert!(b.get_channel("UC1").unwrap().unwrap().member);
        assert_eq!(channel_stamps(&b, "UC1")[1], Some(600));
        assert_eq!(anon(&a), anon(&b));
    }

    #[test]
    fn apply_leaves_titles_dates_downloads_and_channel_settings_alone() {
        let (a, b) = (library(), library());
        sql(&a, "UPDATE videos SET title='renamed on a', published_at=5, sort_at=5, duration_secs=5;
                 UPDATE channels SET title='Renamed', handle='@r', url='u2'");
        a.set_watched("a", true).unwrap();
        a.set_channel_member("UC1", true).unwrap();
        b.set_download_state("a", DownloadState::Done, Some("/v/a.mkv"), None).unwrap();
        b.set_channel_auto_download("UC1", true).unwrap();
        b.set_backfill_depth("UC1", 30).unwrap();

        sync(&a, &b);
        let v = b.get_video("a").unwrap().unwrap();
        assert!(v.watched);
        assert_eq!(v.title, "t-a");
        assert_eq!((v.published_at, v.sort_at, v.duration_secs), (Some(1_000), Some(1_000), Some(600)));
        assert_eq!(v.download_state, DownloadState::Done);
        assert_eq!(v.file_path.as_deref(), Some("/v/a.mkv"));
        let c = b.get_channel("UC1").unwrap().unwrap();
        assert!(c.member);
        assert_eq!((c.title.as_str(), c.handle, c.url.as_str()),
                   ("T-UC1", None, "https://www.youtube.com/channel/UC1"));
        assert!(c.auto_download);
        assert!(b.channels_shallower_than(30).unwrap().is_empty());
    }

    #[test]
    fn a_snapshot_from_a_newer_build_is_left_alone() {
        let (a, b) = (library(), library());
        a.set_watched("a", true).unwrap();
        let mut s = a.sync_snapshot("a").unwrap();
        s.format = SNAPSHOT_FORMAT + 1;
        assert_eq!(b.apply_sync(&[s]).unwrap(), ApplyReport::default());
        assert!(!b.get_video("a").unwrap().unwrap().watched);
        assert_eq!(b.apply_sync(&[]).unwrap(), ApplyReport::default(), "nor is nothing an error");
    }

    // ---- two machines ----

    /// Two machines that spent a while apart, each with its own edits -- and
    /// some that collide.
    fn apart() -> (Db, Db) {
        let (a, b) = (library(), library());
        // B's poll saw the same uploads a little later than A's did.
        sql(&b, "UPDATE videos SET first_seen_at=110");
        // Both held one manual upload from an ad-hoc uploader before parting.
        for d in [&a, &b] {
            d.upsert_channel(&adhoc("UC2")).unwrap();
            d.insert_video_if_new(&manual("gone", "UC2")).unwrap();
            pin(d, "videos", "gone", "first_seen_at=90");
        }

        // A watches 'a', hides 'b', marks 'b' and 'c', joins UC1, subscribes
        // to UC3 and watches one of its uploads, and adds 'm1' by hand.
        a.set_watched("a", true).unwrap();
        pin(&a, "videos", "a", "watched_at=200, watched_changed_at=200");
        a.set_hidden("b", true).unwrap();
        pin(&a, "videos", "b", "hidden_changed_at=210");
        a.mark_siblings(&["b".into(), "c".into()]).unwrap();
        // The minted key carries the millisecond it was made in; pinned, so two
        // runs of this setup can be compared with each other.
        sql(&a, "UPDATE videos SET sibling_group='k-bc', sibling_changed_at=220
                  WHERE sibling_group IS NOT NULL");
        a.set_channel_member("UC1", true).unwrap();
        pin(&a, "channels", "UC1", "member_changed_at=230");
        a.upsert_channel(&chan("UC3")).unwrap();
        pin(&a, "channels", "UC3", "subscribed_changed_at=240");
        a.insert_video_if_new(&polled("x", "UC3")).unwrap();
        a.set_watched("x", true).unwrap();
        pin(&a, "videos", "x", "first_seen_at=240, watched_at=245, watched_changed_at=245");
        a.insert_video_if_new(&manual("m1", "UC2")).unwrap();
        pin(&a, "videos", "m1", "first_seen_at=250");

        // B watched 'a' earlier and took it back, hides 'c', deletes the shared
        // manual upload, subscribes to UC4 and removes it again, and adds 'm2'.
        b.set_watched("a", true).unwrap();
        b.set_watched("a", false).unwrap();
        pin(&b, "videos", "a", "watched_changed_at=160");
        b.set_hidden("c", true).unwrap();
        pin(&b, "videos", "c", "hidden_changed_at=215");
        b.delete_video("gone").unwrap();
        b.prune_orphan_channel("UC2").unwrap();
        sql(&b, "UPDATE tombstones SET removed_at=260 WHERE id='gone'");
        b.upsert_channel(&chan("UC4")).unwrap();
        b.remove_channel("UC4").unwrap();
        sql(&b, "UPDATE tombstones SET removed_at=280 WHERE id='UC4'");
        b.insert_video_if_new(&manual("m2", "UC1")).unwrap();
        pin(&b, "videos", "m2", "first_seen_at=290");
        (a, b)
    }

    /// Trades snapshots the way the sync clock does -- each side pulls what the
    /// other pushed last, and pushes again whenever that moved its own -- until
    /// a round moves nothing on either side. Returns how many rounds it took.
    fn settle(first: &Db, second: &Db) -> usize {
        for round in 1..=4 {
            let moved_second = sync(first, second).changed;
            let moved_first = sync(second, first).changed;
            if !moved_first && !moved_second { return round; }
        }
        panic!("still moving after four rounds");
    }

    /// [`settle`], with both sides pulling at the same moment each round.
    fn settle_at_once(a: &Db, b: &Db) -> usize {
        for round in 1..=4 {
            let (sa, sb) = (a.sync_snapshot("a").unwrap(), b.sync_snapshot("b").unwrap());
            let moved_a = a.apply_sync(&[sb]).unwrap().changed;
            let moved_b = b.apply_sync(&[sa]).unwrap().changed;
            if !moved_a && !moved_b { return round; }
        }
        panic!("still moving after four rounds");
    }

    /// What the two machines must agree on once they have talked.
    fn assert_settled_as_expected(d: &Db) {
        let v = |id: &str| d.get_video(id).unwrap();
        let a = v("a").unwrap();
        assert!(a.watched, "A's watch at 200 is later than B's un-watch at 160");
        assert_eq!(a.watched_at, Some(200));
        let (b, c) = (v("b").unwrap(), v("c").unwrap());
        assert!(b.hidden && c.hidden);
        assert!(b.sibling_group.is_some() && b.sibling_group == c.sibling_group);
        assert!(v("x").unwrap().watched);
        assert!(v("m1").is_some() && v("m2").is_some());
        assert!(v("gone").is_none(), "B's delete outlives A's older copy");
        let ch = |id: &str| d.get_channel(id).unwrap();
        assert!(ch("UC1").unwrap().member);
        assert!(ch("UC3").unwrap().subscribed);
        assert!(!ch("UC2").unwrap().subscribed, "back as m1's uploader");
        assert!(ch("UC4").is_none());
        assert_eq!(tombstone(d, "channel", "UC4"), Some(280));
        assert_eq!(tombstone(d, "video", "gone"), Some(260));
    }

    #[test]
    fn two_libraries_converge_whichever_pulls_first() {
        let (a, b) = apart();
        assert!(settle(&a, &b) <= 3);
        assert_eq!(anon(&a), anon(&b));
        assert_settled_as_expected(&a);

        let (a2, b2) = apart();
        assert!(settle(&b2, &a2) <= 3);
        assert_eq!(anon(&a2), anon(&b2));
        assert_eq!(anon(&a2), anon(&a), "and the order of the pulls changes nothing");
    }

    #[test]
    fn two_libraries_pulling_at_the_same_moment_converge_too() {
        let (a, b) = apart();
        assert!(settle_at_once(&a, &b) <= 3);
        assert_eq!(anon(&a), anon(&b));
        assert_settled_as_expected(&a);

        // A third machine pulling both of them in one go lands on the same library.
        let c = library();
        c.apply_sync(&[a.sync_snapshot("a").unwrap(), b.sync_snapshot("b").unwrap()]).unwrap();
        assert_eq!(anon(&c), anon(&a));
    }

    #[test]
    fn applying_the_same_snapshots_twice_changes_nothing_the_second_time() {
        let (a, b) = apart();
        let c = library();
        c.upsert_channel(&chan("UC5")).unwrap();
        c.set_hidden("c", true).unwrap();
        let snaps = [a.sync_snapshot("a").unwrap(), c.sync_snapshot("c").unwrap()];

        let first = b.apply_sync(&snaps).unwrap();
        assert!(first.changed);
        assert!(!first.channels_added.is_empty() && !first.videos_added.is_empty());
        let after_first = anon(&b);

        assert_eq!(b.apply_sync(&snaps).unwrap(), ApplyReport::default());
        assert_eq!(anon(&b), after_first);
    }

    // ---- a hidden series ----

    /// Hides or un-hides one part with its series the way
    /// `Db::set_series_hidden` writes it -- both flags together -- with both
    /// stamps pinned at `at`, in one write so the triggers keep them.
    fn hide_series(d: &Db, id: &str, on: bool, at: i64) {
        let v = on as i64;
        pin(d, "videos", id, &format!(
            "hidden={v}, hidden_changed_at={at}, series_hidden={v}, series_hidden_changed_at={at}"));
    }

    #[test]
    fn the_snapshot_carries_a_hidden_series_and_one_taken_back() {
        let d = library();
        hide_series(&d, "a", true, 150);
        // The series stamp alone is enough to carry a row.
        pin(&d, "videos", "b", "series_hidden_changed_at=160");
        d.upsert_channel(&adhoc("UC2")).unwrap();
        d.insert_video_if_new(&polled("z", "UC2")).unwrap();
        pin(&d, "videos", "z", "series_hidden=1, series_hidden_changed_at=170");

        let s = d.sync_snapshot("desk").unwrap();
        let ids: Vec<&str> = s.videos.iter().map(|v| v.id.as_str()).collect();
        assert_eq!(ids, ["a", "b", "z"], "never the untouched 'c'");
        let pair = |id: &str| {
            let v = s.videos.iter().find(|v| v.id == id).unwrap();
            (v.series_hidden, v.series_hidden_changed_at)
        };
        assert_eq!(pair("a"), (true, Some(150)));
        assert_eq!(pair("b"), (false, Some(160)), "taken back, and still carried by its stamp");
        assert!(s.channels.iter().any(|c| c.id == "UC2"), "the parent of a carried row rides along");
    }

    #[test]
    fn a_hidden_series_travels_and_so_does_taking_it_back() {
        let (a, b) = (library(), library());
        for id in ["a", "b"] { hide_series(&a, id, true, 150); }
        assert!(sync(&a, &b).changed);
        for id in ["a", "b"] {
            assert!(b.get_video(id).unwrap().unwrap().hidden, "{id}");
            assert_eq!(series_stamp(&b, id), (true, Some(150)), "copied, never restamped here");
        }
        assert_eq!(series_stamp(&b, "c"), (false, None));

        for id in ["a", "b"] { hide_series(&b, id, false, 200); }
        assert!(sync(&b, &a).changed);
        for id in ["a", "b"] {
            assert!(!a.get_video(id).unwrap().unwrap().hidden, "{id}");
            assert_eq!(series_stamp(&a, id), (false, Some(200)));
        }
        assert!(!sync(&a, &b).changed);
        assert_eq!(anon(&a), anon(&b));
    }

    #[test]
    fn un_hiding_one_part_travels_and_leaves_its_mark_alone() {
        let (a, b) = (library(), library());
        hide_series(&a, "a", true, 150);
        sync(&a, &b);
        a.set_hidden("a", false).unwrap();
        pin(&a, "videos", "a", "hidden_changed_at=200");
        assert!(sync(&a, &b).changed);
        assert!(!b.get_video("a").unwrap().unwrap().hidden);
        assert_eq!(series_stamp(&b, "a"), (true, Some(150)));
    }

    #[test]
    fn the_later_series_stamp_wins_on_either_side() {
        // The local copy is newer: the remote one writes nothing.
        let (a, b) = (library(), library());
        hide_series(&a, "a", true, 150);
        hide_series(&b, "a", false, 200);
        assert!(!sync(&a, &b).changed, "b's word at 200 is the later one");
        assert_eq!(series_stamp(&b, "a"), (false, Some(200)));
        // The remote copy is newer: it lands, stamp and all.
        assert!(sync(&b, &a).changed);
        assert_eq!(series_stamp(&a, "a"), (false, Some(200)));

        // A copy that never carried a mark loses to one that did, however it
        // reaches the row -- here riding along with a watch.
        let (a, b) = (library(), library());
        hide_series(&a, "b", true, 150);
        sync(&a, &b);
        let c = library();
        c.set_watched("b", true).unwrap();
        pin(&c, "videos", "b", "watched_at=120, watched_changed_at=120");
        assert!(sync(&c, &b).changed, "the watch lands");
        assert_eq!(series_stamp(&b, "b"), (true, Some(150)), "the mark stays");
        assert!(b.get_video("b").unwrap().unwrap().hidden);
    }

    #[test]
    fn a_tie_on_the_series_stamp_goes_to_the_mark_without_restamping() {
        let (a, b) = (library(), library());
        hide_series(&a, "a", true, 500);
        // B marked it too and took it back within the same second.
        hide_series(&b, "a", true, 400);
        hide_series(&b, "a", false, 500);
        assert!(sync(&a, &b).changed);
        assert_eq!(series_stamp(&b, "a"), (true, Some(500)), "with the stamp it tied on");
        assert!(b.get_video("a").unwrap().unwrap().hidden);
        assert!(!sync(&b, &a).changed);
        assert_eq!(anon(&a), anon(&b));
    }

    #[test]
    fn a_video_arriving_whole_brings_its_series_mark_and_stamp() {
        // A's poll found a new part of a hidden series before B's did.
        let (a, b) = (library(), library());
        a.insert_video_if_new(&polled("x", "UC1")).unwrap();
        pin(&a, "videos", "x", "first_seen_at=120");
        hide_series(&a, "x", true, 130);
        assert_eq!(sync(&a, &b).videos_added, ["x"]);
        let v = b.get_video("x").unwrap().unwrap();
        assert!(v.hidden && v.series_hidden);
        assert_eq!(series_stamp(&b, "x"), (true, Some(130)), "the insert trigger kept the sender's");
        assert!(!sync(&a, &b).changed);
    }

    #[test]
    fn a_snapshot_from_a_build_without_series_hiding_never_clears_a_mark() {
        let (old, new) = (library(), library());
        hide_series(&new, "a", true, 150);
        // The older machine hid the same part on its own, later: the most it
        // can say about it.
        old.set_hidden("a", true).unwrap();
        pin(&old, "videos", "a", "hidden_changed_at=170");
        let mut json = serde_json::to_value(old.sync_snapshot("old").unwrap()).unwrap();
        for v in json["videos"].as_array_mut().unwrap() {
            let v = v.as_object_mut().unwrap();
            v.remove("series_hidden");
            v.remove("series_hidden_changed_at");
        }
        let s: Snapshot = serde_json::from_value(json).unwrap();

        assert!(new.apply_sync(&[s]).unwrap().changed, "its hide still lands");
        assert_eq!(video_stamps(&new, "a")[1], Some(170));
        assert_eq!(series_stamp(&new, "a"), (true, Some(150)));
    }

    #[test]
    fn an_import_that_lifts_or_brings_a_series_mark_is_stamped() {
        let d = one_video("a");
        let a = Video { series_hidden: true, ..archived("a", "UC1") };
        let b = Video { series_hidden: true, hidden: true, ..archived("b", "UC1") };
        let (_, at) = around(|| {
            d.apply_import(&[chan("UC1")], &[a, b], ImportMode::Merge, &[]).unwrap()
        });
        for id in ["a", "b"] {
            let (on, stamp) = series_stamp(&d, id);
            assert!(on && stamped_now(&at, stamp), "{id}: a merge lifting it, a row arriving with it");
        }
    }

    /// [`apart`], with hidden series on top. A hides 'a' with its series. B
    /// hides 'b' with its series and takes it back, later than A's hide of
    /// 'b'. And both mark 'c' in the same second, B taking it back at once.
    fn apart_with_series() -> (Db, Db) {
        let (a, b) = apart();
        hide_series(&a, "a", true, 300);
        hide_series(&b, "b", true, 305);
        hide_series(&b, "b", false, 310);
        pin(&a, "videos", "c", "series_hidden=1, series_hidden_changed_at=320");
        pin(&b, "videos", "c", "series_hidden=1, series_hidden_changed_at=315");
        pin(&b, "videos", "c", "series_hidden=0, series_hidden_changed_at=320");
        (a, b)
    }

    fn assert_series_settled_as_expected(d: &Db) {
        let v = |id: &str| d.get_video(id).unwrap().unwrap();
        assert!(v("a").hidden && v("a").series_hidden);
        assert_eq!(series_stamp(d, "a"), (true, Some(300)));
        assert!(!v("b").hidden, "B's un-hide at 310 is later than A's hide at 210");
        assert_eq!(series_stamp(d, "b"), (false, Some(310)));
        assert!(v("c").hidden, "B's hide at 215 is untouched by either mark");
        assert_eq!(series_stamp(d, "c"), (true, Some(320)), "the mark takes the tie");
    }

    #[test]
    fn two_libraries_converge_on_their_hidden_series() {
        let (a, b) = apart_with_series();
        assert!(settle(&a, &b) <= 3);
        assert_eq!(anon(&a), anon(&b));
        assert_series_settled_as_expected(&a);

        let (a2, b2) = apart_with_series();
        assert!(settle(&b2, &a2) <= 3);
        assert_eq!(anon(&a2), anon(&b2));
        assert_eq!(anon(&a2), anon(&a), "whichever pulls first");

        let (a3, b3) = apart_with_series();
        assert!(settle_at_once(&a3, &b3) <= 3);
        assert_eq!(anon(&a3), anon(&a), "or both at the same moment");

        let c = library();
        c.apply_sync(&[a3.sync_snapshot("a").unwrap(), b3.sync_snapshot("b").unwrap()]).unwrap();
        assert_eq!(anon(&c), anon(&a), "and a third machine pulling both lands there too");
    }

    #[test]
    fn applying_the_same_series_marks_twice_changes_nothing_the_second_time() {
        let (a, b) = apart_with_series();
        let snaps = [a.sync_snapshot("a").unwrap()];
        assert!(b.apply_sync(&snaps).unwrap().changed);
        let after_first = anon(&b);
        assert_eq!(b.apply_sync(&snaps).unwrap(), ApplyReport::default());
        assert_eq!(anon(&b), after_first);
    }
}

#[cfg(test)]
mod former_titles_tests {
    use super::*;

    const CH: &str = "UC1";

    fn chan(id: &str) -> Channel {
        Channel { id: id.into(), title: format!("T-{id}"), handle: None,
                  url: format!("https://www.youtube.com/channel/{id}"),
                  thumb_path: None, subscribed: true, member: false, added_at: 0,
                  last_polled_at: None, terminated: false, auto_download: false }
    }

    /// A ready row, dated `at` so the two ends of the feed lead with different
    /// parts.
    fn row(id: &str, title: &str, at: i64) -> NewVideo {
        NewVideo { id: id.into(), channel_id: CH.into(), title: title.into(),
                   description: None, thumb_url: None, published_at: Some(at),
                   sort_at: Some(at), feed_rank: 0, added_manually: false,
                   duration_secs: Some(600), view_count: Some(1), status: VideoStatus::Ready }
    }

    fn db() -> Db {
        let d = Db::open_in_memory().unwrap();
        d.upsert_channel(&chan(CH)).unwrap();
        d
    }

    fn add(d: &Db, v: NewVideo) { assert!(d.insert_video_if_new(&v).unwrap()); }

    /// One video, `a`, under `title`.
    fn one(title: &str) -> Db {
        let d = db();
        add(&d, row("a", title, 1_000));
        d
    }

    fn sql(d: &Db, s: &str) { d.conn.lock().unwrap().execute_batch(s).unwrap(); }

    /// The names `id` carried before its current one.
    fn former(d: &Db, id: &str) -> Vec<String> {
        let c = d.conn.lock().unwrap();
        let mut st = c
            .prepare("SELECT title FROM video_titles WHERE video_id=?1 ORDER BY title")
            .unwrap();
        let rows = st.query_map(params![id], |r| r.get(0)).unwrap();
        rows.collect::<rusqlite::Result<Vec<String>>>().unwrap()
    }

    fn names_kept(d: &Db) -> i64 {
        d.conn.lock().unwrap()
            .query_row("SELECT COUNT(*) FROM video_titles", [], |r| r.get(0)).unwrap()
    }

    fn title(d: &Db, id: &str) -> String { d.get_video(id).unwrap().unwrap().title }

    /// A rename the way a poll writes one, from the channel listing.
    fn retitle(d: &Db, id: &str, to: &str) { d.set_titles(&[(id.into(), to.into())]).unwrap(); }

    fn owned(ids: &[&str]) -> Vec<String> { ids.iter().map(|s| s.to_string()).collect() }

    fn sorted(mut ids: Vec<String>) -> Vec<String> { ids.sort(); ids }

    /// What "Find siblings" on `anchor` shows.
    fn series(d: &Db, anchor: &str) -> Vec<String> {
        let f = VideoFilter { sibling_of: Some(anchor.into()), ..VideoFilter::default() };
        sorted(d.list_videos(&f).unwrap().into_iter().map(|v| v.id).collect())
    }

    /// The grouped feed, card by card, each leader first.
    fn cards(d: &Db, sort: SortOrder) -> Vec<Vec<String>> {
        let f = VideoFilter { sort, ..VideoFilter::default() };
        d.list_video_groups(&f).unwrap()
            .into_iter()
            .map(|g| g.videos.into_iter().map(|v| v.id).collect())
            .collect()
    }

    // ---- the v12 migration ----

    /// A database exactly as v11 left it, built step by step the way
    /// `sync_tests` builds v9 -- not by migrating to the top and taking v12
    /// back off, which would quietly stop being v11 the day a v13 lands.
    fn v11() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(SCHEMA_V1).unwrap();
        conn.execute_batch(
            "ALTER TABLE videos ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
             CREATE INDEX idx_videos_hidden ON videos(hidden, sort_at DESC);
             ALTER TABLE videos ADD COLUMN downloaded_at INTEGER;
             CREATE INDEX idx_videos_downloaded ON videos(downloaded_at DESC);
             ALTER TABLE videos ADD COLUMN sibling_group TEXT;
             CREATE INDEX idx_videos_sibling_group ON videos(sibling_group);
             ALTER TABLE channels ADD COLUMN member INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE channels ADD COLUMN backfill_depth INTEGER;
             ALTER TABLE channels ADD COLUMN terminated_at INTEGER;
             ALTER TABLE channels ADD COLUMN auto_download_since INTEGER;
             ALTER TABLE videos ADD COLUMN auto_queued INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN download_quality TEXT;
             ALTER TABLE videos ADD COLUMN watched_changed_at INTEGER;
             ALTER TABLE videos ADD COLUMN hidden_changed_at INTEGER;
             ALTER TABLE videos ADD COLUMN sibling_changed_at INTEGER;
             ALTER TABLE channels ADD COLUMN subscribed_changed_at INTEGER;
             ALTER TABLE channels ADD COLUMN member_changed_at INTEGER;
             CREATE TABLE tombstones (
               kind       TEXT NOT NULL,
               id         TEXT NOT NULL,
               removed_at INTEGER NOT NULL,
               PRIMARY KEY (kind, id)
             );
             ALTER TABLE videos ADD COLUMN series_hidden INTEGER NOT NULL DEFAULT 0;
             ALTER TABLE videos ADD COLUMN series_hidden_changed_at INTEGER;",
        ).unwrap();
        conn.execute_batch(SYNC_TRIGGERS).unwrap();
        conn.execute_batch(SERIES_HIDDEN_TRIGGERS).unwrap();
        conn.pragma_update(None, "user_version", 11).unwrap();
        conn
    }

    /// Every object in the schema with its SQL, whitespace squeezed: the same
    /// statement indented differently is the same schema.
    fn schema(d: &Db) -> Vec<(String, String, Option<String>)> {
        let c = d.conn.lock().unwrap();
        let mut st = c
            .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
            .unwrap();
        let rows = st
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get::<_, Option<String>>(2)?)))
            .unwrap();
        rows.map(|r| {
            let (kind, name, sql) = r.unwrap();
            (kind, name, sql.map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")))
        })
        .collect()
    }

    #[test]
    fn upgrading_a_v11_database_keeps_every_row_and_starts_with_no_former_names() {
        let conn = v11();
        conn.execute_batch(
            "INSERT INTO channels (id,title,url,subscribed,added_at) VALUES ('UC1','One','u',1,7);
             INSERT INTO videos (id,channel_id,title,status,first_seen_at,watched,sibling_group)
               VALUES ('a','UC1','The Blackwood Tapes','ready',1,1,'k'),
                      ('b','UC1','The Blackwood Tapes Part 2','ready',1,0,'k');",
        ).unwrap();
        let d = Db::init(conn).unwrap();

        let version: i64 = d.conn.lock().unwrap()
            .query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        let a = d.get_video("a").unwrap().unwrap();
        assert_eq!((a.title.as_str(), a.watched, a.sibling_group.as_deref()),
                   ("The Blackwood Tapes", true, Some("k")));
        assert_eq!(title(&d, "b"), "The Blackwood Tapes Part 2");
        assert_eq!(names_kept(&d), 0, "no name was ever kept before v12; nothing to seed");

        // A row from before the upgrade is renamed like any other.
        retitle(&d, "b", "EVERYTHING CHANGED");
        assert_eq!(former(&d, "b"), owned(&["The Blackwood Tapes Part 2"]));
    }

    #[test]
    fn an_upgraded_database_has_exactly_the_schema_of_a_fresh_one() {
        let upgraded = schema(&Db::init(v11()).unwrap());
        assert_eq!(upgraded, schema(&Db::open_in_memory().unwrap()));
        assert!(upgraded.iter().any(|(k, n, _)| k == "table" && n == "video_titles"));
        assert!(upgraded.iter().any(|(k, n, _)| k == "trigger" && n == "video_title_kept"));
    }

    // ---- the trigger ----

    #[test]
    fn a_rename_keeps_the_name_it_replaced() {
        let d = one("The Blackwood Tapes Part 2");
        retitle(&d, "a", "EVERYTHING CHANGED");
        assert_eq!(title(&d, "a"), "EVERYTHING CHANGED");
        assert_eq!(former(&d, "a"), owned(&["The Blackwood Tapes Part 2"]));

        retitle(&d, "a", "YOU WON'T BELIEVE WHAT HAPPENED");
        assert_eq!(former(&d, "a"), owned(&["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"]),
                   "a second rename forgets nothing");
    }

    #[test]
    fn a_title_that_did_not_change_records_nothing() {
        let d = one("The Blackwood Tapes");
        retitle(&d, "a", "The Blackwood Tapes");
        // Past `set_titles`' own guard: a write naming the column that leaves
        // it as it was, as a transfer Replace's upsert does on every row.
        sql(&d, "UPDATE videos SET title=title WHERE id='a'");
        d.set_watched("a", true).unwrap();
        assert_eq!(names_kept(&d), 0);
    }

    #[test]
    fn an_empty_title_is_never_kept_as_a_name() {
        let d = one("The Blackwood Tapes");
        // `set_titles` refuses a blank outright...
        retitle(&d, "a", "");
        assert_eq!(title(&d, "a"), "The Blackwood Tapes");
        assert_eq!(names_kept(&d), 0);

        // ...but `scripts/` write the column directly. Blanking it keeps the
        // real name it replaced, and the blank is not kept when a name returns.
        sql(&d, "UPDATE videos SET title='' WHERE id='a'");
        assert_eq!(former(&d, "a"), owned(&["The Blackwood Tapes"]));
        sql(&d, "UPDATE videos SET title='EVERYTHING CHANGED' WHERE id='a'");
        assert_eq!(former(&d, "a"), owned(&["The Blackwood Tapes"]));
    }

    #[test]
    fn a_title_flipping_back_moves_between_current_and_former() {
        // YouTube's title A/B test serves different variants to different
        // reads, so a poll can see a title go and come back.
        let d = one("The Blackwood Tapes Part 2");
        retitle(&d, "a", "EVERYTHING CHANGED");
        retitle(&d, "a", "The Blackwood Tapes Part 2");
        assert_eq!(former(&d, "a"), owned(&["EVERYTHING CHANGED"]),
                   "the current title is never a former one as well");

        retitle(&d, "a", "EVERYTHING CHANGED");
        assert_eq!(former(&d, "a"), owned(&["The Blackwood Tapes Part 2"]));
        retitle(&d, "a", "YOU WON'T BELIEVE WHAT HAPPENED");
        assert_eq!(former(&d, "a"), owned(&["EVERYTHING CHANGED", "The Blackwood Tapes Part 2"]),
                   "and nothing is kept twice");
    }

    #[test]
    fn former_names_go_with_their_video() {
        let d = one("The Blackwood Tapes Part 2");
        add(&d, row("b", "Cooking With Gas", 2_000));
        retitle(&d, "a", "EVERYTHING CHANGED");
        retitle(&d, "b", "Cooking With Gas, Again");

        d.delete_video("a").unwrap();
        assert!(former(&d, "a").is_empty());
        assert_eq!(former(&d, "b"), owned(&["Cooking With Gas"]), "only its own");

        // A channel removed takes its videos' names with the videos.
        d.remove_channel(CH).unwrap();
        assert_eq!(names_kept(&d), 0);
    }

    // ---- the feed ----

    fn feed_title(d: &Db, id: &str, title: &str) {
        d.apply_feed(&[], &[(id.into(), title.into())], &[]).unwrap();
    }

    #[test]
    fn a_feed_title_the_row_no_longer_carries_is_kept_but_never_written_back() {
        // Stored under its new name: ingested from the listing during an RSS
        // outage, or renamed before v12 kept anything.
        let d = one("EVERYTHING CHANGED");
        feed_title(&d, "a", "The Blackwood Tapes Part 2");
        assert_eq!(title(&d, "a"), "EVERYTHING CHANGED", "the feed is no authority on a title");
        assert_eq!(former(&d, "a"), owned(&["The Blackwood Tapes Part 2"]));

        // Every poll hands it over again; it stays one name.
        feed_title(&d, "a", "The Blackwood Tapes Part 2");
        assert_eq!(names_kept(&d), 1);

        // Renamed back by the uploader, it is current again and not former.
        retitle(&d, "a", "The Blackwood Tapes Part 2");
        assert_eq!(former(&d, "a"), owned(&["EVERYTHING CHANGED"]));
        feed_title(&d, "a", "The Blackwood Tapes Part 2");
        assert_eq!(former(&d, "a"), owned(&["EVERYTHING CHANGED"]));
    }

    #[test]
    fn a_feed_title_that_is_blank_current_or_for_no_row_records_nothing() {
        let d = one("The Blackwood Tapes");
        let titles = [
            // Unchanged: nearly every entry of every poll.
            ("a".to_string(), "The Blackwood Tapes".to_string()),
            // A mangled entry.
            ("a".to_string(), String::new()),
            // No such row: the foreign key would refuse it, and must not have to.
            ("gone".to_string(), "Deleted Since".to_string()),
        ];
        assert_eq!(d.apply_feed(&[], &titles, &[]).unwrap(), 0);
        assert_eq!(names_kept(&d), 0);
        assert_eq!(title(&d, "a"), "The Blackwood Tapes");
    }

    #[test]
    fn a_new_row_in_the_same_feed_has_no_former_names() {
        let d = one("EVERYTHING CHANGED");
        let added = d.apply_feed(
            &[("a".into(), 1_234)],
            &[("a".into(), "The Blackwood Tapes Part 2".into())],
            &[row("b", "The Blackwood Tapes Part 3", 2_000)],
        ).unwrap();
        assert_eq!(added, 1);
        assert!(former(&d, "b").is_empty(), "the feed's title is the only one it has had");
        assert_eq!(former(&d, "a"), owned(&["The Blackwood Tapes Part 2"]));
        assert_eq!(d.get_video("a").unwrap().unwrap().published_at, Some(1_234),
                   "and the date lands beside it");
    }

    // ---- the grouping rule sees every name ----

    /// The premise of the tests below, checked rather than assumed: the new
    /// name shares nothing with the series, and the part uploaded after it
    /// reads as a sequel to the new name alone.
    fn the_rename_breaks_the_titles_apart() {
        use crate::siblings::{is_sibling, normalize};
        let sib = |a: &str, b: &str| is_sibling(&normalize(a), &normalize(b));
        assert!(sib("The Blackwood Tapes", "The Blackwood Tapes Part 2"));
        assert!(sib("The Blackwood Tapes Part 2", "The Blackwood Tapes Part 3"));
        for old in ["The Blackwood Tapes", "The Blackwood Tapes Part 2", "The Blackwood Tapes Part 3"] {
            assert!(!sib(old, "EVERYTHING CHANGED Part 2"), "{old}");
        }
        for old in ["The Blackwood Tapes", "The Blackwood Tapes Part 3"] {
            assert!(!sib(old, "EVERYTHING CHANGED"), "{old}");
        }
        assert!(sib("EVERYTHING CHANGED", "EVERYTHING CHANGED Part 2"));
        assert!(sib("EVERYTHING CHANGED Part 2", "EVERYTHING CHANGED Part 3"));
    }

    /// The Blackwood Tapes in three parts, and an unrelated upload among them.
    fn tapes(d: &Db) {
        add(d, row("p1", "The Blackwood Tapes", 1_000));
        add(d, row("p2", "The Blackwood Tapes Part 2", 2_000));
        add(d, row("other", "Cooking With Gas", 2_500));
        add(d, row("p3", "The Blackwood Tapes Part 3", 3_000));
    }

    const FOUR: [&str; 4] = ["p1", "p2", "p3", "p4"];

    #[test]
    fn a_renamed_part_stays_on_its_series_card_and_brings_the_next_one() {
        the_rename_breaks_the_titles_apart();
        let d = db();
        tapes(&d);
        retitle(&d, "p2", "EVERYTHING CHANGED");

        // One card of three whichever end of the feed leads it, "other" alone.
        for sort in [SortOrder::Newest, SortOrder::Oldest] {
            let c = cards(&d, sort);
            assert_eq!(c.len(), 2, "{sort:?}: {c:?}");
            let parts = c.iter().find(|m| m.len() > 1).expect("a series card");
            assert_eq!(sorted(parts.clone()), owned(&["p1", "p2", "p3"]), "{sort:?}");
        }
        // And "Find siblings" from any part, the renamed one too.
        for id in ["p1", "p2", "p3"] {
            assert_eq!(series(&d, id), owned(&["p1", "p2", "p3"]), "{id}");
        }

        // The next part is uploaded under the new name and matches nothing the
        // others were ever called. The renamed part carries both names, so it
        // is still one card -- led by the newcomer or by the first part.
        add(&d, row("p4", "EVERYTHING CHANGED Part 2", 4_000));
        let newest = cards(&d, SortOrder::Newest);
        assert_eq!(newest[0][0], "p4");
        assert_eq!(sorted(newest[0].clone()), owned(&FOUR));
        let oldest = cards(&d, SortOrder::Oldest);
        assert_eq!(oldest[0][0], "p1");
        assert_eq!(sorted(oldest[0].clone()), owned(&FOUR));
        for id in FOUR {
            assert_eq!(series(&d, id), owned(&FOUR), "{id}");
        }

        // Hidden from the card, the series goes whole...
        assert_eq!(sorted(d.set_series_hidden("p4", true).unwrap()), owned(&FOUR));
        for id in FOUR {
            assert!(d.get_video(id).unwrap().unwrap().hidden, "{id}");
        }
        assert!(!d.get_video("other").unwrap().unwrap().hidden);
        // ...and so does the part after it.
        add(&d, row("p5", "EVERYTHING CHANGED Part 3", 5_000));
        assert_eq!(d.hide_series_arrivals().unwrap(), owned(&["p5"]));
    }

    #[test]
    fn hiding_a_series_takes_a_part_only_its_old_name_matches() {
        the_rename_breaks_the_titles_apart();
        let d = db();
        tapes(&d);
        retitle(&d, "p2", "EVERYTHING CHANGED");

        // Led from the first part, which only p2's old name reaches.
        assert_eq!(sorted(d.set_series_hidden("p1", true).unwrap()), owned(&["p1", "p2", "p3"]));
        assert!(d.get_video("p2").unwrap().unwrap().series_hidden);

        // A part under the new name matches no marked part but p2, by the
        // name it carries now.
        add(&d, row("p4", "EVERYTHING CHANGED Part 2", 4_000));
        assert_eq!(d.hide_series_arrivals().unwrap(), owned(&["p4"]));
    }

    #[test]
    fn the_sweep_follows_a_hidden_part_by_the_name_it_was_hidden_under() {
        let d = db();
        add(&d, row("p1", "The Blackwood Tapes", 1_000));
        d.set_series_hidden("p1", true).unwrap();
        // Retitled after it was hidden, as YouTubers do in the days after an
        // upload. Nothing marked is called anything like the series now.
        retitle(&d, "p1", "EVERYTHING CHANGED");

        add(&d, row("p2", "The Blackwood Tapes Part 2", 2_000));
        assert_eq!(d.hide_series_arrivals().unwrap(), owned(&["p2"]));
    }
}
