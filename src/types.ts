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

export type AddKind = "channel" | "video" | "short" | "playlist";

export interface Channel {
  id: string; title: string; handle: string | null; url: string;
  thumb_path: string | null; subscribed: boolean;
  /** Whether you have joined this channel's membership, which is what lets a
   *  poll ingest its members-only uploads. */
  member: boolean;
  /** New uploads are queued for download by the poll on their own. */
  auto_download: boolean;
  added_at: number; last_polled_at: number | null;
  /** YouTube has terminated this channel's account, so it is no longer polled. */
  terminated: boolean;
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
  /** Hidden with its whole series: a part that joins this video's card later
   *  arrives hidden and marked too. Un-hiding one part keeps the mark. */
  series_hidden: boolean;
  /** Unlinked by hand: no description link, and no title, puts it on a card
   *  again. Only marking it places it. */
  part_links_refused: boolean;
  /** A description link of its own put it on its card, so "Unlink from
   *  siblings" applies though nothing was marked by hand. Computed by the
   *  feed's reads; false everywhere else. */
  part_linked: boolean;
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
  /** The channels to show, any of them; empty is every channel. */
  channelIds: string[]; hideWatched: boolean; downloadedOnly: boolean;
  showHidden: boolean;
  /** Grouped feed only: drop lone videos, keeping multi-part series. */
  groupsOnly: boolean;
  /** Grouped feed only: keep just the series you have started and not
   *  finished -- at least one part watched and one not. */
  inProgress: boolean;
  search: string | null;
  /** Id of the video whose siblings to show; overrides every filter above. */
  siblingOf: string | null;
  sort: SortOrder; limit: number; offset: number;
}

export interface TakeoutRow {
  channelId: string; title: string; alreadySubscribed: boolean;
}

/* ------------------------------------------------------------------ *
 * The Add dialog
 *
 * camelCase like every dialog payload; the `Channel` rows it lists beside
 * them stay snake_case.
 * ------------------------------------------------------------------ */

/** One subscribed channel's share of the library (`subscription_stats`).
 *  Counts are over its ready rows only. A channel with none still has a row,
 *  of zeros. */
export interface ChannelStats {
  channelId: string;
  videos: number;
  /** Of those, downloaded (`download_state = done`). */
  downloaded: number;
  watched: number;
  hidden: number;
  /** Their durations added up, an unresolved one counting as zero. */
  runtimeSecs: number;
  /** The newest upload's `published_at`, Unix seconds. */
  latestUpload: number | null;
  /** From the poll's channel listing; null until a listing has said. */
  followerCount: number | null;
}

/** Which videos the feed's stats count (`feed_stats`): the channel pick and the
 *  search, and nothing else. The Show toggles never change the numbers, and
 *  hidden videos are always counted. */
export interface StatsFilter {
  /** Any of these channels; empty is every channel. */
  channelIds: string[];
  search: string | null;
}

/** The Subscriptions feed's stats, drawn as the waveform above the grid
 *  (Settings → Appearance → Show stats). Over `status = 'ready'` rows only. */
export interface FeedStats {
  /** Every video counted: toWatch + watched + hidden. */
  videos: number;
  /** Cards of two or more parts the grouped feed would show for the same pick,
   *  each counted by its whole group. */
  series: number;
  /** Not watched and not hidden (a downloaded one included). */
  toWatch: number;
  /** Watched and not hidden. */
  watched: number;
  /** With a file on disk (`download_state = done`), watched, hidden or not. */
  downloaded: number;
  /** Hidden, watched or not. */
  hidden: number;
  /** Every duration added up, an unresolved one counting as zero. */
  runtimeSecs: number;
  /** Some video has no duration yet, so `runtimeSecs` understates: drawn "+". */
  runtimeUnknown: boolean;
  /** Published in the last seven days. */
  newThisWeek: number;
  /** The local date the stats were counted on (`"2026-10-19"`): the wave's
   *  right edge, on the same clock that dated `days`. */
  today: string;
  /** One per local calendar day that had an upload, oldest first; days with
   *  none are absent. The waveform buckets them into bars itself. A video with
   *  no `published_at` is in the totals and on no day. */
  days: StatsDay[];
}

/** One day of uploads in the waveform's four segments, which never overlap:
 *  they add up to the day's videos. */
export interface StatsDay {
  /** `"2024-03-14"`, local time. */
  day: string;
  /** Watched, not hidden. */
  watched: number;
  /** Downloaded and not watched, not hidden. */
  downloaded: number;
  /** Neither watched nor downloaded, not hidden. */
  unwatched: number;
  /** Hidden, whatever else. */
  hidden: number;
}

/** What a pasted link turned out to be, read before anything is written
 *  (`preview_add`). A channel is not fetched: it only says whether you
 *  already follow it. */
