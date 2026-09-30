import { useEffect, useLayoutEffect, useRef, useState, type ReactElement, type RefObject } from "react";
import {
  IconContinue, IconDownload, IconGroupsOnly, IconHidden, IconSeries, IconUnwatched, IconCheck,
} from "./Icons";
import type { Channel, SortOrder } from "../types";

export const SORTS: { value: SortOrder; label: string }[] = [
  { value: "newest", label: "Newest first" },
  { value: "oldest", label: "Oldest first" },
  { value: "channel", label: "By channel" },
  { value: "length", label: "Longest first" },
  { value: "parts", label: "Most parts first" },
];

/** How many things narrow the list: the five filters and a chosen channel.
 *  Grouping and the sort only arrange it, so they do not count. The two
 *  series-only filters count only while grouped, which is when they apply. */
export function activeFilterCount(f: {
  hideWatched: boolean; downloadedOnly: boolean; showHidden: boolean;
  grouped: boolean; groupsOnly: boolean; inProgress: boolean; channelId: string | null;
}): number {
  return [
    f.hideWatched, f.downloadedOnly, f.showHidden,
    f.grouped && f.groupsOnly, f.grouped && f.inProgress, f.channelId !== null,
  ].filter(Boolean).length;
}

interface Props {
  /** The button that opened it: the panel hangs from its right edge. */
  anchor: RefObject<HTMLElement | null>;
  onClose: (refocus: boolean) => void;
  /** A series is open, so only the sort applies. */
  off: boolean;
  offHint?: string;
  channels: Channel[];
  channelId: string | null;
  onChannelId: (id: string | null) => void;
  hideWatched: boolean;
  onHideWatched: (b: boolean) => void;
  downloadedOnly: boolean;
  onDownloadedOnly: (b: boolean) => void;
  showHidden: boolean;
  onShowHidden: (b: boolean) => void;
  grouped: boolean;
  onGrouped: (b: boolean) => void;
  groupsOnly: boolean;
  onGroupsOnly: (b: boolean) => void;
  inProgress: boolean;
  onInProgress: (b: boolean) => void;
  sort: SortOrder;
  onSort: (s: SortOrder) => void;
}

function Toggle({ label, icon: Icon, on, disabled, title, onChange }: {
  label: string; icon: () => ReactElement; on: boolean; disabled: boolean; title?: string;
  onChange: (b: boolean) => void;
}) {
  return (
    <button
      type="button"
      className={`pop-item${on ? " is-on" : ""}`}
      aria-pressed={on}
      disabled={disabled}
      title={title}
      onClick={() => onChange(!on)}
    >
      <span className="pop-glyph" aria-hidden="true"><Icon /></span>
      <span className="pop-label">{label}</span>
      <span className="pop-check" aria-hidden="true">{on && <IconCheck />}</span>
    </button>
  );
}

/**
 * Everything the folded filter row keeps behind its one button, in two
 * columns: what is in the list and how it is arranged, then the order and the
 * channel. Changes apply as they are made, like the controls they stand in
 * for, so the panel stays open until a click lands outside it or Esc.
 */
export default function FiltersPanel(p: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [right, setRight] = useState(0);

  // Hung from the button's right edge, measured against the nav it sits in —
  // the panel lives outside the button's cluster, which clips while it slides.
  useLayoutEffect(() => {
    const place = () => {
      const btn = p.anchor.current;
      const host = ref.current?.offsetParent as HTMLElement | null;
      if (!btn || !host) return;
      setRight(Math.max(8, host.getBoundingClientRect().right - btn.getBoundingClientRect().right));
    };
    place();
    window.addEventListener("resize", place);
    return () => window.removeEventListener("resize", place);
  }, [p.anchor]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node;
      if (ref.current?.contains(t) || p.anchor.current?.contains(t)) return;
      p.onClose(false);
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      // Esc also leaves an open series; closing the panel is all this one means.
      e.stopPropagation();
      p.onClose(true);
    };
    document.addEventListener("mousedown", onDown);
    // Capture, so it is heard before the shell's series handler on window.
    window.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey, true);
    };
  }, [p.onClose, p.anchor]);

  const hint = p.offHint;
  return (
    <div className="topnav-pop filters-pop" role="dialog" aria-label="Filters" ref={ref} style={{ right }}>
      <div className="pop-col">
        <div className="pop-head" id="filters-show">Show</div>
        <div role="group" aria-labelledby="filters-show">
          <Toggle label="Unwatched" icon={IconUnwatched} on={p.hideWatched} disabled={p.off}
            title={hint} onChange={p.onHideWatched} />
          <Toggle label="Downloaded" icon={IconDownload} on={p.downloadedOnly} disabled={p.off}
            title={hint} onChange={p.onDownloadedOnly} />
          <Toggle label="Include hidden" icon={IconHidden} on={p.showHidden} disabled={p.off}
            title={hint} onChange={p.onShowHidden} />
          {p.grouped && (
            <>
              <Toggle label="Only groups" icon={IconGroupsOnly} on={p.groupsOnly} disabled={p.off}
                title={hint} onChange={p.onGroupsOnly} />
              <Toggle label="Continue watching" icon={IconContinue} on={p.inProgress} disabled={p.off}
                title={hint} onChange={p.onInProgress} />
            </>
          )}
        </div>
        <div className="pop-sep" />
        <div className="pop-head" id="filters-arrange">Arrange</div>
        <div role="group" aria-labelledby="filters-arrange">
          <Toggle label="Group siblings" icon={IconSeries} on={p.grouped} disabled={p.off}
            title={hint} onChange={p.onGrouped} />
        </div>
      </div>

      <div className="pop-col">
        <div className="pop-head" id="filters-sort">Sort</div>
        <div role="radiogroup" aria-labelledby="filters-sort">
          {SORTS.filter((s) => s.value !== "parts" || p.grouped).map((s) => (
            <button
              key={s.value}
              type="button"
              role="radio"
              aria-checked={p.sort === s.value}
              className={`pop-radio${p.sort === s.value ? " is-on" : ""}`}
              onClick={() => p.onSort(s.value)}
            >
              <span className="pop-dot" aria-hidden="true" />
              {s.label}
            </button>
          ))}
        </div>
        <div className="pop-sep" />
        <div className="pop-head">Channel</div>
        <select
          className="select pop-select"
          value={p.channelId ?? ""}
          aria-label="Filter by channel"
          disabled={p.off}
          title={hint}
          onChange={(e) => p.onChannelId(e.currentTarget.value || null)}
        >
          <option value="">All channels</option>
          {p.channels.map((c) => (
            <option key={c.id} value={c.id}>{c.title}</option>
          ))}
        </select>
      </div>
    </div>
  );
}
