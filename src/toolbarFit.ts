import { useLayoutEffect, useRef, useState, type RefObject } from "react";

/**
 * How much of the filter tools the top row has room for.
 *
 * The logo and the three tabs never change: they are where you are, and a row
 * whose sections fold into icons or turn into a breadcrumb is a row you have
 * to read again. Everything to their right gives way instead, one step at a
 * time, and only the user's own starred filters ever leave the row:
 *
 * 1. every starred filter spelled out, the search box giving up some of its
 *    width first (down to `SEARCH_MIN`) to keep them;
 * 2. the starred toggles as their glyphs (the sort and the channel pick wear
 *    their values, so they have no words to give up);
 * 3. the starred filters folding back into All filters, last first, until the
 *    rest fit -- the badge on All filters then counts any that were on;
 * 4. search behind a magnifier, every starred filter folded;
 * 5. All filters and Add as their glyphs too.
 *
 * Decided by measuring rather than by breakpoints, for the reason the row
 * always has been: what it needs moves with the stars, grouping (three more
 * filters), the first picked channel's name, the update chip and the text
 * size, so any fixed width is wrong for most of them.
 */
export interface Fit {
  /** The starred toggles wear their words; false, their glyphs alone. */
  words: boolean;
  /** How many starred filters, counted from the first, are in the row. */
  shown: number;
  /** Search is behind its magnifier. */
  folded: boolean;
  /** All filters and Add give up their words too. */
  tight: boolean;
}

/** The steps for `n` starred filters, roomiest first: 0 is everything spelled
 *  out, `lastLevel(n)` the tightest. */
export function planAt(level: number, n: number): Fit {
  if (level <= 0) return { words: true, shown: n, folded: false, tight: false };
  if (level <= n + 1) return { words: false, shown: n - (level - 1), folded: false, tight: false };
  return { words: false, shown: 0, folded: true, tight: level >= n + 3 };
}

export const lastLevel = (n: number) => n + 3;

/** The search box at its widest, the narrowest it is squeezed to before the
 *  starred filters start giving way, and its magnifier -- as App.css draws
 *  them. Between the first two the box simply shrinks (flex) to what is left. */
export const SEARCH_FULL = 240;
export const SEARCH_MIN = 160;
export const SEARCH_FOLDED = 32;
/** Between two starred filters: a margin on each slot after the first, so a
 *  folded slot (zero width, at the end) takes its spacing with it. */
export const QUICK_GAP = 6;

/** One starred filter: as wide as it is with its words, and without. */
export interface ItemSize { full: number; glyph: number }

export interface Sizes {
  /** The room the tools have now. They take whatever the row leaves. */
  tools: number;
  /** Add's words as wide as they are now, and spelled out -- the room they
   *  hand the tools at the last step. */
  addNow: number;
  addFull: number;
  /** All filters without its words, and the words. */
  allBase: number;
  allWords: number;
  /** The starred filters in the row, in order. */
  items: ItemSize[];
  /** Between the tools' parts: search, the starred group, All filters. */
  gap: number;
}

/** What the tools need at `level`. The starred group is a child of the row
 *  whatever it holds, so its gap counts even with every filter folded. */
export function need(s: Sizes, level: number): number {
  const n = s.items.length;
  const f = planAt(level, n);
  let w = f.folded ? SEARCH_FOLDED : SEARCH_MIN;
  if (n > 0) {
    w += s.gap;
    for (let i = 0; i < f.shown; i++) {
      w += (f.words ? s.items[i].full : s.items[i].glyph) + (i > 0 ? QUICK_GAP : 0);
    }
  }
  return w + s.gap + s.allBase + (f.tight ? 0 : s.allWords);
}

/** The room the tools would have at `level`: Add folding its words at the
 *  last step hands them that much more. */
export function room(s: Sizes, level: number): number {
  return s.tools + s.addNow - (planAt(level, s.items.length).tight ? 0 : s.addFull);
}

/**
 * Headroom a roomier step must have before the row steps back up to it.
 * Without it a window resting on a boundary would flip between two steps on
 * every pixel of a drag, and each flip is an animation.
 */
export const SLACK = 16;

/** The roomiest step that fits; one roomier than `current` needs `SLACK` spare. */
export function pickLevel(s: Sizes, current: number): number {
  const last = lastLevel(s.items.length);
  for (let level = 0; level < last; level++) {
    const headroom = level < current ? SLACK : 0;
    if (room(s, level) >= need(s, level) + headroom) return level;
  }
  return last;
}

const width = (el: Element | null | undefined) => (el as HTMLElement | null | undefined)?.offsetWidth ?? 0;

/**
 * Reads the row. Every word that comes and goes is a `.fold-label`, kept in
 * the layout at zero width rather than `display: none` -- that is what lets it
 * animate -- so its natural width is still there as `scrollWidth` while its
 * current one is `offsetWidth`. A folded filter's slot is zero wide too, but
 * the filter inside it keeps its own width, so it can be read whatever step
 * the row is at or on its way to.
 */
export function measure(tools: HTMLElement, add: HTMLElement | null): Sizes | null {
  // No layout to read (not on screen yet, or jsdom): nothing to decide on.
  if (tools.clientWidth === 0) return null;
  const words = (el: Element | null | undefined) => el?.querySelector<HTMLElement>(".fold-label") ?? null;
  const items = Array.from(tools.querySelectorAll(".quick-slot")).map((slot) => {
    const el = slot.firstElementChild;
    const label = words(el);
    const glyph = width(el) - width(label);
    return { full: glyph + (label?.scrollWidth ?? 0), glyph };
  });
  const all = tools.querySelector(".all-filters");
  const allLabel = words(all);
  const addLabel = words(add);
  const cs = getComputedStyle(tools);
  const pad = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
  // `offsetWidth` rounds each piece to a whole pixel, so the sum can come up a
  // pixel or two short of what the row really lays out.
  const rounding = 4;
  return {
    tools: tools.clientWidth - pad - rounding,
    addNow: width(addLabel),
    addFull: addLabel?.scrollWidth ?? 0,
    allBase: width(all) - width(allLabel),
    allWords: allLabel?.scrollWidth ?? 0,
    items,
    gap: parseFloat(cs.columnGap) || 0,
  };
}

/**
 * The row's current `Fit` for `count` starred filters, re-measured whenever
 * the row or anything in it changes size. `layout` names whatever changes
 * which filters are in the row (a star, grouping), so the observer is
 * re-attached to the new ones. Unmeasured -- a tab with no tools, or jsdom --
 * everything is spelled out.
 */
export function useToolbarFit(
  tools: RefObject<HTMLElement | null>,
  add: RefObject<HTMLElement | null>,
  count: number,
  layout: string,
): Fit {
  const [level, setLevel] = useState(0);
  const current = useRef(0);

  useLayoutEffect(() => {
    const el = tools.current;
    if (!el) return;
    const update = () => {
      const s = measure(el, add.current);
      if (!s) return;
      const next = pickLevel(s, current.current);
      if (next === current.current) return;
      current.current = next;
      setLevel(next);
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(el);
    if (add.current) ro.observe(add.current);
    // A filter that changes its own width (a channel picked, the sort
    // changed, the text size) leaves the tools as wide as they were.
    for (const k of Array.from(el.querySelectorAll(".quick-slot > *, .all-filters"))) ro.observe(k);
    return () => ro.disconnect();
  }, [tools, add, layout]);

  return planAt(level, count);
}
