import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { VideoGroup } from "./types";

/**
 * Selecting and dragging cards to mark them siblings by hand.
 *
 * The title matcher gives up exactly where a YouTuber renames a follow-up to
 * chase the algorithm, and this is how you tell it what it could not work out.
 * Two gestures, both operating on cards rather than on menus: ctrl+click a few
 * and mark them from the right-click menu, or drag one card onto another.
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

  const forGroup = useCallback(
    (g: VideoGroup): CardMark => {
      const id = g.videos[0].id;
      return {
        selected: selected.has(id),
        dropTarget: dropOn === id,

        onClick: (e) => {
          if (!e.ctrlKey && !e.metaKey) {
            // A plain click drops the selection and then does whatever it
            // always did — play, download, open the series.
            clear();
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
            const ids = selected.has(id) && selected.size > 1 ? [...selected] : [id];
            dragged.current = ids;
            e.dataTransfer.effectAllowed = "link";
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
            if (src.length === 0 || src.includes(id) || !sameChannel(src, id)) return;
            e.preventDefault();
            e.dataTransfer.dropEffect = "link";
            setDropOn(id);
          },
          onDragLeave: () => setDropOn((prev) => (prev === id ? null : prev)),
          onDrop: (e) => {
            e.preventDefault();
            const src = dragged.current;
            dragged.current = [];
            setDropOn(null);
            if (src.length === 0 || src.includes(id)) return;
            if (!sameChannel(src, id)) {
              o.refuse(CROSS_CHANNEL);
              return;
            }
            submit([...src, id]);
          },
        },
      };
    },
    [selected, dropOn, clear, sameChannel, submit, o],
  );

  return { selected, clear, markSelected, forGroup };
}
