//! Recommendations: channels found on new subscription videos, checked for
//! being alive, offered in the Add dialog. See "Recommendations" in
//! `docs/architecture/recommendations.md`.

use serde_json::Value;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use crate::descriptions::{self, Fetched, Pace};
use crate::models::{Feed, RecUpload};
use crate::poll::AppState;
use crate::{avatar, config, watch_page};

/// The channels one watch page points at, the uploader left out.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Leads {
    /// Credited on the video itself: the Collaborators dialog.
    pub collab: Vec<String>,
    /// In the "up next" sidebar.
    pub next: Vec<String>,
}

/// What a channel's `/videos` page says about it.
#[derive(Debug, Clone, PartialEq)]
pub struct ChannelFacts {
    pub id: String,
    pub title: String,
    pub handle: Option<String>,
    pub avatar_url: Option<String>,
    pub subscribers: Option<i64>,
    pub video_count: Option<i64>,
}

/// The page's `ytInitialData`, read as one value and no further.
fn initial_data(html: &str) -> Option<Value> {
    watch_page::object_at(html, "var ytInitialData = ", "{")
}

fn is_channel_id(s: &str) -> bool {
    s.len() == 24
        && s.starts_with("UC")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// The first value under `key`, depth first.
fn find<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|x| find(x, key))),
        Value::Array(a) => a.iter().find_map(|x| find(x, key)),
        _ => None,
    }
}

/// Every channel id a `browseId` names under `v`, once each, in the order met,
/// `except` left out.
fn channel_ids(v: &Value, except: &str, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match x.as_str() {
                    Some(s) if k == "browseId" => {
                        if is_channel_id(s) && s != except && !out.iter().any(|o| o == s) {
                            out.push(s.to_string());
                        }
                    }
                    _ => channel_ids(x, except, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| channel_ids(x, except, out)),
        _ => {}
    }
}

/// The channels a watch page points at. `None` when the page carries no
/// `ytInitialData` at all -- a bot check, a consent wall -- which is not the
/// same as a page that points nowhere.
///
/// A collab's owner block holds the Collaborators dialog, a `browseId` per
/// credited channel (verified on `bZitriM8N28`, 2026-11-19: five); a plain
/// video's holds only the uploader's. The sidebar is signed out, so what it
/// suggests is the topic's, never the user's.
pub fn parse_leads(html: &str, uploader: &str) -> Option<Leads> {
    let data = initial_data(html)?;
    let mut leads = Leads::default();
    if let Some(owner) = find(&data, "videoOwnerRenderer") {
        channel_ids(owner, uploader, &mut leads.collab);
    }
    if let Some(sidebar) = find(&data, "secondaryResults") {
        channel_ids(sidebar, uploader, &mut leads.next);
    }
    Some(leads)
}

