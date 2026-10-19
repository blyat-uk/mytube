import { formatLibraryRuntime } from "../../format";
import type { FeedStats, StatsDay } from "../../types";

/**
 * The feed's waveform, worked out without a DOM: how long a bar is, which
 * quiet stretches are cut out, where each bar stands, how tall it is, what the
 * axis says and what a tooltip says. `FeedWaveform` only measures its box and
 * draws what these hand back.
 *
 * The wave adapts to the library it is given rather than to the calendar. A
 * library is rarely an even spread of years: most are a few busy months (a
 * subscription's backfill reaches back only so far) behind a handful of
 * stragglers years older -- on the real library that prompted this, five
 * hidden uploads from 2019 and nothing else before 2025. Laid out as plain
 * calendar months, those five stretched the wave over seven years and left
 * everything that mattered crammed against its right edge. So:
 *
 * - a stretch of half a year or more with no uploads is **cut out**, and drawn
 *   as a break between the stretches either side of it;
 * - a bar is the **shortest span that fits** what is left -- a day, a week, a
 *   month, a quarter, half a year or a year -- so a library a few months old
 *   gets a wave as full as one ten years old;
 * - heights follow the **square root** of a bar's count, the way a sound wave
 *   is drawn for the ear, so a quiet spell is a small bar rather than a dot
 *   beside a backfill's spike. The tooltip carries the exact numbers.
 */

/** The wave's drawing box is this tall, in px; its axis runs through the middle. */
export const WAVE_HEIGHT = 96;
/** The wave never runs wider than this, however wide the window. */
export const WAVE_MAX_WIDTH = 720;
/**
 * The closest two bars may stand. Any closer and a bar and its gap stop reading
 * as two things, so the bars get longer instead -- a week, then a month.
 */
export const MIN_PITCH = 6;
/**
 * The furthest apart two bars may stand. A library a few days old would
 * otherwise spread its bars across the whole 720px like fence posts; capped,
 * its wave is short and the flanks close in around it.
 */
export const MAX_PITCH = 12;
/** A stretch with no uploads at least this long is cut out of the wave. */
export const QUIET_DAYS = 183;
/** What a cut takes on the wave: the room of this many bars. */
export const BREAK_SLOTS = 3;
/** A bar is never wider than this, however few there are. */
const MAX_BAR_WIDTH = 5;
/** Every non-empty bar shows at least this much each side, so a single video is a dot, not nothing. */
const MIN_HALF = 2.5;

const DAY_MS = 86_400_000;
const fmt = (n: number) => n.toLocaleString("en");

/* ------------------------------------------------------------------ *
 * Calendar days
 *
 * A day is a whole number: days since 1970-01-01 *on the calendar*, built
 * from the "YYYY-MM-DD" the backend filed it under in local time. Arithmetic
 * on those never meets a clock change, which arithmetic on instants would.
 * ------------------------------------------------------------------ */

/** "2024-03-14" as a day number. */
export function dayNumber(iso: string): number {
  const [y, m, d] = iso.split("-").map(Number);
  return Math.round(Date.UTC(y, m - 1, d) / DAY_MS);
}

interface Ymd { y: number; m: number; d: number; dow: number }

function ymd(day: number): Ymd {
  const t = new Date(day * DAY_MS);
  // Monday is 0: a week starts on Monday, as YouTube's own analytics count it.
  return { y: t.getUTCFullYear(), m: t.getUTCMonth() + 1, d: t.getUTCDate(), dow: (t.getUTCDay() + 6) % 7 };
}

const dayOf = (y: number, m: number, d = 1) => Math.round(Date.UTC(y, m - 1, d) / DAY_MS);

/** How long one bar is, shortest first: the order they are tried in. */
export type Unit = "day" | "week" | "month" | "quarter" | "half" | "year";
export const UNITS: readonly Unit[] = ["day", "week", "month", "quarter", "half", "year"];
const MONTHS_IN: Partial<Record<Unit, number>> = { month: 1, quarter: 3, half: 6, year: 12 };

