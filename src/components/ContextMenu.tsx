import { Fragment, useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";

export interface MenuItem {
  label: string;
  onSelect: () => void;
  /** Can't be undone. Red is kept for that alone, so it keeps meaning it. */
  danger?: boolean;
  disabled?: boolean;
  /** The glyph the card and the Downloads rows use for the same action. */
  icon?: ReactNode;
  /** Starts a new group: a rule is drawn above it. */
  divider?: boolean;
}

/**
 * One menu out of groups of items, a rule between each two that have anything
 * in them. Menus group by intent -- act on the video, go somewhere or
 * organise, remove -- so removal always sits behind a line, out of reach of a
 * slip off the item above it.
 */
export function menuGroups(...groups: MenuItem[][]): MenuItem[] {
  return groups
    .filter((g) => g.length > 0)
    .flatMap((g, i) => g.map((item, j) => (i > 0 && j === 0 ? { ...item, divider: true } : item)));
}

interface Props {
  x: number;
  y: number;
  items: MenuItem[];
  onClose: () => void;
}

/**
 * A small right-click menu positioned at the pointer, nudged back inside the
 * viewport if it would overflow. Closes on Escape, on any outside click, and on
 * scroll — a menu left floating over a grid that has moved underneath it is
 * worse than no menu.
 */
export default function ContextMenu({ x, y, items, onClose }: Props) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState({ x, y });

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    const pad = 8;
    setPos({
      x: Math.min(x, window.innerWidth - r.width - pad),
      y: Math.min(y, window.innerHeight - r.height - pad),
    });
  }, [x, y]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("mousedown", onDown);
    window.addEventListener("scroll", onClose, true);
    window.addEventListener("resize", onClose);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("scroll", onClose, true);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  return (
    <div
      ref={ref}
      className="context-menu"
      role="menu"
      style={{ left: pos.x, top: pos.y }}
      onContextMenu={(e) => e.preventDefault()}
    >
      {items.map((item, i) => (
        <Fragment key={i}>
          {item.divider && i > 0 && <div className="context-sep" role="separator" />}
          <button
            type="button"
            role="menuitem"
            className={`context-item${item.danger ? " is-danger" : ""}`}
            disabled={item.disabled}
            onClick={() => {
              onClose();
              item.onSelect();
            }}
          >
            {item.icon && <span className="context-icon" aria-hidden="true">{item.icon}</span>}
            {item.label}
          </button>
        </Fragment>
      ))}
    </div>
  );
}
