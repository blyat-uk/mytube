export type VideoStatus = "pending" | "ready" | "upcoming" | "live";
export type DownloadState = "none" | "queued" | "downloading" | "done" | "failed";
export type SortOrder = "newest" | "oldest" | "channel";

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
  file_path: string | null; first_seen_at: number;
}

export interface VideoFilter {
  channelId: string | null; hideWatched: boolean; downloadedOnly: boolean;
  showHidden: boolean;
  search: string | null; sort: SortOrder; limit: number; offset: number;
}

export interface TakeoutRow {
  channelId: string; title: string; alreadySubscribed: boolean;
}
export interface ImportProgress { done: number; total: number; current: string; }

export interface Settings {
  download_dir: string; filename_template: string; player_command: string;
  max_concurrent_downloads: number; poll_interval_minutes: number;
  poll_on_startup: boolean; backfill_count: number; card_size: number;
}

export interface PollSummary {
  channelsPolled: number; newVideos: number; shortsRejected: number; errors: string[];
}
export interface ImportResult { added: number; skipped: number; failed: string[]; }
export interface DownloadProgress { videoId: string; percent: number; speed: string; eta: string; }
export interface DownloadStateEvent {
  videoId: string; state: DownloadState; filePath: string | null; error: string | null;
}
