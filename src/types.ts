export type VideoStatus = "pending" | "ready" | "upcoming" | "live";
export type DownloadState = "none" | "queued" | "downloading" | "done" | "failed";
/** "downloaded" is the Downloads tab's own ordering; the Subscriptions
 *  sort picker deliberately does not offer it. "parts" ranks the grouped feed's
 *  cards by how many parts they hold, and means plain "newest" without
 *  grouping, so the picker only offers it while Group siblings is on.
 *  "length" is longest-first: a video's own runtime ungrouped, a series' parts
 *  added up when grouped — meaningful either way, so it is always offered. */
export type SortOrder =
  "newest" | "oldest" | "channel" | "downloaded" | "parts" | "length";

export type AddKind = "channel" | "video" | "short";

export interface Channel {
  id: string; title: string; handle: string | null; url: string;
  thumb_path: string | null; subscribed: boolean;
  added_at: number; last_polled_at: number | null;
}

export interface Video {
  id: string; channel_id: string; channel_title: string; title: string;
  description: string | null; thumb_url: string | null; thumb_path: string | null;
  published_at: number | null; sort_at: number | null; feed_rank: number;
  added_manually: boolean; duration_secs: number | null;
  view_count: number | null; status: VideoStatus; hidden: boolean;
  watched: boolean; watched_at: number | null;
  download_state: DownloadState; download_error: string | null;
  file_path: string | null;
  /** Queue time while pending, finish time once done; null when nothing is
   *  downloaded. Ordering key for the Downloads tab. */
  downloaded_at: number | null;
  first_seen_at: number;
  /** Key shared with every video marked its sibling by hand. Null — all but a
   *  handful of rows — leaves the title matcher to work alone. */
  sibling_group: string | null;
}

/**
 * One card in the grouped feed: a lone video, or a whole multi-part upload
 * behind a single entry. Both field names are single words, so the camel/snake
 * split above does not apply here — and the nested Video keeps its snake_case
 * fields either way.
 */
export interface VideoGroup {
  /** The parts, leader first. The leader is the part that earned the group its
   *  place in the feed, and the anchor the series view opens on. */
  videos: Video[];
  /** The name the parts share, once their part markers are stripped. Null for
   *  a lone video, and for a series whose titles share nothing showable. */
  stem: string | null;
}

export interface VideoFilter {
  channelId: string | null; hideWatched: boolean; downloadedOnly: boolean;
  showHidden: boolean;
  search: string | null;
  /** Id of the video whose siblings to show; overrides every filter above. */
  siblingOf: string | null;
  sort: SortOrder; limit: number; offset: number;
}

export interface TakeoutRow {
  channelId: string; title: string; alreadySubscribed: boolean;
}
export interface ImportProgress { done: number; total: number; current: string; }

/**
 * The Subscriptions feed's filters, persisted between runs. The active tab is
 * deliberately absent: a launch always lands on Subscriptions.
 */
export interface ViewState {
  channel_id: string | null; search: string;
  hide_watched: boolean; downloaded_only: boolean; show_hidden: boolean;
  grouped: boolean; sort: SortOrder;
}

export interface Settings {
  download_dir: string; filename_template: string; player_command: string;
  max_concurrent_downloads: number; poll_interval_minutes: number;
  poll_on_startup: boolean; backfill_count: number; card_size: number;
  /** Written by the Rust side on close; the UI only carries them through. */
  window_width: number; window_height: number;
  window_x: number | null; window_y: number | null;
  window_maximized: boolean;
  view: ViewState;
}

export interface PollSummary {
  channelsPolled: number; newVideos: number; shortsRejected: number; errors: string[];
}
export interface ImportResult { added: number; skipped: number; failed: string[]; }
export interface DownloadProgress { videoId: string; percent: number; speed: string; eta: string; }
export interface DownloadStateEvent {
  videoId: string; state: DownloadState; filePath: string | null; error: string | null;
}
