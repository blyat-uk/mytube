import { describe, it, expect } from "vitest";
import {
  lastLevel, need, pickLevel, planAt, room, SEARCH_FOLDED, SEARCH_MIN, SLACK, type Sizes,
} from "./toolbarFit";

/** Three starred filters of 100px spelled out, 40px as glyphs. */
function sizes(tools: number, over: Partial<Sizes> = {}): Sizes {
  return {
    tools, addNow: 30, addFull: 30, allBase: 40, allWords: 60, gap: 8,
    items: [{ full: 100, glyph: 40 }, { full: 100, glyph: 40 }, { full: 100, glyph: 40 }],
    ...over,
  };
}

describe("planAt", () => {
  it("steps from everything spelled out to glyphs, then folds the last filters first", () => {
    expect(planAt(0, 3)).toEqual({ words: true, shown: 3, folded: false, tight: false });
    expect(planAt(1, 3)).toEqual({ words: false, shown: 3, folded: false, tight: false });
    expect(planAt(2, 3)).toEqual({ words: false, shown: 2, folded: false, tight: false });
    expect(planAt(4, 3)).toEqual({ words: false, shown: 0, folded: false, tight: false });
  });

  it("folds search only once every starred filter has gone, and the words of All filters and Add last", () => {
    expect(planAt(5, 3)).toEqual({ words: false, shown: 0, folded: true, tight: false });
    expect(planAt(lastLevel(3), 3)).toEqual({ words: false, shown: 0, folded: true, tight: true });
  });
});

describe("need", () => {
  it("adds up search, the starred filters with their spacing, and All filters", () => {
    // search squeezed + gap + 3 × 100 + 2 × 6 + gap + All filters with its words
    expect(need(sizes(0), 0)).toBe(SEARCH_MIN + 8 + 300 + 12 + 8 + 100);
    expect(need(sizes(0), 1)).toBe(SEARCH_MIN + 8 + 120 + 12 + 8 + 100);
  });

  it("keeps the starred group's gap with every filter folded, since the group is still there", () => {
    expect(need(sizes(0), 4)).toBe(SEARCH_MIN + 8 + 8 + 100);
    expect(need(sizes(0), 5)).toBe(SEARCH_FOLDED + 8 + 8 + 100);
    expect(need(sizes(0), 6)).toBe(SEARCH_FOLDED + 8 + 8 + 40);
  });

  it("has no starred group to space when nothing is starred", () => {
    expect(need(sizes(0, { items: [] }), 0)).toBe(SEARCH_MIN + 8 + 100);
  });
});

describe("room", () => {
  it("is the tools' width, plus what Add's words hand over at the last step", () => {
    expect(room(sizes(500), 0)).toBe(500);
    expect(room(sizes(500), lastLevel(3))).toBe(530);
    // Already folded, Add's words take no room now and would take 30 back.
    expect(room(sizes(530, { addNow: 0 }), 0)).toBe(500);
  });
});

describe("pickLevel", () => {
  const full = need(sizes(0), 0);

  it("spells everything out when there is room for it", () => {
    expect(pickLevel(sizes(full), 0)).toBe(0);
    expect(pickLevel(sizes(full + 200), 0)).toBe(0);
  });

  it("gives up the words before folding any starred filter away", () => {
    expect(pickLevel(sizes(full - 1), 0)).toBe(1);
  });

  it("folds the starred filters from the last until the rest fit", () => {
    expect(pickLevel(sizes(need(sizes(0), 2)), 0)).toBe(2);
    expect(pickLevel(sizes(need(sizes(0), 4)), 0)).toBe(4);
  });

  it("folds search, then the last words, when even that is too much", () => {
    expect(pickLevel(sizes(need(sizes(0), 5)), 0)).toBe(5);
    expect(pickLevel(sizes(10), 0)).toBe(lastLevel(3));
  });

  it("steps back up only with headroom to spare", () => {
    expect(pickLevel(sizes(full), 1)).toBe(1);
    expect(pickLevel(sizes(full + SLACK), 1)).toBe(0);
    // Stepping down, or staying put, needs none.
    expect(pickLevel(sizes(need(sizes(0), 2)), 2)).toBe(2);
  });

  it("jumps straight to the roomiest step that fits", () => {
    expect(pickLevel(sizes(full + SLACK), lastLevel(3))).toBe(0);
  });
});