/** The first day of the bar `day` falls in. */
export function bucketStart(day: number, unit: Unit): number {
  if (unit === "day") return day;
  const t = ymd(day);
  if (unit === "week") return day - t.dow;
  const span = MONTHS_IN[unit]!;
  return dayOf(t.y, t.m - ((t.m - 1) % span));
}

/** The first day of the bar after the one starting on `start`. */
export function nextBucket(start: number, unit: Unit): number {
  if (unit === "day") return start + 1;
  if (unit === "week") return start + 7;
  const t = ymd(start);
  return dayOf(t.y, t.m + MONTHS_IN[unit]!);
}

/* ------------------------------------------------------------------ *
 * Layout
 * ------------------------------------------------------------------ */

/** One drawn bar: a day, a week, a month or longer, by the layout's `unit`. */
export interface WaveBar {
  /** Its first and last day (day numbers, inclusive). */
  from: number;
  to: number;
  watched: number;
  downloaded: number;
  unwatched: number;
  hidden: number;
  /** All four added up. */
  total: number;
}

/** A quiet stretch cut out of the wave: the first and last day with nothing in it. */
export interface WaveBreak {
  from: number;
  to: number;
}

export type WaveItem =
  | { kind: "bar"; bar: WaveBar; at: number }
  | { kind: "break"; gap: WaveBreak; at: number };

export interface WaveLayout {
  unit: Unit;
  /** Bars and breaks, oldest first. `at` is where each starts, in bar slots. */
  items: WaveItem[];
  /** The slots they take: a bar is one, a break `BREAK_SLOTS`. */
  slots: number;
}

/** The stretches with uploads, as first and last day, split wherever `QUIET_DAYS` pass with none. */
interface Run { from: number; to: number; first: number; last: number }

function runs(days: number[], today: number): Run[] {
  const out: Run[] = [];
  for (const d of days) {
    const run = out[out.length - 1];
    if (run && d - run.last <= QUIET_DAYS) run.last = run.to = d;
    else out.push({ from: d, to: d, first: d, last: d });
  }
  // The newest stretch runs on to today, so the right edge means now -- unless
  // nothing has come for so long that the stretch to today would be a quiet
  // one in its own right; then the wave stops where the uploads did.
  const tail = out[out.length - 1];
  if (tail && today > tail.last && today - tail.last <= QUIET_DAYS) tail.to = today;
  return out;
}

/** How many bars lie strictly between the bars starting `a` and `b`. */
function barsBetween(a: number, b: number, unit: Unit, cap: number): number {
  let n = 0;
  for (let s = nextBucket(a, unit); s < b && n <= cap; s = nextBucket(s, unit)) n++;
  return n;
}

/**
 * The wave with bars `unit` long. A quiet stretch is cut only when that saves
 * room: one only a couple of bars long at this length is drawn as the empty
 * bars it is, which is also what keeps two stretches that land in the same or
 * neighbouring bars from being cut apart.
 */
export function layoutFor(days: StatsDay[], today: number, unit: Unit): WaveLayout {
  const nums = days.map((d) => dayNumber(d.day));
  const spans: { from: number; to: number; first: number; last: number }[] = [];
  for (const r of runs(nums, today)) {
    const from = bucketStart(r.from, unit), to = bucketStart(r.to, unit);
    const prev = spans[spans.length - 1];
    if (prev && barsBetween(prev.to, from, unit, BREAK_SLOTS) <= BREAK_SLOTS) {
      prev.to = Math.max(prev.to, to);
      prev.last = r.last;
    } else {
      spans.push({ from, to, first: r.first, last: r.last });
    }
  }

  const items: WaveItem[] = [];
  let at = 0, k = 0;
  spans.forEach((span, i) => {
    if (i > 0) {
      items.push({ kind: "break", gap: { from: spans[i - 1].last + 1, to: span.first - 1 }, at });
      at += BREAK_SLOTS;
    }
    for (let s = span.from; s <= span.to; s = nextBucket(s, unit)) {
      const end = nextBucket(s, unit);
      const bar: WaveBar = { from: s, to: end - 1, watched: 0, downloaded: 0, unwatched: 0, hidden: 0, total: 0 };
      for (; k < days.length && nums[k] < end; k++) {
        bar.watched += days[k].watched;
        bar.downloaded += days[k].downloaded;
        bar.unwatched += days[k].unwatched;
        bar.hidden += days[k].hidden;
      }
      bar.total = bar.watched + bar.downloaded + bar.unwatched + bar.hidden;
      items.push({ kind: "bar", bar, at });
      at += 1;
    }
  });
  return { unit, items, slots: at };
}

