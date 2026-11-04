import { describe, it, expect } from "vitest";
import {
  activeFilterCount, activeFilters, DEFAULT_STARRED, normalizeStarred, QUICK_ORDER, sortsFor,
  toggleStar, TOGGLES,
} from "./filters";

const NONE = {
  hideWatched: false, downloadedOnly: false, showHidden: false,
  grouped: false, groupsOnly: false, inProgress: false, autoDownloading: false,
  channelIds: [] as string[],
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
  it("orders the toggles as QUICK_ORDER does, the sort and channels after them", () => {
    expect(TOGGLES.map((t) => t.id)).toEqual(QUICK_ORDER.slice(0, TOGGLES.length));
    expect(QUICK_ORDER.slice(TOGGLES.length)).toEqual(["sort", "channel_ids"]);
  });

  it("marks exactly the three that need grouping", () => {
    expect(TOGGLES.filter((t) => t.seriesOnly).map((t) => t.id))
      .toEqual(["groups_only", "in_progress", "auto_downloading"]);
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
    const series = { ...NONE, groupsOnly: true, inProgress: true, autoDownloading: true };
    expect(activeFilterCount(series)).toBe(0);
    expect(activeFilters({ ...series, grouped: true }))
      .toEqual(["groups_only", "in_progress", "auto_downloading"]);
  });
});
