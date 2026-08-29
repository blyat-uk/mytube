import { useEffect, useRef } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { DownloadProgress, DownloadStateEvent, PollSummary } from "./types";

/**
 * Attaches Tauri event listeners for the lifetime of the calling component.
 *
 * Handlers are held in a ref so a re-render never tears the subscription down;
 * every view that needs live download updates subscribes independently rather
 * than routing events through shared React state, which would drop an event
 * whenever two arrived in the same batch.
 */
function useTauriEvent<T>(name: string, handler?: (payload: T) => void) {
  const ref = useRef(handler);
  ref.current = handler;

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    listen<T>(name, (e) => ref.current?.(e.payload))
      .then((fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      })
      // Outside a Tauri window (plain `vite dev` in a browser) there is no
      // event bridge; the UI still renders, it just never gets live updates.
      .catch(() => {});

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [name]);
}

export function useDownloadEvents(handlers: {
  onProgress?: (p: DownloadProgress) => void;
  onState?: (s: DownloadStateEvent) => void;
}) {
  useTauriEvent<DownloadProgress>("download://progress", handlers.onProgress);
  useTauriEvent<DownloadStateEvent>("download://state", handlers.onState);
}

export function usePollEvents(handlers: {
  onStarted?: () => void;
  onFinished?: (summary: PollSummary) => void;
}) {
  useTauriEvent<null>("poll://started", () => handlers.onStarted?.());
  useTauriEvent<PollSummary>("poll://finished", handlers.onFinished);
}