/**
 * The wave for a box `width` px wide: the shortest bars that fit at
 * `MIN_PITCH`. A year-long bar is the last resort, kept even if it overflows
 * (a library would need more than a century of uploads).
 */
export function chooseLayout(days: StatsDay[], today: number, width: number): WaveLayout {
  const fit = Math.max(1, Math.floor(width / MIN_PITCH));
  let layout: WaveLayout | null = null;
  for (const unit of UNITS) {
    layout = layoutFor(days, today, unit);
    if (layout.slots <= fit) break;
  }
  return layout!;
}

/** The stats' today as a day number. A stats object without one (an older
 *  backend) ends the wave at its newest upload. */
export function todayOf(stats: FeedStats): number {
  if (stats.today) return dayNumber(stats.today);
  const last = stats.days[stats.days.length - 1];
  return last ? dayNumber(last.day) : 0;
}

/**
 * How wide the wave may grow: as wide as its bars fill at `MAX_PITCH`, laid
 * out for the widest wave there is, and never past `WAVE_MAX_WIDTH`. The
 * layout uses it as the wave's column, so a short history draws a short wave.
 * (The stylesheet keeps the column at least as wide as the "+N this week"
 * pill, in rem, since only it knows how big text is.)
 */
export function waveCap(days: StatsDay[], today: number): number {
  return Math.min(WAVE_MAX_WIDTH, chooseLayout(days, today, WAVE_MAX_WIDTH).slots * MAX_PITCH);
}

/* ------------------------------------------------------------------ *
 * Geometry
 * ------------------------------------------------------------------ */

export interface WaveGeometry {
  /** Distance between two slots' starts. */
  pitch: number;
  barWidth: number;
  /** Where the first slot starts. Non-zero only when a few bars sit in the
   *  minimum width: they gather in its middle, between the flanks, rather than
   *  leaving a hole on one side. */
  offset: number;
  /** The width it was laid out for. */
  width: number;
}

export function waveGeometry(slots: number, width: number): WaveGeometry {
  if (slots <= 0) return { pitch: 0, barWidth: 0, offset: 0, width };
  const pitch = Math.min(width / slots, MAX_PITCH);
  return {
    pitch,
    barWidth: Math.min(MAX_BAR_WIDTH, pitch * 0.62),
    offset: (width - slots * pitch) / 2,
    width,
  };
}

const spanOf = (item: WaveItem) => (item.kind === "bar" ? 1 : BREAK_SLOTS);

/** The left edge of the bar starting at slot `at`. */
export function barX(at: number, g: WaveGeometry): number {
  return g.offset + at * g.pitch + (g.pitch - g.barWidth) / 2;
}

/** The middle of an item: where its tooltip points, its tick stands, a break's mark sits. */
export function itemCentre(item: WaveItem, g: WaveGeometry): number {
  return g.offset + (item.at + spanOf(item) / 2) * g.pitch;
}