/// Every `content` text under `v`.
fn texts<'a>(v: &'a Value, out: &mut Vec<&'a str>) {
    match v {
        Value::Object(m) => {
            for (k, x) in m {
                match x.as_str() {
                    Some(s) if k == "content" => out.push(s),
                    _ => texts(x, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| texts(x, out)),
        _ => {}
    }
}

/// A channel's `/videos` page, fetched with `hl=en`: its name, handle and
/// picture from `channelMetadataRenderer`, and the header's "7.91K
/// subscribers" and "11 videos" lines -- the only place the total upload count
/// is written down (the feed stops at 15).
pub fn parse_channel_page(html: &str) -> Option<ChannelFacts> {
    let data = initial_data(html)?;
    let meta = find(&data, "channelMetadataRenderer")?;
    let text = |key: &str| meta.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let id = text("externalId").filter(|s| is_channel_id(s))?.to_string();
    let mut lines = Vec::new();
    if let Some(header) = data.get("header") {
        texts(header, &mut lines);
    }
    let count_of = |unit: &str| {
        lines.iter()
            .find(|l| l.ends_with(&format!(" {unit}s")) || l.ends_with(&format!(" {unit}")))
            .and_then(|l| parse_count(l))
    };
    Some(ChannelFacts {
        title: text("title").unwrap_or(&id).to_string(),
        handle: text("vanityChannelUrl")
            .and_then(|u| u.rsplit('/').next())
            .filter(|h| h.starts_with('@'))
            .map(str::to_string),
        avatar_url: meta.pointer("/avatar/thumbnails/0/url").and_then(Value::as_str).map(str::to_string),
        subscribers: count_of("subscriber"),
        video_count: count_of("video"),
        id,
    })
}

/// The number at the start of "7.91K subscribers", "1,234 videos", "1.2M".
pub fn parse_count(text: &str) -> Option<i64> {
    let word = text.split_whitespace().next()?.replace(',', "");
    let (digits, scale) = match word.chars().last()? {
        'K' => (&word[..word.len() - 1], 1e3),
        'M' => (&word[..word.len() - 1], 1e6),
        'B' => (&word[..word.len() - 1], 1e9),
        _ => (&word[..], 1.0),
    };
    let n: f64 = digits.parse().ok()?;
    Some((n * scale).round() as i64)
}

/// What one request came to, for the pace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Answer { Answered, Unreadable, Throttled }

/// What a scan learned: `Some(leads)` to record (`Some(None)`: the video is
/// scanned with nothing found), `None` to ask again later. A page without
/// `ytInitialData` -- usually a bot check -- is scanned with nothing rather
/// than retried: the pace slows instead, as in [`crate::descriptions`].
pub fn scanned(fetched: &Fetched, uploader: &str) -> (Option<Option<Leads>>, Answer) {
    match fetched {
        Fetched::Page(html) => match parse_leads(html, uploader) {
            Some(leads) => (Some(Some(leads)), Answer::Answered),
            None => (Some(None), Answer::Unreadable),
        },
        Fetched::Status(404 | 410) => (Some(None), Answer::Answered),
        Fetched::Status(_) | Fetched::Failed => (None, Answer::Throttled),
    }
}

/// What a check learned, to record.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked {
    pub facts: Option<ChannelFacts>,
    pub uploads: Vec<RecUpload>,
    pub active: bool,
}

/// What a check of a channel came to, from its `/videos` page and its feed;
/// `None` to ask again later (the queue notes the try and moves on, see
/// `Db::note_rec_attempt`), leaving what the channel last showed in place.
///
/// Only a channel that is gone (404, 410) is put away on the page alone. A
/// page without `ytInitialData` is a bot check, which says nothing about the
/// channel. A missing feed means nothing was uploaded only when the page
/// agrees; a page that counts videos beside a 404 feed is an RSS outage
/// (`polling.md`, "When RSS fails": hours long, platform-wide), which must
/// not park every channel it touches for a month.
pub fn checked(page: &Fetched, feed: &Fetched, now: i64) -> (Option<Checked>, Answer) {
    let gone = Checked { facts: None, uploads: vec![], active: false };
    let html = match page {
        Fetched::Page(html) => html,
        Fetched::Status(404 | 410) => return (Some(gone), Answer::Answered),
        Fetched::Status(_) | Fetched::Failed => return (None, Answer::Throttled),
    };
    let Some(facts) = parse_channel_page(html) else { return (None, Answer::Unreadable) };
    let feed = match feed {
        // A 200 that is no feed (a consent wall, an error page) parses as an
        // empty one; a real feed always names its channel.
        Fetched::Page(xml) => match crate::rss::parse_feed(xml) {
            Ok(feed) if !feed.channel_id.is_empty() => Some(feed),
            _ => return (None, Answer::Throttled),
        },
        Fetched::Status(404 | 410) if facts.video_count.unwrap_or(0) == 0 => None,
        Fetched::Status(_) | Fetched::Failed => return (None, Answer::Throttled),
    };
    let active = feed.as_ref().is_some_and(|f| is_active(f, now));
    let uploads = match (&feed, active) {
        (Some(f), true) => strip(f),
        _ => vec![],
    };
    (Some(Checked { facts: Some(facts), uploads, active }), Answer::Answered)
}

/// How recent a channel's newest real upload must be: this is what every
/// booster fails -- a feed of 1–4 videos from 2006–2009, where not even the
/// collab it was credited on appears.
const ACTIVE_WITHIN: i64 = 90 * 86_400;

/// Whether the channel uploaded anything but Shorts (which the feed parser
/// already dropped) in the last [`ACTIVE_WITHIN`].
pub fn is_active(feed: &Feed, now: i64) -> bool {
    feed.entries.iter().any(|e| e.published_at >= now - ACTIVE_WITHIN)
}

/// The four newest uploads, Shorts already gone.
pub fn strip(feed: &Feed) -> Vec<RecUpload> {
    let mut entries: Vec<&crate::models::FeedEntry> = feed.entries.iter().collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.published_at));
    entries.into_iter().take(4).map(|e| RecUpload {
        id: e.video_id.clone(),
        title: e.title.clone(),
        published_at: Some(e.published_at),
        thumb_path: None,
    }).collect()
}

