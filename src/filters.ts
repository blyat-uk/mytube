import type { QuickFilter, SavedView, SortOrder, ViewFilters } from "./types";

/**
 * The feed's filters as one list: what each is called, what it does, and the
 * order the top bar and the All filters sidebar both show them in.
 *
 * Every filter lives in the sidebar. The ones the user stars also sit in the
 * top bar, so the bar holds what this person reaches for and nothing else --
 * which is what lets it keep one row without folding the tabs.
 */

/** Sidebar order, and so the bar's: a starred filter always lands in the same
 *  place, whatever order the stars were given in. */
export const QUICK_ORDER: readonly QuickFilter[] = [
  "hide_watched", "downloaded_only", "show_hidden",
  "grouped", "in_progress", "auto_downloading",
  "length", "sort", "channel_ids",
];

/**
 * What a first run pins, and what a view saved before stars existed reads as:
 * the filter most people want, the arrangement the app is built around, and
 * the order -- close to the row this replaced, without the rest of it.
 * `config.rs` holds the same list for a settings.json with no key.
 */
export const DEFAULT_STARRED: readonly QuickFilter[] = ["hide_watched", "grouped", "sort"];

/**
 * Known ids only, each once, in `QUICK_ORDER`. Not an array at all (a view from
 * a build without stars) reads as the default. The shell compares the saved
 * view's JSON to decide whether to write it back, so the same stars must
 * always serialise the same way -- `ViewState::sanitize` does exactly this too.
 */
export function normalizeStarred(ids: unknown): QuickFilter[] {
  if (!Array.isArray(ids)) return [...DEFAULT_STARRED];
  const set = new Set<unknown>(ids);
  return QUICK_ORDER.filter((id) => set.has(id));
}

/** Stars an id or takes its star away, keeping the list in order. */
export function toggleStar(starred: readonly QuickFilter[], id: QuickFilter): QuickFilter[] {
  const set = new Set(starred);
  if (set.has(id)) set.delete(id);
  else set.add(id);
  return QUICK_ORDER.filter((x) => set.has(x));
}

/** The filters that are an on/off switch: everything but length, the sort and
 *  the channel pick. */
export type ToggleId = Exclude<QuickFilter, "length" | "sort" | "channel_ids">;

export interface ToggleDef {
  id: ToggleId;
  /** Its name in the sidebar, and its accessible name everywhere. */
  label: string;
  /** Its name on a bar chip, where it is shorter; defaults to `label`. */
  chip?: string;
  /** The sidebar's second line: what it does, short enough for one line. */
  desc: string;
  /** The bar chip's tooltip, which has room to say it in full. */
  hint: string;
  /** Applies to series cards, so only while grouped. */
  seriesOnly: boolean;
}

export const TOGGLES: readonly ToggleDef[] = [
  {
    id: "hide_watched", label: "Unwatched", seriesOnly: false,
    desc: "Only videos you have not watched", hint: "Show only videos you have not watched",
  },
  {
    id: "downloaded_only", label: "Downloaded", seriesOnly: false,
    desc: "Only videos on disk", hint: "Show only videos you have downloaded",
  },
  {
    id: "show_hidden", label: "Include hidden", seriesOnly: false,
    desc: "Also hidden videos, to un-hide them",
    hint: "Also show videos you have hidden, so you can un-hide them",
  },
  {
    id: "grouped", label: "Group siblings", chip: "Grouped", seriesOnly: false,
    desc: "One card per multi-part series",
    hint: "Collapse each channel's multi-part uploads into one card",
  },
  {
    id: "in_progress", label: "Continue watching", seriesOnly: true,
    desc: "Series started and not finished",
    hint: "Show only series you have started and not finished",
  },
  {
    id: "auto_downloading", label: "Auto-downloading", seriesOnly: true,
    desc: "New parts download on their own",
    hint: "Show only series whose new parts download on their own",
  },
];

export const toggleDef = (id: ToggleId): ToggleDef => TOGGLES.find((t) => t.id === id)!;

