import type { QuickFilter, SortOrder } from "./types";

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
  "grouped", "groups_only", "in_progress", "auto_downloading",
  "sort", "channel_ids",
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

/** The filters that are an on/off switch: everything but the sort and the channel pick. */
export type ToggleId = Exclude<QuickFilter, "sort" | "channel_ids">;

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
    id: "groups_only", label: "Only groups", seriesOnly: true,
    desc: "Series only, no lone videos", hint: "Show only multi-part series, not lone videos",
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

export interface FilterState {
  hideWatched: boolean; downloadedOnly: boolean; showHidden: boolean;
  grouped: boolean; groupsOnly: boolean; inProgress: boolean; autoDownloading: boolean;
  channelIds: readonly string[];
}

/**
 * What narrows the list, by the star that would pin it: the six filters and the
 * channel pick, which is one filter however many channels it holds. Grouping
 * and the sort only arrange the list, so they are never in it. The three
 * series-only filters count only while grouped, which is when they apply.
 */
export function activeFilters(f: FilterState): QuickFilter[] {
  const on: [QuickFilter, boolean][] = [
    ["hide_watched", f.hideWatched],
    ["downloaded_only", f.downloadedOnly],
    ["show_hidden", f.showHidden],
    ["groups_only", f.grouped && f.groupsOnly],
    ["in_progress", f.grouped && f.inProgress],
    ["auto_downloading", f.grouped && f.autoDownloading],
    ["channel_ids", f.channelIds.length > 0],
  ];
  return on.filter(([, v]) => v).map(([id]) => id);
}

export const activeFilterCount = (f: FilterState): number => activeFilters(f).length;
