import type { Video } from "./types";

export function formatDuration(secs: number | null): string {
  if (!secs || secs <= 0) return "";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
}

/**
 * A series' total runtime: worded, and never down to the second.
 *
 * Deliberately not `formatDuration`'s clock form. A sum of seven parts is not a
 * timestamp -- its seconds are noise, and `4:10:00` beside a thumbnail reads as
 * "this is four hours long to watch through", which is exactly the thing it is
 * not. The words also match the register of the panel it sits in ("7 parts",
 * "3 watched").
 */
export function formatTotalRuntime(secs: number): string {
  if (!secs || secs <= 0) return "";
  if (secs < 60) return `${Math.floor(secs)}s`;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  if (!h) return `${m}m`;
  // A clean two hours says "2h": a trailing "0m" is noise, not precision.
  return m ? `${h}h ${m}m` : `${h}h`;
}

/**
 * A series' runtime, and how much of it is guesswork.
 *
 * A part whose duration has not resolved yet contributes nothing, which would
 * silently understate the total -- so the count of those comes back with it and
 * the caller marks the number "4h 10m+" rather than letting it read as exact.
 * Shared by the series card and the nav's breadcrumb so the two never disagree.
 */
export function seriesRuntime(
  videos: { duration_secs: number | null }[],
): { runtime: string; unknown: number } {
  let total = 0;
  let unknown = 0;
  for (const v of videos) {
    if (v.duration_secs) total += v.duration_secs;
    else unknown += 1;
  }
  return { runtime: formatTotalRuntime(total), unknown };
}

export function formatRelative(ts: number | null, now = Date.now() / 1000): string {
  if (!ts) return "—";
  const d = Math.max(0, now - ts);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  if (d < 86400 * 7) return `${Math.floor(d / 86400)}d ago`;
  if (d < 86400 * 365) return `${Math.floor(d / (86400 * 7))}w ago`;
  return `${Math.floor(d / (86400 * 365))}y ago`;
}

export function formatViews(n: number | null): string {
  if (n === null || n === undefined) return "";
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return String(n);
}

/**
 * Whether there is a file on disk to play or to delete. `done` alone is not
 * enough: the row keeps its state after the file is removed, and `file_path`
 * is what actually clears.
 */
export function hasDownloadedFile(v: Pick<Video, "download_state" | "file_path">): boolean {
  return v.download_state === "done" && !!v.file_path;
}

export type CardAction = "download" | "play" | "cancel" | "retry" | "open";

/** Clicking a card does different things depending on its state. */
export function cardAction(v: Video): CardAction {
  if (v.download_state === "downloading" || v.download_state === "queued") return "cancel";
  if (hasDownloadedFile(v)) return "play";
  if (v.download_state === "failed") return "retry";
  return "download";
}

/** Human label for what a click on the card will do. */
export function cardActionLabel(a: CardAction): string {
  switch (a) {
    case "download": return "Download";
    case "play": return "Play";
    case "cancel": return "Cancel";
    case "retry": return "Retry";
    case "open": return "Open on YouTube";
  }
}

/**
 * Where this video lives on YouTube. Shorts play from `/watch` too, so one
 * shape covers everything we store.
 */
export function videoUrl(v: { id: string }): string {
  return `https://www.youtube.com/watch?v=${encodeURIComponent(v.id)}`;
}

/** Absolute upload date, shown in the card tooltip alongside the relative one. */
export function formatDate(ts: number | null): string {
  if (!ts) return "";
  return new Date(ts * 1000).toLocaleDateString(undefined, {
    year: "numeric", month: "short", day: "numeric",
  });
}

/* ------------------------------------------------------------------ *
 * Card zoom
 * ------------------------------------------------------------------ */

export const CARD_MIN = 160;
export const CARD_MAX = 640;
export const CARD_DEFAULT = 260;
const CARD_STEP = 20;

export function clampCardSize(px: number): number {
  if (!Number.isFinite(px)) return CARD_DEFAULT;
  return Math.min(CARD_MAX, Math.max(CARD_MIN, Math.round(px)));
}

/** `deltaY` follows wheel convention: negative scrolls up, which zooms in. */
export function nextCardSize(current: number, deltaY: number): number {
  const dir = deltaY < 0 ? 1 : -1;
  return clampCardSize(current + dir * CARD_STEP);
}

/* ------------------------------------------------------------------ *
 * Filename template helpers
 * ------------------------------------------------------------------ */

export interface TemplatePreset {
  label: string;
  value: string;
  hint: string;
}

/** Whole templates, applied by replacing the field. */
export const TEMPLATE_PRESETS: TemplatePreset[] = [
  {
    label: "Channel folder",
    value: "%(uploader)s/%(title)s [%(id)s].%(ext)s",
    hint: "Veritasium/Some video [abc123].mkv",
  },
  {
    label: "Channel folder, dated",
    value: "%(uploader)s/%(upload_date)s - %(title)s [%(id)s].%(ext)s",
    hint: "Veritasium/20260818 - Some video [abc123].mkv",
  },
  {
    label: "Flat",
    value: "%(title)s [%(id)s].%(ext)s",
    hint: "Some video [abc123].mkv",
  },
  {
    label: "Channel / year",
    value: "%(uploader)s/%(upload_date>%Y)s/%(title)s [%(id)s].%(ext)s",
    hint: "Veritasium/2026/Some video [abc123].mkv",
  },
  {
    label: "Dated, flat",
    value: "%(upload_date)s - %(uploader)s - %(title)s.%(ext)s",
    hint: "20260818 - Veritasium - Some video.mkv",
  },
];

/** Individual fields, inserted at the caret. */
export const TOKEN_CHIPS = [
  "%(title)s",
  "%(id)s",
  "%(uploader)s",
  "%(channel)s",
  "%(upload_date)s",
  "%(duration)s",
  "%(resolution)s",
  "%(view_count)s",
  "%(playlist_index)d",
  "%(ext)s",
];

export function insertToken(
  value: string,
  caret: number | null,
  token: string,
): { value: string; caret: number } {
  const at = caret === null || caret > value.length ? value.length : caret;
  return {
    value: value.slice(0, at) + token + value.slice(at),
    caret: at + token.length,
  };
}
