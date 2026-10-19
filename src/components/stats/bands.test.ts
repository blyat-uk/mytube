import { describe, it, expect } from "vitest";
import type { Video } from "../../types";
import {
  bandGap, bandStates, bandWeights, cssUrl, nextIndex, placeAround, runtimeOf, seriesFigures,
  seriesModel, uploadOrder,
} from "./bands";

function video(over: Partial<Video> = {}): Video {
  return {
    id: "p1", channel_id: "UC1", channel_title: "Chills Narrated",
    title: "The Blackwood Tapes", description: null, thumb_url: null, thumb_path: null,
    published_at: 1_700_000_000, sort_at: 1_700_000_000, feed_rank: 0,
    added_manually: false, duration_secs: 3600, view_count: 1500,
    status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "none", download_error: null,
    file_path: null, downloaded_at: null, first_seen_at: 0, sibling_group: null, series_hidden: false,
    part_links_refused: false, part_linked: false,
    ...over,
  };
}

/** A series from a pattern, one letter per part in upload order: W watched,
 *  L left, H hidden (and not watched), X hidden after being watched. A
 *  lowercase letter is the same part, downloaded. Every part runs an hour. */
function series(pattern: string, over: (i: number) => Partial<Video> = () => ({})): Video[] {
  return [...pattern].map((c, i) => video({
    id: `p${String(i + 1).padStart(2, "0")}`,
    title: `Part ${i + 1}`,
    published_at: 1_700_000_000 + i * 86_400,
    sort_at: 1_700_000_000 + i * 86_400,
    watched: c.toUpperCase() === "W" || c.toUpperCase() === "X",
    hidden: c.toUpperCase() === "H" || c.toUpperCase() === "X",
    download_state: c === c.toLowerCase() ? "done" : "none",
    ...over(i),
  }));
}

describe("uploadOrder", () => {
  it("puts the parts in upload order whatever order they arrive in", () => {
    const parts = series("LLLL");
    const shuffled = [parts[2], parts[0], parts[3], parts[1]];
    expect(uploadOrder(shuffled).map((v) => v.id)).toEqual(["p01", "p02", "p03", "p04"]);
  });

  it("puts a part with no upload date last, not first", () => {
    const parts = [
      video({ id: "a", published_at: null, sort_at: 1 }),
      video({ id: "b", published_at: 200 }),
      video({ id: "c", published_at: 100 }),
    ];
    expect(uploadOrder(parts).map((v) => v.id)).toEqual(["c", "b", "a"]);
  });

  it("settles a shared date by sort_at, unknown last, then by id", () => {
    const parts = [
      video({ id: "d", published_at: 100, sort_at: null }),
      video({ id: "c", published_at: 100, sort_at: 5 }),
      video({ id: "b", published_at: 100, sort_at: 9 }),
      video({ id: "a", published_at: 100, sort_at: 9 }),
    ];
    expect(uploadOrder(parts).map((v) => v.id)).toEqual(["c", "a", "b", "d"]);
  });

  it("leaves the array it was given alone", () => {
    const parts = [video({ id: "b", published_at: 2 }), video({ id: "a", published_at: 1 })];
    uploadOrder(parts);
    expect(parts.map((v) => v.id)).toEqual(["b", "a"]);
  });
});

describe("bandWeights", () => {
  it("is each part's runtime", () => {
    expect(bandWeights([600, 1200, 300])).toEqual([600, 1200, 300]);
  });

  it("gives a part with no runtime (null or 0) the average of the known ones", () => {
    expect(bandWeights([600, null, 1200, 0])).toEqual([600, 900, 1200, 900]);
  });

  it("makes every band the same when no runtime is known", () => {
    expect(bandWeights([null, 0, null])).toEqual([1, 1, 1]);
  });
});

describe("bandGap", () => {
  it("narrows as the series grows, never below a pixel", () => {
    expect(bandGap(2)).toBe(3);
    expect(bandGap(24)).toBe(3);
    expect(bandGap(25)).toBe(2);
    expect(bandGap(60)).toBe(2);
    expect(bandGap(61)).toBe(1);
    expect(bandGap(500)).toBe(1);
  });
});

