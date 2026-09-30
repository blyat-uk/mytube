import { useCallback, useSyncExternalStore } from "react";
import type { DownloadProgress } from "./types";

/**
 * Download progress by video, outside React state.
 *
 * A tick used to be a `setProgress` on the view, which re-rendered the view and
 * with it every card in the grid -- hundreds of them, several times a second
 * per download, to move one ring. Held here instead, a tick wakes only the
 * subscribers of the video it names, so the one card showing that download is
 * the one card that renders.
 *
 * Each view owns one (the Subscriptions grid and the Downloads list hear the
 * same events independently, as they always have), fills it from its own
 * `download://progress` listener, and clears an entry whenever the video leaves
 * `downloading`.
 */
export interface ProgressStore {
  get: (videoId: string) => DownloadProgress | undefined;
  set: (p: DownloadProgress) => void;
  clear: (videoId: string) => void;
  subscribe: (videoId: string, fn: () => void) => () => void;
}

export function createProgressStore(): ProgressStore {
  const values = new Map<string, DownloadProgress>();
  const listeners = new Map<string, Set<() => void>>();

  const notify = (videoId: string) => {
    for (const fn of listeners.get(videoId) ?? []) fn();
  };

  return {
    get: (videoId) => values.get(videoId),
    set: (p) => {
      values.set(p.videoId, p);
      notify(p.videoId);
    },
    clear: (videoId) => {
      if (values.delete(videoId)) notify(videoId);
    },
    subscribe: (videoId, fn) => {
      let set = listeners.get(videoId);
      if (!set) listeners.set(videoId, (set = new Set()));
      set.add(fn);
      return () => {
        set.delete(fn);
        if (set.size === 0) listeners.delete(videoId);
      };
    },
  };
}

const noSubscription = () => () => {};
const nothing = () => undefined;

/** One video's progress, re-rendering the caller only when that video ticks.
 *  With no store -- a card rendered on its own, as in its tests -- it is none. */
export function useProgress(
  store: ProgressStore | undefined,
  videoId: string,
): DownloadProgress | undefined {
  const subscribe = useCallback(
    (fn: () => void) => store?.subscribe(videoId, fn) ?? noSubscription(),
    [store, videoId],
  );
  const read = useCallback(() => store?.get(videoId), [store, videoId]);
  return useSyncExternalStore(subscribe, store ? read : nothing);
}
