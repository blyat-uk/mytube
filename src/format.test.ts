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
