import type { Video } from "./types";

export function formatDuration(secs: number | null): string {
  if (!secs || secs <= 0) return "";
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  const s = Math.floor(secs % 60);
  const pad = (n: number) => String(n).padStart(2, "0");
  return h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
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

export type CardAction = "download" | "play" | "cancel" | "retry";

/** Clicking a card does different things depending on its state. */
export function cardAction(v: Video): CardAction {
  if (v.download_state === "downloading" || v.download_state === "queued") return "cancel";
  if (v.download_state === "done" && v.file_path) return "play";
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
  }
}

/** Absolute upload date, shown in the card tooltip alongside the relative one. */
export function formatDate(ts: number | null): string {
  if (!ts) return "";
  return new Date(ts * 1000).toLocaleDateString(undefined, {
    year: "numeric", month: "short", day: "numeric",
  });
}