/// Where an upload's picture is kept: under the asset scope, apart from the
/// library's own thumbnails so nothing else ever sweeps it.
pub fn thumb_path(video_id: &str) -> PathBuf {
    config::thumbs_dir().join("recs").join(format!("{video_id}.jpg"))
}

/// The 16:9 320×180 picture (the feed's own is 4:3 with bars).
fn thumb_url(video_id: &str) -> String {
    format!("https://i.ytimg.com/vi/{video_id}/mqdefault.jpg")
}

fn channel_page_url(id: &str) -> String {
    format!("https://www.youtube.com/channel/{id}/videos?hl=en")
}

/// How long after launch the first request waits: clear of the startup poll
/// and of `descriptions`' own first request.
const START_DELAY: Duration = Duration::from_secs(180);

/// How long it sleeps with nothing to scan or check.
const IDLE: Duration = Duration::from_secs(5 * 60);

enum Work { Scan(String, String), Check(String), Idle }

/// The clock, for the life of the process: one request at a time on the
/// `descriptions` pace, scans before checks, nothing while a poll runs.
pub async fn run(state: Arc<AppState>) {
    tokio::time::sleep(START_DELAY).await;
    let mut pace = Pace::new();
    loop {
        // Checked and let go at once, as `descriptions` does.
        if state.poll_lock.try_lock().is_err() {
            tokio::time::sleep(pace.interval()).await;
            continue;
        }
        let now = chrono::Utc::now().timestamp();
        let db = state.db.clone();
        let work = tokio::task::spawn_blocking(move || -> anyhow::Result<Work> {
            if let Some((video, channel)) = db.next_rec_scan(now)? {
                return Ok(Work::Scan(video, channel));
            }
            Ok(db.next_rec_check(now)?.map_or(Work::Idle, Work::Check))
        }).await;
        let answer = match work {
            Ok(Ok(Work::Scan(video, channel))) => scan(&state, &video, &channel, now).await,
            Ok(Ok(Work::Check(id))) => check(&state, &id, now).await,
            Ok(Ok(Work::Idle)) => {
                tokio::time::sleep(IDLE).await;
                continue;
            }
            Ok(Err(e)) => {
                eprintln!("mytube: recommendations: {e}");
                tokio::time::sleep(IDLE).await;
                continue;
            }
            Err(_) => {
                tokio::time::sleep(IDLE).await;
                continue;
            }
        };
        match answer {
            Answer::Answered => pace.answered(),
            Answer::Unreadable => pace.unreadable(),
            Answer::Throttled => pace.throttled(),
        }
        tokio::time::sleep(pace.interval()).await;
    }
}

async fn scan(state: &AppState, video: &str, channel: &str, now: i64) -> Answer {
    let fetched = descriptions::fetch_page(&state.http, video).await;
    let (record, answer) = scanned(&fetched, channel);
    if let Some(leads) = record {
        let (db, video, channel) = (state.db.clone(), video.to_string(), channel.to_string());
        let pruned = tokio::task::spawn_blocking(move || {
            db.record_rec_scan(&video, &channel, leads.as_ref(), now)?;
            db.prune_recs(now)
        }).await;
        match pruned {
            Ok(Ok(files)) => delete(files).await,
            Ok(Err(e)) => eprintln!("mytube: recommendations: keeping a scan failed: {e}"),
            Err(_) => {}
        }
    }
    answer
}

