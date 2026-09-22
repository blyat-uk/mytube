use anyhow::Result;
use futures::stream::{self, StreamExt};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::db::Db;
use crate::models::*;
use crate::queue::Queue;
use crate::{config, rss, tray, upload_date, ytdlp};

const FEED_CONCURRENCY: usize = 8;
const RETRY_WINDOW_SECS: i64 = 30 * 24 * 3600;

pub struct AppState {
    pub db: Arc<Db>,
    pub http: reqwest::Client,
    pub queue: Arc<Queue>,
    pub poll_lock: Arc<Mutex<()>>,
}

pub fn cache_thumb_path(video_id: &str) -> std::path::PathBuf {
    config::thumbs_dir().join(format!("{video_id}.jpg"))
}

pub async fn cache_thumb(http: &reqwest::Client, db: &Db, video_id: &str, url: &str) {
    let path = cache_thumb_path(video_id);
    if path.exists() {
        let _ = db.set_thumb_path(video_id, &path.to_string_lossy());
        return;
    }
    let Ok(resp) = http.get(url).send().await else {
        return;
    };
    if !resp.status().is_success() {
        return;
    }
    let Ok(bytes) = resp.bytes().await else {
        return;
    };
    if bytes.is_empty() {
        return;
    }
    if std::fs::write(&path, &bytes).is_ok() {
        let _ = db.set_thumb_path(video_id, &path.to_string_lossy());
    }
}

const THUMB_CONCURRENCY: usize = 8;

/// Thumbnails were the real cost of an import: fetching ~30 per channel one at a
/// time meant a 300-channel Takeout CSV spent most of its time waiting on HTTP.
pub async fn cache_thumbs(state: &AppState, items: impl Iterator<Item = (String, String)>) {
    let jobs: Vec<(String, String)> = items.collect();
    stream::iter(jobs)
        .for_each_concurrent(THUMB_CONCURRENCY, |(id, url)| async move {
            cache_thumb(&state.http, &state.db, &id, &url).await;
        })
        .await;
}

/// How far back a poll looks for renames when it has no other reason to read
/// the listing. A YouTuber retitles to chase the algorithm, which happens in
/// the days after an upload and then stops, so the recent window is where the
/// changes are; going deeper costs a slower listing on every channel every
/// poll to catch renames that are not being made. Measured against a 30-video
/// channel: 15 entries 0.9s, 200 entries 2.7s.
pub const TITLE_REFRESH_LIMIT: u32 = 30;

/// How deep the one listing this poll fetches should go.
///
/// `backfill_count` clamps as low as 1 and says how far back a *new* channel
/// is read -- it is not a budget for the rename sweep, so it can only deepen
/// the listing, never shorten it below the window.
fn listing_limit(pending: usize, backfill: u32) -> u32 {
    if pending == 0 { TITLE_REFRESH_LIMIT } else { backfill.max(TITLE_REFRESH_LIMIT) }
}

/// The channel listing, read once per poll, for the two things only it knows:
/// the current title of every entry, and duration/status for rows still
/// unresolved.
///
/// Titles are applied to every entry, not just the unresolved ones -- a rename
/// lands on a video that resolved perfectly well weeks ago. See
/// [`Db::set_titles`] for why the feed cannot be asked instead.
///
/// Hands the entries back so the one call can also serve [`ingest_listing`],
/// which needs the same listing to find what RSS never carried. Empty when the
/// listing could not be read and the failure was swallowed.
async fn refresh_from_listing(db: &Db, channel_id: &str, backfill: u32, mode: Ingest)
    -> Result<Vec<FlatEntry>> {
    let cutoff = chrono::Utc::now().timestamp() - RETRY_WINDOW_SECS;
    let pending = db.unresolved_video_ids(channel_id, cutoff)?;

    let entries = match ytdlp::flat_playlist(channel_id, listing_limit(pending.len(), backfill))
        .await
    {
        Ok(entries) => entries,
        // With nothing to resolve this call exists only to catch renames, and
        // the rest of the poll has already succeeded: RSS landed the new
        // videos and their dates. Failing the channel over a rename that can
        // wait 30 minutes would report an error for a poll that did its job.
        //
        // None of that holds under `Ingest::Everything`: RSS is what failed, so
        // there is no poll that did its job to protect, and a swallowed error
        // here would report a clean poll for a channel nothing was read from.
        Err(err) if pending.is_empty() && mode == Ingest::MembersOnly => {
            eprintln!("mytube: title refresh for {channel_id} failed: {err}");
            return Ok(Vec::new());
        }
        Err(err) => return Err(err),
    };

    db.set_titles(
        &entries.iter().map(|e| (e.id.clone(), e.title.clone())).collect::<Vec<_>>(),
    )?;

    for e in &entries {
        if !pending.contains(&e.id) {
            continue;
        }
        let status = ytdlp::status_from(e.live_status.as_deref(), e.duration_secs);
        db.update_video_meta(&e.id, e.duration_secs, e.view_count, status)?;
        if let Some(ts) = e.published_at {
            db.set_published_at_if_missing(&e.id, ts)?;
        }
    }
    Ok(entries)
}

