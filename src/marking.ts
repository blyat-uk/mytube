import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { Video, VideoGroup } from "./types";

/**
 * Selecting and dragging cards to mark them siblings by hand.
 *
 * The title matcher gives up exactly where a YouTuber renames a follow-up to
 * chase the algorithm, and this is how you tell it what it could not work out.
 * Two gestures, both operating on cards rather than on menus: ctrl+click a few
 * and mark them from the right-click menu, or drag one card onto another.
 *
 * Only the drag asks first. Picking a menu item is a decision; letting go of a
 * card over its neighbour is just as often a slip of the mouse, and a link made
 * by accident is invisible until a series turns up with a stranger in it.
 *
 * State lives here rather than in the view so the rules stay testable and
 * `SubscriptionsView` stays readable.
 */

/** Everything one card needs to take part. */
export interface CardMark {
  selected: boolean;
  /** A drag is hovering this card and would land on it. */
  dropTarget: boolean;
  /** Handles the pointer. Returns true when the card's own action must not
   *  run — a ctrl+click is a selection, not a download. */
  onClick: (e: React.MouseEvent) => boolean;
  drag: {
    draggable: boolean;
    onDragStart: (e: React.DragEvent) => void;
    onDragEnd: () => void;
    onDragOver: (e: React.DragEvent) => void;
    onDragLeave: () => void;
    onDrop: (e: React.DragEvent) => void;
  };
}

type CardHandlers = Omit<CardMark, "selected" | "dropTarget">;

export interface Marking {
  /** Leader ids of the selected cards, in the order they were picked. */
  selected: ReadonlySet<string>;
  clear: () => void;
  /** Marks everything selected. No-op below two, which is also when the menu
   *  item is hidden. */
  markSelected: () => void;
  forGroup: (g: VideoGroup) => CardMark;
}

export const CROSS_CHANNEL = "Siblings must come from the same channel.";

interface Options {
  /** Sends the ids on; the caller owns the toast and the refetch. */
  mark: (ids: string[]) => void;
  /** A drop landed. The caller asks before marking, and calls `mark` itself
   *  on a yes. Leaders in drag order, the card dropped on last. */
  confirmDrop: (videos: Video[]) => void;
  /** Told why a gesture was declined. */
  refuse: (message: string) => void;
}

