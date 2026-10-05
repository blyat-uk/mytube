/**
 * The channel filter's pick: which channels the feed is narrowed to, held as a
 * sorted list of ids, empty for every channel. Pure, so the rules the button
 * and the list share are testable without a DOM.
 */
import type { Channel } from "./types";

/**
 * The one shape a pick is held in: sorted, no duplicates, no empty ids. Picking
 * A then B must save the same view as B then A -- the shell compares the
 * serialised view to decide whether to write, and the feed keys its scroll
 * memory on it. `ViewState::sanitize` sorts the same way; channel ids are
 * ASCII, so JS and Rust agree on the order.
 */
export function normalizeIds(ids: readonly string[]): string[] {
  return [...new Set(ids.filter((id) => id !== ""))].sort();
}

/** The pick with one channel ticked or unticked. */
export function togglePick(ids: readonly string[], id: string): string[] {
  return ids.includes(id) ? ids.filter((x) => x !== id) : normalizeIds([...ids, id]);
}

/** The picked channels, in the list's own order (by title). */
function picked(channels: readonly Channel[], ids: readonly string[]): Channel[] {
  const set = new Set(ids);
  return channels.filter((c) => set.has(c.id));
}

/**
 * What the closed button wears: the first picked channel's name and how many
 * more there are, like the sort picker beside it wears its order. First in the
 * list's order, so the name is the one you would find at the top of the list.
 */
export function pickLabel(
  channels: readonly Channel[], ids: readonly string[],
): { name: string; more: number } {
  if (ids.length === 0) return { name: "All channels", more: 0 };
  const known = picked(channels, ids);
  // A restored pick can arrive before the channel list does.
  if (known.length === 0) {
    return { name: `${ids.length} channel${ids.length === 1 ? "" : "s"}`, more: 0 };
  }
  return { name: known[0].title, more: known.length - 1 };
}

/** The label in words, for a screen reader: "Alpha and 2 more". */
export function pickSpoken(channels: readonly Channel[], ids: readonly string[]): string {
  const { name, more } = pickLabel(channels, ids);
  return more > 0 ? `${name} and ${more} more` : name;
}

/** Every picked channel's name, one per line: the button's tooltip. */
export function pickTitles(channels: readonly Channel[], ids: readonly string[]): string {
  if (ids.length === 0) return "All channels";
  return picked(channels, ids).map((c) => c.title).join("\n") || pickLabel(channels, ids).name;
}

/**
 * The list's order while it is open: what was picked when it opened, then the
 * rest, each in the list's own order. Taken from the pick as it stood on
 * opening, not as it stands now, so a row never moves under the pointer.
 */
export function pickedFirst(channels: readonly Channel[], opened: ReadonlySet<string>): Channel[] {
  return [
    ...channels.filter((c) => opened.has(c.id)),
    ...channels.filter((c) => !opened.has(c.id)),
  ];
}

/**
 * The pick without channels that have left the list. The same array comes back
 * when nothing was dropped: a new one on every channel refresh would read as a
 * new filter and refetch the feed for nothing.
 */
export function pruneIds(ids: string[], channels: readonly Channel[]): string[] {
  const live = new Set(channels.map((c) => c.id));
  return ids.every((id) => live.has(id)) ? ids : ids.filter((id) => live.has(id));
}