async fn check(state: &AppState, id: &str, now: i64) -> Answer {
    let page = descriptions::fetch(&state.http, &channel_page_url(id)).await;
    let feed = match page {
        Fetched::Page(_) => descriptions::fetch(&state.http, &crate::rss::feed_url(id)).await,
        _ => Fetched::Failed,
    };
    let (record, answer) = checked(&page, &feed, now);
    let Some(c) = record else {
        let (db, owned) = (state.db.clone(), id.to_string());
        let _ = tokio::task::spawn_blocking(move || db.note_rec_attempt(&owned, now)).await;
        return answer;
    };
    let (db, owned) = (state.db.clone(), id.to_string());
    let facts = c.facts.clone();
    let uploads = c.uploads.clone();
    let written = tokio::task::spawn_blocking(move || {
        db.record_rec_check(&owned, facts.as_ref(), &uploads, c.active, now)
    }).await;
    match written {
        Ok(Ok(files)) => delete(files).await,
        Ok(Err(e)) => {
            eprintln!("mytube: recommendations: keeping a check failed: {e}");
            return answer;
        }
        Err(_) => return answer,
    }
    if c.active {
        pictures(state, id, &c).await;
    }
    answer
}

/// The strip's pictures and the channel's, fetched once each: a file already
/// on disk is only named again.
async fn pictures(state: &AppState, id: &str, c: &Checked) {
    for u in &c.uploads {
        let path = thumb_path(&u.id);
        if !path.is_file() {
            let Ok(bytes) = avatar::download(&state.http, &thumb_url(&u.id)).await else { continue };
            let dest = path.clone();
            if !matches!(tokio::task::spawn_blocking(move || avatar::write_atomically(&dest, &bytes)).await,
                         Ok(Ok(()))) {
                continue;
            }
        }
        let (db, video, p) = (state.db.clone(), u.id.clone(), path.to_string_lossy().into_owned());
        let _ = tokio::task::spawn_blocking(move || db.set_rec_thumb(&video, &p)).await;
    }
    let (Some(url), Some(path)) = (c.facts.as_ref().and_then(|f| f.avatar_url.clone()), avatar::cached_path(id))
        else { return };
    if !path.is_file() {
        let Ok(bytes) = avatar::download(&state.http, &avatar::sized_url(&url)).await else { return };
        let dest = path.clone();
        if !matches!(tokio::task::spawn_blocking(move || avatar::write_atomically(&dest, &bytes)).await,
                     Ok(Ok(()))) {
            return;
        }
    }
    let (db, id, p) = (state.db.clone(), id.to_string(), path.to_string_lossy().into_owned());
    let _ = tokio::task::spawn_blocking(move || db.set_rec_avatar(&id, &p)).await;
}

