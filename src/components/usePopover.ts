import { useEffect, useLayoutEffect, useState, type RefObject } from "react";

/**
 * A `.topnav-pop` hung from the button that opened it: returns its `right`
 * offset, and closes it on a click outside it or Esc.
 *
 * Hung from the button's right edge, measured against the nav it sits in --
 * the popover lives outside the button's cluster, which clips while it slides.
 */
export function usePopover(
  ref: RefObject<HTMLElement | null>,
  anchor: RefObject<HTMLElement | null>,
  onClose: (refocus: boolean) => void,
): number {
  const [right, setRight] = useState(0);

  useLayoutEffect(() => {
    const place = () => {
      const btn = anchor.current;
      const host = ref.current?.offsetParent as HTMLElement | null;
      if (!btn || !host) return;
      setRight(Math.max(8, host.getBoundingClientRect().right - btn.getBoundingClientRect().right));
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [ref, anchor]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (ref.current?.contains(t) || anchor.current?.contains(t)) return;
      onClose(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Esc also leaves an open series; closing the popover is all this one means.
      e.stopPropagation();
      onClose(true);
    };
    document.addEventListener("mousedown", onDown);
    // Capture, so it is heard before the shell's series handler on window.
    window.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [ref, anchor, onClose]);

  return right;
}
