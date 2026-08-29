import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import type {
  AddKind, Channel, Video, VideoFilter, Settings, PollSummary, ImportResult,
} from "./types";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  listChannels: () => invoke<Channel[]>("list_channels"),
  addChannel: (input: string) => invoke<Channel>("add_channel", { input }),
  addVideo: (input: string) => invoke<Video>("add_video", { input }),
  classifyAddInput: (input: string) => invoke<AddKind>("classify_add_input", { input }),
  removeChannel: (channelId: string) => invoke<void>("remove_channel", { channelId }),
  importTakeoutCsv: (path: string) => invoke<ImportResult>("import_takeout_csv", { path }),
  pollAll: () => invoke<PollSummary>("poll_all"),
  pollChannel: (channelId: string) => invoke<PollSummary>("poll_channel", { channelId }),
  listVideos: (filter: VideoFilter) => invoke<Video[]>("list_videos", { filter }),
  setWatched: (videoId: string, watched: boolean) =>
    invoke<void>("set_watched", { videoId, watched }),
  enqueueDownload: (videoId: string) => invoke<void>("enqueue_download", { videoId }),
  cancelDownload: (videoId: string) => invoke<void>("cancel_download", { videoId }),
  openInPlayer: (videoId: string) => invoke<void>("open_in_player", { videoId }),
  deleteDownload: (videoId: string) => invoke<void>("delete_download", { videoId }),
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
