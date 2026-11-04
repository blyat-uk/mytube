import { Fragment, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent } from "react";
import { IconCheck, IconMinus } from "./Icons";
import { EVERY_CHANNEL, isShown, pickedFirst, shownCount, tickChannel, type ChannelPick } from "../channelPick";
import type { Channel } from "../types";

interface Props {
  channels: Channel[];
  /** The pick: the channels shown, or with `except` the ones left out. */
  pick: ChannelPick;
  onPick: (pick: ChannelPick) => void;
  /** A series is open, which shows every part whatever the channel filter. */
  disabled?: boolean;
  title?: string;
  /** Focus the filter box on opening: the toolbar's popover exists to pick. */
  autoFocus?: boolean;
}

/**
 * The channel filter's checklist, the same in the top bar's popover and the
 * All filters sidebar: a filter box, All channels, then every channel with a
 * box to tick. Ticks apply as they are made, like every other filter.
 *
 * Every box starts ticked: no channel filter is every channel shown, so seeing
 * everything except one or two is a matter of unticking them. All channels is
 * the Select all box over them -- ticked, half-ticked or (never) empty -- and
 * ticks them all again. Hovering a row shows Only, which switches to that one
 * channel in one click.
 */
export default function ChannelList(p: Props) {
  const [query, setQuery] = useState("");
  // What the pick named on opening goes first, and stays first until the list
  // closes: re-sorting on every tick would move the row under the pointer.
  const [opened] = useState(() => new Set(p.pick.ids));
  const order = useMemo(() => pickedFirst(p.channels, opened), [p.channels, opened]);

  const q = query.trim().toLowerCase();
  const visible = q ? order.filter((c) => c.title.toLowerCase().includes(q)) : order;
  // One hairline between what the pick named on opening and the rest.
  const split = q ? -1 : visible.findIndex((c, i) => i > 0 && opened.has(visible[i - 1].id) && !opened.has(c.id));
  const every = p.pick.ids.length === 0;
  const shown = shownCount(p.pick, p.channels);
  const countId = useId();

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
            {/* Select all: ticked while every channel is shown, half-ticked while
                some are left out. Pressing it shows them all again. */}
            <div className="chan-all">
              <button
                type="button"
                className={`chan-row${every ? " is-on" : " is-mixed"}`}
                aria-pressed={every ? true : "mixed"}
                aria-label="All channels"
                aria-describedby={countId}
                disabled={p.disabled}
                title={p.title ?? (every ? "Every channel is shown" : "Show every channel again")}
                onClick={() => p.onPick(EVERY_CHANNEL)}
              >
                <span className="chan-box" aria-hidden="true">{every ? <IconCheck /> : <IconMinus />}</span>
                <span className="chan-name">All channels</span>
                <span className="chan-count" id={countId}>{shown} of {p.channels.length} shown</span>
              </button>
            </div>
            <div className="pop-sep" />
          </>
        )}
        {visible.map((c, i) => {
          const on = isShown(p.pick, c.id);
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
                  onClick={() => p.onPick(tickChannel(p.pick, c.id, p.channels))}
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
                  onClick={() => p.onPick({ ids: [c.id], except: false })}
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
