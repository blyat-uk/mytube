//! The Add box's videos and playlists: what a pasted link would add (the
//! preview), and adding it.
//!
//! A video or a playlist's parts go into the library as ad-hoc rows
//! (`added_manually`, uploader stored unsubscribed), exactly as one pasted
//! video always did, and nothing here downloads: the dialog's Download is an
//! add followed by an ordinary enqueue. A playlist is never followed and its
//! parts are never hand-linked -- they are separate videos, and the matcher
//! groups them on its own as it does any channel's series.
//!
//! A channel gets a preview too, but one that reads nothing from the network:
//! it only says whether you already follow the channel, so the box can say so
//! instead of offering Subscribe.
//!
//! The payloads are dialog payloads, so camelCase, and they live here rather
//! than in `models.rs`: nothing else speaks them.

use anyhow::{anyhow, bail, Result};
use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::db::Db;
use crate::models::{Channel, DownloadState, NewVideo, ProbeInfo, Video, VideoStatus};
use crate::poll::{self, AppState};
use crate::quality::Quality;
use crate::resolve::{self, AddTarget, ChannelInput};
use crate::watch_page::{self, WatchPage};
use crate::{config, ytdlp};

/// Said when a pasted video turns out to be a Short. A `/shorts/` link is
/// refused earlier, by its URL, with the sentence `add_video` always used.
pub const SHORT_REFUSAL: &str = "That video is a YouTube Short — MyTube does not handle Shorts.";

/// Where a video whose uploader could not be learned at all is filed -- the
/// placeholder `add_video` has always used, so such rows keep sharing one.
const UNKNOWN_CHANNEL_ID: &str = "UC000000000000000000000";
const UNKNOWN_CHANNEL_TITLE: &str = "Added manually";

/// Watch pages and `/shorts/` checks in flight at once while a playlist is
/// added: the concurrency feeds, thumbnails and date repair already use
/// against YouTube.
const PAGE_CONCURRENCY: usize = 8;

// ------------------------------------------------------------- payloads

/// What the Add box would add. Tagged by `kind`, which matches `AddKind`.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AddPreview {
    /// The id of a channel you already follow that the input names, else
    /// `None` -- including for a `/c/` or `/user/` link, which only a page
    /// fetch could resolve and this deliberately does not make.
    Channel { subscribed_id: Option<String> },
    /// `playlist_id` is the link's `list=`, so the box can offer the whole
    /// playlist instead; never a mix.
    Video { video: VideoPreview, playlist_id: Option<String> },
    Playlist { playlist: PlaylistPreview },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPreview {
    pub id: String,
    pub title: String,
    pub channel_id: String,
    pub channel_title: String,
    pub duration_secs: Option<i64>,
    pub published_at: Option<i64>,
    /// Remote `hqdefault.jpg`, for a video not cached yet.
    pub thumb_url: String,
    /// The cached thumbnail of a video already in the library.
    pub thumb_path: Option<String>,
    pub in_library: bool,
    /// `none` for a video not in the library.
    pub download_state: DownloadState,
    /// Best effort: read from the watch page, and `false` when unknown -- a
    /// library row does not record it, and the yt-dlp fallback cannot say.
    pub members_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistPreview {
    pub id: String,
    pub title: String,
    pub channel_id: Option<String>,
    pub channel_title: Option<String>,
    /// `playlist_count` as YouTube reports it; the listed entries when it
    /// reports none.
    pub total: usize,
    /// At most [`ytdlp::PLAYLIST_LIMIT`], in playlist order, private and
    /// deleted ones removed and each video once.
    pub entries: Vec<PlaylistEntry>,
    /// Private and deleted entries that were removed.
    pub unavailable: usize,
    /// The listing stopped at [`ytdlp::PLAYLIST_LIMIT`] with more left. Not
    /// the same as `total > entries.len()`, which is also true of a short
    /// playlist holding a deleted video, or one YouTube counts hidden videos in.
    pub truncated: bool,
}

/// One part of a playlist preview. The dialog hands the ticked ones back to
/// [`add_playlist`] as they came, so it deserialises too; everything but the
/// id and title has a default, so a leaner object still adds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistEntry {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub channel_id: Option<String>,
    #[serde(default)]
    pub channel_title: Option<String>,
    #[serde(default)]
    pub duration_secs: Option<i64>,
    #[serde(default)]
    pub thumb_url: String,
    #[serde(default)]
    pub thumb_path: Option<String>,
    #[serde(default)]
    pub in_library: bool,
    #[serde(default = "nothing_downloaded")]
    pub download_state: DownloadState,
    #[serde(default)]
    pub members_only: bool,
}

