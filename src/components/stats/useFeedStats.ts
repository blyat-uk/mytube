import { useCallback, useEffect, useRef, useState } from "react";
import { api } from "../../api";
import type { FeedStats } from "../../types";

/** How long a bump waits for the next one. A run of clicks -- three parts
 *  marked watched in a row, a series' downloads landing one after another --
 *  is one request at its end rather than one per click. */
export const BUMP_DELAY = 600;

export interface FeedStatsOptions {
  /** Settings → Appearance → Show stats. Off asks for nothing and holds
   *  nothing: the stats are null. */
  enabled: boolean;
  /**
   * A series is open. Nothing is asked -- the feed is not what the view is
   * about -- but the stats already in are kept, because the grid stays the
   * feed's until the series' first page lands, and the waveform above it must
   * not vanish and come back. Coming back from the series asks again at once:
   * what was done inside it moved the feed's counts.
   */
  paused?: boolean;
  /** The channel pick, as the shell holds it (sorted, so the same pick keeps
   *  its identity). */
  channelIds: string[];
  /** The search box. Trimmed here, as the feed's own fetch trims it, so a
   *  trailing space is not a different question. */
  search: string;
  /** Bumped by the shell after a poll, an add, an import or a sync. */
  reloadToken: number;
}

export interface FeedStatsState {
  /** The last answer, kept while the next is out so nothing above the grid
   *  flashes or jumps. Null while off, and until the first answer lands. */
  stats: FeedStats | null;
  /** Asks again shortly, for a change made from the grid. Stable. */
  bump: () => void;
}

/**
 * The Subscriptions feed's stats, for the waveform above the grid. They follow
 * the channel pick and the search box only -- the Show toggles never change
 * them -- so those and the shell's reloads are what ask again, plus `bump`
 * after anything done on a card that moves a count.
 *
 * Decoration, not data: a failed read is not toasted, and the last good stats
 * stay up.
 */
export function useFeedStats({
  enabled, paused = false, channelIds, search, reloadToken,
}: FeedStatsOptions): FeedStatsState {
  const [stats, setStats] = useState<FeedStats | null>(null);
  // Only the newest request may land: a pick's answer arriving after the next
  // pick's would put the wrong counts over the grid.
  const request = useRef(0);
  const timer = useRef<number | undefined>(undefined);
  const trimmed = search.trim();
  const live = enabled && !paused;

  const fetchNow = useCallback(() => {
    // A bump still waiting is answered by this one: it was asked for after its
    // change had landed, and this read starts later still.
    window.clearTimeout(timer.current);
    timer.current = undefined;
    const id = ++request.current;
    // Through `Promise.resolve()` so a missing bridge (`vite dev`) cannot
    // throw out of the effect that asked.
    Promise.resolve()
      .then(() => api.feedStats({ channelIds, search: trimmed || null }))
      .then((s) => { if (id === request.current) setStats(s); })
      .catch(() => {});
  }, [channelIds, trimmed]);

  // What a bump's timer reads when it fires, rather than what was current when
  // it was set: the pick may have moved on, or the switch gone off, since.
  const latest = useRef({ live, fetchNow });
  latest.current = { live, fetchNow };

  useEffect(() => {
    if (!live) {
      // Nothing still out may land after this, and nothing waits to go out.
      request.current++;
      window.clearTimeout(timer.current);
      timer.current = undefined;
      if (!enabled) setStats(null);
      return;
    }
    fetchNow();
  }, [live, enabled, fetchNow, reloadToken]);

  useEffect(() => () => {
    window.clearTimeout(timer.current);
    request.current++;
  }, []);

  const bump = useCallback(() => {
    if (!latest.current.live) return;
    window.clearTimeout(timer.current);
    timer.current = window.setTimeout(() => {
      timer.current = undefined;
      if (latest.current.live) latest.current.fetchNow();
    }, BUMP_DELAY);
  }, []);

  return { stats, bump };
}
