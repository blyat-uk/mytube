import { describe, it, expect } from "vitest";
import { formatDuration, formatRelative, formatViews, cardAction } from "./format";

describe("formatDuration", () => {
  it("formats minutes and seconds", () => expect(formatDuration(754)).toBe("12:34"));
  it("formats hours", () => expect(formatDuration(3661)).toBe("1:01:01"));
  it("pads seconds", () => expect(formatDuration(65)).toBe("1:05"));
  it("handles null and zero", () => {
    expect(formatDuration(null)).toBe("");
    expect(formatDuration(0)).toBe("");
  });
});

describe("formatRelative", () => {
  const now = 1_700_000_000;
  it("uses coarse buckets", () => {
    expect(formatRelative(now - 30, now)).toBe("just now");
    expect(formatRelative(now - 3600 * 5, now)).toBe("5h ago");
    expect(formatRelative(now - 86400 * 3, now)).toBe("3d ago");
    expect(formatRelative(now - 86400 * 14, now)).toBe("2w ago");
    expect(formatRelative(now - 86400 * 400, now)).toBe("1y ago");
  });
  it("renders unknown dates as an em dash", () => expect(formatRelative(null, now)).toBe("—"));
});

describe("formatViews", () => {
  it("abbreviates", () => {
    expect(formatViews(999)).toBe("999");
    expect(formatViews(1500)).toBe("1.5K");
    expect(formatViews(1_800_000)).toBe("1.8M");
    expect(formatViews(null)).toBe("");
  });
});

describe("cardAction", () => {
  it("downloads when there is no local file", () =>
    expect(cardAction({ download_state: "none", file_path: null } as any)).toBe("download"));
  it("plays when downloaded", () =>
    expect(cardAction({ download_state: "done", file_path: "/x.mkv" } as any)).toBe("play"));
  it("cancels while downloading", () => {
    expect(cardAction({ download_state: "downloading", file_path: null } as any)).toBe("cancel");
    expect(cardAction({ download_state: "queued", file_path: null } as any)).toBe("cancel");
  });
  it("retries after a failure", () =>
    expect(cardAction({ download_state: "failed", file_path: null } as any)).toBe("retry"));
  it("re-downloads when the state says done but the path is gone", () =>
    expect(cardAction({ download_state: "done", file_path: null } as any)).toBe("download"));
  it("treats a manually added video no differently once downloaded", () =>
    expect(cardAction({ download_state: "done", file_path: "/x.mkv", added_manually: true } as any))
      .toBe("play"));
});

import { clampCardSize, nextCardSize, insertToken, TEMPLATE_PRESETS, TOKEN_CHIPS } from "./format";

describe("card size zoom", () => {
  it("clamps to the supported range", () => {
    expect(clampCardSize(10)).toBe(160);
    expect(clampCardSize(9999)).toBe(460);
    expect(clampCardSize(300)).toBe(300);
    expect(clampCardSize(Number.NaN)).toBe(260);
  });
  it("steps up when scrolling up and down when scrolling down", () => {
    expect(nextCardSize(260, -1)).toBeGreaterThan(260);
    expect(nextCardSize(260, 1)).toBeLessThan(260);
  });
  it("cannot step outside the range", () => {
    expect(nextCardSize(160, 1)).toBe(160);
    expect(nextCardSize(460, -1)).toBe(460);
  });
});

describe("filename template helpers", () => {
  it("inserts a token at the caret", () => {
    expect(insertToken("a/b.%(ext)s", 2, "%(id)s")).toEqual({
      value: "a/%(id)sb.%(ext)s",
      caret: 2 + "%(id)s".length,
    });
  });
  it("appends when the caret is at the end", () => {
    expect(insertToken("abc", 3, "X").value).toBe("abcX");
  });
  it("treats a null caret as the end of the string", () => {
    expect(insertToken("abc", null, "X").value).toBe("abcX");
  });
  it("offers presets that all produce an extension", () => {
    expect(TEMPLATE_PRESETS.length).toBeGreaterThanOrEqual(4);
    for (const p of TEMPLATE_PRESETS) expect(p.value).toContain("%(ext)s");
  });
  it("offers token chips that are all yt-dlp fields", () => {
    expect(TOKEN_CHIPS.length).toBeGreaterThanOrEqual(6);
    for (const t of TOKEN_CHIPS) expect(t).toMatch(/^%\(.+\)[sd]$/);
  });
});