export const SORTS: { value: SortOrder; label: string }[] = [
  { value: "newest", label: "Newest first" },
  { value: "oldest", label: "Oldest first" },
  { value: "channel", label: "By channel" },
  { value: "length", label: "Longest first" },
  { value: "parts", label: "Most parts first" },
];

/** The sorts on offer: Most parts first ranks groups, so only while grouped. */
export const sortsFor = (grouped: boolean) => SORTS.filter((s) => s.value !== "parts" || grouped);

/**
 * The Length filter's steps. Both rows ask for "at least": nobody hunts for the
 * shortest thing, and what Length is for is finding the well-developed shows,
 * long and in many parts. A series counts as all its parts added up.
 */
export const LENGTH_STEPS: readonly number[] = [0, 3600, 7200, 18000, 36000];
export const PARTS_STEPS: readonly number[] = [0, 2, 5, 10, 20];

/** A length step in words: "5h", or "Any" for 0. */
export function lengthWord(secs: number): string {
  if (secs <= 0) return "Any";
  const h = Math.floor(secs / 3600);
  const m = Math.round((secs % 3600) / 60);
  return h > 0 ? (m > 0 ? `${h}h ${m}m` : `${h}h`) : `${m}m`;
}

/**
 * The Length filter as its bar chip wears it -- "5h+ · 10+ parts" -- or "" while
 * it lets everything through. The parts count only applies while grouped, so it
 * is left unsaid otherwise.
 */
export function lengthLabel(minLength: number, minParts: number, grouped: boolean): string {
  const parts = grouped && minParts > 1 ? `${minParts}+ parts` : "";
  const long = minLength > 0 ? `${lengthWord(minLength)}+` : "";
  return [long, parts].filter(Boolean).join(" · ");
}

export interface FilterState {
  hideWatched: boolean; downloadedOnly: boolean; showHidden: boolean;
  grouped: boolean; inProgress: boolean; autoDownloading: boolean;
  minLength: number; minParts: number;
  channelIds: readonly string[];
}

/**
 * What narrows the list, by the star that would pin it: the five filters,
 * Length, and the channel pick, which is one filter however many channels it
 * holds or leaves out. Grouping and the sort only arrange the list, so they
 * are never in it. The series-only filters, the parts count among them, count
 * only while grouped, which is when they apply.
 */
export function activeFilters(f: FilterState): QuickFilter[] {
  const on: [QuickFilter, boolean][] = [
    ["hide_watched", f.hideWatched],
    ["downloaded_only", f.downloadedOnly],
    ["show_hidden", f.showHidden],
    ["in_progress", f.grouped && f.inProgress],
    ["auto_downloading", f.grouped && f.autoDownloading],
    ["length", f.minLength > 0 || (f.grouped && f.minParts > 1)],
    ["channel_ids", f.channelIds.length > 0],
  ];
  return on.filter(([, v]) => v).map(([id]) => id);
}

export const activeFilterCount = (f: FilterState): number => activeFilters(f).length;

/* ------------------------------------------------------------------ *
 * Saved views
 * ------------------------------------------------------------------ */

/** The filters a fresh library starts on: what Clear and a view's missing keys
 *  fall back to. Grouped, like `ViewState::default`. */
export const DEFAULT_FILTERS: ViewFilters = {
  hide_watched: false, downloaded_only: false, show_hidden: false,
  grouped: true, in_progress: false, auto_downloading: false,
  min_length: 0, min_parts: 0, sort: "newest", channel_ids: [], channels_except: false,
};

const SORT_VALUES = new Set<string>(SORTS.map((s) => s.value));

/**
 * One saved view's filters in their one shape: every key, in this order, with
 * the values the sidebar can show -- what `ViewFilters::sanitize` makes of
 * them too. The shell compares the saved view block as JSON, so a view read
 * back from settings.json must serialise exactly as the one that was saved.
 */
