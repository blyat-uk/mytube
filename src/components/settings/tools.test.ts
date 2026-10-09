import { describe, it, expect } from "vitest";
import { TOOL_LABEL, toolProgressPercent, toolProgressText, toolsReadyMessage } from "./tools";
import type { ToolProgress, ToolStatus } from "../../types";

const tool = (over: Partial<ToolStatus> & Pick<ToolStatus, "kind">): ToolStatus => ({
  path: null, version: null, source: "missing", state: "ready", error: null, lastCheck: null,
  ...over,
});

describe("TOOL_LABEL", () => {
  it("names each tool as the Rust side does", () => {
    expect(TOOL_LABEL).toEqual({ ytdlp: "yt-dlp", ffmpeg: "ffmpeg", deno: "deno" });
  });
});

describe("toolsReadyMessage", () => {
  const states = (...pairs: [ToolStatus["kind"], ToolStatus["state"]][]) =>
    new Map(pairs);

  it("announces one tool that finished installing", () => {
    const next = [tool({ kind: "ytdlp", state: "ready" })];
    expect(toolsReadyMessage(states(["ytdlp", "installing"]), next)).toBe("yt-dlp is ready.");
  });

  it("lists several in one sentence", () => {
    const next = [
      tool({ kind: "ytdlp", state: "ready" }),
      tool({ kind: "ffmpeg", state: "ready" }),
      tool({ kind: "deno", state: "ready" }),
    ];
    const prev = states(["ytdlp", "installing"], ["ffmpeg", "installing"], ["deno", "installing"]);
    expect(toolsReadyMessage(prev, next)).toBe("yt-dlp, ffmpeg and deno are ready.");
  });

  /** An update is housekeeping the user never asked to hear about; only a
   *  first install changes what the app can do. */
  it("stays quiet for an update, a failure, and a tool already ready", () => {
    const next = [
      tool({ kind: "ytdlp", state: "ready" }),
      tool({ kind: "ffmpeg", state: "error" }),
      tool({ kind: "deno", state: "ready" }),
    ];
    const prev = states(["ytdlp", "updating"], ["ffmpeg", "installing"], ["deno", "ready"]);
    expect(toolsReadyMessage(prev, next)).toBeNull();
  });
});

describe("toolProgressText", () => {
  const MB = 1024 * 1024;
  const p = (over: Partial<ToolProgress>): ToolProgress => ({
    tool: "ytdlp", phase: "download", received: 0, total: null, ...over,
  });

  it("says it is starting before the first byte count arrives", () => {
    expect(toolProgressText("installing")).toBe("Installing · Starting…");
    expect(toolProgressText("updating")).toBe("Updating · Starting…");
  });

  it("counts against the total when there is one", () => {
    expect(toolProgressText("updating", p({ received: 18.2 * MB, total: 34.1 * MB })))
      .toBe("Updating · 18 MB of 34 MB");
    expect(toolProgressText("installing", p({ received: 5 * MB, total: 8 * MB })))
      .toBe("Installing · 5.0 MB of 8.0 MB");
  });

  it("gives the bytes alone when the download has no length", () => {
    expect(toolProgressText("installing", p({ received: 3 * MB }))).toBe("Installing · 3.0 MB");
  });

  it("says unpacking once the download is in", () => {
    expect(toolProgressText("installing", p({ phase: "extract" }))).toBe("Installing · Unpacking…");
  });
});

describe("toolProgressPercent", () => {
  it("measures against the total, never past it", () => {
    expect(toolProgressPercent({ tool: "ffmpeg", phase: "download", received: 25, total: 100 })).toBe(25);
    expect(toolProgressPercent({ tool: "ffmpeg", phase: "download", received: 120, total: 100 })).toBe(100);
  });

  /** The bar travels instead: a made-up fraction would be worse than none. */
  it("has no number without a total: before the first event, no length, unpacking", () => {
    expect(toolProgressPercent(undefined)).toBeNull();
    expect(toolProgressPercent({ tool: "ffmpeg", phase: "download", received: 99, total: null })).toBeNull();
    expect(toolProgressPercent({ tool: "ffmpeg", phase: "extract", received: 0, total: null })).toBeNull();
  });
});
