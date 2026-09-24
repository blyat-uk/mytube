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
  /** Whether you have joined this channel's membership, which is what lets a
   *  poll ingest its members-only uploads. */
  member: boolean;
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
  /* Machine-local keys, never exported. Optional here, not because the backend
   * ever omits them — `#[serde(default)]` fills each one — but so a caller that
   * builds a Settings by hand is not forced to restate defaults it has no
   * opinion on. The UI reads each through the same default Rust applies. */
  /** `"auto"` (Firefox if a profile exists, else none), `""` for none, or a
   *  `--cookies-from-browser` spec verbatim (`firefox`, `chrome:Profile 1`). */
  cookies_browser?: string;
  /** A Netscape cookies.txt. Non-empty wins over `cookies_browser`. */
  cookies_file?: string;
  /** `"nightly"` or `"stable"`; anything else reads as nightly. */
  ytdlp_channel?: string;
  ytdlp_auto_update?: boolean;
  /** Hand-edit-only overrides. No control writes them; they ride through every
   *  save untouched because `commit()` always spreads the whole object. */
  ytdlp_path?: string; ffmpeg_path?: string; deno_path?: string;
}

/** The `cookies_browser` value meaning "Firefox if there is one, else none". */
export const COOKIES_AUTO = "auto";

export interface PollSummary {
  channelsPolled: number; newVideos: number; shortsRejected: number; errors: string[];
}
export interface ImportResult { added: number; skipped: number; failed: string[]; }
export interface DownloadProgress { videoId: string; percent: number; speed: string; eta: string; }
export interface DownloadStateEvent {
  videoId: string; state: DownloadState; filePath: string | null; error: string | null;
}

/* ------------------------------------------------------------------ *
 * Config export / import
 *
 * All camelCase: the Rust payloads carry `#[serde(rename_all = "camelCase")]`,
 * unlike `Channel`/`Video`/`Settings`, which cross the boundary snake_case.
 * ------------------------------------------------------------------ */

/** Merge keeps everything already here; replace makes the picked channels
 *  exactly what the archive holds — database rows only, never files. */
export type ImportMode = "merge" | "replace";
/** Which half of the transfer a progress event belongs to. One event channel
 *  carries both, and each side's progress bar ignores the other's. */
export type TransferPhase = "export" | "import";

export interface ArchiveChannel {
  channelId: string; title: string; videoCount: number;
  subscribed: boolean; member: boolean;
  /** Already in this library, so importing it updates rather than adds. */
  alreadyHere: boolean;
}

/**
 * What an archive says about itself, read without unpacking it. Everything the
 * import checklist needs to describe the file before anything is written.
 */
export interface ArchiveSummary {
  format: number; appVersion: string; exportedAt: number; exportedFrom: string;
  includesThumbs: boolean; thumbCount: number;
  /** The exporting machine's download folder, and whether this machine has it.
   *  Missing means the import keeps the local one rather than pointing the
   *  library at a path that is not there. */
  downloadDir: string; downloadDirExists: boolean;
  channels: ArchiveChannel[]; videoCount: number;
  /** How much of this library the archive never mentions — exactly what a
   *  Replace would remove. Measured against the whole archive, not the ticked
   *  subset, so it does not move as the checklist is worked through: unticking
   *  a channel skips it entirely rather than marking it for deletion. */
  localOnlyChannels: number; localOnlyVideos: number;
}

/**
 * What an import actually did. The removal counts are rows, not files: an
 * import never deletes a download or a cached thumbnail from disk.
 */
export interface ImportReport {
  channelsAdded: number; channelsUpdated: number;
  videosAdded: number; videosUpdated: number;
  channelsRemoved: number; videosRemoved: number;
  /** Videos whose file the archive named and this machine turned out to have. */
  downloadsRelinked: number; thumbsWritten: number;
  settingsApplied: boolean;
  /** True when the archive's download folder was refused for a local one. */
  downloadDirKept: boolean;
}

export interface TransferProgress {
  phase: TransferPhase; done: number; total: number; current: string;
}

/** Sizes the Export tick quotes, so "Include thumbnails" names a real cost. */
export interface TransferEstimate {
  channelCount: number; videoCount: number; thumbCount: number; thumbBytes: number;
}

/* ------------------------------------------------------------------ *
 * Player, cookies and the external tools
 *
 * camelCase like every other new payload; the settings keys they feed are
 * snake_case above.
 * ------------------------------------------------------------------ */

export type ToolKind = "ytdlp" | "ffmpeg" | "deno";
/** Where a resolved tool came from. `override` is a `*_path` key in
 *  settings.json, which is hand-edit-only. */
export type ToolSource = "managed" | "system" | "override" | "missing";
export type ToolState = "ready" | "installing" | "updating" | "error";

/** One row of the Tools section, and (as a list of three) `tools://status`. */
export interface ToolStatus {
  kind: ToolKind; path: string | null; version: string | null;
  source: ToolSource; state: ToolState;
  error: string | null;
  /** Unix seconds of the last update check; managed yt-dlp only. */
  lastCheck: number | null;
}

/** `tools://progress`: bytes of one tool's download so far. */
export interface ToolProgress {
  tool: ToolKind; phase: "download" | "extract"; received: number; total: number | null;
}

/** A detected player. `command` is exactly what goes into `player_command`;
 *  the "System default" entry, always first, has `""`. */
export interface PlayerOption { id: string; label: string; command: string; }

/** A browser whose cookies yt-dlp could read. `supported` is false where it
 *  cannot on this OS; `note` says why, or what the OS will ask for. */
export interface BrowserOption {
  id: string; label: string; supported: boolean; note: string | null;
}