/// Deletes files the tables let go of, off the runtime's threads.
pub async fn delete(files: Vec<String>) {
    if files.is_empty() {
        return;
    }
    let _ = tokio::task::spawn_blocking(move || {
        for f in files {
            match std::fs::remove_file(&f) {
                Err(err) if err.kind() != std::io::ErrorKind::NotFound => {
                    eprintln!("mytube: could not delete {f}: {err}");
                }
                _ => {}
            }
        }
    }).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    const COLLAB: &str = include_str!("../tests/fixtures/recs_collab_watch.html");
    const PLAIN: &str = include_str!("../tests/fixtures/recs_plain_watch.html");
    const CHANNEL: &str = include_str!("../tests/fixtures/recs_channel_videos.html");

    fn sorted(mut v: Vec<String>) -> Vec<String> {
        v.sort();
        v
    }

    #[test]
    fn collab_page_lists_its_collaborators() {
        // bZitriM8N28, uploaded by AnimeCapped Manga with four others.
        let leads = parse_leads(COLLAB, "UC5qhtUpMhpBacu9sm3V2fag").unwrap();
        assert_eq!(sorted(leads.collab), vec![
            "UC0-KdRW0iLL5igOsyGZd8kg", // Ever AU Manhwa
            "UCHxxNV44sQhvuXjkj2cpIBA", // Ackerman Manhwa
            "UCLZl9M3zsIYCWx3M0FU1eGg", // Evergarden Manhwa
            "UCeqqGV2RmuYSah-7H1o1Lsw", // Jinwoo Manhwa
        ]);
    }

    #[test]
    fn sidebar_channels_are_leads() {
        let leads = parse_leads(COLLAB, "UC5qhtUpMhpBacu9sm3V2fag").unwrap();
        assert_eq!(sorted(leads.next), vec!["UC6x41swVZP3rEmy-ODxLMFA", "UCtBu8Wb2BUoduUXJS9Uss7Q"],
            "the uploader's own videos in the sidebar are not a lead");
    }

    #[test]
    fn plain_video_has_no_collab() {
        // dQw4w9WgXcQ: one owner, whose id is the uploader's.
        let leads = parse_leads(PLAIN, "UCuAXFkgsw1L7xaCfnd5JJOw").unwrap();
        assert!(leads.collab.is_empty());
        assert_eq!(leads.next.len(), 7);
        assert!(!leads.next.contains(&"UCuAXFkgsw1L7xaCfnd5JJOw".to_string()));
    }

    #[test]
    fn page_without_initial_data_is_none() {
        assert_eq!(parse_leads("<html>Our systems have detected unusual traffic</html>", "UCx"), None);
    }

    #[test]
    fn channel_page_facts() {
        let f = parse_channel_page(CHANNEL).unwrap();
        assert_eq!(f.id, "UCfdAUuUI_gNd8BQmsJOyVdg");
        assert_eq!(f.title, "Manga Potato");
        assert_eq!(f.handle.as_deref(), Some("@MangaPotatoTOP"));
        assert_eq!(f.subscribers, Some(7910));
        assert_eq!(f.video_count, Some(11));
        assert!(f.avatar_url.unwrap().starts_with("https://yt3."));
    }

    #[test]
    fn channel_page_without_data_is_none() {
        assert_eq!(parse_channel_page("<html></html>"), None);
    }

    #[test]
    fn counts_parse() {
        assert_eq!(parse_count("7.91K subscribers"), Some(7910));
        assert_eq!(parse_count("1.2M subscribers"), Some(1_200_000));
        assert_eq!(parse_count("1,234 videos"), Some(1234));
        assert_eq!(parse_count("1 video"), Some(1));
        assert_eq!(parse_count("19 subscribers"), Some(19));
        assert_eq!(parse_count("2.5B"), Some(2_500_000_000));
        assert_eq!(parse_count("No videos"), None);
    }

    const FEED: &str = include_str!("../tests/fixtures/veritasium_feed.xml");
    const DAY: i64 = 86_400;

    fn feed(entries: &[(&str, i64)]) -> Feed {
        Feed {
            channel_id: "UCx".into(), channel_title: "X".into(), shorts_rejected: 0,
            entries: entries.iter().map(|(id, at)| crate::models::FeedEntry {
                video_id: id.to_string(), title: format!("t {id}"), description: None,
                thumb_url: None, published_at: *at, view_count: None,
            }).collect(),
        }
    }

    #[test]
    fn inactive_when_newest_real_upload_is_old() {
        let now = 2_000_000_000;
        assert!(!is_active(&feed(&[("a", now - 91 * DAY), ("b", now - 6000 * DAY)]), now),
            "a booster: nothing since 2009");
        assert!(is_active(&feed(&[("a", now - 6000 * DAY), ("b", now - 89 * DAY)]), now));
    }

    #[test]
    fn shorts_only_feed_is_inactive() {
        let mut f = feed(&[]);
        f.shorts_rejected = 15;
        assert!(!is_active(&f, 2_000_000_000));
    }

    #[test]
    fn strip_takes_four_newest_non_shorts() {
        let s = strip(&feed(&[("a", 10), ("e", 50), ("b", 20), ("d", 40), ("c", 30)]));
        let ids: Vec<&str> = s.iter().map(|u| u.id.as_str()).collect();
        assert_eq!(ids, ["e", "d", "c", "b"]);
        assert_eq!(s[0].title, "t e");
        assert_eq!(s[0].published_at, Some(50));
        assert_eq!(s[0].thumb_path, None);
    }

    #[test]
    fn scan_outcomes() {
        let page = Fetched::Page(COLLAB.into());
        let (leads, answer) = scanned(&page, "UC5qhtUpMhpBacu9sm3V2fag");
        assert_eq!(answer, Answer::Answered);
        assert_eq!(leads.unwrap().unwrap().collab.len(), 4);

        let bot = Fetched::Page("<html>unusual traffic</html>".into());
        assert_eq!(scanned(&bot, "UCx"), (Some(None), Answer::Unreadable),
            "scanned with nothing: a bot check is not asked again, the pace slows instead");
        assert_eq!(scanned(&Fetched::Status(404), "UCx"), (Some(None), Answer::Answered));
        assert_eq!(scanned(&Fetched::Status(429), "UCx"), (None, Answer::Throttled));
        assert_eq!(scanned(&Fetched::Failed, "UCx"), (None, Answer::Throttled));
    }

    #[test]
    fn check_outcomes() {
        let page = Fetched::Page(CHANNEL.into());
        let fresh = crate::rss::parse_feed(FEED).unwrap().entries[0].published_at + DAY;

        let (c, a) = checked(&page, &Fetched::Page(FEED.into()), fresh);
        let c = c.unwrap();
        assert_eq!(a, Answer::Answered);
        assert!(c.active);
        assert_eq!(c.facts.unwrap().video_count, Some(11));
        assert_eq!(c.uploads.len(), 4);

        let (c, _) = checked(&page, &Fetched::Page(FEED.into()), fresh + 365 * DAY);
        let c = c.unwrap();
        assert!(!c.active, "nothing in 90 days");
        assert!(c.facts.is_some(), "its facts are still kept");

        let gone = Checked { facts: None, uploads: vec![], active: false };
        assert_eq!(checked(&Fetched::Status(404), &Fetched::Failed, fresh),
            (Some(gone.clone()), Answer::Answered), "a terminated channel");
        assert_eq!(checked(&Fetched::Page("<html></html>".into()), &Fetched::Failed, fresh),
            (None, Answer::Unreadable), "a bot check says nothing about the channel");
        assert_eq!(checked(&page, &Fetched::Status(404), fresh), (None, Answer::Throttled),
            "a page that counts 11 videos and no feed is an RSS outage, not a dormant channel");
        assert_eq!(checked(&page, &Fetched::Page("<html>not a feed</html>".into()), fresh),
            (None, Answer::Throttled));
        let empty = Fetched::Page(CHANNEL.replace("11 videos", ""));
        let (c, a) = checked(&empty, &Fetched::Status(404), fresh);
        assert_eq!(a, Answer::Answered);
        assert!(!c.unwrap().active, "no videos on the page and no feed: nothing uploaded");
        assert_eq!(checked(&Fetched::Status(503), &Fetched::Failed, fresh), (None, Answer::Throttled));
        assert_eq!(checked(&page, &Fetched::Status(429), fresh), (None, Answer::Throttled));
    }

    /// Real YouTube: a collab's watch page, then a booster and a live channel.
    #[tokio::test]
    #[ignore = "network"]
    async fn live_scan_and_checks() {
        let http = reqwest::Client::new();
        let now = chrono::Utc::now().timestamp();
        let page = descriptions::fetch_page(&http, "bZitriM8N28").await;
        let (leads, answer) = scanned(&page, "UC5qhtUpMhpBacu9sm3V2fag");
        assert_eq!(answer, Answer::Answered);
        assert_eq!(leads.unwrap().unwrap().collab.len(), 4);

        for (id, alive) in [("UC4QcdYGH6IKUlNUhn8lzzWQ", false), ("UCfdAUuUI_gNd8BQmsJOyVdg", true)] {
            let page = descriptions::fetch(&http, &channel_page_url(id)).await;
            let feed = descriptions::fetch(&http, &crate::rss::feed_url(id)).await;
            let (c, answer) = checked(&page, &feed, now);
            let c = c.unwrap();
            assert_eq!(answer, Answer::Answered, "{id}");
            assert_eq!(c.active, alive, "{id}: Kyzen Remake is a 2007 booster");
            assert!(c.facts.unwrap().video_count.is_some(), "{id}");
            assert_eq!(c.uploads.len(), if alive { 4 } else { 0 });
        }
    }
}
