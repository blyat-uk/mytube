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

const DAY_MS = 86_400_000;

function daysInMonth(y: number, m: number): number {
  return new Date(y, m + 1, 0).getDate();
}

/**
 * `date` shifted by whole calendar months, clamped to the target month's length.
 * 31 Jan plus one month is 28 Feb, not the 3 Mar that `setMonth` alone rolls
 * over to -- a rollover would make the anchor below overshoot `to` and cost the
 * span a whole month.
 */
function addMonths(date: Date, n: number): Date {
  const day = date.getDate();
  const d = new Date(date.getTime());
  d.setDate(1);
  d.setMonth(d.getMonth() + n);
  d.setDate(Math.min(day, daysInMonth(d.getFullYear(), d.getMonth())));
  return d;
}

const startOfDay = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();

/**
 * Whole days between two instants, counted over the calendar rather than in
 * 24-hour blocks. A span crossing a spring-forward is an hour short of the days
 * it plainly covers -- 28 Feb to 30 Mar 2026 is 30 days but 29h23 of them -- and
 * dividing by 86 400 000 would round that down to 29. The date difference is
 * rounded (that stray hour is all it can be off by) and the final day only
 * counts once the time of day has come round again.
 */
function daysBetween(from: Date, to: Date): number {
  const whole = Math.round((startOfDay(to) - startOfDay(from)) / DAY_MS);
  const sameDayIncomplete =
    to.getTime() - startOfDay(to) < from.getTime() - startOfDay(from);
  return Math.max(0, sameDayIncomplete ? whole - 1 : whole);
}

/**
 * Whole calendar months between two instants, plus the leftover days.
 *
 * Calendar, not 30.44-day arithmetic: a video uploaded on the 8th is "3m 0d"
 * three months later on the 8th whatever those months were worth, which is the
 * only reading that survives being checked against a calendar.
 */
function calendarSpan(from: Date, to: Date): { months: number; days: number } {
  let months =
    (to.getFullYear() - from.getFullYear()) * 12 + (to.getMonth() - from.getMonth());
  if (addMonths(from, months) > to) months -= 1;
  if (months < 0) months = 0;
  return { months, days: daysBetween(addMonths(from, months), to) };
}

/**
 * Age of an upload, as the card and the downloads list show it.
 *
 * Days run the whole first month -- "23d ago" places an upload where "3w ago"
 * only gestures at it -- and past 31 days the label carries two units so it
 * still narrows down to the day: "3m 8d ago", then "1y 2m ago" once a year is
 * up, rather than a bare year that says nothing for the eleven months after it.
 *
 * The months form keeps its day part even at zero ("3m 0d ago"). `m` is already
 * minutes in the tier above, so a bare "3m ago" would read as three minutes;
 * the second unit is what disambiguates it. Years have no such clash, so a
 * clean anniversary is just "1y ago".
 */
export function formatRelative(ts: number | null, now = Date.now() / 1000): string {
  if (!ts) return "—";
  const d = Math.max(0, now - ts);
  if (d < 60) return "just now";
  if (d < 3600) return `${Math.floor(d / 60)}m ago`;
  if (d < 86400) return `${Math.floor(d / 3600)}h ago`;
  if (d < 86400 * 32) return `${Math.floor(d / 86400)}d ago`;
  const { months, days } = calendarSpan(new Date(ts * 1000), new Date(now * 1000));
  if (months < 12) return `${months}m ${days}d ago`;
  const years = Math.floor(months / 12);
  const rest = months % 12;
  return rest ? `${years}y ${rest}m ago` : `${years}y ago`;
}

export function formatViews(n: number | null): string {
  if (n === null || n === undefined) return "";
  if (n >= 1_000_000_000) return `${(n / 1_000_000_000).toFixed(1)}B`;
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return String(n);
}

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"];

/**
 * A size on disk, as the export tick quotes it: "112 MB", "1.4 GB", "0 B".
 *
 * Three significant figures at most -- a decimal under ten, none above it --
 * because the number is there to answer "is this worth carrying?", and
 * "117.4 MB" answers it no better than "117 MB" while reading as precision
 * nobody asked for. 1024 to the step, matching what a file manager reports for
 * the same directory; a thumbnail cache measured decimally would disagree with
 * every other tool on the machine.
 */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0 B";
  let value = n;
  let unit = 0;
  // Promote a hair early: 1 048 575 B is 1023.999 KB, and rounding that for
  // display would print "1024 KB" -- a unit that is really the next one up.
  while (value >= 1023.95 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  const shown = value < 9.95 ? value.toFixed(1) : String(Math.round(value));
  return `${shown} ${BYTE_UNITS[unit]}`;
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

/** What a popped-out thumbnail keeps between itself and the scroll area's edge. */
const POPOUT_MARGIN = 8;

export interface Edges {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/**
 * Where a Downloads thumbnail lands when it pops out of its row: `width` wide,
 * pinned to the slot's left edge, `offsetY` below the slot's top.
 *
 * It aims for the Subscriptions card size with its top-left corner exactly on
 * the slot's, and gives ground only to stay inside `area`, the scroll container
 * that would otherwise clip it: narrower where the right edge runs out, slid up
 * near the bottom, and down when its row is half scrolled off the top. The top
 * wins when the area is too short for the whole picture, because that is where
 * the eye already is. It never ends up smaller than the slot it came out of.
 */
export function popoutPlacement(
  slot: Edges, area: Edges, cardSize: number,
): { width: number; offsetY: number } {
  const slotWidth = slot.right - slot.left;
  const width = Math.max(slotWidth, Math.min(cardSize, area.right - POPOUT_MARGIN - slot.left));
  const height = (width * 9) / 16;
  const top = Math.max(
    area.top + POPOUT_MARGIN,
    Math.min(slot.top, area.bottom - POPOUT_MARGIN - height),
  );
  return { width, offsetY: top - slot.top };
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
