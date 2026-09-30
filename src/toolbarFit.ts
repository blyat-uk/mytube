import { useLayoutEffect, useRef, useState, type RefObject } from "react";

/**
 * How much of the filter row the window has room for.
 *
 * - `labels`: every control spelled out.
 * - `glyphs`: the filter segment shows its icons instead of its words.
 * - `folded`: search, the filters, Grouped, the sort and the channel picker
 *   all fold behind a magnifier and one "Filters" button.
 *
 * Decided by measuring the row rather than by fixed breakpoints: what the full
 * row needs moves with grouping (two more filters), the channel's name, the
 * version pill and an open series' breadcrumb, and a breakpoint sized for the
 * worst of those folds far too early for everything else — or, sized for the
 * common case, lets the row spill out of the window, which is what it did.
 */
export type Fit = "labels" | "glyphs" | "folded";

export interface FitSizes {
  /** Width left for the filter controls once everything else has its share. */
  available: number;
  /** What the full controls need with the segment spelled out… */
  labels: number;
  /** …and with it as icons. */
  glyphs: number;
}

const RANK: Record<Fit, number> = { labels: 0, glyphs: 1, folded: 2 };

/**
 * Headroom a roomier state must have before the row steps back up to it. Without
 * it a window resting on the boundary would flip between two states on every
 * pixel of a drag, and each flip is an animation.
 */
export const SLACK = 16;

/** The roomiest state that fits; a state roomier than `current` needs `SLACK` spare. */
export function pickFit(s: FitSizes, current: Fit): Fit {
  for (const [fit, need] of [["labels", s.labels], ["glyphs", s.glyphs]] as const) {
    const headroom = RANK[fit] < RANK[current] ? SLACK : 0;
    if (s.available >= need + headroom) return fit;
  }
  return "folded";
}

const sum = (els: Iterable<Element>, w: (el: HTMLElement) => number) =>
  Array.from(els).reduce((n, el) => n + w(el as HTMLElement), 0);

/**
 * Reads the row. Every collapsible piece stays in the layout at zero width
 * (that is what lets it animate), so its natural width is still there to read
 * as `scrollWidth` while its current, possibly mid-animation, width is
 * `offsetWidth`. Subtracting the one and adding the other gives what each
 * state needs, whatever state the row is in or on its way to.
 */
export function measureFit(row: HTMLElement, full: HTMLElement, compact: HTMLElement | null): FitSizes | null {
  const inner = full.firstElementChild as HTMLElement | null;
  // No layout to read (not on screen yet, or jsdom): nothing to decide on.
  if (!inner || row.clientWidth === 0) return null;
  const cs = getComputedStyle(row);
  const pad = (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0);
  const gap = parseFloat(cs.columnGap) || 0;
  const kids = Array.from(row.children) as HTMLElement[];
  // Both clusters are always children -- one of them at zero width -- so the
  // number of gaps never changes with the state.
  const others = kids.filter((k) => k !== full && k !== compact && !k.classList.contains("topnav-spacer"));
  const fixed = pad + gap * Math.max(0, kids.length - 1) + sum(others, (el) => el.offsetWidth);

  const labels = inner.querySelectorAll(".seg-label");
  const glyphs = inner.querySelectorAll(".seg-glyph");
  const base = inner.offsetWidth - sum(labels, (el) => el.offsetWidth) - sum(glyphs, (el) => el.offsetWidth);
  // `offsetWidth` rounds each piece to a whole pixel, so the sum can come up a
  // pixel or two short of what the row really lays out.
  const rounding = 4;
  return {
    available: row.clientWidth - fixed - rounding,
    labels: base + sum(labels, (el) => el.scrollWidth),
    glyphs: base + sum(glyphs, (el) => el.scrollWidth),
  };
}

/**
 * Slides a cluster between zero and its natural width. `auto` cannot be
 * animated, so the move is pinned in pixels at both ends and handed back to
 * `auto` once it lands — after which the cluster follows its own content
 * (a label shrinking, the search box opening) without any help.
 */
export function slideWidth(el: HTMLElement, show: boolean) {
  const inner = el.firstElementChild as HTMLElement | null;
  const from = el.getBoundingClientRect().width;
  const to = show && inner ? inner.offsetWidth : 0;
  const token = String(Math.random());
  el.dataset.slide = token;
  const release = () => {
    // A later slide owns the element now; leave its pinned width alone.
    if (el.dataset.slide === token && show) el.style.width = "";
  };
  // Nothing to animate -- and a transition that does not run never ends, so
  // pinning the width here would pin it for good.
  if (Math.abs(from - to) < 0.5) {
    el.style.width = show ? "" : "0px";
    return;
  }
  el.style.width = `${from}px`;
  void el.offsetWidth; // commit the starting width, or there is nothing to animate from
  el.style.width = `${to}px`;
  if (!show) return;
  const settle = (e: TransitionEvent) => {
    if (e.target !== el || e.propertyName !== "width") return;
    el.removeEventListener("transitionend", settle);
    release();
  };
  el.addEventListener("transitionend", settle);
  // An interrupted transition fires no `transitionend` either.
  window.setTimeout(() => {
    el.removeEventListener("transitionend", settle);
    release();
  }, SLIDE_MS + 100);
}

/** Matches `--fold-time` in App.css. */
const SLIDE_MS = 260;

interface Refs {
  row: RefObject<HTMLElement | null>;
  full: RefObject<HTMLElement | null>;
  compact: RefObject<HTMLElement | null>;
}

/**
 * The row's current `Fit`, re-measured whenever the row or anything in it
 * changes size. `active` is false on the tabs with no filter row. `layout`
 * names whatever else changes the row's make-up (the breadcrumb replacing the
 * tabs), so the observer is re-attached to the new children.
 */
export function useToolbarFit(refs: Refs, active: boolean, layout: string): Fit {
  const [fit, setFit] = useState<Fit>("labels");
  const current = useRef<Fit>("labels");
  const placed = useRef(false);

  useLayoutEffect(() => {
    if (!active) return;
    const row = refs.row.current;
    const full = refs.full.current;
    if (!row || !full) return;
    const update = () => {
      const s = measureFit(row, full, refs.compact.current);
      if (!s) return;
      const next = pickFit(s, current.current);
      if (next === current.current) return;
      current.current = next;
      setFit(next);
    };
    update();
    if (typeof ResizeObserver === "undefined") return;
    const ro = new ResizeObserver(update);
    ro.observe(row);
    for (const kid of Array.from(row.children)) {
      ro.observe(kid);
      if (kid.firstElementChild) ro.observe(kid.firstElementChild);
    }
    return () => ro.disconnect();
  }, [active, layout, refs.row, refs.full, refs.compact]);

  // Folding and unfolding slide the two clusters past each other. The very
  // first placement is not a change anybody watched happen, so it just lands.
  useLayoutEffect(() => {
    const full = refs.full.current;
    const compact = refs.compact.current;
    if (!full || !compact) {
      // Unmounted (a tab with no filter row): whatever mounts next is placed
      // afresh rather than slid in from nowhere.
      placed.current = false;
      return;
    }
    const folded = fit === "folded";
    if (!placed.current) {
      placed.current = true;
      full.style.width = folded ? "0px" : "";
      compact.style.width = folded ? "" : "0px";
      return;
    }
    slideWidth(full, !folded);
    slideWidth(compact, folded);
  }, [fit, active, refs.full, refs.compact]);

  return active ? fit : "labels";
}
