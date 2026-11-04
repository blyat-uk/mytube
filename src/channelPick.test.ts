import { describe, expect, it } from "vitest";
import {
  EVERY_CHANNEL, isShown, normalizeIds, pickedFirst, pickLabel, pickSpoken, pickTitles, pruneIds,
  shownCount, tickChannel, type ChannelPick,
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

const only = (...ids: string[]): ChannelPick => ({ ids, except: false });
const but = (...ids: string[]): ChannelPick => ({ ids, except: true });

describe("isShown", () => {
  it("shows every channel with nothing picked", () => {
    expect(CHANNELS.every((c) => isShown(EVERY_CHANNEL, c.id))).toBe(true);
  });

  it("shows the picked ones, or all but the ones left out", () => {
    expect(isShown(only("UCa"), "UCa")).toBe(true);
    expect(isShown(only("UCa"), "UCb")).toBe(false);
    expect(isShown(but("UCa"), "UCa")).toBe(false);
    expect(isShown(but("UCa"), "UCb")).toBe(true);
    // A channel subscribed after a few were left out is shown.
    expect(isShown(but("UCa"), "UCnew")).toBe(true);
  });
});

describe("tickChannel", () => {
  it("leaves a channel out when it is unticked from all of them", () => {
    expect(tickChannel(EVERY_CHANNEL, "UCa", CHANNELS)).toEqual(but("UCa"));
    expect(tickChannel(but("UCa"), "UCc", CHANNELS)).toEqual(but("UCa", "UCc"));
    expect(shownCount(but("UCa", "UCc"), CHANNELS)).toBe(1);
  });

  it("ticks one left out back in, and every channel back is every channel", () => {
    expect(tickChannel(but("UCa", "UCc"), "UCc", CHANNELS)).toEqual(but("UCa"));
    expect(tickChannel(but("UCa"), "UCa", CHANNELS)).toBe(EVERY_CHANNEL);
  });

  it("adds to a pick of a few in sorted place, and takes one away", () => {
    expect(tickChannel(only("UCc"), "UCa", CHANNELS)).toEqual(only("UCa", "UCc"));
    expect(tickChannel(only("UCa", "UCc"), "UCc", CHANNELS)).toEqual(only("UCa"));
    // Ticking the last one in shows them all.
    expect(tickChannel(only("UCa", "UCc"), "UCb", CHANNELS)).toBe(EVERY_CHANNEL);
  });

  it("never unticks the last channel shown", () => {
    const one = only("UCa");
    expect(tickChannel(one, "UCa", CHANNELS)).toBe(one);
    const left = but("UCa", "UCb");
    expect(tickChannel(left, "UCc", CHANNELS)).toBe(left);
  });
});

describe("pickLabel", () => {
  it("reads All channels with nothing picked", () => {
    expect(pickLabel(CHANNELS, EVERY_CHANNEL)).toEqual({ name: "All channels", more: 0 });
  });

  it("names the first pick in list order, not id order, and counts the rest", () => {
    // UCa sorts first by id but is "Gamma", last by title.
    expect(pickLabel(CHANNELS, only("UCa", "UCc"))).toEqual({ name: "Beta", more: 1 });
    expect(pickLabel(CHANNELS, only("UCc"))).toEqual({ name: "Beta", more: 0 });
  });

  it("counts picks it cannot name yet, before the channel list arrives", () => {
    expect(pickLabel([], only("UCa", "UCc"))).toEqual({ name: "2 channels", more: 0 });
    expect(pickLabel([], only("UCa"))).toEqual({ name: "1 channel", more: 0 });
  });

  it("names the one channel left out, and counts several", () => {
    expect(pickLabel(CHANNELS, but("UCa"))).toEqual({ name: "All but Gamma", more: 0 });
    expect(pickLabel(CHANNELS, but("UCa", "UCb"))).toEqual({ name: "All but 2", more: 0 });
    expect(pickTitles(CHANNELS, but("UCa", "UCb"))).toBe("All channels except:\nAlpha\nGamma");
  });
});

describe("pickSpoken", () => {
  it("says what the button shows, in words", () => {
    expect(pickSpoken(CHANNELS, EVERY_CHANNEL)).toBe("All channels");
    expect(pickSpoken(CHANNELS, only("UCc"))).toBe("Beta");
    expect(pickSpoken(CHANNELS, only("UCa", "UCb", "UCc"))).toBe("Alpha and 2 more");
    expect(pickSpoken(CHANNELS, but("UCa", "UCb"))).toBe("all channels except Alpha and Gamma");
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
