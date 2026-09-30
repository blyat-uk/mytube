import { useEffect, useRef } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  AppUpdate, DownloadProgress, DownloadStateEvent, ImportReport, PollSummary, ToolProgress, ToolStatus,
  TransferProgress, VersionInfo,
} from "./types";

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

/**
 * Export and import progress.
 *
 * Deliberately its own channel rather than `import://progress`: TakeoutDialog
 * subscribes to that one for as long as the Add dialog is open, so sharing it
 * would light the Takeout progress bar up in the middle of an unrelated export.
 * `phase` then separates the two halves, since one channel carries both.
 */
export function useTransferProgress(handler?: (p: TransferProgress) => void) {
  useTauriEvent<TransferProgress>("transfer://progress", handler);
}

/** An import rewrites channels and videos underneath every view, and the view
 *  that started it takes no props — so the shell hears about it here and
 *  refetches. */
export function useTransferFinished(handler?: (report: ImportReport) => void) {
  useTauriEvent<ImportReport>("transfer://finished", handler);
}

/** Bytes of a tool download in flight, for the Tools section's bar. */
export function useToolsProgress(handler?: (p: ToolProgress) => void) {
  useTauriEvent<ToolProgress>("tools://progress", handler);
}

/** All three tools' status, sent whenever any of them changes. The shell hears
 *  it for the "ready" toast, the Tools section for its rows. */
export function useToolsStatus(handler?: (list: ToolStatus[]) => void) {
  useTauriEvent<ToolStatus[]>("tools://status", handler);
}

/** A newer MyTube release. The backend sends each version at most once, ever,
 *  so the shell can toast it without keeping any record of its own. */
export function useAppUpdate(handler?: (update: AppUpdate) => void) {
  useTauriEvent<AppUpdate>("app://update-available", handler);
}

/** The version state — not the news: sent after every check, so the nav's
 *  version pill and Settings' About block follow it without polling. */
export function useVersionInfo(handler?: (info: VersionInfo) => void) {
  useTauriEvent<VersionInfo>("app://version-info", handler);
}
