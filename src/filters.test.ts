import { describe, it, expect } from "vitest";
import {
  activeFilterCount, activeFilters, DEFAULT_FILTERS, DEFAULT_STARRED, describeFilters, lengthLabel,
  lengthWord, normalizeStarred, normalizeViewFilters, normalizeViews, QUICK_ORDER, sameFilters,
  sortsFor, toggleStar, TOGGLES, viewIsShowing,
} from "./filters";
import type { SavedView } from "./types";

const NONE = {
  hideWatched: false, downloadedOnly: false, showHidden: false,
  grouped: false, inProgress: false, autoDownloading: false,
  minLength: 0, minParts: 0, channelIds: [] as string[],
};

describe("normalizeStarred", () => {
  it("keeps known ids once each, in the bar's order", () => {
    expect(normalizeStarred(["sort", "nonsense", "hide_watched", "sort"])).toEqual(["hide_watched", "sort"]);
  });

  it("keeps an empty list, which is a bar with no filters in it", () => {
    expect(normalizeStarred([])).toEqual([]);
  });

  it("reads a view with no stars at all as the default three", () => {
    expect(normalizeStarred(undefined)).toEqual(["hide_watched", "grouped", "sort"]);
    expect(normalizeStarred(undefined)).toEqual([...DEFAULT_STARRED]);
  });
});

describe("toggleStar", () => {
  it("adds a star in its place in the order, whatever order the stars came in", () => {
    expect(toggleStar(["sort"], "hide_watched")).toEqual(["hide_watched", "sort"]);
    expect(toggleStar(["hide_watched"], "channel_ids")).toEqual(["hide_watched", "channel_ids"]);
  });

  it("takes a star away", () => {
    expect(toggleStar(["hide_watched", "sort"], "hide_watched")).toEqual(["sort"]);
  });
});

describe("the filter list", () => {
  it("orders the toggles as QUICK_ORDER does, length, the sort and channels after them", () => {
    expect(TOGGLES.map((t) => t.id)).toEqual(QUICK_ORDER.slice(0, TOGGLES.length));
    expect(QUICK_ORDER.slice(TOGGLES.length)).toEqual(["length", "sort", "channel_ids"]);
  });

  it("marks exactly the switches that need grouping", () => {
    expect(TOGGLES.filter((t) => t.seriesOnly).map((t) => t.id))
      .toEqual(["in_progress", "auto_downloading"]);
  });

  it("offers Most parts first only while grouped", () => {
    expect(sortsFor(false).map((s) => s.value)).not.toContain("parts");
    expect(sortsFor(true).map((s) => s.value)).toContain("parts");
  });
});

describe("activeFilters", () => {
  it("counts what narrows the list, and the channel pick as one however many it holds", () => {
    expect(activeFilters({ ...NONE, hideWatched: true, channelIds: ["UC1", "UC2"] }))
      .toEqual(["hide_watched", "channel_ids"]);
    expect(activeFilterCount({ ...NONE, downloadedOnly: true, showHidden: true })).toBe(2);
  });

  it("does not count grouping, which only arranges the list", () => {
    expect(activeFilterCount({ ...NONE, grouped: true })).toBe(0);
  });

  it("counts the series-only filters only while grouped, when they apply", () => {
    const series = { ...NONE, minParts: 5, inProgress: true, autoDownloading: true };
    expect(activeFilterCount(series)).toBe(0);
    expect(activeFilters({ ...series, grouped: true }))
      .toEqual(["in_progress", "auto_downloading", "length"]);
  });

  it("counts a length as one filter, the parts count with it", () => {
    expect(activeFilters({ ...NONE, minLength: 3600 })).toEqual(["length"]);
    expect(activeFilters({ ...NONE, grouped: true, minLength: 3600, minParts: 5 })).toEqual(["length"]);
  });
});

describe("the Length filter in words", () => {
  it("words a step, and the chip's value", () => {
    expect(lengthWord(0)).toBe("Any");
    expect(lengthWord(18000)).toBe("5h");
    expect(lengthWord(5400)).toBe("1h 30m");
    expect(lengthLabel(18000, 10, true)).toBe("5h+ · 10+ parts");
    expect(lengthLabel(0, 2, true)).toBe("2+ parts");
    // The parts count means nothing ungrouped, so it is left unsaid.
    expect(lengthLabel(3600, 10, false)).toBe("1h+");
    expect(lengthLabel(0, 0, true)).toBe("");
  });
});

describe("saved views", () => {
  const view = (over: Partial<SavedView> = {}): SavedView => ({
    id: "v1", name: "Long shows", starred: false,
    filters: { ...DEFAULT_FILTERS, min_length: 18000, min_parts: 5 }, search: null, ...over,
  });

  it("puts any set of filters in one shape, as the backend does", () => {
    expect(normalizeViewFilters({})).toEqual(DEFAULT_FILTERS);
    const f = normalizeViewFilters({
      grouped: false, min_parts: 5, in_progress: true, sort: "parts", sort_extra: 1,
      channel_ids: ["UC2", "UC1", "UC2", ""], channels_except: true, min_length: -4,
    });
    expect(f).toEqual({
      ...DEFAULT_FILTERS, grouped: false, sort: "newest", channel_ids: ["UC1", "UC2"],
      channels_except: true,
    });
    // Leaving nothing out is every channel.
    expect(normalizeViewFilters({ channels_except: true }).channels_except).toBe(false);
    // The keys always come in one order, which the view writer compares.
    expect(Object.keys(normalizeViewFilters({ sort: "oldest", hide_watched: true })))
      .toEqual(Object.keys(DEFAULT_FILTERS));
  });

  it("drops a view with no id, no name, or an id already taken", () => {
    const views = normalizeViews([
      { id: "a", name: "  First ", filters: {}, search: "  " },
      { id: "", name: "No id" }, { id: "b", name: "" }, { id: "a", name: "Again" },
      { id: "c", name: "Searching", search: " tapes " }, "junk",
    ]);
    expect(views.map((v) => [v.id, v.name, v.search])).toEqual([["a", "First", null], ["c", "Searching", "tapes"]]);
    expect(normalizeViews(undefined)).toEqual([]);
  });

  it("is showing while the filters are its own, and the search too if it has one", () => {
    const v = view();
    const same = { ...DEFAULT_FILTERS, min_parts: 5, min_length: 18000 };
    expect(viewIsShowing(v, same, "anything")).toBe(true);
    expect(viewIsShowing(v, { ...same, hide_watched: true }, "")).toBe(false);
    const searching = view({ search: "tapes" });
    expect(viewIsShowing(searching, same, " tapes ")).toBe(true);
    expect(viewIsShowing(searching, same, "")).toBe(false);
    // A parts count nothing reads (ungrouped) does not tell two apart.
    expect(sameFilters({ ...DEFAULT_FILTERS, grouped: false, min_parts: 5 }, { ...DEFAULT_FILTERS, grouped: false }))
      .toBe(true);
  });

  it("says what a view sets, in words", () => {
    expect(describeFilters({ ...DEFAULT_FILTERS, hide_watched: true, min_length: 18000, min_parts: 5, sort: "length" },
      "All channels", "tapes"))
      .toEqual(["Unwatched", "Grouped", "5h+ long", "5+ parts", "Longest first", "All channels", "“tapes”"]);
  });
});