export function useSiblingMarking(groups: VideoGroup[], o: Options): Marking {
  const [selected, setSelected] = useState<Set<string>>(() => new Set());
  const [dropOn, setDropOn] = useState<string | null>(null);
  // The ids under the pointer right now. A ref, not state: `dragover` fires
  // constantly and cannot read `dataTransfer` anyway — only `drop` can.
  const dragged = useRef<string[]>([]);

  /**
   * A card stands for its leader alone, even when it is a series of seven.
   * Hand-freezing all seven to attach one stray would be far more than was
   * asked: the leader's own title still pulls the other six in by itself, so
   * one id does the whole job and the group stays as soft as it was.
   */
  const byLeader = useMemo(
    () => new Map(groups.map((g) => [g.videos[0].id, g.videos[0]])),
    [groups],
  );

  const clear = useCallback(() => setSelected((prev) => (prev.size ? new Set() : prev)), []);

  // A card that has left the grid cannot stay picked. Covers filtering,
  // searching and re-sorting, all of which replace the list wholesale.
  useEffect(() => {
    setSelected((prev) => {
      if (prev.size === 0) return prev;
      const next = new Set([...prev].filter((id) => byLeader.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [byLeader]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") clear();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [clear]);

  const channelOf = useCallback(
    (leaderId: string) => byLeader.get(leaderId)?.channel_id,
    [byLeader],
  );

  /** Whether every id named, plus `target`, sits on one channel. */
  const sameChannel = useCallback(
    (ids: string[], target?: string) => {
      const all = target === undefined ? ids : [...ids, target];
      const channels = new Set(all.map(channelOf));
      return channels.size === 1 && !channels.has(undefined);
    },
    [channelOf],
  );

  const submit = useCallback(
    (ids: string[]) => {
      if (ids.length < 2) return;
      if (!sameChannel(ids)) {
        o.refuse(CROSS_CHANNEL);
        return;
      }
      o.mark(ids);
      clear();
    },
    [o, sameChannel, clear],
  );

  const markSelected = useCallback(() => submit([...selected]), [submit, selected]);

  /**
   * Everything a card's handlers read, as of the latest render. The handlers
   * below are built once per card and read through this, so they never go
   * stale and never change identity -- which is what lets a memoised card skip
   * a render when nothing about *it* changed.
   */
  const latest = useRef({ selected, clear, sameChannel, byLeader, o });
  latest.current = { selected, clear, sameChannel, byLeader, o };

  /** Per leader: its handlers, and the last `CardMark` handed out for it. */
  const cards = useRef(new Map<string, { handlers: CardHandlers; last?: CardMark }>());

  // A card that has left the grid needs no handlers kept for it.
  useEffect(() => {
    for (const id of cards.current.keys()) if (!byLeader.has(id)) cards.current.delete(id);
  }, [byLeader]);

  /**
   * The same object back for as long as the card's two flags hold, so the grid
   * can hand it straight to a memoised card: a fresh object per render would
   * re-render every card in the grid for a change that touched one.
   */
  const forGroup = useCallback(
    (g: VideoGroup): CardMark => {
      const id = g.videos[0].id;
      let entry = cards.current.get(id);
      if (!entry) {
        entry = { handlers: handlersFor(id) };
        cards.current.set(id, entry);
      }
      const isSelected = selected.has(id);
      const isTarget = dropOn === id;
      const last = entry.last;
      if (last && last.selected === isSelected && last.dropTarget === isTarget) return last;
      entry.last = { selected: isSelected, dropTarget: isTarget, ...entry.handlers };
      return entry.last;
    },
    [selected, dropOn],
  );

  /** One card's pointer handlers, reading everything else through `latest`. */
  function handlersFor(id: string): CardHandlers {
    return {
      onClick: (e) => {
        if (!e.ctrlKey && !e.metaKey) {
          // A plain click drops the selection and then does whatever it
          // always did — play, download, open the series.
          latest.current.clear();
          return false;
        }
        e.preventDefault();
        setSelected((prev) => {
          const next = new Set(prev);
          if (!next.delete(id)) next.add(id);
          return next;
        });
        return true;
      },

      drag: {
        draggable: true,
        onDragStart: (e) => {
          // Dragging a card that is part of the selection drags the whole
          // selection, the way a file manager does.
          const picked = latest.current.selected;
          const ids = picked.has(id) && picked.size > 1 ? [...picked] : [id];
          dragged.current = ids;
          // "move", not the more fitting "link": Wayland's drag protocol has
          // no link action at all (copy, move, ask), so under WebKitGTK on a
          // Wayland session a link-only drag is refused by every target and
          // the drop never fires.
          e.dataTransfer.effectAllowed = "move";
          // Some payload is required or the drag never starts in Firefox.
          e.dataTransfer.setData("text/plain", ids.join(" "));
        },
        onDragEnd: () => {
          dragged.current = [];
          setDropOn(null);
        },
        onDragOver: (e) => {
          const src = dragged.current;
          // Not our drag, dropping onto itself, or across channels: no
          // preventDefault, so the browser refuses the drop and the card
          // never lights up.
          if (src.length === 0 || src.includes(id) || !latest.current.sameChannel(src, id)) return;
          e.preventDefault();
          e.dataTransfer.dropEffect = "move";
          setDropOn(id);
        },
        onDragLeave: () => setDropOn((prev) => (prev === id ? null : prev)),
        onDrop: (e) => {
          e.preventDefault();
          const src = dragged.current;
          dragged.current = [];
          setDropOn(null);
          if (src.length === 0 || src.includes(id)) return;
          const now = latest.current;
          if (!now.sameChannel(src, id)) {
            now.o.refuse(CROSS_CHANNEL);
            return;
          }
          const videos = [...src, id].map((v) => now.byLeader.get(v));
          if (videos.some((v) => v === undefined)) return;
          now.o.confirmDrop(videos as Video[]);
        },
      },
    };
  }

  return { selected, clear, markSelected, forGroup };
}
