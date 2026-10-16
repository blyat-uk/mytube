import { useLayoutEffect, useRef } from "react";

/**
 * Lets a box follow its content's height, sliding when the content is swapped
 * and following at once when it only grows or shrinks in place.
 *
 * CSS cannot transition `height: auto`, and the Add dialog's preview zone
 * changes height every time a paste is read: a one-line hint becomes a
 * skeleton card, the skeleton becomes the real card, a video becomes a
 * playlist. So the outer box is given the inner one's height in pixels,
 * measured by a ResizeObserver, and `is-sliding` (a height transition with
 * the overflow clipped) is switched on only for the measurement that follows
 * a change of `key`. Everything else -- the parts list unfolding inside a
 * playlist card, which animates itself, a window resize, a text-size change --
 * is followed frame by frame with no transition, because one transition
 * chasing another lags behind it and clips the content it is meant to show.
 *
 * Without a ResizeObserver (jsdom) the box is simply `auto`.
 */
export function useAutoHeight(key: string) {
  const outer = useRef<HTMLDivElement | null>(null);
  const inner = useRef<HTMLDivElement | null>(null);
  const slideNext = useRef(false);
  const first = useRef(true);

  // A layout effect runs in the commit, before the browser lays out and
  // calls the observer, so the flag is up by the time the new height is read.
  useLayoutEffect(() => {
    if (first.current) { first.current = false; return; }
    slideNext.current = true;
  }, [key]);

  useLayoutEffect(() => {
    const o = outer.current;
    const i = inner.current;
    if (!o || !i || typeof ResizeObserver === "undefined") return;
    let height = i.offsetHeight;
    o.style.height = `${height}px`;
    let settle: number | undefined;
    const done = () => {
      window.clearTimeout(settle);
      o.classList.remove("is-sliding");
    };
    // Transitions inside (a button's hover, the parts list unfolding) bubble
    // their `transitionend` up here too; only the box's own height ends it.
    const ended = (e: TransitionEvent) => {
      if (e.target === o && e.propertyName === "height") done();
    };
    const ro = new ResizeObserver(() => {
      const next = i.offsetHeight;
      if (next === height) return;
      if (slideNext.current) {
        slideNext.current = false;
        o.classList.add("is-sliding");
        // A transition that never runs (reduced motion, a hidden window)
        // never fires `transitionend`, so a timer takes the clip off too.
        window.clearTimeout(settle);
        settle = window.setTimeout(done, 400);
      }
      o.style.height = `${next}px`;
      height = next;
    });
    o.addEventListener("transitionend", ended);
    ro.observe(i);
    return () => {
      ro.disconnect();
      o.removeEventListener("transitionend", ended);
      window.clearTimeout(settle);
    };
  }, []);

  return { outer, inner };
}