/// yt-dlp's word for a members-only upload.
const MEMBERS_ONLY: &str = "subscriber_only";

/// How much of the channel listing a poll may take as brand-new rows.
///
/// The two variants are the two situations a poll can be in, and the whole
/// difference between them is whether RSS delivered this channel's new videos.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ingest {
    /// RSS did its job. The listing contributes only what the feed structurally
    /// cannot: a joined channel's members-only uploads.
    MembersOnly,
    /// RSS failed and the listing is the only thing that knows this channel
    /// published anything, so it stands in for the feed entirely.
    Everything,
}

/// The entries a listing may contribute as brand-new rows.
///
/// A channel listing carries everything a channel has published; RSS carries
/// only what is public. Under [`Ingest::MembersOnly`] the difference between
/// them is the members-only upload, which is the one thing the feed can never
/// report -- so it is the one thing that pass adds. An unknown *public* id is
/// left alone: ingesting every unrecognised entry would deepen each channel's
/// library to the listing window on the next poll, overriding `backfill_count`
/// without being asked.
///
/// Under [`Ingest::Everything`] that restraint has nothing left to protect --
/// there is no feed to defer to -- so every unknown entry in the window is
/// taken. One rule survives the switch: `availability` describes the video,
/// never your access to it, and the listing serves members-only entries to
/// anyone, joined or not. `member` is the only thing that can say whether those
/// are yours to download, so a channel you have not joined contributes its
/// public uploads and nothing else.
///
/// Returns owned entries rather than borrows: the caller awaits a watch page
/// between this call and the insert, and a borrow held across that await is
/// exactly the higher-ranked lifetime `generate_handler!` cannot prove.
fn ingestable(entries: &[FlatEntry], known: &HashSet<String>, member: bool, mode: Ingest)
    -> Vec<FlatEntry> {
    entries
        .iter()
        .filter(|e| !known.contains(&e.id))
        .filter(|e| {
            let locked = e.availability.as_deref() == Some(MEMBERS_ONLY);
            match mode {
                Ingest::MembersOnly => locked && member,
                Ingest::Everything => !locked || member,
            }
        })
        .cloned()
        .collect()
}

/// Writes listing entries as new rows, returning how many were actually new.
///
/// `dates` supplies the real upload instant wherever a watch page gave one; an
/// entry missing from it keeps the listing's approximate bucket, which at least
/// interleaves it into the feed instead of piling it at the bottom.
fn insert_from_listing(db: &Db, channel_id: &str, entries: &[FlatEntry],
                       dates: &HashMap<String, i64>) -> Result<usize> {
    let mut added = 0;
    for e in entries {
        let when = dates.get(&e.id).copied().or(e.published_at);
        let inserted = db.insert_video_if_new(&NewVideo {
            id: e.id.clone(),
            channel_id: channel_id.to_string(),
            title: e.title.clone(),
            description: None,
            thumb_url: Some(ytdlp::thumb_url_for(&e.id)),
            published_at: when,
            sort_at: when,
            feed_rank: 0,
            added_manually: false,
            duration_secs: e.duration_secs,
            view_count: e.view_count,
            status: ytdlp::status_from(e.live_status.as_deref(), e.duration_secs),
        })?;
        if inserted {
            added += 1;
        }
    }
    Ok(added)
}

