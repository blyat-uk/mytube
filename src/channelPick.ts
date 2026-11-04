/**
 * The channel filter's pick: which channels the feed shows, held as a sorted
 * list of ids and whether that list is the channels shown or the ones left
 * out. Empty is every channel. Pure, so the rules the button and the list
 * share are testable without a DOM.
 *
 * The checklist starts with every channel ticked. Unticking one or two leaves
 * them out (`except`), so a channel subscribed to later is shown without
 * anyone having to come back and tick it; Only, or ticking from there, builds
 * a list of the channels shown instead.
 */
import type { Channel } from "./types";

export interface ChannelPick {
  ids: string[];
  /** `ids` are the channels left out, not the ones shown. */
  except: boolean;
}

/** Every channel: nothing picked, nothing left out. */
export const EVERY_CHANNEL: ChannelPick = { ids: [], except: false };

/**
 * The one shape a list of ids is held in: sorted, no duplicates, no empty ids.
 * Picking A then B must save the same view as B then A -- the shell compares
 * the serialised view to decide whether to write, and the feed keys its scroll
 * memory on it. `ViewState::sanitize` sorts the same way; channel ids are
 * ASCII, so JS and Rust agree on the order.
 */
export function normalizeIds(ids: readonly string[]): string[] {
  return [...new Set(ids.filter((id) => id !== ""))].sort();
}

/** Whether the pick shows this channel. */
export function isShown(pick: ChannelPick, id: string): boolean {
  if (pick.ids.length === 0) return true;
  return pick.except ? !pick.ids.includes(id) : pick.ids.includes(id);
}

/** How many of `channels` the pick shows. */
export function shownCount(pick: ChannelPick, channels: readonly Channel[]): number {
  return channels.filter((c) => isShown(pick, c.id)).length;
}

/**
 * The pick with one channel's box flipped. A box never empties the feed: the
 * last channel shown cannot be unticked (Only, or All channels, is how to go
 * elsewhere from there). Ticking every channel back is every channel, which has
 * one shape only.
 */
export function tickChannel(pick: ChannelPick, id: string, channels: readonly Channel[]): ChannelPick {
  const all = channels.map((c) => c.id);
  const shown = isShown(pick, id);
  if (shown && shownCount(pick, channels) <= 1) return pick;
  if (pick.ids.length === 0 || pick.except) {
    const ids = shown ? normalizeIds([...pick.ids, id]) : pick.ids.filter((x) => x !== id);
    return ids.length === 0 ? EVERY_CHANNEL : { ids, except: true };
  }
  const ids = shown ? pick.ids.filter((x) => x !== id) : normalizeIds([...pick.ids, id]);
  if (all.length > 0 && all.every((x) => ids.includes(x))) return EVERY_CHANNEL;
  return { ids, except: false };
}

/** The channels the ids name, in the list's own order (by title). */
function named(channels: readonly Channel[], ids: readonly string[]): Channel[] {
  const set = new Set(ids);
  return channels.filter((c) => set.has(c.id));
}

/**
 * What the closed button wears: the first picked channel's name and how many
 * more there are, like the sort picker beside it wears its order -- or, when
 * channels are left out, "All but Nexpo" / "All but 3".
 */
export function pickLabel(
  channels: readonly Channel[], pick: ChannelPick,
): { name: string; more: number } {
  if (pick.ids.length === 0) return { name: "All channels", more: 0 };
  const known = named(channels, pick.ids);
  if (pick.except) {
    return { name: known.length === 1 ? `All but ${known[0].title}` : `All but ${pick.ids.length}`, more: 0 };
  }
  // A restored pick can arrive before the channel list does.
  if (known.length === 0) {
    return { name: `${pick.ids.length} channel${pick.ids.length === 1 ? "" : "s"}`, more: 0 };
  }
  return { name: known[0].title, more: known.length - 1 };
}

/** The pick in words, for a screen reader: "Alpha and 2 more", or "all
 *  channels except Alpha and Beta". */
export function pickSpoken(channels: readonly Channel[], pick: ChannelPick): string {
  if (pick.except && pick.ids.length > 0) {
    const names = named(channels, pick.ids).map((c) => c.title);
    return `all channels except ${names.length ? names.join(" and ") : `${pick.ids.length}`}`;
  }
  const { name, more } = pickLabel(channels, pick);
  return more > 0 ? `${name} and ${more} more` : name;
}

/** Every channel the pick names, one per line: the button's tooltip. */
export function pickTitles(channels: readonly Channel[], pick: ChannelPick): string {
  if (pick.ids.length === 0) return "All channels";
  const names = named(channels, pick.ids).map((c) => c.title).join("\n");
  if (pick.except) return `All channels except:\n${names || `${pick.ids.length} channels`}`;
  return names || pickLabel(channels, pick).name;
}

/**
 * The list's order while it is open: the channels the pick named when it
 * opened -- the ones shown, or the ones left out -- then the rest, each in the
 * list's own order. Taken from the pick as it stood on opening, not as it
 * stands now, so a row never moves under the pointer.
 */
export function pickedFirst(channels: readonly Channel[], opened: ReadonlySet<string>): Channel[] {
  return [
    ...channels.filter((c) => opened.has(c.id)),
    ...channels.filter((c) => !opened.has(c.id)),
  ];
}

/**
 * The ids without channels that have left the list. The same array comes back
 * when nothing was dropped: a new one on every channel refresh would read as a
 * new filter and refetch the feed for nothing.
 */
export function pruneIds(ids: string[], channels: readonly Channel[]): string[] {
  const live = new Set(channels.map((c) => c.id));
  return ids.every((id) => live.has(id)) ? ids : ids.filter((id) => live.has(id));
}
