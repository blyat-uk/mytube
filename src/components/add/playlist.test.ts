import { describe, it, expect } from "vitest";
import { countTicked, partState, tallyPlaylist, tickedEntries, wantsDownload } from "./playlist";
import type { PlaylistEntry } from "../../types";

function entry(id: string, over: Partial<PlaylistEntry> = {}): PlaylistEntry {
  return {
    id, title: id, channelId: null, channelTitle: null, durationSecs: 600,
    thumbUrl: "", thumbPath: null, inLibrary: false, downloadState: "none", membersOnly: false,
    ...over,
  };
}

const PARTS = [
  entry("a", { inLibrary: true, downloadState: "done" }),
  entry("b", { inLibrary: true, downloadState: "failed" }),
  entry("c", { inLibrary: true, downloadState: "queued" }),
  entry("d", { durationSecs: null }),
  entry("e", { membersOnly: true }),
];

describe("a playlist's counts", () => {
  it("downloads what is not on disk and not on its way, a failure included", () => {
    expect(["none", "failed"].every((s) => wantsDownload(s as never))).toBe(true);
    expect(["queued", "downloading", "done"].some((s) => wantsDownload(s as never))).toBe(false);
  });

  it("tallies the whole playlist for its tags, ticked or not", () => {
    expect(tallyPlaylist(PARTS)).toEqual({ fresh: 2, inLibrary: 3, downloaded: 1, membersOnly: 1 });
  });

  it("counts only the ticked parts for the buttons", () => {
    const all = countTicked(PARTS, new Set(PARTS.map((e) => e.id)));
    expect(all).toMatchObject({ ticked: 5, toAdd: 2, toDownload: 3, downloaded: 1 });
    // A part whose length is not known yet adds nothing, and is counted apart.
    expect(all.runtimeSecs).toBe(2400);
    expect(all.unknownRuntime).toBe(1);
    expect(countTicked(PARTS, new Set(["a", "e"]))).toMatchObject({ ticked: 2, toAdd: 1, toDownload: 1 });
    expect(countTicked(PARTS, new Set())).toMatchObject({ ticked: 0, toAdd: 0, toDownload: 0 });
  });

  it("hands the ticked parts on in the playlist's own order", () => {
    expect(tickedEntries(PARTS, new Set(["e", "a", "c"])).map((e) => e.id)).toEqual(["a", "c", "e"]);
  });

  it("names a part by what matters most about it", () => {
    expect(PARTS.map(partState)).toEqual(["downloaded", "in library", "downloading", "new", "members"]);
  });
});