/// Adds what a listing carries and RSS structurally cannot: a joined channel's
/// members-only uploads.
///
/// Shared by the poll, which already holds a listing, and by joining a channel,
/// which fetches its own and deeper one.
async fn ingest_listing(state: &AppState, channel_id: &str, entries: &[FlatEntry],
                        member: bool, mode: Ingest) -> Result<usize> {
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    let known = state.db.existing_video_ids(&ids)?;
    let fresh = ingestable(entries, &known, member, mode);
    if fresh.is_empty() {
        return Ok(0);
    }

    // The same reasoning as `backfill_channel`: a listing only ever carries an
    // approximate bucket, so ask each watch page for the real instant before
    // these rows are written. A members-only watch page still serves its
    // ld+json to a signed-out request, so no cookies are needed here -- only
    // the download itself needs them.
    let fresh_ids: Vec<String> = fresh.iter().map(|e| e.id.clone()).collect();
    let real = upload_date::fetch_many(&state.http, fresh_ids).await;

    let added = insert_from_listing(&state.db, channel_id, &fresh, &real)?;
    cache_thumbs(state, fresh.iter().map(|e| (e.id.clone(), ytdlp::thumb_url_for(&e.id)))).await;
    Ok(added)
}

/// One listing, read as deep as a backfill, purely to pick up the members-only
/// back catalogue the moment a membership is recorded.
///
/// The poll's own window is [`TITLE_REFRESH_LIMIT`] and a membership backlog
/// routinely reaches past it, so joining reads `depth` entries once rather than
/// leaving the older parts permanently out of sight.
pub async fn ingest_members_only(state: &AppState, channel_id: &str, depth: u32)
    -> Result<usize> {
    let entries = ytdlp::flat_playlist(channel_id, depth).await?;
    ingest_listing(state, channel_id, &entries, true, Ingest::MembersOnly).await
}

/// How many dates one startup repairs. A flat listing costs nothing per video,
/// but a watch page is a request each, so a large library is spread over a few
/// launches rather than fired at YouTube all at once.
const DATE_REPAIR_LIMIT: usize = 1000;

/// Replaces stored dates that are not real upload dates with the true instant
/// from each video's watch page.
///
/// Two kinds of row qualify: those with no date at all, and those carrying an
/// `youtubetab:approximate_date` bucket. A bucket is not an upload date -- it
/// is "roughly this long ago" measured from the poll, so unrelated videos pile
/// onto one value and `sort_at` orders the feed by a fiction.
pub async fn repair_dates(state: &AppState) -> usize {
    let Ok(ids) = state.db.video_ids_needing_real_date(DATE_REPAIR_LIMIT) else {
        return 0;
    };
    if ids.is_empty() {
        return 0;
    }
    let real = upload_date::fetch_many(&state.http, ids).await;
    real.into_iter()
        .filter(|(id, ts)| state.db.set_published_at(id, *ts).is_ok())
        .count()
}

/// Writes a feed's entries as new rows, returning how many were actually new.
///
/// The counterpart of [`insert_from_listing`], and the only path that ever sees
/// an exact publish instant: RSS is the one source that carries one.
fn insert_from_feed(db: &Db, channel: &Channel, feed: &Feed) -> Result<usize> {
    let ids: Vec<String> = feed.entries.iter().map(|e| e.video_id.clone()).collect();
    let existing = db.existing_video_ids(&ids)?;
    let mut new_count = 0;

    for e in &feed.entries {
        if existing.contains(&e.video_id) {
            // Backfilled rows have no date; the feed supplies the real one.
            // This is also what repairs a row the listing ingested during an
            // outage: it lands on an approximate bucket and the feed replaces
            // it with the exact instant the moment RSS comes back.
            if e.published_at > 0 {
                let _ = db.set_published_at(&e.video_id, e.published_at);
            }
            // Deliberately no retitle here: a feed entry's title is frozen at
            // publish time (see `Db::set_titles`), so writing it back would
            // undo the rename `refresh_from_listing` just picked up whenever
            // the listing is unavailable.
            continue;
        }
        let inserted = db.insert_video_if_new(&NewVideo {
            id: e.video_id.clone(),
            channel_id: channel.id.clone(),
            title: e.title.clone(),
            description: e.description.clone(),
            thumb_url: e
                .thumb_url
                .clone()
                .or_else(|| Some(ytdlp::thumb_url_for(&e.video_id))),
            published_at: Some(e.published_at).filter(|t| *t > 0),
            sort_at: Some(e.published_at).filter(|t| *t > 0),
            feed_rank: 0,
            added_manually: false,
            duration_secs: None,
            view_count: e.view_count,
            status: VideoStatus::Pending,
        })?;
        if inserted {
            new_count += 1;
        }
    }
    Ok(new_count)
}

