import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useToast } from "./components/Toast";
import { errText } from "./api";

/**
 * How long a call may run before its control starts to spin. Most commands
 * answer in a few milliseconds, and a spinner that flashes on for one frame on
 * every click reads as flicker, not as feedback -- so a control is disabled at
 * once but only *looks* busy once the wait is long enough to notice.
 */
export const SLOW_AFTER = 150;

export interface RunOptions {
  /**
   * Applies the change on screen before the call goes out, and returns how to
   * take it back. The undo runs only if the call throws; on success the next
   * event or refetch confirms what is already shown.
   */
  optimistic?: () => () => void;
}

export interface Pending {
  /** Keys with a call in flight. Their controls are disabled. */
  pending: ReadonlySet<string>;
  /** The subset that has been running past `SLOW_AFTER`, which is when a
   *  control swaps its glyph for a spinner. */
  slow: ReadonlySet<string>;
  /**
   * Runs `fn` under `key`, unless a call under that key is already in flight,
   * in which case nothing happens at all -- that is the double-fire guard, and
   * it is checked against a ref rather than state so two clicks landing in the
   * same frame, before React has re-rendered, still count as one.
   *
   * Resolves true when `fn` ran and succeeded, false when it was skipped or
   * threw. A throw is rolled back and toasted here; callers need no catch.
   */
  run: (key: string, fn: () => Promise<unknown>, options?: RunOptions) => Promise<boolean>;
}

function withKey(set: ReadonlySet<string>, key: string): ReadonlySet<string> {
  if (set.has(key)) return set;
  const next = new Set(set);
  next.add(key);
  return next;
}

function withoutKey(set: ReadonlySet<string>, key: string): ReadonlySet<string> {
  if (!set.has(key)) return set;
  const next = new Set(set);
  next.delete(key);
  return next;
}

const EMPTY: ReadonlySet<string> = new Set();

/**
 * One place for what every button that calls the backend needs: refuse a second
 * press while the first is out, show that it is working once it is slow enough
 * to notice, and put the screen back when it fails.
 *
 * Keys are the caller's to choose. A video's id covers every action that
 * changes its download, so a cancel and a download on one card can never be
 * out at once; an action that must not lock the card (opening YouTube, marking
 * watched) takes a prefixed key of its own.
 */
export function usePending(): Pending {
  const toast = useToast();
  const [pending, setPending] = useState<ReadonlySet<string>>(EMPTY);
  const [slow, setSlow] = useState<ReadonlySet<string>>(EMPTY);
  // The truth, synchronously. `pending` is its rendered copy.
  const live = useRef(new Set<string>());
  const timers = useRef(new Map<string, number>());

  // A spinner timer outliving the view would set state on nothing.
  useEffect(() => () => {
    for (const t of timers.current.values()) window.clearTimeout(t);
    timers.current.clear();
  }, []);

  const run = useCallback<Pending["run"]>(async (key, fn, options) => {
    if (live.current.has(key)) return false;
    live.current.add(key);
    setPending((s) => withKey(s, key));
    timers.current.set(key, window.setTimeout(() => {
      timers.current.delete(key);
      setSlow((s) => withKey(s, key));
    }, SLOW_AFTER));

    let undo: (() => void) | undefined;
    try {
      undo = options?.optimistic?.();
      await fn();
      return true;
    } catch (err) {
      undo?.();
      toast.error(errText(err));
      return false;
    } finally {
      const t = timers.current.get(key);
      if (t !== undefined) window.clearTimeout(t);
      timers.current.delete(key);
      live.current.delete(key);
      setPending((s) => withoutKey(s, key));
      setSlow((s) => withoutKey(s, key));
    }
  }, [toast]);

  return useMemo(() => ({ pending, slow, run }), [pending, slow, run]);
}

/**
 * Wraps a click handler so the second click of a double-click is ignored. A
 * card's action flips with its state -- Download becomes Cancel the moment the
 * row is queued -- so a double-click read as two clicks downloads and then
 * cancels, however fast the first call returns. `detail` counts the clicks in
 * the burst; a keyboard press reports 0 and always goes through.
 */
export function singleClick<E extends { detail: number }>(fn: (e: E) => void) {
  return (e: E) => {
    if (e.detail > 1) return;
    fn(e);
  };
}
