use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Channel {
    pub id: String,
    pub title: String,
    pub handle: Option<String>,
    pub url: String,
    pub thumb_path: Option<String>,
    pub subscribed: bool,
    pub added_at: i64,
    pub last_polled_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VideoStatus {
    Pending,
    Ready,
    Upcoming,
    Live,
}

impl VideoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            VideoStatus::Pending => "pending",
            VideoStatus::Ready => "ready",
            VideoStatus::Upcoming => "upcoming",
            VideoStatus::Live => "live",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "ready" => VideoStatus::Ready,
            "upcoming" => VideoStatus::Upcoming,
            "live" => VideoStatus::Live,
            _ => VideoStatus::Pending,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DownloadState {
    None,
    Queued,
    Downloading,
    Done,
    Failed,
}

impl DownloadState {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloadState::None => "none",
            DownloadState::Queued => "queued",
            DownloadState::Downloading => "downloading",
            DownloadState::Done => "done",
            DownloadState::Failed => "failed",
        }
    }
    pub fn parse(s: &str) -> Self {
        match s {
            "queued" => DownloadState::Queued,
            "downloading" => DownloadState::Downloading,
            "done" => DownloadState::Done,
            "failed" => DownloadState::Failed,
            _ => DownloadState::None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Video {
    pub id: String,
    pub channel_id: String,
    pub channel_title: String,
    pub title: String,
    pub description: Option<String>,
    pub thumb_url: Option<String>,
    pub thumb_path: Option<String>,
    /// True upload date, for display. NULL when unknown (backfilled rows).
    pub published_at: Option<i64>,
    /// Feed ordering key. See the spec's ordering table.
    pub sort_at: Option<i64>,
    pub feed_rank: i64,
    pub added_manually: bool,
    pub duration_secs: Option<i64>,
    pub view_count: Option<i64>,
    pub status: VideoStatus,
    pub hidden: bool,
    pub watched: bool,
    pub watched_at: Option<i64>,
    pub download_state: DownloadState,
    pub download_error: Option<String>,
    pub file_path: Option<String>,
    /// When this row last moved through the download pipeline: the moment it
    /// was queued, then overwritten with the moment the file finished. NULL
    /// once nothing is downloaded. Ordering key for the Downloads tab, which
    /// is browsed by when you fetched a video rather than by when it aired.
    pub downloaded_at: Option<i64>,
    pub first_seen_at: i64,
}

/// A row to insert. Separate from `Video` because inserts have no joined
/// channel title and no user-owned state (watched / download).
#[derive(Debug, Clone, PartialEq)]
pub struct NewVideo {
    pub id: String,
    pub channel_id: String,
    pub title: String,
    pub description: Option<String>,
    pub thumb_url: Option<String>,
    pub published_at: Option<i64>,
    pub sort_at: Option<i64>,
    pub feed_rank: i64,
    pub added_manually: bool,
    pub duration_secs: Option<i64>,
    pub view_count: Option<i64>,
    pub status: VideoStatus,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortOrder {
    Newest,
    Oldest,
    Channel,
    /// Most recently downloaded first. Only the Downloads tab asks for this;
    /// it is deliberately absent from the Subscriptions sort picker, where
    /// most rows have no `downloaded_at` at all.
    Downloaded,
    /// Longest first. Ungrouped that is simply the video's own runtime, which
    /// SQL can order on; grouped, a card is ranked by its parts added up, since
    /// the card stands for the whole series. A `NULL` duration counts as
    /// nothing, so an unresolved row cannot outrank a real one.
    Length,
    /// Longest series first: the grouped feed's cards ranked by how many parts
    /// they hold. There is no column to sort on -- a group's size is only known
    /// once the walk in [`crate::db::Db::list_video_groups`] has built it -- so
    /// in SQL this is simply `Newest`, and the ranking is applied to the
    /// finished groups. Ungrouped it therefore *is* `Newest`, which is why the
    /// picker only offers it while Group siblings is on.
    Parts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFilter {
    pub channel_id: Option<String>,
    pub hide_watched: bool,
    pub downloaded_only: bool,
    pub show_hidden: bool,
    pub search: Option<String>,
    /// Id of the video whose siblings to show. When set it replaces every other
    /// filter above: the point of the view is the whole series, so a watched,
    /// hidden or undownloaded part still belongs in it.
    pub sibling_of: Option<String>,
    pub sort: SortOrder,
    pub limit: i64,
    pub offset: i64,
}

impl Default for VideoFilter {
    fn default() -> Self {
        Self {
            channel_id: None,
            hide_watched: false,
            downloaded_only: false,
            show_hidden: false,
            search: None,
            sibling_of: None,
            sort: SortOrder::Newest,
            limit: 100,
            offset: 0,
        }
    }
}

/// One card in the grouped feed: either a lone video, or a whole multi-part
/// upload collapsed behind a single entry.
///
/// Both field names are single words, so the camelCase/snake_case split the
/// rest of this IPC boundary lives with cannot bite here -- and the nested
/// `Video` keeps its own snake_case names either way.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VideoGroup {
    /// The parts, leader first. The leader is the row that earned the group its
    /// place in the feed -- the first part to survive the filters, in the
    /// requested sort order -- and is the anchor the series view opens on.
    pub videos: Vec<Video>,
    /// A name for the series: what the parts' titles share once their part
    /// markers are stripped. `None` for a lone video, and for a series whose
    /// titles share nothing showable.
    pub stem: Option<String>,
}

/// One `<entry>` from a channel RSS feed, after Shorts have been rejected.
#[derive(Debug, Clone, PartialEq)]
pub struct FeedEntry {
    pub video_id: String,
    pub title: String,
    pub description: Option<String>,
    pub thumb_url: Option<String>,
    pub published_at: i64,
    pub view_count: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Feed {
    pub channel_id: String,
    pub channel_title: String,
    pub entries: Vec<FeedEntry>,
    pub shorts_rejected: usize,
}

/// One JSON line from `yt-dlp --flat-playlist -j`.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatEntry {
    pub id: String,
    pub title: String,
    pub duration_secs: Option<i64>,
    pub live_status: Option<String>,
    pub view_count: Option<i64>,
    /// Approximate upload date, from `youtubetab:approximate_date`. Day-granular,
    /// which is enough to interleave backfilled videos into the feed by date.
    /// RSS later overwrites the recent window with exact timestamps.
    pub published_at: Option<i64>,
}

/// What the Add box was handed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AddKind {
    Channel,
    Video,
    Short,
}

/// Result of the phase-1 `--simulate` probe: metadata plus the intended output path.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeInfo {
    pub id: String,
    pub title: String,
    pub duration_secs: Option<i64>,
    pub published_at: Option<i64>,
    pub channel_title: String,
    pub channel_id: String,
    pub intended_path: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub video_id: String,
    pub percent: f64,
    pub speed: String,
    pub eta: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadStateEvent {
    pub video_id: String,
    pub state: DownloadState,
    pub file_path: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct PollSummary {
    pub channels_polled: usize,
    pub new_videos: usize,
    pub shorts_rejected: usize,
    pub errors: Vec<String>,
}

/// One row of a Takeout CSV, for the pre-import checklist.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TakeoutRow {
    pub channel_id: String,
    pub title: String,
    pub already_subscribed: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportProgress {
    pub done: usize,
    pub total: usize,
    pub current: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImportResult {
    pub added: usize,
    pub skipped: usize,
    pub failed: Vec<String>,
}