/// What one channel's poll produced.
pub struct ChannelPoll {
    pub new_videos: usize,
    pub shorts_rejected: usize,
    /// True when RSS failed and the channel listing stood in for it. Carried
    /// only so [`summary_line`] can record the outage in the journal.
    pub via_listing: bool,
}

pub async fn poll_one(
    state: &AppState,
    channel: &Channel,
    backfill: u32,
) -> Result<ChannelPoll> {
    // A feed failure is not fatal any more. YouTube's syndication endpoint goes
    // down platform-wide for hours at a time -- 404s and 500s for every channel
    // id, from every network, in every URL form -- and the poll used to report
    // each subscription as broken for the duration. The channel listing knows
    // everything the feed does except the exact timestamps, and this poll was
    // about to run one anyway for renames, so it stands in and the failure goes
    // no further than the journal. The error is kept: if the listing fails too,
    // both reasons are what the toast should say.
    let feed = rss::fetch_feed(&state.http, &channel.id).await;
    let rss_error = match &feed {
        Ok(_) => None,
        Err(err) => {
            eprintln!("mytube: {} has no feed ({err}); reading its listing instead",
                      channel.id);
            Some(err.to_string())
        }
    };
    let feed = feed.ok();
    let mode = if feed.is_some() { Ingest::MembersOnly } else { Ingest::Everything };

    let mut new_count = 0;
    if let Some(feed) = &feed {
        // Only the feed carries the channel's current name; a listing entry
        // does not, so a rename waits for the feed to come back.
        if !feed.channel_title.is_empty() && feed.channel_title != channel.title {
            let mut c = channel.clone();
            c.title = feed.channel_title.clone();
            state.db.upsert_channel(&c)?;
        }
        new_count += insert_from_feed(&state.db, channel, feed)?;
    }

    let entries = match refresh_from_listing(&state.db, &channel.id, backfill, mode).await {
        Ok(entries) => entries,
        // Both sources gone. Only now is the channel an error, and it says so
        // with both reasons: a feed 404 alone is an outage to wait out, while a
        // feed *and* a listing failing together is a channel that is really
        // gone -- renamed, deleted, or never a channel at all.
        Err(listing_err) => match rss_error {
            Some(rss_err) => {
                return Err(anyhow::anyhow!("{rss_err}; and the listing failed too: {listing_err}"))
            }
            None => return Err(listing_err),
        },
    };
    // Counted as new videos like any other: these are uploads you had not seen,
    // and the toast and the tray badge would be wrong without them.
    new_count += ingest_listing(state, &channel.id, &entries, channel.member, mode).await?;

    // Only the feed carries a thumbnail URL. Rows ingested from the listing get
    // theirs inside `ingest_listing`, from `ytdlp::thumb_url_for`.
    if let Some(feed) = &feed {
        cache_thumbs(
            state,
            feed.entries.iter().map(|e| {
                (
                    e.video_id.clone(),
                    e.thumb_url.clone().unwrap_or_else(|| ytdlp::thumb_url_for(&e.video_id)),
                )
            }),
        )
        .await;
    }
    state.db.set_channel_polled(&channel.id)?;
    Ok(ChannelPoll {
        new_videos: new_count,
        // A listing carries no Shorts to reject: it reads the /videos tab,
        // which does not serve them -- so during a fallback the Shorts guard
        // is the tab rather than `rss::is_short`. Verified 2026-09-22 against
        // a Shorts-heavy channel: its /shorts and /videos tabs returned 30
        // entries each with zero ids in common, and the shortest /videos entry
        // ran 194s. See `the_videos_tab_a_fallback_reads_serves_no_shorts`.
        shorts_rejected: feed.map(|f| f.shorts_rejected).unwrap_or(0),
        via_listing: rss_error.is_some(),
    })
}

