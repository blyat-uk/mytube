import { describe, expect, it } from "vitest";
import {
  normalizeIds, pickedFirst, pickLabel, pickSpoken, pruneIds, togglePick,
} from "./channelPick";
import type { Channel } from "./types";

const ch = (id: string, title: string): Channel => ({
  id, title, handle: null, url: "", thumb_path: null, subscribed: true, member: false,
  auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
});
// The order `list_channels` answers in: by title, ignoring case.
const CHANNELS = [ch("UCb", "Alpha"), ch("UCc", "Beta"), ch("UCa", "Gamma")];

describe("normalizeIds", () => {
  it("sorts, drops duplicates and empty ids", () => {
    // Sorted so that A-then-B and B-then-A save the same view: the shell
    // compares the serialised view to decide whether to write it.
    expect(normalizeIds(["UCc", "", "UCa", "UCc"])).toEqual(["UCa", "UCc"]);
  });
});

describe("togglePick", () => {
  it("adds a channel in sorted place, and takes it away again", () => {
    const on = togglePick(["UCc"], "UCa");
    expect(on).toEqual(["UCa", "UCc"]);
    expect(togglePick(on, "UCc")).toEqual(["UCa"]);
  });
});

describe("pickLabel", () => {
  it("reads All channels with nothing picked", () => {
    expect(pickLabel(CHANNELS, [])).toEqual({ name: "All channels", more: 0 });
  });

  it("names the first pick in list order, not id order, and counts the rest", () => {
    // UCa sorts first by id but is "Gamma", last by title.
    expect(pickLabel(CHANNELS, ["UCa", "UCc"])).toEqual({ name: "Beta", more: 1 });
    expect(pickLabel(CHANNELS, ["UCc"])).toEqual({ name: "Beta", more: 0 });
  });

  it("counts picks it cannot name yet, before the channel list arrives", () => {
    expect(pickLabel([], ["UCa", "UCc"])).toEqual({ name: "2 channels", more: 0 });
    expect(pickLabel([], ["UCa"])).toEqual({ name: "1 channel", more: 0 });
  });
});

describe("pickSpoken", () => {
  it("says what the button shows, in words", () => {
    expect(pickSpoken(CHANNELS, [])).toBe("All channels");
    expect(pickSpoken(CHANNELS, ["UCc"])).toBe("Beta");
    expect(pickSpoken(CHANNELS, ["UCa", "UCb", "UCc"])).toBe("Alpha and 2 more");
  });
});

describe("pickedFirst", () => {
  it("puts the picked channels first, each part keeping the list's own order", () => {
    const order = pickedFirst(CHANNELS, new Set(["UCa", "UCc"])).map((c) => c.title);
    expect(order).toEqual(["Beta", "Gamma", "Alpha"]);
  });
});

describe("pruneIds", () => {
  it("drops channels that are no longer in the list", () => {
    expect(pruneIds(["UCa", "UCgone"], CHANNELS)).toEqual(["UCa"]);
  });

  it("hands back the very same array when nothing was dropped", () => {
    // Identity matters: a fresh array on every channel refresh would read as a
    // new filter and refetch the feed for nothing.
    const ids = ["UCa", "UCb"];
    expect(pruneIds(ids, CHANNELS)).toBe(ids);
  });
});