export function normalizeViewFilters(raw: unknown): ViewFilters {
  const f = (raw && typeof raw === "object" ? raw : {}) as Partial<Record<keyof ViewFilters, unknown>>;
  const bool = (v: unknown, d: boolean) => (typeof v === "boolean" ? v : d);
  const count = (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v > 0 ? Math.floor(v) : 0);
  const grouped = bool(f.grouped, true);
  let sort = typeof f.sort === "string" && SORT_VALUES.has(f.sort) ? (f.sort as SortOrder) : "newest";
  if (sort === "parts" && !grouped) sort = "newest";
  const minParts = grouped ? count(f.min_parts) : 0;
  const ids = Array.isArray(f.channel_ids)
    ? [...new Set(f.channel_ids.filter((x): x is string => typeof x === "string" && x !== ""))].sort()
    : [];
  return {
    hide_watched: bool(f.hide_watched, false),
    downloaded_only: bool(f.downloaded_only, false),
    show_hidden: bool(f.show_hidden, false),
    grouped,
    in_progress: grouped && bool(f.in_progress, false),
    auto_downloading: grouped && bool(f.auto_downloading, false),
    min_length: count(f.min_length),
    min_parts: minParts === 1 ? 0 : minParts,
    sort,
    channel_ids: ids,
    channels_except: ids.length > 0 && bool(f.channels_except, false),
  };
}

/** Saved views in their one shape, the unusable ones dropped as
 *  `ViewState::sanitize` drops them: no id, no name, or an id already taken. */
export function normalizeViews(raw: unknown): SavedView[] {
  if (!Array.isArray(raw)) return [];
  const seen = new Set<string>();
  const out: SavedView[] = [];
  for (const item of raw) {
    if (!item || typeof item !== "object") continue;
    const v = item as Partial<Record<keyof SavedView, unknown>>;
    const id = typeof v.id === "string" ? v.id.trim().slice(0, 64) : "";
    const name = typeof v.name === "string" ? v.name.trim().slice(0, 60) : "";
    if (!id || !name || seen.has(id)) continue;
    seen.add(id);
    const search = typeof v.search === "string" ? v.search.trim().slice(0, 200) : "";
    out.push({
      id, name, starred: v.starred === true, filters: normalizeViewFilters(v.filters),
      search: search || null,
    });
  }
  return out.slice(0, 100);
}

/** Whether two sets of filters show the same feed. Both normalised first, so a
 *  parts count while ungrouped (which nothing reads) cannot tell them apart. */
export function sameFilters(a: ViewFilters, b: ViewFilters): boolean {
  return JSON.stringify(normalizeViewFilters(a)) === JSON.stringify(normalizeViewFilters(b));
}

/** A view is showing while the filters are its own -- and, if it was saved with
 *  a search, while the search is that one. */
export function viewIsShowing(view: SavedView, filters: ViewFilters, search: string): boolean {
  return sameFilters(view.filters, filters) && (view.search === null || view.search === search.trim());
}

/** A fresh id for a view: unique on this machine, which is all a view needs. */
export function newViewId(): string {
  return `v${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
}

/**
 * What a view sets, in words: the sidebar row's second line and the save
 * form's tags. `channelsLabel` words the channel pick, which needs the list.
 */
export function describeFilters(f: ViewFilters, channelsLabel: string, search: string | null): string[] {
  const words: string[] = [];
  for (const t of TOGGLES) {
    if (t.id === "grouped") continue;
    if (t.seriesOnly && !f.grouped) continue;
    if (f[t.id]) words.push(t.label);
  }
  words.push(f.grouped ? "Grouped" : "Not grouped");
  if (f.min_length > 0) words.push(`${lengthWord(f.min_length)}+ long`);
  if (f.grouped && f.min_parts > 1) words.push(`${f.min_parts}+ parts`);
  words.push(SORTS.find((s) => s.value === f.sort)?.label ?? "Newest first");
  words.push(channelsLabel);
  if (search) words.push(`“${search}”`);
  return words;
}
