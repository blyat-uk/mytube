//! The background catch-up: a description for every row that never got one.
//!
//! Descriptions are where a series' other parts are listed (see
//! [`crate::part_links`]), and a real library had one on only a third of its
//! rows (1,601 of 4,826, 2026-10-19): the RSS feed carries one, but only for
//! the rows it inserts, and everything a listing inserted -- backfill, a
//! deepen, an RSS outage -- had none. New rows now get theirs from the feed or
//! the watch page their date already came from; this is for the rest.
//!
//! **Slowly, because the quota is YouTube's patience.** One watch page every
//! [`BASE`] seconds, one at a time, newest first -- ~3,200 rows is about
//! thirteen hours of running, spread over however many sessions that takes.
//! The table is the progress, so a restart resumes where it stopped. Any
//! refusal but a 404 or 410 doubles the wait up to [`CAP`] and a page that
//! parses brings it back down; a run of pages without player details counts as a throttle too,
//! since that is usually a bot check. With nothing to fetch it makes no
//! request at all, and a tick is skipped while a poll is running, which makes
//! requests of its own. Its own clock, like the tools and the app update:
//! folded into the poll it would make every poll slower by the backlog.
//!
//! Nothing reaches the UI: the next refetch -- a poll's, an action's -- shows
//! the cards a description regrouped. The journal is the whole record.

use std::sync::Arc;
use std::time::Duration;

use crate::db::Db;
use crate::poll::AppState;
use crate::rss::USER_AGENT;
use crate::{watch_page, ytdlp};

/// The pace when YouTube is answering normally.
pub const BASE: Duration = Duration::from_secs(15);

/// The longest a throttled catch-up waits between requests.
pub const CAP: Duration = Duration::from_secs(30 * 60);

/// How long after launch the first request waits: clear of the startup poll
/// and of a first run's tool downloads.
const START_DELAY: Duration = Duration::from_secs(120);

/// How long it sleeps once the backlog is empty before looking again.
const IDLE: Duration = Duration::from_secs(5 * 60);

/// How many unreadable pages in a row count as one throttle.
const UNREADABLE_RUN: u32 = 3;

/// How often the journal hears how far it got.
const JOURNAL_EVERY: usize = 100;

/// What one fetch of a watch page came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// The page's description (`""` for none).
    Store(String),
    /// A page came back without player details.
    Unreadable,
    /// The video is not there (a 404, a 410).
    Gone,
    /// YouTube asked us to slow down, or could not be reached.
    Throttled,
}

/// What a GET of one watch page returned.
pub enum Fetched {
    Page(String),
    Status(u16),
    Failed,
}

/// What a fetch means for the row and for the pace. Only a 404 or a 410 says
/// the video is gone; every other refusal -- a 403 to everything above all --
/// is a throttle, or the catch-up would mark the whole backlog tried at full
/// pace. A network failure is a throttle too: offline for an hour must not
/// park a row for a week.
pub fn outcome(video_id: &str, fetched: Fetched) -> Outcome {
    match fetched {
        Fetched::Page(html) => match watch_page::parse_description(&html, video_id) {
            Some(text) => Outcome::Store(text),
            None => Outcome::Unreadable,
        },
        Fetched::Status(404 | 410) => Outcome::Gone,
        Fetched::Status(_) => Outcome::Throttled,
        Fetched::Failed => Outcome::Throttled,
    }
}

/// One GET of a watch page, with the consent cookie `watch_page::fetch` sends.
pub async fn fetch_page(http: &reqwest::Client, video_id: &str) -> Fetched {
    let sent = http
        .get(ytdlp::watch_url(video_id))
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .header(reqwest::header::COOKIE, watch_page::CONSENT_COOKIE)
        .send()
        .await;
    let Ok(resp) = sent else { return Fetched::Failed };
    if !resp.status().is_success() {
        return Fetched::Status(resp.status().as_u16());
    }
    match resp.text().await {
        Ok(html) => Fetched::Page(html),
        Err(_) => Fetched::Failed,
    }
}

/// The catch-up's interval between requests.
pub struct Pace {
    interval: Duration,
    misses: u32,
}

