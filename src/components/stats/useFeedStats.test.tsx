import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, renderHook, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import type { FeedStats, StatsFilter } from "../../types";

const feedStats = vi.fn<(f: StatsFilter) => Promise<FeedStats>>();
vi.mock("../../api", () => ({
  api: { feedStats: (f: StatsFilter) => feedStats(f) },
  errText: (e: unknown) => String(e),
}));

import { BUMP_DELAY, useFeedStats, type FeedStatsOptions } from "./useFeedStats";
import { ToastProvider } from "../Toast";

/** The stats as the backend would answer, `videos` set to tell answers apart. */
function stats(videos: number): FeedStats {
  return {
    videos, series: 0, toWatch: videos, watched: 0, downloaded: 0, hidden: 0,
    runtimeSecs: 0, runtimeUnknown: false, newThisWeek: 0, today: "2026-10-19", days: [],
  };
}

/** A request that stays out until the test answers it. */
function held() {
  let resolve!: (s: FeedStats) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<FeedStats>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

const wrapper = ({ children }: { children: ReactNode }) => <ToastProvider>{children}</ToastProvider>;

/** The shell's pick, held once: it keeps its identity there until it changes. */
const EVERY: string[] = [];
const ONE = ["UC1"];

function mount(initial: Partial<FeedStatsOptions> = {}) {
  const base: FeedStatsOptions = {
    enabled: true, channelIds: EVERY, search: "", reloadToken: 0, ...initial,
  };
  const hook = renderHook((p: FeedStatsOptions) => useFeedStats(p), { initialProps: base, wrapper });
  return {
    ...hook,
    /** Re-renders with these options changed, the rest as first mounted. */
    set: (next: Partial<FeedStatsOptions>) => hook.rerender({ ...base, ...next }),
  };
}

/** Lets every answer already given land. */
const settle = () => act(async () => {});

beforeEach(() => {
  vi.clearAllMocks();
  feedStats.mockImplementation(() => Promise.resolve(stats(1)));
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("useFeedStats", () => {
  it("asks for nothing, and holds nothing, while off", async () => {
    const { result } = mount({ enabled: false });
    await settle();
    expect(feedStats).not.toHaveBeenCalled();
    expect(result.current.stats).toBeNull();
    // A bump while off is nothing too.
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    act(() => { result.current.bump(); });
    act(() => { vi.advanceTimersByTime(BUMP_DELAY * 2); });
    expect(feedStats).not.toHaveBeenCalled();
  });

  it("asks with the pick and the trimmed search, and none for a blank box", async () => {
    const { result, set } = mount({ channelIds: ONE, search: "  tapes " });
    await settle();
    expect(feedStats).toHaveBeenLastCalledWith({ channelIds: ["UC1"], search: "tapes" });
    expect(result.current.stats).toEqual(stats(1));

    set({ channelIds: ONE, search: "   " });
    await settle();
    expect(feedStats).toHaveBeenLastCalledWith({ channelIds: ["UC1"], search: null });
  });

  it("asks again on a reload, a new pick and a new search, and not for a trailing space", async () => {
    const { set } = mount({ search: "tapes" });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
    set({ search: "tapes", reloadToken: 1 });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
    set({ search: "tapes", reloadToken: 1, channelIds: ONE });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(3);
    set({ search: "tapes ", reloadToken: 1, channelIds: ONE });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(3);
    set({ search: "blackwood", reloadToken: 1, channelIds: ONE });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(4);
    expect(feedStats).toHaveBeenLastCalledWith({ channelIds: ["UC1"], search: "blackwood" });
  });

  it("drops an answer that a newer question overtook", async () => {
    const first = held();
    const second = held();
    feedStats.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { result, set } = mount();
    await settle();
    set({ channelIds: ONE });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);

    await act(async () => { second.resolve(stats(2)); });
    await act(async () => { first.resolve(stats(1)); });
    expect(result.current.stats?.videos).toBe(2);
  });

  it("keeps the last stats up while the next are out", async () => {
    const { result, set } = mount();
    await settle();
    expect(result.current.stats?.videos).toBe(1);

    const next = held();
    feedStats.mockReturnValueOnce(next.promise);
    set({ reloadToken: 1 });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
    expect(result.current.stats?.videos).toBe(1);

    await act(async () => { next.resolve(stats(7)); });
    expect(result.current.stats?.videos).toBe(7);
  });

  it("keeps the last good stats when a read fails, and says nothing", async () => {
    const { result, set } = mount();
    await settle();
    feedStats.mockImplementationOnce(() => Promise.reject("database is locked"));
    set({ reloadToken: 1 });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
    expect(result.current.stats?.videos).toBe(1);
    expect(screen.queryByText("database is locked")).toBeNull();
    expect(screen.getByRole("status").children).toHaveLength(0);
  });

  it("drops the stats when switched off, and an answer still out with them", async () => {
    const { result, set } = mount();
    await settle();
    const late = held();
    feedStats.mockReturnValueOnce(late.promise);
    set({ reloadToken: 1 });
    await settle();
    set({ enabled: false, reloadToken: 1 });
    await settle();
    expect(result.current.stats).toBeNull();
    await act(async () => { late.resolve(stats(9)); });
    expect(result.current.stats).toBeNull();
  });

  it("keeps the stats but asks nothing while paused, and asks at once when it resumes", async () => {
    const { result, set } = mount();
    await settle();
    set({ paused: true, reloadToken: 1 });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
    expect(result.current.stats?.videos).toBe(1);

    feedStats.mockImplementation(() => Promise.resolve(stats(3)));
    set({ paused: false, reloadToken: 1 });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
    expect(result.current.stats?.videos).toBe(3);
  });

  it("answers a burst of bumps with one request, a beat after the last", async () => {
    const { result } = mount();
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
    const bump = result.current.bump;

    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    act(() => { result.current.bump(); });
    act(() => { vi.advanceTimersByTime(BUMP_DELAY - 100); });
    act(() => { result.current.bump(); });
    act(() => { vi.advanceTimersByTime(BUMP_DELAY - 100); });
    act(() => { result.current.bump(); });
    act(() => { vi.advanceTimersByTime(BUMP_DELAY - 1); });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
    act(() => { vi.advanceTimersByTime(1); });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
    // Still the same function after every render the answers caused.
    expect(result.current.bump).toBe(bump);
  });

  it("lets a reload answer a bump still waiting", async () => {
    const { result, set } = mount();
    await settle();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    act(() => { result.current.bump(); });
    set({ reloadToken: 1 });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
    act(() => { vi.advanceTimersByTime(BUMP_DELAY * 2); });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(2);
  });

  it("forgets a bump while paused: resuming asks anyway", async () => {
    const { result, set } = mount({ paused: true });
    await settle();
    expect(feedStats).not.toHaveBeenCalled();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    act(() => { result.current.bump(); });
    act(() => { vi.advanceTimersByTime(BUMP_DELAY * 2); });
    await settle();
    expect(feedStats).not.toHaveBeenCalled();
    set({ paused: false });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
  });

  it("sends nothing for a bump still waiting when it unmounts", async () => {
    const { result, unmount } = mount();
    await settle();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    act(() => { result.current.bump(); });
    unmount();
    act(() => { vi.advanceTimersByTime(BUMP_DELAY * 2); });
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
  });
});
