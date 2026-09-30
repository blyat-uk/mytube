import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, renderHook, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { SLOW_AFTER, singleClick, usePending } from "./pending";
import { createProgressStore } from "./progress";
import { ToastProvider } from "./components/Toast";

const wrapper = ({ children }: { children: ReactNode }) => <ToastProvider>{children}</ToastProvider>;

/** A call that stays out until the test says otherwise. */
function deferred() {
  let resolve = () => {};
  let reject = (_e: unknown) => {};
  const promise = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

beforeEach(() => { vi.useFakeTimers(); });
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("usePending", () => {
  it("runs a key once while its first call is still out", async () => {
    const { result } = renderHook(() => usePending(), { wrapper });
    const call = deferred();
    const fn = vi.fn(() => call.promise);

    let first!: Promise<boolean>;
    let second!: Promise<boolean>;
    act(() => {
      first = result.current.run("vid", fn);
      // The same frame, before any re-render: the ref is what refuses it.
      second = result.current.run("vid", fn);
    });
    expect(fn).toHaveBeenCalledTimes(1);
    await act(async () => { call.resolve(); });
    expect(await first).toBe(true);
    expect(await second).toBe(false);

    // Once it has landed, the key is free again.
    await act(async () => { await result.current.run("vid", () => Promise.resolve()); });
    expect(result.current.pending.has("vid")).toBe(false);
  });

  it("keeps different keys apart", () => {
    const { result } = renderHook(() => usePending(), { wrapper });
    const a = vi.fn(() => new Promise<void>(() => {}));
    const b = vi.fn(() => new Promise<void>(() => {}));
    act(() => {
      void result.current.run("a", a);
      void result.current.run("b", b);
    });
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
    expect([...result.current.pending].sort()).toEqual(["a", "b"]);
  });

  it("is pending at once, but slow only after the threshold", async () => {
    const { result } = renderHook(() => usePending(), { wrapper });
    const call = deferred();
    act(() => { void result.current.run("vid", () => call.promise); });
    expect(result.current.pending.has("vid")).toBe(true);
    expect(result.current.slow.has("vid")).toBe(false);

    act(() => { vi.advanceTimersByTime(SLOW_AFTER - 1); });
    // A fast call never flashes a spinner.
    expect(result.current.slow.has("vid")).toBe(false);
    act(() => { vi.advanceTimersByTime(1); });
    expect(result.current.slow.has("vid")).toBe(true);

    await act(async () => { call.resolve(); });
    expect(result.current.pending.has("vid")).toBe(false);
    expect(result.current.slow.has("vid")).toBe(false);
  });

  it("never turns slow for a call that answered in time", async () => {
    const { result } = renderHook(() => usePending(), { wrapper });
    await act(async () => { await result.current.run("vid", () => Promise.resolve()); });
    act(() => { vi.advanceTimersByTime(SLOW_AFTER * 4); });
    expect(result.current.slow.size).toBe(0);
  });

  it("applies the optimistic change first and keeps it on success", async () => {
    const { result } = renderHook(() => usePending(), { wrapper });
    const order: string[] = [];
    const undo = vi.fn();
    await act(async () => {
      await result.current.run("vid", async () => { order.push("call"); }, {
        optimistic: () => { order.push("shown"); return undo; },
      });
    });
    expect(order).toEqual(["shown", "call"]);
    expect(undo).not.toHaveBeenCalled();
  });

  it("rolls back and says why when the call fails", async () => {
    const { result } = renderHook(() => usePending(), { wrapper });
    const undo = vi.fn();
    let ok: boolean | undefined;
    await act(async () => {
      ok = await result.current.run("vid", () => Promise.reject("yt-dlp is gone"), {
        optimistic: () => undo,
      });
    });
    expect(ok).toBe(false);
    expect(undo).toHaveBeenCalledTimes(1);
    expect(screen.getByText("yt-dlp is gone")).toBeTruthy();
    expect(result.current.pending.size).toBe(0);
  });
});

describe("singleClick", () => {
  it("drops the second click of a double-click and passes keyboard presses", () => {
    const fn = vi.fn();
    const onClick = singleClick(fn);
    onClick({ detail: 1 });
    onClick({ detail: 2 });
    onClick({ detail: 0 });
    expect(fn).toHaveBeenCalledTimes(2);
  });
});

describe("progress store", () => {
  it("wakes only the video that ticked", () => {
    const store = createProgressStore();
    const a = vi.fn();
    const b = vi.fn();
    store.subscribe("a", a);
    const stopB = store.subscribe("b", b);
    store.set({ videoId: "a", percent: 10, speed: "", eta: "" });
    expect(a).toHaveBeenCalledTimes(1);
    expect(b).not.toHaveBeenCalled();
    expect(store.get("a")?.percent).toBe(10);

    store.clear("a");
    expect(store.get("a")).toBeUndefined();
    // Clearing what is not there wakes nobody.
    store.clear("b");
    expect(b).not.toHaveBeenCalled();
    stopB();
  });
});
