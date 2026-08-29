use anyhow::Result;
use futures::stream::{self, StreamExt};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};
use tokio::sync::Mutex;

use crate::db::Db;
use crate::models::*;
use crate::queue::Queue;
use crate::{config, rss, ytdlp};

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
    }
    Ok(())
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

    for e in &feed.entries {
        let url = e
            .thumb_url
            .clone()
            .unwrap_or_else(|| ytdlp::thumb_url_for(&e.video_id));
        cache_thumb(&state.http, &state.db, &e.video_id, &url).await;
    }
    state.db.set_channel_polled(&channel.id)?;
    Ok((new_count, feed.shorts_rejected))
}

pub async fn poll_channels(
    state: &AppState,
    app: &AppHandle,
    channels: Vec<Channel>,
) -> PollSummary {
    let Ok(_guard) = state.poll_lock.try_lock() else {
        return PollSummary {
            errors: vec!["A refresh is already running".into()],
            ..Default::default()
        };
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
    let _ = app.emit("poll://finished", summary.clone());
    summary
}

/// One flat-playlist call seeds a newly added channel's back catalogue.
pub async fn backfill_channel(state: &AppState, channel_id: &str, count: u32) -> Result<usize> {
    let entries = ytdlp::flat_playlist(channel_id, count).await?;
    let mut added = 0;
    for (rank, e) in entries.iter().enumerate() {
        let status = ytdlp::status_from(e.live_status.as_deref(), e.duration_secs);
        let inserted = state.db.insert_video_if_new(&NewVideo {
            id: e.id.clone(),
            channel_id: channel_id.to_string(),
            title: e.title.clone(),
            description: None,
            thumb_url: Some(ytdlp::thumb_url_for(&e.id)),
            published_at: None, // flat-playlist provides no date
            sort_at: None,      // sorts last, ordered by feed_rank
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
    for e in &entries {
        cache_thumb(&state.http, &state.db, &e.id, &ytdlp::thumb_url_for(&e.id)).await;
    }
    Ok(added)
}
