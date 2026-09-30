import { describe, it, expect } from "vitest";
import { pickFit, SLACK } from "./toolbarFit";
import { activeFilterCount } from "./components/FiltersPanel";

const sizes = (available: number) => ({ available, labels: 900, glyphs: 700 });

describe("pickFit", () => {
  it("spells everything out when there is room for it", () => {
    expect(pickFit(sizes(1000), "labels")).toBe("labels");
    expect(pickFit(sizes(900), "labels")).toBe("labels");
  });

  it("gives up the segment's words before folding anything", () => {
    expect(pickFit(sizes(899), "labels")).toBe("glyphs");
    expect(pickFit(sizes(700), "labels")).toBe("glyphs");
  });

  it("folds once even the glyphs do not fit", () => {
    expect(pickFit(sizes(699), "glyphs")).toBe("folded");
    expect(pickFit(sizes(0), "labels")).toBe("folded");
  });

  // A window resting on a boundary must not flip on every pixel of a drag:
  // each flip is an animation.
  it("steps back up only with headroom to spare", () => {
    expect(pickFit(sizes(700), "folded")).toBe("folded");
    expect(pickFit(sizes(700 + SLACK), "folded")).toBe("glyphs");
    expect(pickFit(sizes(900), "glyphs")).toBe("glyphs");
    expect(pickFit(sizes(900 + SLACK), "glyphs")).toBe("labels");
  });

  it("jumps straight to the roomiest state that fits", () => {
    expect(pickFit(sizes(2000), "folded")).toBe("labels");
    expect(pickFit(sizes(10), "labels")).toBe("folded");
  });
});

describe("activeFilterCount", () => {
  const none = {
    hideWatched: false, downloadedOnly: false, showHidden: false,
    grouped: false, groupsOnly: false, inProgress: false, channelId: null,
  };

  it("counts what narrows the list, and a chosen channel", () => {
    expect(activeFilterCount(none)).toBe(0);
    expect(activeFilterCount({ ...none, hideWatched: true, downloadedOnly: true, channelId: "UC1" })).toBe(3);
  });

  it("does not count grouping, which only arranges the list", () => {
    expect(activeFilterCount({ ...none, grouped: true })).toBe(0);
  });

  it("counts the series-only filters only while grouped, when they apply", () => {
    expect(activeFilterCount({ ...none, groupsOnly: true, inProgress: true })).toBe(0);
    expect(activeFilterCount({ ...none, grouped: true, groupsOnly: true, inProgress: true })).toBe(2);
  });
});
