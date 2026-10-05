import { useRef, type RefObject } from "react";
import ChannelList from "./ChannelList";
import { usePopover } from "./usePopover";
import { pickLabel, pickSpoken, pickTitles } from "../channelPick";
import type { Channel } from "../types";

/**
 * The full row's channel filter, closed: it wears its value instead of a label
 * beside it, as the sort picker does -- the first picked channel's name and
 * how many more, "Haru Senpai +2". Tinted like any other filter while it
 * narrows the list.
 */
export function ChannelButton(p: {
  channels: Channel[];
  picked: string[];
  open: boolean;
  onToggle: () => void;
  disabled: boolean;
  offHint?: string;
  buttonRef: RefObject<HTMLButtonElement | null>;
}) {
  const { name, more } = pickLabel(p.channels, p.picked);
  const on = p.picked.length > 0;
  return (
    <button
      ref={p.buttonRef}
      type="button"
      className={`select channel-picker${on ? " is-on" : ""}${p.open ? " is-open" : ""}`}
      aria-haspopup="dialog"
      aria-expanded={p.open}
      aria-label={`Filter by channel: ${pickSpoken(p.channels, p.picked)}`}
      disabled={p.disabled}
      // A long name is cut short in the box, so every picked name lives here.
      title={p.offHint ?? pickTitles(p.channels, p.picked)}
      onClick={p.onToggle}
    >
      <span className="channel-picker-name">{name}</span>
      {more > 0 && <span className="channel-picker-more">+{more}</span>}
    </button>
  );
}

/** The checklist the button opens, hung from it like the Filters panel. */
export function ChannelPopover(p: {
  anchor: RefObject<HTMLElement | null>;
  onClose: (refocus: boolean) => void;
  channels: Channel[];
  picked: string[];
  onPick: (ids: string[]) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const right = usePopover(ref, p.anchor, p.onClose);
  return (
    <div className="topnav-pop channel-pop" role="dialog" aria-label="Channels" ref={ref} style={{ right }}>
      <ChannelList channels={p.channels} picked={p.picked} onPick={p.onPick} autoFocus />
    </div>
  );
}
