use anyhow::Result;
use futures::stream::{self, StreamExt};
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

/// Fills duration/status for a channel's unresolved videos with ONE yt-dlp call.
async fn resolve_metadata(db: &Db, channel_id: &str, backfill: u32) -> Result<()> {
    let cutoff = chrono::Utc::now().timestamp() - RETRY_WINDOW_SECS;
    let pending = db.unresolved_video_ids(channel_id, cutoff)?;
    if pending.is_empty() {
        return Ok(());
    }

    let entries = ytdlp::flat_playlist(channel_id, backfill).await?;
    for e in entries {
        if !pending.contains(&e.id) {
            continue;
        }
        let status = ytdlp::status_from(e.live_status.as_deref(), e.duration_secs);
        db.update_video_meta(&e.id, e.duration_secs, e.view_count, status)?;
        if let Some(ts) = e.published_at {
            db.set_published_at_if_missing(&e.id, ts)?;
        }
    }
    Ok(())
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

pub async fn poll_one(
    state: &AppState,
    channel: &Channel,
    backfill: u32,
) -> Result<(usize, usize)> {
    let feed = rss::fetch_feed(&state.http, &channel.id).await?;

    if !feed.channel_title.is_empty() && feed.channel_title != channel.title {
        let mut c = channel.clone();
        c.title = feed.channel_title.clone();
        state.db.upsert_channel(&c)?;
    }

    let ids: Vec<String> = feed.entries.iter().map(|e| e.video_id.clone()).collect();
    let existing = state.db.existing_video_ids(&ids)?;
    let mut new_count = 0;

    for e in &feed.entries {
        if existing.contains(&e.video_id) {
            // Backfilled rows have no date; the feed supplies the real one.
            if e.published_at > 0 {
                let _ = state.db.set_published_at(&e.video_id, e.published_at);
            }
            // A retitled upload. Called for every entry the feed carries --
            // `set_title` itself ignores an empty or unchanged title -- which
            // scopes the refresh to the ~15 recent videos RSS knows about.
            let _ = state.db.set_title(&e.video_id, &e.title);
            continue;
        }
        let inserted = state.db.insert_video_if_new(&NewVideo {
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

    resolve_metadata(&state.db, &channel.id, backfill).await?;

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
    state.db.set_channel_polled(&channel.id)?;
    Ok((new_count, feed.shorts_rejected))
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
fn summary_line(s: &PollSummary) -> String {
    let mut line = format!("polled {} {}", s.channels_polled, plural(s.channels_polled, "channel"));
    match s.new_videos {
        0 => line.push_str(", nothing new"),
        n => line.push_str(&format!(", {n} new")),
    }
    if s.shorts_rejected > 0 {
        let word = plural(s.shorts_rejected, "short");
        line.push_str(&format!(", {} {word} rejected", s.shorts_rejected));
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
        eprintln!("mytube: {}", summary_line(&refused));
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
    for (title, r) in results {
        match r {
            Ok((n, shorts)) => {
                summary.new_videos += n;
                summary.shorts_rejected += shorts;
            }
            Err(e) => summary.errors.push(format!("{title}: {e}")),
        }
    }
    eprintln!("mytube: {}", summary_line(&summary));
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
        assert_eq!(summary_line(&summary(27, 0, 0)), "polled 27 channels, nothing new");
    }

    #[test]
    fn a_haul_is_counted() {
        assert_eq!(summary_line(&summary(27, 2, 0)), "polled 27 channels, 2 new");
    }

    #[test]
    fn rejected_shorts_are_only_mentioned_when_there_were_some() {
        assert_eq!(
            summary_line(&summary(27, 2, 3)),
            "polled 27 channels, 2 new, 3 shorts rejected"
        );
    }

    #[test]
    fn one_channel_is_not_pluralised() {
        assert_eq!(summary_line(&summary(1, 1, 1)), "polled 1 channel, 1 new, 1 short rejected");
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
            summary_line(&refused),
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
            summary_line(&s),
            "polled 27 channels, nothing new, 2 errors: Alpha: boom; Beta: bang"
        );
    }
}