export type AddPreview =
  | { kind: "channel"; subscribedId: string | null }
  /** `playlistId` is the link's `list=`, when it carried one (never a mix). */
  | { kind: "video"; video: VideoPreview; playlistId: string | null }
  | { kind: "playlist"; playlist: PlaylistPreview };

export interface VideoPreview {
  id: string; title: string;
  channelId: string; channelTitle: string;
  durationSecs: number | null; publishedAt: number | null;
  /** The remote `hqdefault.jpg`. */
  thumbUrl: string;
  /** The cached thumbnail, when the video is already in the library. */
  thumbPath: string | null;
  inLibrary: boolean;
  /** `none` when it is not in the library. */
  downloadState: DownloadState;
  /** Best effort: false when unknown. */
  membersOnly: boolean;
}

export interface PlaylistPreview {
  id: string; title: string;
  channelId: string | null; channelTitle: string | null;
  /** How many videos YouTube says the playlist holds. */
  total: number;
  /** The first 500 at most, private and deleted ones left out. */
  entries: PlaylistEntry[];
  /** Private or deleted entries that were left out. */
  unavailable: number;
  /** The playlist runs past the 500 that were read. Not `total >
   *  entries.length`: left-out private parts make that true of a playlist
   *  read to its end. */
  truncated: boolean;
}

/** One part of a previewed playlist. Handed back whole to `add_playlist`. */
export interface PlaylistEntry {
  id: string; title: string;
  channelId: string | null; channelTitle: string | null;
  durationSecs: number | null;
  thumbUrl: string; thumbPath: string | null;
  inLibrary: boolean; downloadState: DownloadState;
  membersOnly: boolean;
}

/** What `add_playlist` did with the parts it was given, by id. */
export interface AddPlaylistResult {
  /** Newly in the library, in the order given. */
  added: string[];
  /** Already there, and left untouched. */
  present: string[];
  /** Refused, as MyTube refuses every Short. */
  skippedShorts: string[];
}
export interface ImportProgress { done: number; total: number; current: string; }

/**
 * The Subscriptions feed's filters, persisted between runs. The active tab is
 * deliberately absent: a launch always lands on Subscriptions.
 */
export interface ViewState {
  /** Sorted channel ids; empty for every channel. */
  channel_ids: string[]; search: string;
  hide_watched: boolean; downloaded_only: boolean; show_hidden: boolean;
  grouped: boolean; groups_only: boolean; in_progress: boolean; sort: SortOrder;
  /** The Add dialog's subscription list, folded away. Not a filter: it rides
   *  in this block because it is remembered the same way, per machine. */
  subs_folded: boolean;
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
  /** How every download picks its formats, unless a video carries an override
   *  from Download (custom)…. Portable: it travels in an export. */
  quality: Quality;
  /* Machine-local keys, never exported. Always present: `#[serde(default)]`
   * fills each one before the object ever crosses the IPC boundary. */
  /** `"auto"` (the browser signed in to YouTube, else the one used last; see
   *  `BrowserScan.automatic`), `""` for none, or a `--cookies-from-browser`
   *  spec verbatim (`chrome`, `firefox:work`). */
  cookies_browser: string;
  /** A cookies file in any shape `inspect_cookies_file` reads: Netscape, JSON
   *  or name=value pairs. Non-empty wins over `cookies_browser`. */
  cookies_file: string;
  /** `"nightly"` or `"stable"`; anything else reads as nightly. */
  ytdlp_channel: string;
  ytdlp_auto_update: boolean;
  /** Ask GitHub once a day whether a newer MyTube release exists. Off means no
   *  request at all. */
  check_app_updates: boolean;
  /** Text size in percent, 80–200 in steps of 10 (Settings → Appearance), set
   *  as `--text-scale`: text only, never icons, thumbnails or spacing. It
   *  makes up for this machine's screen, which is why it is in this block. */
  text_scale: number;
  /** The stats above the Subscriptions feed and inside a series (Settings →
   *  Appearance). Off unless switched on; machine-local like `text_scale`. */
  show_stats: boolean;
  /** Hand-edit-only overrides. No control writes them; they ride through every
   *  save untouched because `commit()` always spreads the whole object. */
  ytdlp_path: string; ffmpeg_path: string; deno_path: string;
}

/* ------------------------------------------------------------------ *
 * Download quality
 *
 * `Quality` lives inside `Settings`, so it is snake_case like the rest of it.
 * `VideoFormats` and its tracks are a dialog payload, camelCase like
 * `ArchiveSummary`.
 * ------------------------------------------------------------------ */

export type QualityMode = "video" | "audio";
/** A preference, not a filter: yt-dlp sorts by it and never fails over it. */
export type QualityCodec = "any" | "av1" | "vp9" | "h264";
export type QualityFps = "any" | "prefer60" | "max30";
export type QualityContainer = "mkv" | "mp4";
/** `original` keeps whatever audio stream was downloaded, unconverted. */
export type QualityAudioFormat = "original" | "m4a" | "mp3" | "opus";