/// One line per poll, for the journal.
///
/// mytube is launched from a `.desktop` file, so the session wraps it in a
/// transient systemd user service and stderr lands in
/// `journalctl --user -u 'app-mytube@*'`. Without this the outcome of a poll
/// existed only as the `poll://finished` toast, which a window closed to the
/// tray can never show -- so a loop that had quietly stopped finding anything
/// was indistinguishable from a night with nothing to find, and the only way
/// to tell them apart afterwards was to reconstruct it from `first_seen_at`.
/// `via_listing` is how many channels this poll read from the channel listing
/// because their feed failed. It is deliberately not a [`PollSummary`] field:
/// a fallback shows nothing on screen, and this line is the whole record of it.
fn summary_line(s: &PollSummary, via_listing: usize) -> String {
    let mut line = format!("polled {} {}", s.channels_polled, plural(s.channels_polled, "channel"));
    match s.new_videos {
        0 => line.push_str(", nothing new"),
        n => line.push_str(&format!(", {n} new")),
    }
    if s.shorts_rejected > 0 {
        let word = plural(s.shorts_rejected, "short");
        line.push_str(&format!(", {} {word} rejected", s.shorts_rejected));
    }
    if via_listing > 0 {
        line.push_str(&format!(", {via_listing} via yt-dlp (RSS down)"));
    }
    if !s.errors.is_empty() {
        let word = plural(s.errors.len(), "error");
        line.push_str(&format!(", {} {word}: {}", s.errors.len(), s.errors.join("; ")));
    }
    line
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 { word.to_string() } else { format!("{word}s") }
}

pub async fn poll_channels(
    state: &AppState,
    app: &AppHandle,
    channels: Vec<Channel>,
) -> PollSummary {
    let Ok(_guard) = state.poll_lock.try_lock() else {
        // Logged like any other outcome: "every tick prints a line" is what
        // makes a gap in the journal mean something.
        let refused = PollSummary {
            errors: vec!["A refresh is already running".into()],
            ..Default::default()
        };
        eprintln!("mytube: {}", summary_line(&refused, 0));
        return refused;
    };
    let _ = app.emit("poll://started", ());
    let backfill = config::load().map(|s| s.backfill_count).unwrap_or(30);

    // Each Channel is moved into its own future rather than borrowed from the
    // iterator: returning a future that borrows the closure argument produces a
    // higher-ranked lifetime that `generate_handler!` cannot prove.
    let results: Vec<_> = stream::iter(channels.into_iter())
        .map(|c| async move {
            let title = c.title.clone();
            (title, poll_one(state, &c, backfill).await)
        })
        .buffer_unordered(FEED_CONCURRENCY)
        .collect()
        .await;

    let mut summary = PollSummary {
        channels_polled: results.len(),
        ..Default::default()
    };
    let mut via_listing = 0;
    for (title, r) in results {
        match r {
            Ok(p) => {
                summary.new_videos += p.new_videos;
                summary.shorts_rejected += p.shorts_rejected;
                via_listing += usize::from(p.via_listing);
            }
            Err(e) => summary.errors.push(format!("{title}: {e}")),
        }
    }
    eprintln!("mytube: {}", summary_line(&summary, via_listing));
    let _ = app.emit("poll://finished", summary.clone());
    // Polls only. A backfill or a manually added video is something you asked
    // for with the window in front of you, so it is read the moment it lands.
    tray::note_new_videos(app, summary.new_videos);
    summary
}

