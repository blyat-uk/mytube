//! Auto-clear (Settings -> New videos): deletes the videos YouTube no longer
//! has -- deleted, or made private -- unless they are downloaded. Neither can
//! ever be downloaded again, so the row is only clutter; a downloaded video's
//! file is now the only copy, so it stays.
//!
//! The only witness that covers a whole channel is the whole channel: once a
//! day each subscription's uploads playlist is read to the end and every
//! clearable row it no longer names goes. The poll's own listing (the newest
//! 30) and the feed (the newest 15) cannot see a deletion further back, and a
//! per-video question to YouTube costs a request per video. Verified
//! 2026-11-27 against the user's library: one channel had deleted 126 of its
//! 128 videos, another had made 23 of 461 private, at positions 6 to 61.
//!
//! The pace is one channel at a time, at random gaps that average a day
//! divided by the number of subscriptions, so the checks spread across the
//! day instead of arriving as a burst; the channel checked longest ago goes
//! next, and only once its last check is a day old. The clock sits out while a
//! poll runs, as `descriptions` and `recs` do.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tauri::{AppHandle, Emitter};

use crate::poll::AppState;
use crate::{config, ytdlp};

const START_DELAY: Duration = Duration::from_secs(5 * 60);

/// Rows left the library: the shell refetches. Its own event, like
/// `sync://finished`: `poll://finished` would toast "N new videos".
pub const CLEARED_EVENT: &str = "library://cleared";
const DAY: i64 = 86_400;

/// A random wait averaging a day shared among `channels`: anywhere from half
/// to one and a half shares, kept between a minute and six hours.
fn gap(channels: usize, random: u64) -> Duration {
    let share = DAY as u64 / channels.max(1) as u64;
    Duration::from_secs((share / 2 + random % (share + 1)).clamp(60, 6 * 3600))
}

pub async fn run(state: Arc<AppState>, app: AppHandle) {
    tokio::time::sleep(START_DELAY).await;
    loop {
        let db = state.db.clone();
        let channels = tokio::task::spawn_blocking(move || db.list_subscribed_channels())
            .await.ok().and_then(Result::ok).map_or(0, |c| c.len());
        tokio::time::sleep(gap(channels, getrandom::u64().unwrap_or(0))).await;

        if !config::load().unwrap_or_default().auto_clear_removed { continue; }
        // Checked and let go at once, as `descriptions` does: holding it would
        // turn a poll the user asked for into "A refresh is already running".
        if state.poll_lock.try_lock().is_err() { continue; }

        let now = chrono::Utc::now().timestamp();
        let db = state.db.clone();
        let due = tokio::task::spawn_blocking(move || db.channel_due_for_removal_check(now - DAY))
            .await.ok().and_then(Result::ok).flatten();
        let Some((channel_id, member)) = due else { continue };

        match check(&state, &channel_id, member).await {
            Ok(0) => {}
            Ok(n) => {
                eprintln!("mytube: {channel_id}: cleared {n} videos deleted or made private on YouTube");
                let _ = app.emit(CLEARED_EVENT, ());
            }
            Err(e) => eprintln!("mytube: {channel_id}: checking for deleted videos failed: {e}"),
        }
        // Stamped whatever happened: a channel whose listing fails waits a day
        // like any other rather than being asked again at every wake.
        let db = state.db.clone();
        let _ = tokio::task::spawn_blocking(move || db.set_removal_checked(&channel_id, now)).await;
    }
}

/// One channel's check; returns how many rows went.
async fn check(state: &AppState, channel_id: &str, member: bool) -> Result<usize> {
    let inv = state.tools.ytdlp_without_cookies(&config::load().unwrap_or_default()).await?;
    let mut listed: HashSet<String> = ytdlp::full_listing(&inv, &ytdlp::uploads_url(channel_id))
        .await?.entries.into_iter().map(|e| e.id).collect();
    // The Videos tab serves members-only entries to anyone; the uploads
    // playlist may not, and only a channel you are a member of has such rows.
    if member {
        listed.extend(ytdlp::full_listing(&inv, &ytdlp::channel_videos_url(channel_id))
            .await?.entries.into_iter().map(|e| e.id));
    }
    drop(inv);

    let db = state.db.clone();
    let id = channel_id.to_string();
    let thumbs = tokio::task::spawn_blocking(move || -> Result<(usize, Vec<String>)> {
        let rows = db.removal_rows(&id)?;
        let gone = doomed(&rows, &listed);
        if gone.is_empty() { return Ok((0, Vec::new())); }
        db.delete_removed(&gone)
    }).await??;
    for path in &thumbs.1 {
        let _ = std::fs::remove_file(path);
    }
    Ok(thumbs.0)
}

/// The clearable rows `listed` does not name. A listing that names none of
/// the channel's rows at all says nothing about them -- a channel id that now
/// answers with something else, a listing YouTube cut short -- and dooms none.
fn doomed(rows: &[(String, bool)], listed: &HashSet<String>) -> Vec<String> {
    if !rows.iter().any(|(id, _)| listed.contains(id)) { return Vec::new(); }
    rows.iter()
        .filter(|(id, clearable)| *clearable && !listed.contains(id))
        .map(|(id, _)| id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(list: &[(&str, bool)]) -> Vec<(String, bool)> {
        list.iter().map(|(id, c)| (id.to_string(), *c)).collect()
    }

    fn set(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn what_the_channel_no_longer_lists_goes_unless_it_must_stay() {
        // "kept" is downloaded (or pasted in, or queued): never clearable.
        let r = rows(&[("live", true), ("gone", true), ("kept", false), ("private", true)]);
        assert_eq!(doomed(&r, &set(&["live"])), vec!["gone".to_string(), "private".to_string()]);
        assert!(doomed(&r, &set(&["live", "gone", "private"])).is_empty());
    }

    #[test]
    fn a_listing_that_names_none_of_the_rows_dooms_nothing() {
        let r = rows(&[("a", true), ("b", true)]);
        assert!(doomed(&r, &set(&[])).is_empty());
        assert!(doomed(&r, &set(&["x", "y"])).is_empty());
        // Even when the only row it names is one that stays.
        let r = rows(&[("kept", false), ("b", true)]);
        assert_eq!(doomed(&r, &set(&["kept"])), vec!["b".to_string()]);
    }

    #[test]
    fn the_gaps_share_a_day_among_the_channels() {
        let channels = 120; // a 12-minute share
        assert_eq!(gap(channels, 0), Duration::from_secs(360));
        assert_eq!(gap(channels, 720), Duration::from_secs(1080));
        assert_eq!(gap(channels, 721), Duration::from_secs(360));
        // Never a burst, and never longer than six hours.
        assert_eq!(gap(100_000, 0), Duration::from_secs(60));
        assert_eq!(gap(0, 0), Duration::from_secs(6 * 3600));
        assert_eq!(gap(1, u64::MAX), Duration::from_secs(6 * 3600));
    }
}