/** The item nearest `x` (px from the wave's left edge), the ends taking whatever lies beyond them. */
export function itemIndexAt(x: number, items: WaveItem[], g: WaveGeometry): number {
  if (items.length === 0 || g.pitch <= 0) return -1;
  const slot = (x - g.offset) / g.pitch;
  for (let i = items.length - 1; i >= 0; i--) if (slot >= items[i].at) return i;
  return 0;
}

/**
 * How far a bar reaches from the axis on each side: the square root of its
 * share of the busiest bar, which fills the half, and never under 2.5px for a
 * bar with anything in it. An empty bar draws nothing at all.
 */
export function halfHeight(total: number, busiest: number, height = WAVE_HEIGHT): number {
  if (total <= 0 || busiest <= 0) return 0;
  return Math.max(MIN_HALF, Math.sqrt(total / busiest) * (height / 2 - 3));
}

export type SegmentKind = "watched" | "downloaded" | "unwatched" | "hidden";

export interface Segment {
  kind: SegmentKind;
  /** Distance from the axis where it starts, and how far it runs outwards. */
  start: number;
  length: number;
}

/**
 * One half of a bar cut into its four states, from the axis outwards: what you
 * watched sits at the heart of it, what waits on disk next, then what is left,
 * and what you hid at the tips. Empty states take no room. Shares are of the
 * bar's own height, so only the height is scaled, never the split.
 */
export function segments(bar: WaveBar, half: number): Segment[] {
  if (bar.total <= 0 || half <= 0) return [];
  const out: Segment[] = [];
  let at = 0;
  for (const kind of ["watched", "downloaded", "unwatched", "hidden"] as const) {
    const n = bar[kind];
    if (n <= 0) continue;
    const length = (n / bar.total) * half;
    out.push({ kind, start: at, length });
    at += length;
  }
  return out;
}

/* ------------------------------------------------------------------ *
 * The axis
 * ------------------------------------------------------------------ */

const MONTHS = [
  "January", "February", "March", "April", "May", "June",
  "July", "August", "September", "October", "November", "December",
];
const short = (m: number) => MONTHS[m - 1].slice(0, 3);

export interface Tick {
  label: string;
  /** Px from the wave's left edge. */
  x: number;
  /** A label this close to an end hangs inwards from its tick instead of
   *  centring on it, so it never pokes out past the wave. */
  align: "start" | "centre" | "end";
}

/** The width of one character of an axis label (1.15rem tabular figures and
 *  short month names), in rem, and the clearance kept between two labels. */
const CHAR_REM = 0.64;
const CLEAR_REM = 1.4;

/**
 * The labels under the wave, placed in order of importance, each skipped if it
 * would crowd one already down:
 *
 * 1. where each stretch after a break begins, hanging rightwards from its
 *    first bar -- the one thing that says how far the wave jumped. "Feb 2025",
 *    or just "2025" for a stretch too short to sit under its month and year,
 *    or once a bar is a quarter or longer;
 * 2. each new year, under the bar holding its 1 January, at one even step
 *    (every year, every other, every fifth, every tenth: see 3), and then the
 *    wave's own start, the same way as 1, if it still fits;
 * 3. months, at one even step -- every month, every other, every quarter or
 *    every half year, the shortest whose labels all fit beside one another --
 *    only while a bar is a month or shorter. An even step reads as a scale;
 *    whichever months happened to fit would read as noise;
 * 4. Mondays by their date ("13"), only while a bar is a day and every month
 *    already has its label.
 */
