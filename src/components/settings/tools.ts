import { formatBytes } from "../../format";
import type { ToolKind, ToolProgress, ToolState, ToolStatus } from "../../types";

/** The names a person reads — the same as `ToolKind::label()` on the Rust side. */
export const TOOL_LABEL: Record<ToolKind, string> = {
  ytdlp: "yt-dlp",
  ffmpeg: "ffmpeg",
  deno: "deno",
};

/**
 * The shell's one toast for provisioning: which tools just went from
 * installing to ready, as a sentence, or null when none did. An update is left
 * out on purpose — it changes nothing the user can do, whereas a first install
 * is the moment downloads start working.
 */
export function toolsReadyMessage(
  prev: ReadonlyMap<ToolKind, ToolState>,
  next: ToolStatus[],
): string | null {
  const ready = next
    .filter((t) => t.state === "ready" && prev.get(t.kind) === "installing")
    .map((t) => TOOL_LABEL[t.kind]);
  if (ready.length === 0) return null;
  if (ready.length === 1) return `${ready[0]} is ready.`;
  return `${ready.slice(0, -1).join(", ")} and ${ready[ready.length - 1]} are ready.`;
}

/**
 * How far an install or update has got, in words: "Updating · 18.2 MB of
 * 34.1 MB", "Installing · Unpacking…". Before the first `tools://progress`
 * lands there is nothing to count yet, hence "Starting…"; a download with no
 * Content-Length has no total, so only the bytes so far are given.
 */
export function toolProgressText(state: ToolState, progress?: ToolProgress): string {
  let words = "Starting…";
  if (progress?.phase === "extract") words = "Unpacking…";
  else if (progress) {
    words = progress.total
      ? `${formatBytes(progress.received)} of ${formatBytes(progress.total)}`
      : formatBytes(progress.received);
  }
  return `${state === "updating" ? "Updating" : "Installing"} · ${words}`;
}

/** The fraction done, as a percentage, or null when there is no total to
 *  measure against -- before the first event, a download with no
 *  Content-Length, and unpacking, which reports none -- so the bar travels
 *  rather than inventing a number. */
export function toolProgressPercent(progress?: ToolProgress): number | null {
  if (!progress?.total) return null;
  return Math.min(100, (progress.received / progress.total) * 100);
}