export interface Quality {
  mode: QualityMode;
  /** 0 is "best available"; otherwise a ceiling in pixels. */
  max_height: number;
  vcodec: QualityCodec;
  fps: QualityFps;
  container: QualityContainer;
  audio_format: QualityAudioFormat;
  /** A raw yt-dlp `-f` string. Non-empty overrides the stream selects. */
  format: string;
  /** Embed every subtitle track the uploader provided — never YouTube's
   *  automatic captions. Only a merged MKV or MP4 takes them; see
   *  `canEmbedSubtitles`. */
  subtitles: boolean;
  /** Embed the video's chapter markers; every container MyTube makes takes them. */
  chapters: boolean;
}

export interface VideoTrack {
  id: string; height: number; fps: number | null;
  vcodec: "av1" | "vp9" | "h264" | "other";
  /** The raw vcodec string, e.g. `avc1.640028`. */
  codec: string;
  hdr: boolean; ext: string;
  /** Bytes, possibly approximate; null when YouTube does not say. */
  size: number | null;
}

export interface AudioTrack {
  id: string; acodec: "opus" | "aac" | "other"; codec: string;
  abr: number | null; language: string | null; ext: string; size: number | null;
}

/** What one video actually offers, from `probe_formats`. */
export interface VideoFormats {
  title: string;
  /** Sorted by height, then size, descending. */
  video: VideoTrack[];
  /** Sorted by bitrate, descending. */
  audio: AudioTrack[];
  /** The format ids the Settings default would pick for this video. */
  defaultVideo: string | null;
  defaultAudio: string | null;
}

/** `app://update-available`: a newer release on GitHub. `version` carries no
 *  leading `v`; `url` is the release page, since how to update depends on how
 *  MyTube was installed. */
export interface AppUpdate { version: string; url: string }

/** `app_version_info` / `check_app_update_now`, and `app://version-info`, which
 *  the backend sends after every update check and whenever the setting flips.
 *  camelCase, like every dialog payload. `update` is a release newer than this
 *  build — the same test the tray item uses — and always null while checks are
 *  off. `checkedAt` is Unix seconds. */
export interface VersionInfo {
  current: string;
  update: AppUpdate | null;
  checkedAt: number | null;
  checksEnabled: boolean;
}

/** The `cookies_browser` value meaning "whichever browser is signed in to
 *  YouTube" -- resolved by the backend, named in `BrowserScan.automatic`. */
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

/** A browser whose data is on this machine. `supported` is false where
 *  choosing it cannot work: yt-dlp cannot read it on this OS, or it is
 *  `blocked` -- the OS refuses MyTube its data, as macOS 27 does for Firefox,
 *  Chrome, Brave and Edge. `note` says why, or what the OS will ask for. */
export interface BrowserOption {
  id: string; label: string; supported: boolean; blocked: boolean;
  /** Whether the cookies yt-dlp would read hold a YouTube sign-in; null where
   *  MyTube cannot look. */
  signedIn: boolean | null;
  note: string | null;
}

/** What the cookies field is drawn from (`detect_browsers`). */
export interface BrowserScan {
  browsers: BrowserOption[];
  /** The id Automatic resolves to right now; null when no browser can be used. */
  automatic: string | null;
  /** How to let MyTube see a browser the OS hides from it; macOS only. */
  accessHint: string | null;
}

/** What a cookies file holds (`inspect_cookies_file`). */
export interface CookiesFileInfo {
  /** Which shape it was in, worded for the hint ("JSON cookie export"). */
  format: string;
  cookies: number;
  /** Entries that were there but could not be read as a cookie. */
  skipped: number;
  /** Whether any cookie is for youtube.com at all. */
  youtube: boolean;
  signedIn: boolean;
}

/* ------------------------------------------------------------------ *
 * Cloud sync
 *
 * camelCase like every dialog payload. Times are Unix seconds.
 * ------------------------------------------------------------------ */

/** Another computer in this sync group. `lastWriteAt` is when it last sent
 *  its library; null until it has. */
export interface SyncPeer { name: string; lastWriteAt: number | null }

/**
 * `sync_status` and every command that changes sync, and `sync://status`,
 * which the backend sends after every sync attempt and state change.
 */
export interface SyncStatus {
  enabled: boolean;
  /** The group's code. Null while sync is off. */
  code: string | null;
  /** This computer's name, as the others see it. */
  machineName: string;
  /** The other computers. This one is never among them. */
  peers: SyncPeer[];
  lastSyncAt: number | null;
  /** This library has changes the server has not had yet. */
  pending: boolean;
  /** Why the last attempt failed, worded for the user; null once one succeeds. */
  error: string | null;
  /** Nothing synced with this code for 90 days, so the server let it go. */
  expired: boolean;
}