fn nothing_downloaded() -> DownloadState {
    DownloadState::None
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AddPlaylistResult {
    /// Newly inserted, in the order given.
    pub added: Vec<String>,
    /// Already in the library, and left exactly as they were.
    pub present: Vec<String>,
    /// Refused as Shorts.
    pub skipped_shorts: Vec<String>,
}

// ------------------------------------------------------------- pure parts

fn fold_handle(h: &str) -> String {
    h.trim().trim_start_matches('@').to_lowercase()
}

/// The channel you already follow that `input` names, if any: by its `UC`
/// id (bare or in a `/channel/` link) or by its `@handle`, compared without
/// case as YouTube does. Handles are only known for channels a poll has
/// listed since they started being recorded, so a miss here is not proof.
pub fn subscribed_id(input: &str, channels: &[Channel]) -> Option<String> {
    let mut followed = channels.iter().filter(|c| c.subscribed);
    let found = match resolve::parse_channel_input(input).ok()? {
        ChannelInput::Id(id) => followed.find(|c| c.id == id),
        ChannelInput::FetchUrl(_) => {
            let want = fold_handle(&resolve::handle_of(input)?);
            followed.find(|c| c.handle.as_deref().is_some_and(|h| fold_handle(h) == want))
        }
    };
    found.map(|c| c.id.clone())
}

/// A library row as the preview shows it: instant, and the only source that
/// knows the download state.
pub fn video_from_row(v: &Video) -> VideoPreview {
    VideoPreview {
        id: v.id.clone(),
        title: v.title.clone(),
        channel_id: v.channel_id.clone(),
        channel_title: v.channel_title.clone(),
        duration_secs: v.duration_secs,
        published_at: v.published_at,
        thumb_url: ytdlp::thumb_url_for(&v.id),
        thumb_path: v.thumb_path.clone(),
        in_library: true,
        download_state: v.download_state,
        members_only: false,
    }
}

/// One video as found on YouTube, from whichever source answered.
#[derive(Debug, Clone, PartialEq)]
struct Found {
    id: String,
    title: String,
    channel_id: Option<String>,
    channel_title: Option<String>,
    duration_secs: Option<i64>,
    published_at: Option<i64>,
    members_only: bool,
    description: Option<String>,
}

impl From<WatchPage> for Found {
    fn from(p: WatchPage) -> Self {
        Found {
            id: p.id,
            title: p.title,
            channel_id: p.channel_id,
            channel_title: p.channel_title,
            duration_secs: p.duration_secs,
            published_at: p.published_at,
            members_only: p.members_only,
            description: p.description,
        }
    }
}

impl From<ProbeInfo> for Found {
    fn from(p: ProbeInfo) -> Self {
        let some = |s: String| Some(s).filter(|s| !s.trim().is_empty());
        Found {
            id: p.id,
            title: p.title,
            channel_id: some(p.channel_id),
            channel_title: some(p.channel_title),
            duration_secs: p.duration_secs,
            published_at: p.published_at,
            members_only: false,
            // The probe prints no description; the pasted video's next feed
            // or the background catch-up brings it.
            description: None,
        }
    }
}

/// The uploader to file a video under: the id with the name that belongs to
/// it, or the shared placeholder when no id is known at all.
fn uploader(id: Option<String>, title: Option<String>) -> (String, String) {
    match id {
        Some(id) => (id, title.unwrap_or_else(|| UNKNOWN_CHANNEL_TITLE.to_string())),
        None => (UNKNOWN_CHANNEL_ID.to_string(), UNKNOWN_CHANNEL_TITLE.to_string()),
    }
}

/// subscribed = false: keeps the uploader's name on the card without adding
/// them to the subscription list or any poll cycle.
fn ad_hoc_channel(id: &str, title: &str, now: i64) -> Channel {
    Channel {
        id: id.to_string(),
        title: title.to_string(),
        handle: None,
        url: format!("https://www.youtube.com/channel/{id}"),
        thumb_path: None,
        subscribed: false,
        member: false,
        added_at: now,
        last_polled_at: None,
        terminated: false,
        auto_download: false,
    }
}

/// A pasted video's row: it sorts to the top of the grid by `sort_at`, while
/// its card still shows the real upload date.
fn manual_row(id: &str, title: String, channel_id: String, duration_secs: Option<i64>,
              published_at: Option<i64>, feed_rank: i64, now: i64) -> NewVideo {
    NewVideo {
        id: id.to_string(),
        channel_id,
        title,
        description: None,
        thumb_url: Some(ytdlp::thumb_url_for(id)),
        published_at,
        sort_at: Some(now),
        feed_rank,
        added_manually: true,
        duration_secs,
        view_count: None,
        // Ready even without a duration (a premiere): an ad-hoc uploader is
        // never polled, so a row left pending would never surface at all.
        status: VideoStatus::Ready,
    }
}

impl Found {
    fn preview(&self) -> VideoPreview {
        let (channel_id, channel_title) = uploader(self.channel_id.clone(), self.channel_title.clone());
        VideoPreview {
            id: self.id.clone(),
            title: self.title.clone(),
            channel_id,
            channel_title,
            duration_secs: self.duration_secs,
            published_at: self.published_at,
            thumb_url: ytdlp::thumb_url_for(&self.id),
            thumb_path: None,
            in_library: false,
            download_state: DownloadState::None,
            members_only: self.members_only,
        }
    }

    fn plan(&self, now: i64) -> Plan {
        let (channel_id, channel_title) = uploader(self.channel_id.clone(), self.channel_title.clone());
        Plan {
            uploaders: vec![ad_hoc_channel(&channel_id, &channel_title, now)],
            rows: vec![NewVideo {
                description: self.description.clone(),
                ..manual_row(&self.id, self.title.clone(), channel_id, self.duration_secs,
                             self.published_at, 0, now)
            }],
            skipped_shorts: Vec::new(),
        }
    }
}

/// Whether to refuse a video as a Short. The `/shorts/` endpoint decides when
/// it answered; when it could not, the watch page's own canonical URL does,
/// and with neither the video is let through (see [`watch_page::is_short`]).
fn is_a_short(endpoint: Option<bool>, page: Option<&WatchPage>) -> bool {
    endpoint.unwrap_or_else(|| page.is_some_and(|p| p.canonical_short))
}

fn is_video_id(id: &str) -> bool {
    id.len() == 11 && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// An entry the listing names but nobody can add: YouTube's placeholders for
/// a private or deleted video, or an entry yt-dlp could only describe as
/// needing access it does not have. A private video your own cookies can see
/// lists with its real title and a duration, and stays.
fn is_unavailable(item: &ytdlp::PlaylistItem) -> bool {
    matches!(item.title.as_str(), "[Private video]" | "[Deleted video]")
        || (matches!(item.availability.as_deref(), Some("private" | "needs_auth"))
            && item.duration_secs.is_none())
}

/// Turns a listing into the preview: drops what cannot be added, lists each
/// video once, and marks every entry against the library.
///
/// An entry without a `channel_id` -- a collaboration, which lists its
/// `creators` instead -- is filed under the playlist's own channel when the
/// names agree (or the entry names nobody), which on `PLEgOOfzzbNNk` is the
/// right uploader. Otherwise it stays unknown here, and the watch page read
/// when it is added supplies the real one.
pub fn build_playlist(
    list: ytdlp::Playlist,
    library: &HashMap<String, (DownloadState, Option<String>)>,
) -> PlaylistPreview {
    let listed = list.entries.len();
    let mut unavailable = 0;
    let mut seen = HashSet::new();
    let mut entries = Vec::new();
    for item in list.entries {
        if is_unavailable(&item) {
            unavailable += 1;
            continue;
        }
        if !is_video_id(&item.id) || !seen.insert(item.id.clone()) {
            continue;
        }
        let owner_agrees = item.channel_title.is_none() || item.channel_title == list.channel_title;
        let (channel_id, channel_title) = match item.channel_id {
            Some(id) => (Some(id), item.channel_title),
            None if owner_agrees && list.channel_id.is_some() =>
                (list.channel_id.clone(), list.channel_title.clone()),
            None => (None, item.channel_title),
        };
        let held = library.get(&item.id);
        entries.push(PlaylistEntry {
            thumb_url: ytdlp::thumb_url_for(&item.id),
            thumb_path: held.and_then(|(_, p)| p.clone()),
            in_library: held.is_some(),
            download_state: held.map_or(DownloadState::None, |(s, _)| *s),
            members_only: item.availability.as_deref() == Some("subscriber_only"),
            title: if item.title.is_empty() { item.id.clone() } else { item.title },
            id: item.id,
            channel_id,
            channel_title,
            duration_secs: item.duration_secs,
        });
    }
    let total = list.count.unwrap_or(listed);
    PlaylistPreview {
        truncated: listed >= ytdlp::PLAYLIST_LIMIT as usize && total > listed,
        id: list.id,
        title: list.title,
        channel_id: list.channel_id,
        channel_title: list.channel_title,
        total,
        entries,
        unavailable,
    }
}

/// What one new playlist part turned out to be.
#[derive(Debug, Clone, PartialEq)]
enum Lookup {
    Short,
    /// Its watch page, or `None` when that could not be read -- the part is
    /// still added, from what the listing said, with no date.
    Page(Option<WatchPage>),
}

/// The writes an add comes to, worked out before any of them is made.
#[derive(Debug, Clone, PartialEq, Default)]
struct Plan {
    /// One per distinct uploader, in order of first appearance.
    uploaders: Vec<Channel>,
    rows: Vec<NewVideo>,
    skipped_shorts: Vec<String>,
}

/// The rows for the new parts of a playlist, in the order given.
///
/// The listing's title is kept over the watch page's: a flat listing serves
/// a video's current title, while the watch page can still serve the one it
/// was published under (see "The date pipeline" in CLAUDE.md). The page
/// supplies what the listing lacks -- the upload date, and the uploader of a
/// collaboration. `feed_rank` is the position, so parts added in one go,
/// which share a `sort_at`, sit in the grid in playlist order.
fn plan_playlist(fresh: &[PlaylistEntry], looked: &HashMap<String, Lookup>, now: i64) -> Plan {
    let mut plan = Plan::default();
    let mut filed: HashMap<String, usize> = HashMap::new();
    for (rank, e) in fresh.iter().enumerate() {
        let page = match looked.get(&e.id) {
            Some(Lookup::Short) => {
                plan.skipped_shorts.push(e.id.clone());
                continue;
            }
            Some(Lookup::Page(p)) => p.as_ref(),
            None => None,
        };
        let (channel_id, channel_title) = match page.and_then(|p| p.channel_id.clone()) {
            Some(id) => {
                let named_here = (e.channel_id.as_ref() == Some(&id)).then(|| e.channel_title.clone()).flatten();
                uploader(Some(id), page.and_then(|p| p.channel_title.clone()).or(named_here))
            }
            None => uploader(e.channel_id.clone(), e.channel_title.clone()),
        };
        match filed.get(&channel_id) {
            // A later part may know the name an earlier one did not.
            Some(&at) if plan.uploaders[at].title == UNKNOWN_CHANNEL_TITLE => {
                plan.uploaders[at].title = channel_title.clone();
            }
            Some(_) => {}
            None => {
                filed.insert(channel_id.clone(), plan.uploaders.len());
                plan.uploaders.push(ad_hoc_channel(&channel_id, &channel_title, now));
            }
        }
        let title = Some(e.title.trim())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| page.map(|p| p.title.clone()))
            .unwrap_or_else(|| e.id.clone());
        plan.rows.push(NewVideo {
            description: page.and_then(|p| p.description.clone()),
            ..manual_row(
                &e.id,
                title,
                channel_id,
                e.duration_secs.or_else(|| page.and_then(|p| p.duration_secs)),
                page.and_then(|p| p.published_at),
                rank as i64,
                now,
            )
        });
    }
    plan
}

/// Makes a plan's writes and returns the ids it inserted, in order.
///
/// An uploader is written only when it is new, or an ad-hoc one whose name
/// this add actually knows. A channel you follow is left alone: the poll owns
/// its name, and an upsert would also clear a terminated mark that only a poll
/// may clear. `upsert_channel` keeps `subscribed` a one-way upgrade either way.
///
/// The rows go in as one batch. Any that arrived meanwhile (a poll landing
/// between the preview and the click) are left as they are and not reported
/// as added.
fn apply_plan(db: &Db, plan: &Plan) -> Result<Vec<String>> {
    let known: HashMap<String, Channel> =
        db.list_channels()?.into_iter().map(|c| (c.id.clone(), c)).collect();
    for c in &plan.uploaders {
        let write = match known.get(&c.id) {
            None => true,
            Some(k) => !k.subscribed && c.title != UNKNOWN_CHANNEL_TITLE && c.title != k.title,
        };
        if write {
            db.upsert_channel(c)?;
        }
    }
    let ids: Vec<String> = plan.rows.iter().map(|r| r.id.clone()).collect();
    let already = db.existing_video_ids(&ids)?;
    let rows: Vec<NewVideo> = plan.rows.iter().filter(|r| !already.contains(&r.id)).cloned().collect();
    db.insert_videos_if_new(&rows)?;
    Ok(rows.into_iter().map(|r| r.id).collect())
}

/// Stores "Download (custom)…"'s one quality on every video about to be
/// queued, sanitised, as `enqueue_download` does for one: the job reads it
/// when it starts, and a Retry after a failure uses it again.
pub fn store_quality(db: &Db, ids: &[String], q: &Quality) -> Result<()> {
    let q = q.clone().sanitized();
    for id in ids {
        db.set_download_quality(id, Some(&q))?;
    }
    Ok(())
}

// ------------------------------------------------------------- the network

/// [`crate::commands`]' `db_blocking`, for this module: SQL off the runtime.
async fn on_db<T, F>(db: &Arc<Db>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Db) -> Result<T> + Send + 'static,
{
    let db = db.clone();
    tauri::async_runtime::spawn_blocking(move || f(&db)).await?
}

/// Finds one video on YouTube: its watch page, with the `/shorts/` check run
/// alongside it, and yt-dlp's probe only when the page did not parse -- a
/// private video the user's cookies can see, or a page YouTube has reshaped.
/// A Short is an error.
async fn look_up(state: &AppState, video_id: &str) -> Result<Found> {
    let (page, short) = tokio::join!(
        watch_page::fetch(&state.http, video_id),
        watch_page::is_short(video_id),
    );
    if is_a_short(short, page.as_ref()) {
        bail!(SHORT_REFUSAL);
    }
    if let Some(page) = page {
        return Ok(page.into());
    }
    let s = tauri::async_runtime::spawn_blocking(config::load).await??;
    // The lease lives exactly as long as the probe.
    let inv = state.tools.ytdlp(&s).await?;
    let probe = ytdlp::probe(&inv, &s.quality, &ytdlp::watch_url(video_id),
                             &s.download_dir, &s.filename_template).await?;
    Ok(probe.into())
}

/// Lists a playlist through yt-dlp, with the user's cookies, and marks every
/// entry against the library in one query.
async fn preview_playlist(state: &AppState, list_id: &str) -> Result<PlaylistPreview> {
    let s = tauri::async_runtime::spawn_blocking(config::load).await??;
    let listing = {
        let inv = state.tools.ytdlp(&s).await?;
        ytdlp::playlist(&inv, list_id).await?
    };
    let ids: Vec<String> = listing.entries.iter().map(|e| e.id.clone()).collect();
    let library = on_db(&state.db, move |db| db.library_state(&ids)).await?;
    Ok(build_playlist(listing, &library))
}

/// What `input` would add. Errors are sentences for under the input box.
pub async fn preview(state: &AppState, input: &str) -> Result<AddPreview> {
    match resolve::parse_add_input(input)? {
        AddTarget::Short(_) => bail!(SHORT_REFUSAL),
        AddTarget::Channel(_) => {
            let input = input.to_string();
            let subscribed_id = on_db(&state.db, move |db| {
                Ok(subscribed_id(&input, &db.list_subscribed_channels()?))
            })
            .await?;
            Ok(AddPreview::Channel { subscribed_id })
        }
        AddTarget::Video(id) => {
            let playlist_id = resolve::playlist_id_of(input);
            let row = {
                let id = id.clone();
                on_db(&state.db, move |db| db.get_video(&id)).await?
            };
            let video = match row {
                Some(v) => video_from_row(&v),
                None => look_up(state, &id).await?.preview(),
            };
            Ok(AddPreview::Video { video, playlist_id })
        }
        AddTarget::Playlist(list_id) => {
            Ok(AddPreview::Playlist { playlist: preview_playlist(state, &list_id).await? })
        }
    }
}

/// Adds one video to the library, without subscribing and without
/// downloading it. A video already there comes back untouched.
pub async fn add_video(state: &AppState, input: &str) -> Result<Video> {
    let video_id = match resolve::parse_add_input(input)? {
        AddTarget::Video(id) => id,
        AddTarget::Short(_) => bail!("MyTube does not handle YouTube Shorts."),
        AddTarget::Channel(_) => bail!("That is a channel link. Use Add channel to subscribe."),
        AddTarget::Playlist(_) => bail!("That is a playlist link. Add its videos from the playlist's preview."),
    };
    let known = {
        let id = video_id.clone();
        on_db(&state.db, move |db| db.get_video(&id)).await?
    };
    if let Some(v) = known {
        return Ok(v);
    }

    let plan = look_up(state, &video_id).await?.plan(chrono::Utc::now().timestamp());
    on_db(&state.db, move |db| apply_plan(db, &plan)).await?;
    poll::cache_thumb(&state.http, &state.db, &video_id, &ytdlp::thumb_url_for(&video_id)).await;

    on_db(&state.db, move |db| db.get_video(&video_id))
        .await?
        .ok_or_else(|| anyhow!("Video disappeared after being added"))
}

/// Adds the ticked parts of a playlist as separate videos. Parts already in
/// the library are left exactly as they are; every new one is checked for
/// being a Short and has its watch page read for the real upload date (and,
/// for a collaboration, its uploader), eight at a time.
pub async fn add_playlist(state: &AppState, entries: Vec<PlaylistEntry>) -> Result<AddPlaylistResult> {
    let mut seen = HashSet::new();
    let entries: Vec<PlaylistEntry> = entries
        .into_iter()
        .filter(|e| is_video_id(&e.id) && seen.insert(e.id.clone()))
        .collect();
    let ids: Vec<String> = entries.iter().map(|e| e.id.clone()).collect();
    let held = on_db(&state.db, move |db| db.existing_video_ids(&ids)).await?;
    let (present, fresh): (Vec<PlaylistEntry>, Vec<PlaylistEntry>) =
        entries.into_iter().partition(|e| held.contains(&e.id));

    let http = &state.http;
    // Owned ids, each moved into its own future: nothing borrowed from an
    // iterator lives across an await (see "Async gotchas" in CLAUDE.md).
    let fresh_ids: Vec<String> = fresh.iter().map(|e| e.id.clone()).collect();
    let looked: HashMap<String, Lookup> = stream::iter(fresh_ids)
        .map(|id| async move {
            // The Shorts check first, and the page only for what passes: a
            // Short's page would be a megabyte read for nothing.
            let short = watch_page::is_short(&id).await;
            if short == Some(true) {
                return (id, Lookup::Short);
            }
            let page = watch_page::fetch(http, &id).await;
            if is_a_short(short, page.as_ref()) {
                return (id, Lookup::Short);
            }
            (id, Lookup::Page(page))
        })
        .buffer_unordered(PAGE_CONCURRENCY)
        .collect()
        .await;

    let plan = plan_playlist(&fresh, &looked, chrono::Utc::now().timestamp());
    let skipped_shorts = plan.skipped_shorts.clone();
    let added = on_db(&state.db, move |db| apply_plan(db, &plan)).await?;
    let thumbs: Vec<(String, String)> =
        added.iter().map(|id| (id.clone(), ytdlp::thumb_url_for(id))).collect();
    poll::cache_thumbs(state, thumbs.into_iter()).await;

    Ok(AddPlaylistResult {
        added,
        present: present.into_iter().map(|e| e.id).collect(),
        skipped_shorts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../tests/fixtures/PLEgOOfzzbNNk_flat.json");
    const OWNER: &str = "UC5qhtUpMhpBacu9sm3V2fag";

    fn db() -> Db {
        Db::open_in_memory().unwrap()
    }

    fn channel(id: &str, title: &str, handle: Option<&str>, subscribed: bool) -> Channel {
        Channel {
            handle: handle.map(str::to_string),
            subscribed,
            ..ad_hoc_channel(id, title, 1_000)
        }
    }

    fn page(id: &str, channel: Option<(&str, &str)>, published_at: Option<i64>) -> WatchPage {
        WatchPage {
            id: id.into(),
            title: format!("page title of {id}"),
            channel_id: channel.map(|c| c.0.to_string()),
            channel_title: channel.map(|c| c.1.to_string()),
            duration_secs: Some(600),
            published_at,
            members_only: false,
            canonical_short: false,
            description: None,
        }
    }

    fn entry(id: &str, title: &str, channel: Option<(&str, &str)>, duration: Option<i64>) -> PlaylistEntry {
        PlaylistEntry {
            id: id.into(),
            title: title.into(),
            channel_id: channel.map(|c| c.0.to_string()),
            channel_title: channel.map(|c| c.1.to_string()),
            duration_secs: duration,
            thumb_url: ytdlp::thumb_url_for(id),
            thumb_path: None,
            in_library: false,
            download_state: DownloadState::None,
            members_only: false,
        }
    }

    // ---- channels ----

    #[test]
    fn a_followed_channel_is_found_by_id_link_or_handle() {
        let channels = vec![
            channel(OWNER, "AnimeCapped Manga", Some("@AnimeCapped25"), true),
            channel("UCHnyfMqiRRG1u-2MsSQLbXA", "Veritasium", None, true),
        ];
        for input in [OWNER,
                      "https://www.youtube.com/channel/UC5qhtUpMhpBacu9sm3V2fag/videos",
                      "@AnimeCapped25",
                      "@animecapped25",
                      "https://www.youtube.com/@ANIMECAPPED25/featured"] {
            assert_eq!(subscribed_id(input, &channels).as_deref(), Some(OWNER), "{input}");
        }
    }

    #[test]
    fn a_channel_you_do_not_follow_is_not_found() {
        let channels = vec![
            // An ad-hoc uploader: a video of theirs is in the library, but
            // you never subscribed.
            channel(OWNER, "AnimeCapped Manga", Some("@AnimeCapped25"), false),
            // Followed, but no handle recorded yet.
            channel("UCHnyfMqiRRG1u-2MsSQLbXA", "Veritasium", None, true),
        ];
        for input in [OWNER, "@AnimeCapped25", "@veritasium",
                      "https://www.youtube.com/c/Veritasium", "UCXuqSBlHAE6Xw-yeJA0Tunw"] {
            assert_eq!(subscribed_id(input, &channels), None, "{input}");
        }
    }

    // ---- videos ----

    #[test]
    fn a_library_row_previews_with_its_download_state_and_cached_thumb() {
        let d = db();
        d.upsert_channel(&channel(OWNER, "AnimeCapped Manga", None, true)).unwrap();
        d.insert_video_if_new(&manual_row("bZitriM8N28", "Part one".into(), OWNER.into(),
                                          Some(7980), Some(1_700_000_000), 0, 1_000)).unwrap();
        d.set_thumb_path("bZitriM8N28", "/thumbs/bZitriM8N28.jpg").unwrap();
        d.set_download_state("bZitriM8N28", DownloadState::Done, Some("/v/part one.mkv"), None).unwrap();

        let p = video_from_row(&d.get_video("bZitriM8N28").unwrap().unwrap());
        assert!(p.in_library);
        assert_eq!(p.download_state, DownloadState::Done);
        assert_eq!(p.thumb_path.as_deref(), Some("/thumbs/bZitriM8N28.jpg"));
        assert_eq!(p.thumb_url, "https://i.ytimg.com/vi/bZitriM8N28/hqdefault.jpg");
        assert_eq!(p.channel_title, "AnimeCapped Manga");
        assert_eq!(p.duration_secs, Some(7980));
    }

    #[test]
    fn a_video_found_online_previews_as_new() {
        let mut p = page("dI1xnt0sw5I", Some(("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Recap")), Some(1_760_573_161));
        p.members_only = true;
        let v = Found::from(p).preview();
        assert!(!v.in_library);
        assert_eq!(v.download_state, DownloadState::None);
        assert_eq!(v.thumb_path, None);
        assert_eq!(v.channel_title, "Arashi Recap");
        assert_eq!(v.published_at, Some(1_760_573_161));
        assert!(v.members_only);
    }

    #[test]
    fn an_unknown_uploader_previews_and_files_under_the_placeholder() {
        let probe = ProbeInfo {
            id: "dI1xnt0sw5I".into(),
            title: "T".into(),
            duration_secs: None,
            published_at: None,
            channel_title: String::new(),
            channel_id: String::new(),
            intended_path: "/x".into(),
        };
        let found = Found::from(probe);
        let v = found.preview();
        assert_eq!((v.channel_id.as_str(), v.channel_title.as_str()), (UNKNOWN_CHANNEL_ID, UNKNOWN_CHANNEL_TITLE));
        let plan = found.plan(5);
        assert_eq!(plan.rows[0].channel_id, UNKNOWN_CHANNEL_ID);
        assert_eq!(plan.uploaders[0].id, UNKNOWN_CHANNEL_ID);
    }

    #[test]
    fn the_shorts_endpoint_decides_and_the_page_only_breaks_a_tie() {
        let mut short_page = page("8Pae6Wy1abE", None, None);
        short_page.canonical_short = true;
        let plain = page("dI1xnt0sw5I", None, None);
        assert!(is_a_short(Some(true), Some(&plain)));
        assert!(!is_a_short(Some(false), Some(&short_page)), "the endpoint outranks the page");
        assert!(is_a_short(None, Some(&short_page)));
        assert!(!is_a_short(None, Some(&plain)));
        assert!(!is_a_short(None, None), "could not tell is let through");
    }

    #[test]
    fn adding_a_video_files_it_as_an_ad_hoc_manual_row() {
        let d = db();
        let found = Found::from(page("dI1xnt0sw5I", Some(("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Recap")),
                                     Some(1_760_573_161)));
        assert_eq!(apply_plan(&d, &found.plan(2_000_000_000)).unwrap(), vec!["dI1xnt0sw5I"]);

        let v = d.get_video("dI1xnt0sw5I").unwrap().unwrap();
        assert!(v.added_manually);
        assert_eq!(v.sort_at, Some(2_000_000_000), "pinned to the top of the grid");
        assert_eq!(v.published_at, Some(1_760_573_161), "while the card shows the real date");
        assert_eq!(v.download_state, DownloadState::None, "an add downloads nothing");
        assert_eq!(v.status, VideoStatus::Ready);
        let c = d.get_channel("UCksnvnKYJ7f5_LrJcg7ZxaQ").unwrap().unwrap();
        assert!(!c.subscribed);
        assert_eq!(c.title, "Arashi Recap");
    }

    // ---- playlists: the listing ----

    #[test]
    fn the_real_playlist_listing_parses() {
        let list = ytdlp::parse_playlist(FIXTURE).unwrap();
        assert_eq!(list.id, "PLEgOOfzzbNNk");
        assert_eq!(list.title, "Others Cultivate to Get Stronger, I Just Raised My Kids! | Manhwa Recap");
        assert_eq!(list.channel_id.as_deref(), Some(OWNER));
        assert_eq!(list.channel_title.as_deref(), Some("AnimeCapped Manga"));
        assert_eq!(list.count, Some(15));
        assert_eq!(list.entries.len(), 15);
        let first = &list.entries[0];
        assert_eq!(first.id, "bZitriM8N28");
        assert_eq!(first.duration_secs, Some(7980));
        assert_eq!(first.channel_id, None, "a collaboration lists creators, not a channel id");
        assert_eq!(first.channel_title.as_deref(), Some("AnimeCapped Manga"));
        assert_eq!(list.entries[1].channel_id.as_deref(), Some(OWNER));
        let locked: Vec<&str> = list.entries.iter()
            .filter(|e| e.availability.as_deref() == Some("subscriber_only"))
            .map(|e| e.id.as_str()).collect();
        assert_eq!(locked, ["yi8Ehs1Mrwc", "7gsxZjG_zeI"]);
    }

    #[test]
    fn a_listing_that_is_not_a_playlist_is_an_error() {
        assert!(ytdlp::parse_playlist("null").is_err(), "what yt-dlp prints beside an error");
        assert!(ytdlp::parse_playlist("").is_err());
        assert!(ytdlp::parse_playlist(r#"{"title":"no id"}"#).is_err());
        let bare = ytdlp::parse_playlist(r#"{"id":"WL","entries":[{"title":"no id"}]}"#).unwrap();
        assert_eq!(bare.title, "WL");
        assert!(bare.entries.is_empty());
        assert_eq!(bare.count, None);
    }

    #[test]
    fn listing_errors_read_as_sentences() {
        assert_eq!(
            ytdlp::listing_error("ERROR: [youtube:tab] PLEgOOfzzbNNx: YouTube said: The playlist does not exist.\n"),
            "The playlist does not exist."
        );
        assert!(ytdlp::listing_error(
            "ERROR: [youtube:tab] PLzzzzzzzzzzzzzzzzzzzzzz: Playlists that require authentication may not \
             extract correctly without a successful webpage download. If you are not downloading private \
             content, or your cookies are only for the first account and channel, pass \
             \"--extractor-args youtubetab:skip=authcheck\" to skip this check")
            .starts_with("That playlist is private"));
        assert_eq!(
            ytdlp::listing_error("WARNING: x\nERROR: [youtube:tab] PLx: Unable to download API page: HTTP Error 400\n"),
            "Unable to download API page: HTTP Error 400"
        );
        assert_eq!(ytdlp::listing_error("ERROR: something odd"), "something odd");
        assert_eq!(ytdlp::listing_error(""), "yt-dlp could not list that playlist");
    }

    #[test]
    fn a_playlist_is_listed_with_cookies_and_a_ceiling() {
        let r = ytdlp::Runner {
            program: "yt-dlp".into(),
            ffmpeg_location: None,
            deno: None,
            cookies: ytdlp::Cookies::Browser("firefox".into()),
        };
        let a = ytdlp::playlist_args(&r, "PLEgOOfzzbNNk");
        let at = |flag: &str| a.iter().position(|x| x == flag).unwrap_or_else(|| panic!("{flag} in {a:?}"));
        assert_eq!(a[at("--cookies-from-browser") + 1], "firefox");
        assert_eq!(a[at("--playlist-end") + 1], "500");
        at("--flat-playlist");
        at("-J");
        assert!(!a.contains(&"-j".to_string()), "one document, not lines");
        assert_eq!(a.last().unwrap(), "https://www.youtube.com/playlist?list=PLEgOOfzzbNNk");
    }

    // ---- playlists: the preview ----

    #[test]
    fn the_real_playlist_previews_whole_with_its_owner_filled_in() {
        let p = build_playlist(ytdlp::parse_playlist(FIXTURE).unwrap(), &HashMap::new());
        assert_eq!((p.total, p.entries.len(), p.unavailable), (15, 15, 0));
        assert!(!p.truncated);
        assert!(p.entries.iter().all(|e| e.channel_id.as_deref() == Some(OWNER)),
                "the collaboration is filed under the playlist's owner, whose name it carries");
        assert_eq!(p.entries.iter().filter(|e| e.members_only).count(), 2);
        assert!(p.entries.iter().all(|e| !e.in_library && e.download_state == DownloadState::None));
        assert_eq!(p.entries[3].thumb_url, "https://i.ytimg.com/vi/6jsvSugLFkE/hqdefault.jpg");
        assert_eq!(p.entries.iter().filter_map(|e| e.duration_secs).sum::<i64>(), 172_523);
    }

    #[test]
    fn entries_are_marked_against_the_library() {
        let library = HashMap::from([
            ("bZitriM8N28".to_string(), (DownloadState::Done, Some("/t/bZitriM8N28.jpg".to_string()))),
            ("j82sVqeOoOY".to_string(), (DownloadState::None, None)),
        ]);
        let p = build_playlist(ytdlp::parse_playlist(FIXTURE).unwrap(), &library);
        let e0 = &p.entries[0];
        assert!(e0.in_library);
        assert_eq!(e0.download_state, DownloadState::Done);
        assert_eq!(e0.thumb_path.as_deref(), Some("/t/bZitriM8N28.jpg"));
        assert!(p.entries[1].in_library);
        assert_eq!(p.entries[1].download_state, DownloadState::None);
        assert!(!p.entries[2].in_library);
    }

    fn item(id: &str, title: &str, availability: Option<&str>, duration: Option<i64>) -> ytdlp::PlaylistItem {
        ytdlp::PlaylistItem {
            id: id.into(),
            title: title.into(),
            channel_id: None,
            channel_title: Some("Someone Else".into()),
            duration_secs: duration,
            availability: availability.map(str::to_string),
        }
    }

    fn list_of(entries: Vec<ytdlp::PlaylistItem>, count: Option<usize>) -> ytdlp::Playlist {
        ytdlp::Playlist {
            id: "PLx0000000000".into(),
            title: "Mixed bag".into(),
            channel_id: Some(OWNER.into()),
            channel_title: Some("AnimeCapped Manga".into()),
            count,
            entries,
        }
    }

    #[test]
    fn private_deleted_and_repeated_entries_are_dropped() {
        let p = build_playlist(list_of(vec![
            item("aaaaaaaaaaa", "Kept", None, Some(60)),
            item("bbbbbbbbbbb", "[Private video]", None, None),
            item("ccccccccccc", "[Deleted video]", None, None),
            item("ddddddddddd", "Locked away", Some("needs_auth"), None),
            // Private, but the user's cookies can see it: listed for real.
            item("eeeeeeeeeee", "My own private upload", Some("private"), Some(90)),
            item("aaaaaaaaaaa", "Kept", None, Some(60)),
            item("not-a-video-id", "Junk", None, Some(1)),
        ], Some(7)), &HashMap::new());
        let ids: Vec<&str> = p.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["aaaaaaaaaaa", "eeeeeeeeeee"]);
        assert_eq!(p.unavailable, 3);
        assert_eq!(p.total, 7);
        assert!(!p.truncated, "seven listed of seven: nothing was cut off");
    }

    #[test]
    fn another_channels_video_is_not_filed_under_the_playlists_owner() {
        let p = build_playlist(list_of(vec![item("aaaaaaaaaaa", "Theirs", None, Some(60))], None),
                               &HashMap::new());
        assert_eq!(p.entries[0].channel_id, None, "left for the watch page to settle");
        assert_eq!(p.entries[0].channel_title.as_deref(), Some("Someone Else"));
        assert_eq!(p.total, 1, "no count reported: the entries listed");
    }

    #[test]
    fn truncated_means_the_listing_hit_its_ceiling() {
        let many: Vec<ytdlp::PlaylistItem> = (0..ytdlp::PLAYLIST_LIMIT)
            .map(|i| item(&format!("v{i:010}"), "Part", None, Some(60)))
            .collect();
        let p = build_playlist(list_of(many.clone(), Some(812)), &HashMap::new());
        assert!(p.truncated);
        assert_eq!((p.total, p.entries.len()), (812, 500));
        let exact = build_playlist(list_of(many, Some(500)), &HashMap::new());
        assert!(!exact.truncated);
    }

    // ---- playlists: adding ----

    #[test]
    fn new_parts_take_their_date_from_the_page_and_the_rest_from_the_listing() {
        let fresh = vec![
            entry("aaaaaaaaaaa", "Part 1", Some((OWNER, "AnimeCapped Manga")), Some(100)),
            entry("bbbbbbbbbbb", "Part 2", Some((OWNER, "AnimeCapped Manga")), None),
            entry("ccccccccccc", "", Some((OWNER, "AnimeCapped Manga")), Some(300)),
        ];
        let looked = HashMap::from([
            ("aaaaaaaaaaa".to_string(), Lookup::Page(Some(page("aaaaaaaaaaa", Some((OWNER, "AnimeCapped Manga")), Some(1_760_000_123))))),
            ("bbbbbbbbbbb".to_string(), Lookup::Page(Some(page("bbbbbbbbbbb", Some((OWNER, "AnimeCapped Manga")), None)))),
            ("ccccccccccc".to_string(), Lookup::Page(None)),
        ]);
        let plan = plan_playlist(&fresh, &looked, 9_000);
        let rows = &plan.rows;
        assert_eq!(rows[0].title, "Part 1", "the listing's title, not the page's");
        assert_eq!(rows[0].published_at, Some(1_760_000_123));
        assert_eq!(rows[1].duration_secs, Some(600), "the page fills a duration the listing lacked");
        assert_eq!(rows[2].title, "ccccccccccc", "neither source named it");
        assert_eq!(rows[2].published_at, None, "an unread page leaves the date unknown");
        assert!(rows.iter().all(|r| r.added_manually && r.sort_at == Some(9_000)
                                     && r.status == VideoStatus::Ready));
        assert_eq!(rows.iter().map(|r| r.feed_rank).collect::<Vec<_>>(), [0, 1, 2],
                   "playlist order, for rows sharing one sort_at");
        assert_eq!(plan.uploaders.len(), 1, "one uploader, filed once");
    }

    #[test]
    fn a_collaborations_uploader_comes_from_its_page() {
        let fresh = vec![
            entry("aaaaaaaaaaa", "Collab", None, Some(100)),
            entry("bbbbbbbbbbb", "Unreadable", None, Some(100)),
        ];
        let looked = HashMap::from([
            ("aaaaaaaaaaa".to_string(),
             Lookup::Page(Some(page("aaaaaaaaaaa", Some(("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Recap")), None)))),
            ("bbbbbbbbbbb".to_string(), Lookup::Page(None)),
        ]);
        let plan = plan_playlist(&fresh, &looked, 1);
        assert_eq!(plan.rows[0].channel_id, "UCksnvnKYJ7f5_LrJcg7ZxaQ");
        assert_eq!(plan.rows[1].channel_id, UNKNOWN_CHANNEL_ID);
        let names: Vec<(&str, &str)> =
            plan.uploaders.iter().map(|c| (c.id.as_str(), c.title.as_str())).collect();
        assert_eq!(names, [("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Recap"),
                           (UNKNOWN_CHANNEL_ID, UNKNOWN_CHANNEL_TITLE)]);
    }

    #[test]
    fn shorts_are_skipped_and_reported() {
        let fresh = vec![
            entry("aaaaaaaaaaa", "Video", Some((OWNER, "AnimeCapped Manga")), Some(100)),
            entry("bbbbbbbbbbb", "Short", Some((OWNER, "AnimeCapped Manga")), Some(30)),
        ];
        let looked = HashMap::from([
            ("aaaaaaaaaaa".to_string(), Lookup::Page(None)),
            ("bbbbbbbbbbb".to_string(), Lookup::Short),
        ]);
        let plan = plan_playlist(&fresh, &looked, 1);
        assert_eq!(plan.skipped_shorts, ["bbbbbbbbbbb"]);
        assert_eq!(plan.rows.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["aaaaaaaaaaa"]);
    }

    #[test]
    fn adding_leaves_followed_channels_and_present_videos_alone() {
        let d = db();
        d.upsert_channel(&channel(OWNER, "AnimeCapped Manga", Some("@AnimeCapped25"), true)).unwrap();
        // Already here from a poll, with its own sort_at and state.
        d.insert_video_if_new(&NewVideo {
            added_manually: false,
            ..manual_row("aaaaaaaaaaa", "From the feed".into(), OWNER.into(), Some(5), Some(77), 3, 1_111)
        }).unwrap();
        d.set_watched("aaaaaaaaaaa", true).unwrap();

        let fresh = vec![
            entry("aaaaaaaaaaa", "Renamed in the playlist", Some((OWNER, "Some Old Name")), Some(9)),
            entry("bbbbbbbbbbb", "New part", Some((OWNER, "Some Old Name")), Some(9)),
        ];
        let plan = plan_playlist(&fresh, &HashMap::new(), 2_000);
        assert_eq!(apply_plan(&d, &plan).unwrap(), vec!["bbbbbbbbbbb"],
                   "a row that arrived meanwhile is not reported as added");

        let old = d.get_video("aaaaaaaaaaa").unwrap().unwrap();
        assert_eq!((old.title.as_str(), old.sort_at, old.added_manually, old.watched),
                   ("From the feed", Some(1_111), false, true));
        let c = d.get_channel(OWNER).unwrap().unwrap();
        assert!(c.subscribed, "subscribed stays a one-way upgrade");
        assert_eq!(c.title, "AnimeCapped Manga", "a followed channel's name is the poll's");
        assert_eq!(c.handle.as_deref(), Some("@AnimeCapped25"));
        assert_eq!(d.get_video("bbbbbbbbbbb").unwrap().unwrap().channel_id, OWNER);
    }

    #[test]
    fn an_ad_hoc_uploader_is_renamed_but_never_to_the_placeholder() {
        let d = db();
        d.upsert_channel(&channel("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Old Name", None, false)).unwrap();
        let named = plan_playlist(
            &[entry("aaaaaaaaaaa", "A", Some(("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Recap")), None)],
            &HashMap::new(), 1);
        apply_plan(&d, &named).unwrap();
        assert_eq!(d.get_channel("UCksnvnKYJ7f5_LrJcg7ZxaQ").unwrap().unwrap().title, "Arashi Recap");

        // A part whose page named the id but not the name.
        let mut nameless = page("bbbbbbbbbbb", Some(("UCksnvnKYJ7f5_LrJcg7ZxaQ", "x")), None);
        nameless.channel_title = None;
        let plan = plan_playlist(&[entry("bbbbbbbbbbb", "B", None, None)],
                                 &HashMap::from([("bbbbbbbbbbb".to_string(), Lookup::Page(Some(nameless)))]), 2);
        apply_plan(&d, &plan).unwrap();
        let c = d.get_channel("UCksnvnKYJ7f5_LrJcg7ZxaQ").unwrap().unwrap();
        assert_eq!(c.title, "Arashi Recap");
        assert!(!c.subscribed);
    }

    #[test]
    fn a_custom_quality_is_stored_sanitised_on_each_video() {
        let d = db();
        d.upsert_channel(&channel(OWNER, "AnimeCapped Manga", None, true)).unwrap();
        for id in ["aaaaaaaaaaa", "bbbbbbbbbbb", "ccccccccccc"] {
            d.insert_video_if_new(&manual_row(id, id.into(), OWNER.into(), None, None, 0, 1)).unwrap();
        }
        let q = Quality { max_height: 720, container: "mp4".into(), vcodec: "nonsense".into(),
                          ..Quality::default() };
        store_quality(&d, &["aaaaaaaaaaa".to_string(), "bbbbbbbbbbb".to_string()], &q).unwrap();
        let stored = d.download_quality("aaaaaaaaaaa").unwrap().unwrap();
        assert_eq!((stored.max_height, stored.container.as_str(), stored.vcodec.as_str()), (720, "mp4", "any"));
        assert!(d.download_quality("bbbbbbbbbbb").unwrap().is_some());
        assert_eq!(d.download_quality("ccccccccccc").unwrap(), None, "only the ids given");
    }

    #[test]
    fn library_state_covers_only_what_the_library_holds() {
        let d = db();
        d.upsert_channel(&channel(OWNER, "AnimeCapped Manga", None, true)).unwrap();
        d.insert_video_if_new(&manual_row("aaaaaaaaaaa", "A".into(), OWNER.into(), None, None, 0, 1)).unwrap();
        d.set_download_state("aaaaaaaaaaa", DownloadState::Failed, None, Some("boom")).unwrap();
        let state = d.library_state(&["aaaaaaaaaaa".to_string(), "zzzzzzzzzzz".to_string()]).unwrap();
        assert_eq!(state.len(), 1);
        assert_eq!(state["aaaaaaaaaaa"], (DownloadState::Failed, None));
        assert!(d.library_state(&[]).unwrap().is_empty());
    }

    // ---- the IPC shapes ----

    #[test]
    fn previews_serialise_tagged_and_camel_cased() {
        let channel = serde_json::to_value(AddPreview::Channel { subscribed_id: Some(OWNER.into()) }).unwrap();
        assert_eq!(channel, serde_json::json!({ "kind": "channel", "subscribedId": OWNER }));

        let video = Found::from(page("dI1xnt0sw5I", Some(("UCksnvnKYJ7f5_LrJcg7ZxaQ", "Arashi Recap")), Some(5)))
            .preview();
        let v = serde_json::to_value(AddPreview::Video { video, playlist_id: None }).unwrap();
        assert_eq!(v["kind"], "video");
        assert!(v["playlistId"].is_null());
        for key in ["id", "title", "channelId", "channelTitle", "durationSecs", "publishedAt",
                    "thumbUrl", "thumbPath", "inLibrary", "downloadState", "membersOnly"] {
            assert!(v["video"].get(key).is_some(), "video.{key}");
        }
        assert_eq!(v["video"]["downloadState"], "none");

        let p = build_playlist(ytdlp::parse_playlist(FIXTURE).unwrap(), &HashMap::new());
        let p = serde_json::to_value(AddPreview::Playlist { playlist: p }).unwrap();
        assert_eq!(p["kind"], "playlist");
        for key in ["id", "title", "channelId", "channelTitle", "total", "entries", "unavailable", "truncated"] {
            assert!(p["playlist"].get(key).is_some(), "playlist.{key}");
        }
        assert_eq!(p["playlist"]["entries"][0]["channelTitle"], "AnimeCapped Manga");

        let r = serde_json::to_value(AddPlaylistResult { skipped_shorts: vec!["x".into()], ..Default::default() })
            .unwrap();
        assert_eq!(r, serde_json::json!({ "added": [], "present": [], "skippedShorts": ["x"] }));
    }

    #[test]
    fn entries_come_back_from_the_dialog_as_they_were_sent() {
        let p = build_playlist(ytdlp::parse_playlist(FIXTURE).unwrap(), &HashMap::new());
        let json = serde_json::to_string(&p.entries).unwrap();
        let back: Vec<PlaylistEntry> = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p.entries);
        // A leaner object -- only what an add needs -- still deserialises.
        let lean: PlaylistEntry = serde_json::from_str(r#"{"id":"aaaaaaaaaaa","title":"A"}"#).unwrap();
        assert_eq!(lean.download_state, DownloadState::None);
        assert_eq!(lean.channel_id, None);
    }

    /// The listing end to end through the managed yt-dlp, cookies and all
    /// (none configured here): the real 15-part playlist, two parts
    /// members-only, every part filed under AnimeCapped Manga.
    ///
    ///     cargo test add:: -- --ignored
    #[tokio::test]
    #[ignore = "requires network and the managed yt-dlp"]
    async fn previews_a_real_playlist() {
        let exe = if cfg!(windows) { "yt-dlp.exe" } else { "yt-dlp" };
        let inv = crate::tools::Invocation {
            runner: ytdlp::Runner {
                program: crate::tools::bin_dir().join(exe),
                ffmpeg_location: None,
                deno: None,
                cookies: ytdlp::Cookies::None,
            },
            _lease: Arc::new(tokio::sync::RwLock::new(())).read_owned().await,
        };
        let list = ytdlp::playlist(&inv, "PLEgOOfzzbNNk").await.expect("listed");
        let p = build_playlist(list, &HashMap::new());
        eprintln!("{} — {:?}, {} of {}, {} unavailable",
                  p.title, p.channel_title, p.entries.len(), p.total, p.unavailable);
        for e in &p.entries {
            eprintln!("  {} {:>6?}s members={} {}", e.id, e.duration_secs, e.members_only, e.title);
        }
        assert_eq!(p.entries.len(), 15);
        assert_eq!(p.channel_title.as_deref(), Some("AnimeCapped Manga"));
        assert_eq!(p.entries.iter().filter(|e| e.members_only).count(), 2);
        assert!(p.entries.iter().all(|e| e.channel_id.as_deref() == Some(OWNER)));
    }

    #[test]
    fn a_pasted_videos_description_is_kept() {
        let p = WatchPage { description: Some("Part 1 https://youtu.be/BBBBBBBBBBB".into()),
                            ..page("aaaaaaaaaaa", Some((OWNER, "Owner")), Some(5)) };
        let plan = Found::from(p).plan(1);
        assert_eq!(plan.rows[0].description.as_deref(), Some("Part 1 https://youtu.be/BBBBBBBBBBB"));
    }

    #[test]
    fn a_playlist_parts_description_is_kept() {
        let e = entry("aaaaaaaaaaa", "Part 1", Some((OWNER, "Owner")), Some(60));
        let mut looked = HashMap::new();
        looked.insert(e.id.clone(), Lookup::Page(Some(WatchPage {
            description: Some("links".into()),
            ..page(&e.id, Some((OWNER, "Owner")), Some(5))
        })));
        let plan = plan_playlist(&[e], &looked, 1);
        assert_eq!(plan.rows[0].description.as_deref(), Some("links"));
    }
}