export function axisTicks(layout: WaveLayout, g: WaveGeometry, rem: number): Tick[] {
  const { unit, items } = layout;
  const coarse = unit === "quarter" || unit === "half" || unit === "year";
  const room = (label: string) => (label.length * CHAR_REM + CLEAR_REM) * rem;
  const placed: { lo: number; hi: number; tick: Tick }[] = [];

  const span = (label: string, x: number, hang: boolean): { lo: number; hi: number; tick: Tick } => {
    const w = room(label);
    let align: Tick["align"] = hang ? "start" : "centre";
    if (x > g.width - (hang ? w : w / 2)) align = "end";
    else if (!hang && x < w / 2) align = "start";
    const [lo, hi] = align === "start" ? [x, x + w] : align === "end" ? [x - w, x] : [x - w / 2, x + w / 2];
    return { lo, hi, tick: { label, x, align } };
  };
  const free = (c: { lo: number; hi: number }) => !placed.some((p) => c.lo < p.hi && c.hi > p.lo);
  const put = (c: { lo: number; hi: number; tick: Tick }) => { if (free(c)) placed.push(c); };

  // The stretches between breaks, and what each bar holds that a label marks.
  const months: { x: number; m: number }[] = [];
  const years: { x: number; y: number }[] = [];
  const mondays: { x: number; d: number }[] = [];
  interface Stretch { x: number; from: number; bars: number }
  let stretch: Stretch | null = null;
  const starts: Stretch[] = [];
  for (const item of items) {
    if (item.kind === "break") { stretch = null; continue; }
    const x = itemCentre(item, g);
    if (!stretch) {
      stretch = { x, from: item.bar.from, bars: 0 };
      starts.push(stretch);
    }
    stretch.bars++;
    if (stretch.bars === 1) continue;
    const a = ymd(item.bar.from), b = ymd(item.bar.to);
    // The first of a month this bar holds, if any: a month's bar starts on
    // one, a week's may hold one inside it.
    const first = a.d === 1 ? a : b.m !== a.m || b.y !== a.y ? b : null;
    if (first && first.m === 1) years.push({ x, y: first.y });
    else if (first && !coarse) months.push({ x, m: first.m });
    else if (!first && unit === "day" && a.dow === 0) mondays.push({ x, d: a.d });
  }

  // A stretch's first label: its month and year when the stretch is long
  // enough to sit under it, else the year alone -- which is also the fallback
  // when the longer one would crowd out what is already down.
  const startOf = (st: Stretch) => {
    const t = ymd(st.from);
    const full = `${short(t.m)} ${t.y}`;
    const forms = !coarse && room(full) <= st.bars * g.pitch + rem ? [full, `${t.y}`] : [`${t.y}`];
    const fits = forms.map((f) => span(f, st.x, true)).find(free);
    if (fits) placed.push(fits);
  };
  // After a break, where the wave picks up is the thing to read, so those go
  // down first. The wave's own start is no jump -- a year beside it says the
  // same -- so it yields to the years.
  // All or nothing at each step: the step whose labels never crowd each other.
  const evenly = <T extends { x: number }>(cands: T[], label: (c: T) => string): boolean => {
    const spans = cands.map((c) => span(label(c), c.x, false));
    for (let i = 1; i < spans.length; i++) if (spans[i].lo < spans[i - 1].hi) return false;
    spans.forEach(put);
    return true;
  };

  for (const st of starts.slice(1).reverse()) startOf(st);
  // Years by the same rule as months, on round numbers: every year, every
  // even one, every fifth, every tenth.
  for (const step of [1, 2, 5, 10]) {
    if (evenly(years.filter((c) => c.y % step === 0), (c) => `${c.y}`)) break;
  }
  if (starts.length > 0) startOf(starts[0]);
  let everyMonth = false;
  for (const step of [1, 2, 3, 6]) {
    if (evenly(months.filter((c) => (c.m - 1) % step === 0), (c) => short(c.m))) {
      everyMonth = step === 1;
      break;
    }
  }
  if (everyMonth && unit === "day") evenly(mondays, (c) => `${c.d}`);

  return placed.sort((p, q) => p.tick.x - q.tick.x).map((p) => p.tick);
}

/* ------------------------------------------------------------------ *
 * Words
 * ------------------------------------------------------------------ */

const dayText = (t: Ymd) => `${t.d} ${short(t.m)} ${t.y}`;

