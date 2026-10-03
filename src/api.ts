import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AddKind, Channel, Video, VideoFilter, VideoGroup, Settings, ViewState, PollSummary,
  ImportResult, TakeoutRow, ArchiveSummary, ImportMode, ImportReport, TransferEstimate,
  PlayerOption, BrowserScan, CookiesFileInfo, ToolStatus, Quality, VideoFormats, VersionInfo,
} from "./types";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  /** Writes only the feed's filters; `save_settings` ignores this block. */
  saveViewState: (view: ViewState) => invoke<void>("save_view_state", { view }),
  listChannels: () => invoke<Channel[]>("list_channels"),
  addChannel: (input: string) => invoke<Channel>("add_channel", { input }),
  addVideo: (input: string) => invoke<Video>("add_video", { input }),
  classifyAddInput: (input: string) => invoke<AddKind>("classify_add_input", { input }),
  removeChannel: (channelId: string) => invoke<void>("remove_channel", { channelId }),
  previewTakeoutCsv: (path: string) => invoke<TakeoutRow[]>("preview_takeout_csv", { path }),
  importTakeoutCsv: (path: string, channelIds: string[]) =>
    invoke<ImportResult>("import_takeout_csv", { path, channelIds }),
  setVideoHidden: (videoId: string, hidden: boolean) =>
    invoke<void>("set_video_hidden", { videoId, hidden }),
  deleteVideo: (videoId: string) => invoke<void>("delete_video", { videoId }),
  pollAll: () => invoke<PollSummary>("poll_all"),
  pollChannel: (channelId: string) => invoke<PollSummary>("poll_channel", { channelId }),
  /** Records a channel membership. Joining reads one deep listing there and
   *  then and resolves with the number of members-only videos it found. */
  setChannelMember: (channelId: string, member: boolean) =>
    invoke<number>("set_channel_member", { channelId, member }),
  /** How many videos already in the library "Everything so far" would queue.
   *  Read-only: the turn-on prompt's live count. */
  autoDownloadBacklogCount: (channelId: string, includeWatched: boolean, includeHidden: boolean) =>
    invoke<number>("auto_download_backlog_count", { channelId, includeWatched, includeHidden }),
  /** Switches auto-download. Off removes and cancels nothing. On with a
   *  `backlog` also queues what is already in the library, and resolves with
   *  how many that was; otherwise 0. */
  setChannelAutoDownload: (
    channelId: string, enabled: boolean,
    backlog: { includeWatched: boolean; includeHidden: boolean } | null,
  ) => invoke<number>("set_channel_auto_download", { channelId, enabled, backlog }),
  listVideos: (filter: VideoFilter) => invoke<Video[]>("list_videos", { filter }),
  /** The same feed with each channel's series collapsed. `limit`/`offset` count
   *  groups here, not videos. */
  listVideoGroups: (filter: VideoFilter) =>
    invoke<VideoGroup[]>("list_video_groups", { filter }),
  /** Marks videos siblings by hand, for a series the titles could never join.
   *  Resolves with the finished group's size, which can exceed what was sent:
   *  marking across two hand-built groups merges both. */
  markSiblings: (videoIds: string[]) => invoke<number>("mark_siblings", { videoIds }),
  unlinkSiblings: (videoId: string) => invoke<void>("unlink_siblings", { videoId }),
  setWatched: (videoId: string, watched: boolean) =>
    invoke<void>("set_watched", { videoId, watched }),
  /** A `quality` is stored on the row as its override and then queued; none
   *  leaves any stored override alone, which is how Retry repeats a custom
   *  download exactly. */
  enqueueDownload: (videoId: string, quality?: Quality | null) =>
    invoke<void>("enqueue_download", { videoId, quality: quality ?? null }),
  /** Everything one video offers, and what the Settings default would pick
   *  from it. Runs yt-dlp, so it takes a few seconds. */
  probeFormats: (videoId: string) => invoke<VideoFormats>("probe_formats", { videoId }),
  cancelDownload: (videoId: string) => invoke<void>("cancel_download", { videoId }),
  openInPlayer: (videoId: string) => invoke<void>("open_in_player", { videoId }),
  deleteDownload: (videoId: string) => invoke<void>("delete_download", { videoId }),
  /** What an export would weigh, so the thumbnails tick can quote a real size
   *  rather than a guess the user has no way to check. */
  transferEstimate: () => invoke<TransferEstimate>("transfer_estimate"),
  exportConfig: (path: string, includeThumbs: boolean) =>
    invoke<void>("export_config", { path, includeThumbs }),
  /** Reads an archive's manifest without unpacking it: nothing is written until
   *  `importConfig` runs, so the checklist can describe the file first. */
  readArchive: (path: string) => invoke<ArchiveSummary>("read_archive", { path }),
  importConfig: (
    path: string, channelIds: string[], mode: ImportMode, applySettings: boolean,
  ) => invoke<ImportReport>("import_config", { path, channelIds, mode, applySettings }),
  /** Players installed on this machine, "System default" first. */
  detectPlayers: () => invoke<PlayerOption[]>("detect_players"),
  /** Browsers whose data is on this machine, and which one Automatic uses. */
  detectBrowsers: () => invoke<BrowserScan>("detect_browsers"),
  /** What a cookies file holds; rejects, saying why, when it cannot be used. */
  inspectCookiesFile: (path: string) => invoke<CookiesFileInfo>("inspect_cookies_file", { path }),
  toolsStatus: () => invoke<ToolStatus[]>("tools_status"),
  /** Forces the yt-dlp update check and retries any failed install. Rejects
   *  while a download holds yt-dlp, since nothing is swapped under a live job. */
  toolsUpdateNow: () => invoke<ToolStatus[]>("tools_update_now"),
  /** This build's version and any newer release already known. No network. */
  appVersionInfo: () => invoke<VersionInfo>("app_version_info"),
  /** Asks GitHub now, whether or not a day has passed; rejects if it cannot. */
  checkAppUpdateNow: () => invoke<VersionInfo>("check_app_update_now"),
  /** Hands a URL to the desktop's default browser. */
  openExternal: (url: string) => openUrl(url),
};

/** Local cached thumb if we have one, else the remote URL, else a blank. */
export function thumbSrc(v: { thumb_path: string | null; thumb_url: string | null }): string {
  if (v.thumb_path) return convertFileSrc(v.thumb_path);
  return v.thumb_url ?? "";
}

/** Commands reject with a plain string; normalise anything else for a toast. */
export function errText(err: unknown): string {
  if (typeof err === "string") return err;
  if (err instanceof Error) return err.message;
  return String(err);
}
