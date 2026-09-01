import { describe, it, expect } from "vitest";
import {
  formatDuration, formatTotalRuntime, formatRelative, formatViews, cardAction, videoUrl,
  hasDownloadedFile, seriesRuntime,
} from "./format";

describe("formatDuration", () => {
  it("formats minutes and seconds", () => expect(formatDuration(754)).toBe("12:34"));
  it("formats hours", () => expect(formatDuration(3661)).toBe("1:01:01"));
  it("pads seconds", () => expect(formatDuration(65)).toBe("1:05"));
  it("handles null and zero", () => {
    expect(formatDuration(null)).toBe("");
    expect(formatDuration(0)).toBe("");
  });
});

/**
 * A series total is a sum, not a timestamp: seconds across seven parts are
 * noise, and the words match the register of the panel it sits in ("7 parts",
 * "3 watched"). `formatDuration` stays the clock-style form for one video.
 */
describe("formatTotalRuntime", () => {
  it("gives hours and minutes", () => expect(formatTotalRuntime(15000)).toBe("4h 10m"));
  it("drops the hours when there are none", () =>
    expect(formatTotalRuntime(2880)).toBe("48m"));
  it("drops seconds rather than rounding a total up", () =>
    expect(formatTotalRuntime(3659)).toBe("1h"));
  it("says just the hours when they land clean, not a bare zero minutes", () =>
    expect(formatTotalRuntime(7200)).toBe("2h"));
  it("keeps seconds only under a minute, where minutes would read as zero", () =>
    expect(formatTotalRuntime(35)).toBe("35s"));
  it("has nothing to say about nothing", () => {
    expect(formatTotalRuntime(0)).toBe("");
    expect(formatTotalRuntime(-5)).toBe("");
  });
});

/**
 * The series card and the nav's breadcrumb both quote a series' runtime, so
 * they add it up in one place -- two sums of the same parts that disagreed
 * would be worse than no number at all.
 */
describe("seriesRuntime", () => {
  const parts = (...secs: (number | null)[]) => secs.map((duration_secs) => ({ duration_secs }));

  it("adds the parts up", () => {
    expect(seriesRuntime(parts(600, 1200, 600))).toEqual({ runtime: "40m", unknown: 0 });
  });
  it("counts out the parts with no runtime yet, rather than quietly dropping them", () => {
    expect(seriesRuntime(parts(3600, null, null))).toEqual({ runtime: "1h", unknown: 2 });
  });
  it("has no runtime for a series that has resolved none of its parts", () => {
    expect(seriesRuntime(parts(null, null))).toEqual({ runtime: "", unknown: 2 });
  });
  it("treats a zero duration as unresolved, which is what it means", () => {
    expect(seriesRuntime(parts(0, 60))).toEqual({ runtime: "1m", unknown: 1 });
  });
  it("says nothing about an empty series", () => {
    expect(seriesRuntime([])).toEqual({ runtime: "", unknown: 0 });
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

describe("hasDownloadedFile", () => {
  it("is true only when the download finished and the file is still recorded", () => {
    expect(hasDownloadedFile({ download_state: "done", file_path: "/v.mkv" } as any)).toBe(true);
  });
  it("is false once the file has been deleted out from under the row", () => {
    expect(hasDownloadedFile({ download_state: "done", file_path: null } as any)).toBe(false);
  });
  it("is false for every other download state", () => {
    for (const download_state of ["none", "queued", "downloading", "failed"] as const) {
      expect(hasDownloadedFile({ download_state, file_path: "/v.mkv" } as any)).toBe(false);
    }
  });
});

describe("videoUrl", () => {
  it("points at the YouTube watch page", () =>
    expect(videoUrl({ id: "J1WoNuemKOg" })).toBe("https://www.youtube.com/watch?v=J1WoNuemKOg"));
  it("escapes anything odd in the id", () =>
    expect(videoUrl({ id: "a&b=c" })).toBe("https://www.youtube.com/watch?v=a%26b%3Dc"));
});

describe("card size zoom", () => {
  it("clamps to the supported range", () => {
    expect(clampCardSize(10)).toBe(160);
    expect(clampCardSize(9999)).toBe(640);
    expect(clampCardSize(300)).toBe(300);
    expect(clampCardSize(Number.NaN)).toBe(260);
  });
  it("steps up when scrolling up and down when scrolling down", () => {
    expect(nextCardSize(260, -1)).toBeGreaterThan(260);
    expect(nextCardSize(260, 1)).toBeLessThan(260);
  });
  it("cannot step outside the range", () => {
    expect(nextCardSize(160, 1)).toBe(160);
    expect(nextCardSize(640, -1)).toBe(640);
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
