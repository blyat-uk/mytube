import { Fragment, useEffect, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { IconCheck } from "./Icons";
import { pickedFirst, togglePick } from "../channelPick";
import type { Channel } from "../types";

interface Props {
  channels: Channel[];
  /** The pick: sorted channel ids, empty for every channel. */
  picked: string[];
  onPick: (ids: string[]) => void;
  /** A series is open, which shows every part whatever the channel filter. */
  disabled?: boolean;
  title?: string;
  /** Focus the filter box on opening: the toolbar's popover exists to pick. */
  autoFocus?: boolean;
}

/**
 * The channel filter's checklist, the same in the toolbar's popover and the
 * folded Filters panel: a filter box, All channels, then every channel with a
 * box to tick. Ticks apply as they are made, like every other filter. Hovering
 * a row shows Only, which switches to that one channel in one click -- the job
 * the single-channel dropdown this replaced did.
 */
export default function ChannelList(p: Props) {
  const [query, setQuery] = useState("");
  // What was picked on opening goes first, and stays first until the list
  // closes: re-sorting on every tick would move the row under the pointer.
  const [opened] = useState(() => new Set(p.picked));
  const order = useMemo(() => pickedFirst(p.channels, opened), [p.channels, opened]);
  const pickedSet = useMemo(() => new Set(p.picked), [p.picked]);

  const q = query.trim().toLowerCase();
  const visible = q ? order.filter((c) => c.title.toLowerCase().includes(q)) : order;
  // One hairline between what was picked on opening and the rest.
  const split = q ? -1 : visible.findIndex((c, i) => i > 0 && opened.has(visible[i - 1].id) && !opened.has(c.id));

  const filterRef = useRef<HTMLInputElement | null>(null);
  const rowsRef = useRef<HTMLDivElement | null>(null);
  useEffect(() => {
    if (p.autoFocus) filterRef.current?.focus();
  }, []);

  const rowButtons = () =>
    Array.from(rowsRef.current?.querySelectorAll<HTMLButtonElement>(".chan-row:not(:disabled)") ?? []);

  // The arrows step row to row, skipping the Only buttons, which Tab reaches;
  // up from the first row is back in the filter box.
  const onRowsKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const all = rowButtons();
    const here = (e.target as HTMLElement).closest(".chan-row-wrap, .chan-all")
      ?.querySelector<HTMLButtonElement>(".chan-row") ?? null;
    const i = here ? all.indexOf(here) : -1;
    if (i < 0) return;
    e.preventDefault();
    if (e.key === "ArrowDown") all[Math.min(i + 1, all.length - 1)].focus();
    else if (i === 0) filterRef.current?.focus();
    else all[i - 1].focus();
  };
  const onFilterKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== "ArrowDown") return;
    e.preventDefault();
    rowButtons()[0]?.focus();
  };

  return (
    <div className="chan-list" title={p.title}>
      <input
        ref={filterRef}
        className="text-input chan-filter"
        type="search"
        value={query}
        placeholder="Filter channels"
        aria-label="Filter channels"
        disabled={p.disabled}
        onChange={(e) => setQuery(e.currentTarget.value)}
        onKeyDown={onFilterKey}
      />
      <div className="chan-rows" role="group" aria-label="Channels" ref={rowsRef} onKeyDown={onRowsKey}>
        {!q && (
          <>
            {/* The state of having picked nothing, so a dot rather than a box. */}
            <div className="chan-all">
              <button
                type="button"
                className={`chan-row${p.picked.length === 0 ? " is-on" : ""}`}
                aria-pressed={p.picked.length === 0}
                disabled={p.disabled}
                onClick={() => p.onPick([])}
              >
                <span className="chan-dot" aria-hidden="true" />
                <span className="chan-name">All channels</span>
              </button>
            </div>
            <div className="pop-sep" />
          </>
        )}
        {visible.map((c, i) => {
          const on = pickedSet.has(c.id);
          return (
            <Fragment key={c.id}>
              {i === split && <div className="pop-sep" />}
              <div className="chan-row-wrap">
                <button
                  type="button"
                  className={`chan-row${on ? " is-on" : ""}`}
                  aria-pressed={on}
                  disabled={p.disabled}
                  // A long name is cut short in the row, so the full one lives here.
                  title={p.title ?? c.title}
                  onClick={() => p.onPick(togglePick(p.picked, c.id))}
                >
                  <span className="chan-box" aria-hidden="true">{on && <IconCheck />}</span>
                  <span className="chan-name">{c.title}</span>
                </button>
                <button
                  type="button"
                  className="chan-only"
                  aria-label={`Show only ${c.title}`}
                  title={p.title ?? `Show only ${c.title}`}
                  disabled={p.disabled}
                  onClick={() => p.onPick([c.id])}
                >
                  Only
                </button>
              </div>
            </Fragment>
          );
        })}
        {q && visible.length === 0 && (
          <div className="chan-empty">No channel matches “{query.trim()}”</div>
        )}
      </div>
    </div>
  );
}
