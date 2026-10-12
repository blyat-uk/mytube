import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";
import {
  BOOT_WAIT_MS, TEXT_SCALE, applyTextScale, bootTextScale, normalizeTextScale,
} from "./textScale";

const scale = () => document.documentElement.style.getPropertyValue("--text-scale");

beforeEach(() => document.documentElement.style.removeProperty("--text-scale"));
afterEach(() => vi.useRealTimers());

describe("the text size range", () => {
  it("runs 80% to 200% in steps of 10, from 100%", () => {
    expect(TEXT_SCALE).toEqual({ min: 80, max: 200, step: 10, default: 100 });
  });

  it("lands any number on a step inside the range, rounding as Rust does", () => {
    expect(normalizeTextScale(150)).toBe(150);
    expect(normalizeTextScale(84)).toBe(80);
    expect(normalizeTextScale(85)).toBe(90);
    expect(normalizeTextScale(126)).toBe(130);
    expect(normalizeTextScale(0)).toBe(80);
    expect(normalizeTextScale(-40)).toBe(80);
    expect(normalizeTextScale(9999)).toBe(200);
  });

  it("reads anything that is not a number as 100%", () => {
    expect(normalizeTextScale(Number.NaN)).toBe(100);
    expect(normalizeTextScale(Number.POSITIVE_INFINITY)).toBe(100);
    expect(normalizeTextScale(undefined as unknown as number)).toBe(100);
  });
});

describe("applyTextScale", () => {
  it("sets --text-scale on the root as a unitless factor", () => {
    applyTextScale(150);
    expect(scale()).toBe("1.5");
    applyTextScale(80);
    expect(scale()).toBe("0.8");
    applyTextScale(200);
    expect(scale()).toBe("2");
    applyTextScale(100);
    expect(scale()).toBe("1");
  });

  it("never sets a size the slider could not show", () => {
    applyTextScale(126);
    expect(scale()).toBe("1.3");
    applyTextScale(5000);
    expect(scale()).toBe("2");
  });
});

describe("bootTextScale", () => {
  const settings = (text_scale: number) => Promise.resolve({ text_scale });

  it("applies the stored size before it lets the first render go", async () => {
    await bootTextScale(() => settings(140));
    expect(scale()).toBe("1.4");
  });

  it("lets the render go at 100% when there are no settings to read", async () => {
    // `bun run dev` in a browser: no IPC bridge, so every invoke rejects.
    await bootTextScale(() => Promise.reject(new TypeError("no __TAURI_INTERNALS__")));
    expect(scale()).toBe("");
  });

  it("treats a read that throws outright as a failed one", async () => {
    await bootTextScale(() => { throw new Error("no bridge"); });
    expect(scale()).toBe("");
  });

  it("stops waiting for a read that never answers", async () => {
    vi.useFakeTimers();
    let settled = false;
    void bootTextScale(() => new Promise(() => {})).then(() => { settled = true; });
    await vi.advanceTimersByTimeAsync(BOOT_WAIT_MS - 1);
    expect(settled).toBe(false);
    await vi.advanceTimersByTimeAsync(1);
    expect(settled).toBe(true);
    expect(scale()).toBe("");
  });

  it("still applies a read that lands after the render went ahead", async () => {
    vi.useFakeTimers();
    let answer: (s: { text_scale: number }) => void = () => {};
    const done = bootTextScale(() => new Promise((r) => { answer = r; }));
    await vi.advanceTimersByTimeAsync(BOOT_WAIT_MS);
    await done;
    answer({ text_scale: 120 });
    await vi.waitFor(() => expect(scale()).toBe("1.2"));
  });

  it("leaves alone a size picked on the Appearance page before a late read lands", async () => {
    vi.useFakeTimers();
    let answer: (s: { text_scale: number }) => void = () => {};
    const done = bootTextScale(() => new Promise((r) => { answer = r; }));
    await vi.advanceTimersByTimeAsync(BOOT_WAIT_MS);
    await done;
    applyTextScale(170);
    answer({ text_scale: 120 });
    await Promise.resolve();
    await Promise.resolve();
    expect(scale()).toBe("1.7");
  });
});