/// One flat-playlist call seeds a newly added channel's back catalogue.
pub async fn backfill_channel(state: &AppState, channel_id: &str, count: u32) -> Result<usize> {
    let entries = ytdlp::flat_playlist(channel_id, count).await?;

    // The listing carries only `approximate_date` buckets, so ask each watch
    // page for the real instant before these rows are written. Ids that fail
    // keep the bucket and get picked up by `repair_dates` on a later start.
    //
    // Collected up front: an iterator borrowing `entries` across the await
    // makes the resulting future fail Tauri's higher-ranked lifetime bounds.
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    let real = upload_date::fetch_many(&state.http, ids).await;

    let mut added = 0;
    for (rank, e) in entries.iter().enumerate() {
        let status = ytdlp::status_from(e.live_status.as_deref(), e.duration_secs);
        let inserted = state.db.insert_video_if_new(&NewVideo {
            id: e.id.clone(),
            channel_id: channel_id.to_string(),
            title: e.title.clone(),
            description: None,
            thumb_url: Some(ytdlp::thumb_url_for(&e.id)),
            // The real date when the watch page gave one; otherwise the
            // approximate bucket, which at least interleaves the video into the
            // feed instead of piling it up at the bottom.
            published_at: real.get(&e.id).copied().or(e.published_at),
            sort_at: real.get(&e.id).copied().or(e.published_at),
            feed_rank: rank as i64,
            added_manually: false,
            duration_secs: e.duration_secs,
            view_count: e.view_count,
            status,
        })?;
        if inserted {
            added += 1;
        }
    }
    cache_thumbs(
        state,
        entries.iter().map(|e| (e.id.clone(), ytdlp::thumb_url_for(&e.id))),
    )
    .await;
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole change, end to end: a stored title that RSS will serve
    /// forever is replaced by the one the channel listing carries. Hits
    /// YouTube, so it is opt-in:
    ///
    ///     cargo test refreshes_a_real_retitled_video -- --ignored --nocapture
    #[tokio::test]
    #[ignore = "requires network"]
    async fn refreshes_a_real_retitled_video() {
        // Renamed a day after upload, 2026-09-08. The feed, the watch page,
        // its ld+json and oembed all still serve the string below; only the
        // /videos listing has the new one.
        const CHANNEL: &str = "UC8KmgQABfwRXJaJYmABwVSw";
        const VIDEO: &str = "O0jqUIRhiY0";
        const PUBLISHED_AS: &str = "Sent to Die in a Besieged City, He Uses His \
SYSTEM to Slaughter 10,000 Barbarians ALONE!";

        let db = Db::open_in_memory().unwrap();
        db.upsert_channel(&Channel {
            id: CHANNEL.into(), title: "Lumos Manhwa Recap".into(), handle: None,
            url: format!("https://www.youtube.com/channel/{CHANNEL}"), thumb_path: None,
            subscribed: true, member: false, added_at: 0, last_polled_at: None,
        })
        .unwrap();
        // Ready, so nothing is pending and the sweep runs at its shallowest.
        db.insert_video_if_new(&NewVideo {
            id: VIDEO.into(), channel_id: CHANNEL.into(), title: PUBLISHED_AS.into(),
            description: None, thumb_url: None, published_at: Some(1_788_816_831),
            sort_at: Some(1_788_816_831), feed_rank: 0, added_manually: false,
            duration_secs: Some(12_834), view_count: Some(1), status: VideoStatus::Ready,
        })
        .unwrap();

        refresh_from_listing(&db, CHANNEL, 200, Ingest::MembersOnly).await.expect("listing read");

        let title = db.get_video(VIDEO).unwrap().unwrap().title;
        println!("stored title is now: {title}");
        assert_ne!(title, PUBLISHED_AS,
                   "the listing's title must replace the publish-time one");
        assert!(!title.is_empty(), "and must not blank the row doing it");
    }

    #[test]
    fn a_title_sweep_is_shallow_but_never_shallower_than_the_window() {
        // Nothing to resolve: the listing is fetched only to catch renames.
        assert_eq!(listing_limit(0, 200), TITLE_REFRESH_LIMIT);
        // Something to resolve: go as deep as the backfill asks.
        assert_eq!(listing_limit(3, 200), 200);
        // A tiny backfill must not starve the rename sweep -- `backfill_count`
        // clamps as low as 1, and it says how far back a new channel is read,
        // not how much of the recent window a poll may look at.
        assert_eq!(listing_limit(3, 1), TITLE_REFRESH_LIMIT);
    }

    /// A fallback poll takes its new videos from the channel listing, which
    /// never passes through `rss::is_short` -- so the only thing keeping Shorts
    /// out of the library is that `flat_playlist` reads the /videos tab and
    /// YouTube serves Shorts from /shorts. That is YouTube's arrangement, not
    /// ours, so it is worth a test that can be re-run when it changes:
    ///
    ///     cargo test the_videos_tab_a_fallback_reads_serves_no_shorts -- --ignored
    ///
    /// A duration cutoff is deliberately not the guard instead: a legitimate
    /// upload can run under a minute, and dropping those would lose real videos
    /// to catch a case the tab already excludes.
    #[tokio::test]
    #[ignore = "requires network"]
    async fn the_videos_tab_a_fallback_reads_serves_no_shorts() {
        // MrBeast: posts to both tabs constantly, so the overlap is meaningful.
        const CHANNEL: &str = "UCX6OQ3DkcsbYNE6H8uQQuVA";

        let videos = ytdlp::flat_playlist(CHANNEL, 30).await.expect("videos tab");

        // The same arguments the poll uses, pointed at the other tab -- the URL
        // is the last one. Built here rather than behind a production helper
        // nothing but this test would call.
        let mut args = ytdlp::flat_playlist_args(CHANNEL, 30);
        *args.last_mut().unwrap() = format!("https://www.youtube.com/channel/{CHANNEL}/shorts");
        let out = tokio::process::Command::new("yt-dlp")
            .args(&args).output().await.expect("yt-dlp runs");
        let shorts: Vec<FlatEntry> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .filter_map(ytdlp::parse_flat_entry)
            .collect();
        assert!(!videos.is_empty() && !shorts.is_empty(), "both tabs must return entries");

        let short_ids: HashSet<String> = shorts.iter().map(|e| e.id.clone()).collect();
        let leaked: Vec<&str> = videos
            .iter()
            .filter(|e| short_ids.contains(&e.id))
            .map(|e| e.id.as_str())
            .collect();
        assert!(leaked.is_empty(), "the /videos tab served Shorts: {leaked:?}");
    }

    const CH: &str = "UC1";

    fn entry(id: &str, availability: Option<&str>) -> FlatEntry {
        FlatEntry {
            id: id.into(),
            title: format!("t-{id}"),
            duration_secs: Some(600),
            live_status: None,
            view_count: None,
            published_at: Some(BUCKET),
            availability: availability.map(str::to_string),
        }
    }

    /// Midnight UTC, which is where an `approximate_date` bucket always lands.
    const BUCKET: i64 = 1_789_862_400;

    fn member_channel(db: &Db) {
        db.upsert_channel(&Channel {
            id: CH.into(), title: "One".into(), handle: None,
            url: format!("https://www.youtube.com/channel/{CH}"), thumb_path: None,
            subscribed: true, member: true, added_at: 0, last_polled_at: None,
        })
        .unwrap();
    }

    /// The listing carries every upload, RSS only the public ones, and the gap
    /// between them is exactly what a poll may add. Taking every unknown id
    /// instead would deepen each channel's library to the listing window and
    /// quietly override `backfill_count`.
    #[test]
    fn a_poll_ingests_only_members_only_uploads_it_has_never_seen() {
        let entries = vec![
            entry("seen-public", None),
            entry("new-public", None),
            entry("seen-members", Some("subscriber_only")),
            entry("new-members", Some("subscriber_only")),
        ];
        let known: HashSet<String> =
            ["seen-public", "seen-members"].iter().map(|s| s.to_string()).collect();

        let fresh = ingestable(&entries, &known, true, Ingest::MembersOnly);
        let got: Vec<&str> = fresh.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(got, vec!["new-members"]);
    }

    /// YouTube's RSS endpoint goes down platform-wide for hours at a time, and
    /// while it is down the listing is the *only* thing that knows a channel
    /// published anything. The members-only rule exists to stop the listing
    /// deepening a library behind your back; when RSS has failed there is no
    /// feed for it to defer to, so it takes every unknown entry in the window.
    #[test]
    fn a_fallback_poll_takes_every_unknown_entry_because_rss_brought_nothing() {
        let entries = vec![
            entry("seen-public", None),
            entry("new-public", None),
            entry("newer-public", None),
        ];
        let known: HashSet<String> = ["seen-public"].iter().map(|s| s.to_string()).collect();

        let fresh = ingestable(&entries, &known, false, Ingest::Everything);
        let got: Vec<&str> = fresh.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(got, vec!["new-public", "newer-public"]);
    }

    /// The one rule that survives the fallback: `availability` describes the
    /// video, never your access to it. A locked video is undownloadable whether
    /// or not RSS is up, so a channel you have not joined still contributes
    /// only its public uploads.
    #[test]
    fn a_fallback_poll_still_refuses_locked_videos_from_a_channel_you_have_not_joined() {
        let entries = vec![
            entry("new-public", None),
            entry("new-members", Some("subscriber_only")),
        ];

        let fresh = ingestable(&entries, &HashSet::new(), false, Ingest::Everything);
        let got: Vec<&str> = fresh.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(got, vec!["new-public"]);

        // Joined, the same listing hands over both.
        let joined = ingestable(&entries, &HashSet::new(), true, Ingest::Everything);
        assert_eq!(joined.len(), 2);
    }

    /// The listing serves members-only entries to everyone, membership or not,
    /// so without this every channel that sells one would fill the feed with
    /// videos that cannot be downloaded.
    #[test]
    fn a_channel_you_have_not_joined_contributes_nothing() {
        let entries = vec![entry("new-members", Some("subscriber_only"))];
        assert!(ingestable(&entries, &HashSet::new(), false, Ingest::MembersOnly).is_empty());
    }

    #[test]
    fn ingested_rows_land_ready_and_prefer_a_real_date_to_the_bucket() {
        const REAL: i64 = 1_789_887_789;
        let db = Db::open_in_memory().unwrap();
        member_channel(&db);

        let entries = vec![entry("a", Some("subscriber_only")), entry("b", Some("subscriber_only"))];
        let dates = HashMap::from([("a".to_string(), REAL)]);
        assert_eq!(insert_from_listing(&db, CH, &entries, &dates).unwrap(), 2);

        let a = db.get_video("a").unwrap().unwrap();
        assert_eq!(a.published_at, Some(REAL));
        assert_eq!(a.sort_at, Some(REAL), "not pinned to the top: nobody added this by hand");
        assert_eq!(a.status, VideoStatus::Ready, "the listing's duration resolves the row");
        assert!(!a.added_manually, "a members-only upload is still a subscription video");

        let b = db.get_video("b").unwrap().unwrap();
        assert_eq!(b.published_at, Some(BUCKET),
                   "no watch page answered, so the bucket at least interleaves it");

        // The next poll sees the same entries and must not count them again.
        assert_eq!(insert_from_listing(&db, CH, &entries, &dates).unwrap(), 0);
    }

    fn summary(channels: usize, new: usize, shorts: usize) -> PollSummary {
        PollSummary {
            channels_polled: channels,
            new_videos: new,
            shorts_rejected: shorts,
            errors: Vec::new(),
        }
    }

    #[test]
    fn a_quiet_poll_still_reports_that_it_ran() {
        // The whole point of the line: proving the loop is alive on a night
        // where nothing was published.
        assert_eq!(summary_line(&summary(27, 0, 0), 0), "polled 27 channels, nothing new");
    }

    #[test]
    fn a_haul_is_counted() {
        assert_eq!(summary_line(&summary(27, 2, 0), 0), "polled 27 channels, 2 new");
    }

    #[test]
    fn rejected_shorts_are_only_mentioned_when_there_were_some() {
        assert_eq!(
            summary_line(&summary(27, 2, 3), 0),
            "polled 27 channels, 2 new, 3 shorts rejected"
        );
    }

    #[test]
    fn one_channel_is_not_pluralised() {
        assert_eq!(summary_line(&summary(1, 1, 1), 0), "polled 1 channel, 1 new, 1 short rejected");
    }

    /// A fallback is invisible on screen by design -- no error toast, and the
    /// ordinary "n new videos" toast fires as always -- so the journal is the
    /// only place an RSS outage is recorded. Without this line, a night spent
    /// entirely on yt-dlp would be indistinguishable from a healthy one.
    #[test]
    fn channels_that_fell_back_to_the_listing_are_named_in_the_journal() {
        assert_eq!(
            summary_line(&summary(36, 3, 0), 36),
            "polled 36 channels, 3 new, 36 via yt-dlp (RSS down)"
        );
        // A healthy poll says nothing about a fallback that did not happen.
        assert_eq!(summary_line(&summary(36, 3, 0), 0), "polled 36 channels, 3 new");
        // Both halves of a partial outage are on the line: 1 channel had no
        // feed *and* no listing, the other 35 were served by the listing.
        let partial = PollSummary {
            errors: vec!["Gone: feed 404; and the listing failed too: no such channel".into()],
            ..summary(36, 0, 0)
        };
        assert_eq!(
            summary_line(&partial, 35),
            "polled 36 channels, nothing new, 35 via yt-dlp (RSS down), \
1 error: Gone: feed 404; and the listing failed too: no such channel"
        );
    }

    #[test]
    fn a_refused_poll_still_renders_a_line() {
        // `poll_channels` bails out before polling anything when the lock is
        // held, and that line is the one worth seeing in the journal.
        let refused = PollSummary {
            errors: vec!["A refresh is already running".into()],
            ..summary(0, 0, 0)
        };
        assert_eq!(
            summary_line(&refused, 0),
            "polled 0 channels, nothing new, 1 error: A refresh is already running"
        );
    }

    #[test]
    fn errors_are_spelled_out_because_a_hidden_window_never_shows_the_toast() {
        let s = PollSummary {
            errors: vec!["Alpha: boom".into(), "Beta: bang".into()],
            ..summary(27, 0, 0)
        };
        assert_eq!(
            summary_line(&s, 0),
            "polled 27 channels, nothing new, 2 errors: Alpha: boom; Beta: bang"
        );
    }
}
