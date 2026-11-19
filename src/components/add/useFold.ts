import { useLayoutEffect, useRef } from "react";

/** `--fold-time`: how long a section takes to fold. */
const FOLD_TIME = 260;

/**
 * One of the Add dialog's folding sections (Subscriptions, Recommendations).
 *
 * The fold slides between two measured heights: what the body stood at when
 * it was folded (read as the button is pressed, before the class changes),
 * and what it settles at once it is out (read after, with the dialog's own
 * limit already applied). Folded, CSS holds it at 0; the animation only
 * carries it between the two, so nothing is left behind on the element.
 * `grid-template-rows: 0fr <-> 1fr` would interpolate against the whole
 * list's height and reach the dialog's limit in the first few frames.
 */
export function useFold(folded: boolean, onFold: (folded: boolean) => void) {
  const body = useRef<HTMLDivElement | null>(null);
  const foldFrom = useRef<number | null>(null);
  const first = useRef(true);

  useLayoutEffect(() => {
    const el = body.current;
    const measured = foldFrom.current;
    foldFrom.current = null;
    if (first.current) { first.current = false; return; }
    // Unfolded from elsewhere ("Show in list") starts from nothing.
    const from = measured ?? (folded ? null : 0);
    if (!el || from === null || typeof el.animate !== "function") return;
    if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return;
    const to = el.offsetHeight;
    if (from === to) return;
    el.animate([{ height: `${from}px` }, { height: `${to}px` }], {
      duration: FOLD_TIME, easing: "cubic-bezier(0.2, 0.7, 0.2, 1)",
    });
  }, [folded]);

  function toggle() {
    foldFrom.current = body.current?.offsetHeight ?? null;
    onFold(!folded);
  }

  return { body, toggle };
}
