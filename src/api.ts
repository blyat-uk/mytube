import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AddKind, Channel, Video, VideoFilter, VideoGroup, Settings, ViewState, PollSummary,
  ImportResult, TakeoutRow,
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
  listVideos: (filter: VideoFilter) => invoke<Video[]>("list_videos", { filter }),
  /** The same feed with each channel's series collapsed. `limit`/`offset` count
   *  groups here, not videos. */
  listVideoGroups: (filter: VideoFilter) =>
    invoke<VideoGroup[]>("list_video_groups", { filter }),
  setWatched: (videoId: string, watched: boolean) =>
    invoke<void>("set_watched", { videoId, watched }),
  enqueueDownload: (videoId: string) => invoke<void>("enqueue_download", { videoId }),
  cancelDownload: (videoId: string) => invoke<void>("cancel_download", { videoId }),
  openInPlayer: (videoId: string) => invoke<void>("open_in_player", { videoId }),
  deleteDownload: (videoId: string) => invoke<void>("delete_download", { videoId }),
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
