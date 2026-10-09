import { useCallback, useEffect, useRef, useState } from "react";
import { api, errText } from "../../api";
import { useToast } from "../Toast";
import type { Settings } from "../../types";

/** The numeric settings and the range Rust clamps each one to. Pages clamp
 *  before saving too, so what is on screen is what lands on disk. */
export const LIMITS = {
  max_concurrent_downloads: { min: 1, max: 16 },
  poll_interval_minutes: { min: 1, max: 1440 },
  backfill_count: { min: 1, max: 500 },
} as const;

export const clamp = (n: number, { min, max }: { min: number; max: number }) =>
  Math.min(max, Math.max(min, Math.round(n)));

export interface SettingsDoc {
  /** Null until the first read lands. */
  settings: Settings | null;
  /** The first read failed; nothing can be shown. */
  failed: boolean;
  /** When the last save landed (ms), 0 before any. Drives the rail's "Saved". */
  savedAt: number;
  /** Typing: held on screen, not saved, until `flush`. */
  set: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  /** A pick, which has no "done editing" moment: saved now. */
  save: <K extends keyof Settings>(key: K, value: Settings[K]) => void;
  /** Several keys that change together, in one save: two writes would race. */
  patch: (patch: Partial<Settings>) => void;
  /** Saves whatever is held, if it differs from disk. Every text field's blur. */
  flush: () => void;
  /** Re-reads settings.json, which an import that applied settings rewrote. */
  reload: () => Promise<void>;
}

/**
 * The one copy of settings.json the Settings view edits.
 *
 * A ref mirrors the state synchronously, so a blur handler never reads a stale
 * value and no side effect runs inside a state updater; `persisted` is the last
 * value known to be on disk, so an unchanged blur is not a write. Every save
 * spreads the whole object the backend handed over, so keys Rust flattened in
 * (hand-written, or from a newer build) survive the round trip.
 */
export function useSettingsDoc(): SettingsDoc {
  const toast = useToast();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [failed, setFailed] = useState(false);
  const [savedAt, setSavedAt] = useState(0);
  const current = useRef<Settings | null>(null);
  const persisted = useRef("");

  const adopt = useCallback((s: Settings) => {
    current.current = s;
    persisted.current = JSON.stringify(s);
    setSettings(s);
  }, []);

  useEffect(() => {
    let alive = true;
    api.getSettings()
      .then((s) => { if (alive) adopt(s); })
      .catch((err) => {
        if (!alive) return;
        setFailed(true);
        toast.error(errText(err));
      });
    return () => { alive = false; };
  }, [toast, adopt]);

  // Saves go out one at a time, each writing whatever is newest when its turn
  // comes. Two in flight at once could land in either order -- the backend
  // reads and writes the file on its blocking pool -- so three quick clicks on
  // a stepper could leave the middle value on disk. Queued, a burst collapses
  // to the writes it needs: one gesture that asks twice (a slider's release
  // fires pointerup, change and blur together) finds its value already on
  // disk by its second turn.
  const queue = useRef<Promise<void>>(Promise.resolve());

  const commit = useCallback(() => {
    queue.current = queue.current.then(async () => {
      const latest = current.current;
      if (!latest) return;
      const serialised = JSON.stringify(latest);
      if (serialised === persisted.current) return;
      try {
        await api.saveSettings(latest);
        persisted.current = serialised;
        setSavedAt(Date.now());
      } catch (err) {
        toast.error(errText(err));
      }
    });
  }, [toast]);

  const apply = useCallback((patch: Partial<Settings>, persist: boolean) => {
    const base = current.current;
    if (!base) return;
    const next = { ...base, ...patch };
    current.current = next;
    setSettings(next);
    if (persist) commit();
  }, [commit]);

  const set = useCallback(<K extends keyof Settings>(key: K, value: Settings[K]) => {
    apply({ [key]: value } as Partial<Settings>, false);
  }, [apply]);

  const save = useCallback(<K extends keyof Settings>(key: K, value: Settings[K]) => {
    apply({ [key]: value } as Partial<Settings>, true);
  }, [apply]);

  const patch = useCallback((p: Partial<Settings>) => apply(p, true), [apply]);

  const flush = useCallback(() => commit(), [commit]);

  const reload = useCallback(async () => {
    adopt(await api.getSettings());
  }, [adopt]);

  return { settings, failed, savedAt, set, save, patch, flush, reload };
}