describe("nextIndex", () => {
  it("is the first part neither watched nor hidden", () => {
    expect(nextIndex(series("WWLLL"))).toBe(2);
  });

  it("skips a hidden part, and a watched one past a gap does not move it", () => {
    expect(nextIndex(series("WHWLW"))).toBe(3);
    expect(nextIndex(series("LWW"))).toBe(0);
  });

  it("is none when everything is watched or hidden", () => {
    expect(nextIndex(series("WWW"))).toBe(-1);
    expect(nextIndex(series("WHH"))).toBe(-1);
  });
});

describe("runtimeOf", () => {
  it("adds the parts up", () => {
    expect(runtimeOf([video({ duration_secs: 3600 }), video({ duration_secs: 1800 })])).toBe("1h 30m");
  });

  it("marks a total missing a part's runtime with a +", () => {
    expect(runtimeOf([video({ duration_secs: 3600 }), video({ duration_secs: null })])).toBe("1h+");
    expect(runtimeOf([video({ duration_secs: 3600 }), video({ duration_secs: 0 })])).toBe("1h+");
  });

  it("says nothing it does not know", () => {
    expect(runtimeOf([video({ duration_secs: null })])).toBe("—");
  });
});

describe("seriesFigures", () => {
  it("brackets a clean prefix, splitting after the last watched part", () => {
    const f = seriesFigures(series("WWWLL"));
    expect(f).toEqual({
      kind: "brackets",
      split: 3,
      watched: { runtime: "3h", caption: "watched, parts 1 to 3", downloaded: 0 },
      left: { runtime: "2h", caption: "left in 2 parts", downloaded: 0 },
    });
  });

  it("keeps a hidden part inside the prefix in the watched bracket, counting only watched time", () => {
    const f = seriesFigures(series("WHWLL"));
    expect(f.kind).toBe("brackets");
    if (f.kind !== "brackets") return;
    expect(f.split).toBe(3);
    expect(f.watched.runtime).toBe("2h");
    expect(f.watched.caption).toBe("watched, parts 1 to 3");
    expect(f.left.caption).toBe("left in 2 parts");
  });

  it("puts a hidden part after the prefix in the right bracket, without counting it as left", () => {
    const f = seriesFigures(series("WWHLL"));
    expect(f.kind).toBe("brackets");
    if (f.kind !== "brackets") return;
    expect(f.split).toBe(2);
    expect(f.watched.caption).toBe("watched, parts 1 and 2");
    expect(f.left).toEqual({ runtime: "2h", caption: "left in 2 parts", downloaded: 0 });
  });

  it("counts a watched part hidden afterwards as watched", () => {
    const f = seriesFigures(series("WXL"));
    expect(f.kind).toBe("brackets");
    if (f.kind !== "brackets") return;
    expect(f.split).toBe(2);
    expect(f.watched.runtime).toBe("2h");
  });

  it("says part 1 alone in the singular", () => {
    const f = seriesFigures(series("WLL"));
    expect(f.kind === "brackets" && f.watched.caption).toBe("watched, part 1");
  });

  it("goes plain when a part left to watch comes before a watched one", () => {
    expect(seriesFigures(series("WLWL"))).toEqual({
      kind: "plain",
      watched: { runtime: "2h", caption: "watched, 2 parts", downloaded: 0 },
      left: { runtime: "2h", caption: "left in 2 parts", downloaded: 0 },
    });
  });

  it("shows only what is left when nothing is watched", () => {
    expect(seriesFigures(series("LLL"))).toEqual({
      kind: "plain",
      watched: null,
      left: { runtime: "3h", caption: "left in 3 parts", downloaded: 0 },
    });
  });

  it("says all of them when every part is watched", () => {
    expect(seriesFigures(series("WWWW"))).toEqual({
      kind: "plain",
      watched: { runtime: "4h", caption: "watched, all 4 parts", downloaded: 0 },
      left: null,
    });
  });

  it("does not say all when a part was hidden unwatched", () => {
    const f = seriesFigures(series("WWH"));
    expect(f.kind === "plain" && f.watched?.caption).toBe("watched, 2 parts");
    expect(f.kind === "plain" && f.left).toBeNull();
  });

  it("has nothing to say when every part is hidden", () => {
    expect(seriesFigures(series("HHH"))).toEqual({ kind: "none" });
  });

  it("counts the parts left to watch that are on disk, and only those", () => {
    // Part 2 is downloaded but watched, part 5 downloaded but hidden: neither is
    // waiting for you, so neither is in the green clause.
    const f = seriesFigures(series("WwLlh"));
    expect(f.kind === "brackets" && f.left).toEqual({ runtime: "2h", caption: "left in 2 parts", downloaded: 1 });
  });

  it("says one part left in the singular", () => {
    const f = seriesFigures(series("WWl"));
    expect(f.kind === "brackets" && f.left).toEqual({ runtime: "1h", caption: "left in 1 part", downloaded: 1 });
  });

  it("marks a figure whose parts' runtimes are not all known", () => {
    const f = seriesFigures(series("WWLL", (i) => (i === 3 ? { duration_secs: null } : {})));
    expect(f.kind === "brackets" && f.watched.runtime).toBe("2h");
    expect(f.kind === "brackets" && f.left.runtime).toBe("1h+");
  });
});