/**
 * A bar's span, in the words its length calls for: "14 Oct 2026", "13 – 19 Oct
 * 2026", "29 Sep – 5 Oct 2026", "October 2026", "Jan – Mar 2024", "2024".
 */
export function barLabel(bar: WaveBar, unit: Unit): string {
  const a = ymd(bar.from), b = ymd(bar.to);
  switch (unit) {
    case "day": return dayText(a);
    case "week":
      if (a.y !== b.y) return `${dayText(a)} – ${dayText(b)}`;
      if (a.m !== b.m) return `${a.d} ${short(a.m)} – ${dayText(b)}`;
      return `${a.d} – ${dayText(b)}`;
    case "month": return `${MONTHS[a.m - 1]} ${a.y}`;
    case "year": return `${a.y}`;
    default: return `${short(a.m)} – ${short(b.m)} ${b.y}`;
  }
}

/** A cut's span by month: "May 2019 – Jan 2025", or "Mar – Oct 2024". */
export function breakLabel(gap: WaveBreak): string {
  const a = ymd(gap.from), b = ymd(gap.to);
  if (a.y === b.y && a.m === b.m) return `${MONTHS[a.m - 1]} ${a.y}`;
  return a.y === b.y ? `${short(a.m)} – ${short(b.m)} ${b.y}` : `${short(a.m)} ${a.y} – ${short(b.m)} ${b.y}`;
}

/** "42 videos", "1 video", "No videos". */
export function videosText(n: number): string {
  if (n === 0) return "No videos";
  return n === 1 ? "1 video" : `${fmt(n)} videos`;
}

export interface TipRow {
  kind: "left" | "watched" | "hidden";
  count: string;
  label: string;
  /** Under "to watch": how many of those are already on disk, when any are. */
  downloaded: string | null;
}

/**
 * A bar's tooltip rows. "To watch" counts the downloaded segment too -- a video
 * on disk is still unwatched -- and names it after a comma, the way the
 * figures under the wave split them. Rows of zero are left out.
 */
export function tipRows(bar: WaveBar): TipRow[] {
  const rows: TipRow[] = [];
  const left = bar.downloaded + bar.unwatched;
  if (left > 0) {
    rows.push({
      kind: "left", count: fmt(left), label: "to watch",
      downloaded: bar.downloaded > 0 ? `${fmt(bar.downloaded)} downloaded` : null,
    });
  }
  if (bar.watched > 0) rows.push({ kind: "watched", count: fmt(bar.watched), label: "watched", downloaded: null });
  if (bar.hidden > 0) rows.push({ kind: "hidden", count: fmt(bar.hidden), label: "hidden", downloaded: null });
  return rows;
}

/** The left flank's caption: "videos in 393 series", "videos in 1 series", or just "videos". */
export function seriesCaption(videos: number, series: number): string {
  const noun = videos === 1 ? "video" : "videos";
  return series > 0 ? `${noun} in ${fmt(series)} series` : noun;
}

/** The right flank's figure: the library's runtime, "+" when some of it is not known yet. */
export function runtimeText(secs: number, unknown: boolean): string {
  const text = formatLibraryRuntime(secs);
  return unknown && secs > 0 ? `${text}+` : text;
}

/** What the wave says to a screen reader: the totals, and the months it spans. */
export function waveSummary(stats: FeedStats): string {
  const days = stats.days;
  const month = (iso: string) => { const t = ymd(dayNumber(iso)); return `${MONTHS[t.m - 1]} ${t.y}`; };
  let span = "";
  if (days.length > 0) {
    const a = month(days[0].day), b = month(days[days.length - 1].day);
    span = a === b ? `, uploaded in ${a}` : `, uploaded from ${a} to ${b}`;
  }
  return `${videosText(stats.videos)}${span}: ${fmt(stats.toWatch)} to watch, ` +
    `${fmt(stats.watched)} watched, ${fmt(stats.downloaded)} downloaded, ${fmt(stats.hidden)} hidden`;
}