impl Default for Pace {
    fn default() -> Self {
        Self::new()
    }
}

impl Pace {
    pub fn new() -> Self {
        Self { interval: BASE, misses: 0 }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Doubles the wait on a throttle (to [`CAP`]), halves it on an answered
    /// request (to [`BASE`]); [`UNREADABLE_RUN`] unreadable pages in a row are
    /// one throttle.
    pub fn note(&mut self, outcome: &Outcome) {
        match outcome {
            Outcome::Throttled => self.slow_down(),
            Outcome::Unreadable => {
                self.misses += 1;
                if self.misses >= UNREADABLE_RUN {
                    self.misses = 0;
                    self.slow_down();
                }
            }
            Outcome::Store(_) | Outcome::Gone => {
                self.misses = 0;
                self.interval = (self.interval / 2).max(BASE);
            }
        }
    }

    fn slow_down(&mut self) {
        self.interval = (self.interval * 2).min(CAP);
    }
}

/// Writes what one fetch learned about `id`. A throttle writes nothing: the
/// row is asked again as soon as the pace allows.
pub fn record(db: &Db, id: &str, outcome: &Outcome, now: i64) -> anyhow::Result<()> {
    match outcome {
        Outcome::Store(text) => { db.set_descriptions(&[(id.to_string(), text.clone())])?; }
        Outcome::Unreadable | Outcome::Gone => db.mark_description_tried(id, now)?,
        Outcome::Throttled => {}
    }
    Ok(())
}

/// The catch-up, for the life of the process. First derives `part_links` for
/// every description nobody has parsed (all of them, the first start after
/// the v14 upgrade), off the main thread.
pub async fn run(state: Arc<AppState>) {
    let db = state.db.clone();
    match tokio::task::spawn_blocking(move || db.derive_part_links()).await {
        Ok(Ok(n)) if n > 0 => eprintln!("mytube: read the part links of {n} descriptions"),
        Ok(Err(e)) => eprintln!("mytube: reading part links failed: {e}"),
        _ => {}
    }
    tokio::time::sleep(START_DELAY).await;

    let mut pace = Pace::new();
    let mut fetched = 0usize;
    let mut reported_empty = false;
    loop {
        // Checked and let go at once: holding it would turn a poll the user
        // asked for into "A refresh is already running".
        let polling = state.poll_lock.try_lock().is_err();
        if polling {
            tokio::time::sleep(pace.interval()).await;
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        let db = state.db.clone();
        let next = tokio::task::spawn_blocking(move || db.videos_needing_description(now, 1))
            .await
            .ok()
            .and_then(Result::ok)
            .and_then(|ids| ids.into_iter().next());
        let Some(id) = next else {
            if fetched > 0 && !reported_empty {
                eprintln!("mytube: descriptions: {fetched} fetched, none left to fetch");
                reported_empty = true;
            }
            tokio::time::sleep(IDLE).await;
            continue;
        };
        reported_empty = false;

        let result = outcome(&id, fetch_page(&state.http, &id).await);
        pace.note(&result);
        let stored = matches!(result, Outcome::Store(_));
        let db = state.db.clone();
        let written = tokio::task::spawn_blocking(move || record(&db, &id, &result, now)).await;
        if let Ok(Err(e)) = written {
            eprintln!("mytube: keeping a description failed: {e}");
        }
        if stored {
            fetched += 1;
            if fetched % JOURNAL_EVERY == 0 {
                let db = state.db.clone();
                let left = tokio::task::spawn_blocking(move || db.count_videos_needing_description(now))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or(0);
                eprintln!("mytube: descriptions: {fetched} fetched, {left} to go");
            }
        }
        tokio::time::sleep(pace.interval()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Channel, NewVideo, VideoStatus};

    const DAY: i64 = 86_400;

    #[test]
    fn pace_backs_off_and_recovers() {
        let mut p = Pace::new();
        assert_eq!(p.interval(), Duration::from_secs(15));
        p.note(&Outcome::Throttled);
        assert_eq!(p.interval(), Duration::from_secs(30));
        p.note(&Outcome::Throttled);
        assert_eq!(p.interval(), Duration::from_secs(60));
        for _ in 0..20 {
            p.note(&Outcome::Throttled);
        }
        assert_eq!(p.interval(), CAP);
        p.note(&Outcome::Store(String::new()));
        assert_eq!(p.interval(), CAP / 2);
        for _ in 0..20 {
            p.note(&Outcome::Gone);
        }
        assert_eq!(p.interval(), BASE, "an answered request never takes it below the base");
    }

    #[test]
    fn three_unreadable_pages_count_as_throttle() {
        let mut p = Pace::new();
        p.note(&Outcome::Unreadable);
        p.note(&Outcome::Unreadable);
        assert_eq!(p.interval(), BASE);
        p.note(&Outcome::Unreadable);
        assert_eq!(p.interval(), BASE * 2, "a run of them is usually a bot-check page");

        p.note(&Outcome::Unreadable);
        p.note(&Outcome::Store("x".into()));
        p.note(&Outcome::Unreadable);
        p.note(&Outcome::Unreadable);
        assert_eq!(p.interval(), BASE, "a readable page in between starts the run over");
    }

    #[test]
    fn outcomes_from_fetches() {
        let id = "abcdefghijk";
        let page = r#""videoDetails":{"videoId":"abcdefghijk","shortDescription":"Part 1"}"#;
        assert_eq!(outcome(id, Fetched::Page(page.into())), Outcome::Store("Part 1".into()));
        assert_eq!(outcome("zzzzzzzzzzz", Fetched::Page(page.into())), Outcome::Unreadable);
        assert_eq!(outcome(id, Fetched::Page("<html>consent</html>".into())), Outcome::Unreadable);
        assert_eq!(outcome(id, Fetched::Status(429)), Outcome::Throttled);
        assert_eq!(outcome(id, Fetched::Status(503)), Outcome::Throttled);
        assert_eq!(outcome(id, Fetched::Status(404)), Outcome::Gone);
        assert_eq!(outcome(id, Fetched::Status(410)), Outcome::Gone);
        assert_eq!(outcome(id, Fetched::Status(403)), Outcome::Throttled,
                   "a refusal to everything must slow the catch-up, not burn the backlog");
        assert_eq!(outcome(id, Fetched::Failed), Outcome::Throttled,
                   "offline is no reason to park a row for a week");
    }

    fn library() -> Db {
        let d = Db::open_in_memory().unwrap();
        d.upsert_channel(&Channel {
            id: "UC1".into(), title: "One".into(), handle: None, url: "u".into(),
            thumb_path: None, subscribed: true, member: false, added_at: 0,
            last_polled_at: None, terminated: false, auto_download: false,
        }).unwrap();
        d.insert_video_if_new(&NewVideo {
            id: "a".into(), channel_id: "UC1".into(), title: "t".into(), description: None,
            thumb_url: None, published_at: Some(10), sort_at: Some(10), feed_rank: 0,
            added_manually: false, duration_secs: Some(60), view_count: None,
            status: VideoStatus::Ready,
        }).unwrap();
        d
    }

    #[test]
    fn non_parsing_page_is_retried_after_a_week() {
        let now = 100 * DAY;
        for kind in [Outcome::Unreadable, Outcome::Gone] {
            let d = library();
            record(&d, "a", &kind, now).unwrap();
            assert!(d.videos_needing_description(now + 6 * DAY, 10).unwrap().is_empty(), "{kind:?}");
            assert_eq!(d.videos_needing_description(now + 8 * DAY, 10).unwrap(), ["a"]);
        }
        let d = library();
        record(&d, "a", &Outcome::Throttled, now).unwrap();
        assert_eq!(d.videos_needing_description(now, 10).unwrap(), ["a"], "asked again soon");
    }

    #[test]
    fn a_read_page_stores_the_description_and_its_links() {
        let d = library();
        record(&d, "a", &Outcome::Store("Part 2 https://youtu.be/BBBBBBBBBBB".into()), 5).unwrap();
        let v = d.get_video("a").unwrap().unwrap();
        assert_eq!(v.description.as_deref(), Some("Part 2 https://youtu.be/BBBBBBBBBBB"));
        assert!(d.videos_needing_description(5, 10).unwrap().is_empty());
    }
}