describe("seriesModel", () => {
  it("lays the bands out in upload order, numbered from 1", () => {
    const parts = series("WWL");
    const m = seriesModel([parts[2], parts[0], parts[1]]);
    expect(m.bands.map((b) => [b.position, b.video.id])).toEqual([[1, "p01"], [2, "p02"], [3, "p03"]]);
  });

  it("marks the next part and nothing else", () => {
    const m = seriesModel(series("WHLL"));
    expect(m.bands.map((b) => b.next)).toEqual([false, false, true, false]);
    expect(m.next?.position).toBe(3);
  });

  it("has no next part once the series is finished", () => {
    expect(seriesModel(series("WWW")).next).toBeNull();
  });

  it("builds one column per part from the weights, with a zero floor", () => {
    const m = seriesModel(series("WL", (i) => ({ duration_secs: i ? null : 1200 })));
    expect(m.columns).toBe("minmax(0, 1200fr) minmax(0, 1200fr)");
    expect(m.gap).toBe(3);
  });

  it("carries each part's own runtime, empty when unknown", () => {
    const m = seriesModel(series("WL", (i) => ({ duration_secs: i ? null : 5400 })));
    expect(m.bands.map((b) => b.runtime)).toEqual(["1h 30m", ""]);
  });

  it("summarises the series for a screen reader", () => {
    const m = seriesModel(series("WwWLlH"));
    expect(m.label).toBe("6 parts, 3 watched, 2 downloaded, 1 hidden, part 4 is next. 3h watched, 2h left.");
    expect(seriesModel(series("WW")).label).toBe("2 parts, all watched. 2h watched.");
  });
});

describe("bandStates", () => {
  const states = (pattern: string, at: number) => bandStates(seriesModel(series(pattern)).bands[at]);

  it("names what a part is", () => {
    expect(states("wL", 0)).toEqual(["watched", "downloaded"]);
    expect(states("WH", 1)).toEqual(["hidden"]);
    expect(states("WX", 1)).toEqual(["watched", "hidden"]);
  });

  it("puts next first, and says to watch only for a part with nothing else to say", () => {
    expect(states("Wl", 1)).toEqual(["next", "downloaded"]);
    expect(states("WLL", 2)).toEqual(["to watch"]);
  });
});

describe("placeAround", () => {
  it("centres on the point while there is room", () => {
    expect(placeAround(500, 100, 0, 1000)).toBe(450);
  });

  it("holds against the edge it would cross", () => {
    expect(placeAround(20, 100, 0, 1000)).toBe(0);
    expect(placeAround(990, 100, 0, 1000)).toBe(900);
  });

  it("keeps the start in view when the box is wider than the span", () => {
    expect(placeAround(50, 300, 0, 200)).toBe(0);
  });
});

describe("cssUrl", () => {
  it("quotes the address and escapes what would end the string", () => {
    expect(cssUrl("https://i.ytimg.com/vi/x/hqdefault.jpg")).toBe('url("https://i.ytimg.com/vi/x/hqdefault.jpg")');
    expect(cssUrl('a"b\\c')).toBe('url("a\\"b\\\\c")');
  });
});
