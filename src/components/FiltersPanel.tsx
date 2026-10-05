import { useRef, type ReactElement, type RefObject } from "react";
import {
  IconContinue, IconDownload, IconGroupsOnly, IconHidden, IconSeries, IconUnwatched, IconCheck,
} from "./Icons";
import ChannelList from "./ChannelList";
import { usePopover } from "./usePopover";
import type { Channel, SortOrder } from "../types";

export const SORTS: { value: SortOrder; label: string }[] = [
  { value: "newest", label: "Newest first" },
  { value: "oldest", label: "Oldest first" },
  { value: "channel", label: "By channel" },
  { value: "length", label: "Longest first" },
  { value: "parts", label: "Most parts first" },
];

/** How many things narrow the list: the five filters and the channel pick,
 *  which is one filter however many channels it holds. Grouping and the sort
 *  only arrange it, so they do not count. The two series-only filters count
 *  only while grouped, which is when they apply. */
export function activeFilterCount(f: {
  hideWatched: boolean; downloadedOnly: boolean; showHidden: boolean;
  grouped: boolean; groupsOnly: boolean; inProgress: boolean; channelIds: readonly string[];
}): number {
  return [
    f.hideWatched, f.downloadedOnly, f.showHidden,
    f.grouped && f.groupsOnly, f.grouped && f.inProgress, f.channelIds.length > 0,
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
  channelIds: string[];
  onChannelIds: (ids: string[]) => void;
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
 * columns: what is in the list, how it is arranged and in what order, then
 * which channels. Changes apply as they are made, like the controls they stand
 * in for, so the panel stays open until a click lands outside it or Esc.
 *
 * The left column alone sets the panel's height. The channel list takes what
 * that leaves on the right and scrolls past it, so a long subscription list
 * never makes the panel taller (`.pop-col-fill`).
 */
export default function FiltersPanel(p: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const right = usePopover(ref, p.anchor, p.onClose);

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
        <div className="pop-sep" />
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
      </div>

      <div className="pop-col pop-col-fill">
        <div className="pop-head">
          Channels
          {p.channelIds.length > 0 && <span className="pop-head-note">{p.channelIds.length} picked</span>}
        </div>
        <ChannelList
          channels={p.channels}
          picked={p.channelIds}
          onPick={p.onChannelIds}
          disabled={p.off}
          title={hint}
        />
      </div>
    </div>
  );
}
