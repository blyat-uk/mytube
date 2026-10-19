import { describe, it, expect } from "vitest";
import {
  BREAK_SLOTS, MAX_PITCH, MIN_PITCH, QUIET_DAYS, WAVE_HEIGHT, axisTicks, barLabel, barX,
  breakLabel, bucketStart, chooseLayout, dayNumber, halfHeight, itemCentre, itemIndexAt,
  layoutFor, nextBucket, runtimeText, segments, seriesCaption, tipRows, todayOf, videosText,
  waveCap, waveGeometry, waveSummary, type WaveBar, type WaveItem, type WaveLayout,
} from "./waveform";
import type { FeedStats, StatsDay } from "../../types";

const DAY_MS = 86_400_000;
const iso = (n: number) => new Date(n * DAY_MS).toISOString().slice(0, 10);
const N = dayNumber;

/** A day with `n` videos still to watch, or the counts given. */
function day(date: string, over: Partial<StatsDay> | number = 1): StatsDay {
  const counts = typeof over === "number" ? { unwatched: over } : over;
  return { day: date, watched: 0, downloaded: 0, unwatched: 0, hidden: 0, ...counts };
}

/** Every `every`th day from `from` to `to`, inclusive, each with one video. */
function daily(from: string, to: string, every = 1): StatsDay[] {
  const out: StatsDay[] = [];
  for (let n = N(from); n <= N(to); n += every) out.push(day(iso(n)));
  return out;
}

const bars = (l: WaveLayout) => l.items.filter((i): i is Extract<WaveItem, { kind: "bar" }> => i.kind === "bar").map((i) => i.bar);
const breaks = (l: WaveLayout) => l.items.filter((i) => i.kind === "break");
const total = (bs: WaveBar[]) => bs.reduce((a, b) => a + b.total, 0);

function bar(from: string, to = from, over: Partial<WaveBar> = {}): WaveBar {
  return { from: N(from), to: N(to), watched: 0, downloaded: 0, unwatched: 0, hidden: 0, total: 0, ...over };
}

describe("calendar days", () => {
  it("numbers a date by the calendar, whatever the clock does", () => {
    expect(N("1970-01-02")).toBe(1);
    // Across a European clock change: still one day apart.
    expect(N("2026-03-30") - N("2026-03-29")).toBe(1);
    expect(iso(N("2024-02-29"))).toBe("2024-02-29");
  });

  it("starts each kind of bar on its calendar boundary", () => {
    const thu = N("2026-10-15"); // a Thursday
    expect(iso(bucketStart(thu, "day"))).toBe("2026-10-15");
    expect(iso(bucketStart(thu, "week"))).toBe("2026-10-12"); // the Monday
    expect(iso(bucketStart(thu, "month"))).toBe("2026-10-01");
    expect(iso(bucketStart(thu, "quarter"))).toBe("2026-10-01");
    expect(iso(bucketStart(N("2026-08-20"), "quarter"))).toBe("2026-07-01");
    expect(iso(bucketStart(N("2026-05-20"), "half"))).toBe("2026-01-01");
    expect(iso(bucketStart(thu, "year"))).toBe("2026-01-01");
  });

  it("steps to the next bar across month and year ends", () => {
    expect(iso(nextBucket(N("2026-12-28"), "week"))).toBe("2027-01-04");
    expect(iso(nextBucket(N("2026-12-01"), "month"))).toBe("2027-01-01");
    expect(iso(nextBucket(N("2026-10-01"), "quarter"))).toBe("2027-01-01");
    expect(iso(nextBucket(N("2026-07-01"), "half"))).toBe("2027-01-01");
    expect(iso(nextBucket(N("2026-01-01"), "year"))).toBe("2027-01-01");
  });
});

describe("layout", () => {
  const today = N("2026-10-19");

  it("cuts a quiet half year out, and draws a break where it was", () => {
    // The library that prompted this: five videos in April 2019, then nothing
    // until February 2025.
    const days = [day("2019-04-04"), day("2019-04-16", 4), ...daily("2025-02-05", "2026-10-19", 3)];
    const l = layoutFor(days, today, "month");
    expect(breaks(l)).toHaveLength(1);
    expect(l.items[0]).toMatchObject({ kind: "bar", at: 0 });
    expect(l.items[1]).toMatchObject({ kind: "break", at: 1, gap: { from: N("2019-04-17"), to: N("2025-02-04") } });
    expect(l.items[2]).toMatchObject({ kind: "bar", at: 1 + BREAK_SLOTS });
    // April 2019, then February 2025 to October 2026: 22 months, not 91.
    expect(bars(l)).toHaveLength(22);
    expect(l.slots).toBe(22 + BREAK_SLOTS);
    expect(total(bars(l))).toBe(days.reduce((a, d) => a + d.unwatched, 0));
  });

  it("keeps a quiet spell shorter than half a year as the empty bars it is", () => {
    const days = [day("2026-01-10"), day("2026-06-20")];
    expect(N("2026-06-20") - N("2026-01-10")).toBeLessThan(QUIET_DAYS);
    const l = layoutFor(days, today, "month");
    expect(breaks(l)).toHaveLength(0);
    expect(bars(l).map((b) => iso(b.from))).toEqual([
      "2026-01-01", "2026-02-01", "2026-03-01", "2026-04-01", "2026-05-01",
      "2026-06-01", "2026-07-01", "2026-08-01", "2026-09-01", "2026-10-01",
    ]);
  });

  it("does not cut what long bars put side by side anyway", () => {
    // Over seven months apart, but in neighbouring years: a year-long bar each.
    const l = layoutFor([day("2024-03-01"), day("2025-10-01"), day("2026-10-01")], today, "year");
    expect(breaks(l)).toHaveLength(0);
    expect(bars(l).map((b) => iso(b.from))).toEqual(["2024-01-01", "2025-01-01", "2026-01-01"]);
  });

  it("runs on to today, so the right edge is now", () => {
    const l = layoutFor([day("2026-08-03")], today, "month");
    expect(bars(l).map((b) => iso(b.from))).toEqual(["2026-08-01", "2026-09-01", "2026-10-01"]);
  });

  it("stops where the uploads did once today is a quiet stretch away", () => {
    const l = layoutFor([day("2025-01-15"), day("2025-02-15")], today, "month");
    expect(bars(l).map((b) => iso(b.from))).toEqual(["2025-01-01", "2025-02-01"]);
    expect(breaks(l)).toHaveLength(0);
  });

  it("carries a video dated after today on to its own bar", () => {
    const l = layoutFor([day("2026-10-01"), day("2026-12-24")], today, "month");
    const all = bars(l);
    expect(iso(all[all.length - 1].from)).toBe("2026-12-01");
  });

  it("splits each bar's counts by state and keeps every video", () => {
    const days = [
      day("2026-10-12", { watched: 2, downloaded: 1, unwatched: 3, hidden: 1 }),
      day("2026-10-18", { watched: 1, hidden: 2 }),
      day("2026-10-19", { unwatched: 4 }),
    ];
    const [a, b] = bars(layoutFor(days, today, "week"));
    expect(a).toMatchObject({ from: N("2026-10-12"), to: N("2026-10-18"), watched: 3, downloaded: 1, unwatched: 3, hidden: 3, total: 10 });
    expect(b).toMatchObject({ from: N("2026-10-19"), total: 4 });
  });

  it("has nothing to draw for no days", () => {
    expect(layoutFor([], today, "week")).toEqual({ unit: "week", items: [], slots: 0 });
  });
});

describe("chooseLayout", () => {
  const today = N("2026-10-19");

  it("picks the shortest bar that fits the width", () => {
    expect(chooseLayout(daily("2026-09-25", "2026-10-19"), today, 720).unit).toBe("day");
    expect(chooseLayout(daily("2025-02-05", "2026-10-19", 2), today, 720).unit).toBe("week");
    expect(chooseLayout(daily("2019-03-01", "2026-10-19", 9), today, 720).unit).toBe("month");
    expect(chooseLayout(daily("2006-05-01", "2026-10-19", 9), today, 720).unit).toBe("quarter");
  });

  it("never draws more bars than fit, and gets longer bars as the room shrinks", () => {
    const days = daily("2025-02-05", "2026-10-19", 2);
    for (const width of [720, 600, 431, 300, 200, 120]) {
      const l = chooseLayout(days, today, width);
      expect(l.slots).toBeLessThanOrEqual(Math.floor(width / MIN_PITCH));
      expect(total(bars(l))).toBe(days.length);
    }
    expect(chooseLayout(days, today, 300).unit).toBe("month");
  });

  it("caps the wave's column at what its bars fill at the widest pitch", () => {
    expect(waveCap(daily("2026-10-10", "2026-10-19"), today)).toBe(10 * MAX_PITCH);
    expect(waveCap(daily("2019-03-01", "2026-10-19", 5), today)).toBe(720);
    expect(waveCap([], today)).toBe(0);
  });

  it("takes today from the stats, or ends at the newest upload without one", () => {
    const stats = { today: "2026-10-19", days: [day("2026-10-01")] } as FeedStats;
    expect(todayOf(stats)).toBe(today);
    expect(todayOf({ ...stats, today: "" })).toBe(N("2026-10-01"));
  });
});

describe("geometry", () => {
  it("spreads slots over the width, a bar 62% of its slot and at most 5px", () => {
    const g = waveGeometry(120, 720);
    expect(g.pitch).toBe(6);
    expect(g.barWidth).toBeCloseTo(3.72);
    expect(waveGeometry(60, 720).barWidth).toBe(5);
  });

  it("gathers a few bars in the middle rather than strewing them across the width", () => {
    const g = waveGeometry(5, 100);
    expect(g.pitch).toBe(MAX_PITCH);
    expect(g.offset).toBe(20);
    expect(barX(0, g)).toBeCloseTo(20 + (12 - g.barWidth) / 2);
  });

  it("finds the bar or break under the pointer, the ends taking what lies past them", () => {
    const l = layoutFor([day("2019-04-04"), ...daily("2025-02-05", "2025-04-30")], N("2025-04-30"), "month");
    const g = waveGeometry(l.slots, l.slots * 10);
    // Slot 0 is April 2019, slots 1..BREAK_SLOTS the break, then Feb 2025.
    expect(itemIndexAt(-50, l.items, g)).toBe(0);
    expect(itemIndexAt(5, l.items, g)).toBe(0);
    expect(l.items[itemIndexAt(15, l.items, g)].kind).toBe("break");
    expect(l.items[itemIndexAt((1 + BREAK_SLOTS) * 10 + 5, l.items, g)]).toMatchObject({ kind: "bar", at: 1 + BREAK_SLOTS });
    expect(itemIndexAt(10_000, l.items, g)).toBe(l.items.length - 1);
    expect(itemCentre(l.items[1], g)).toBe(10 + (BREAK_SLOTS * 10) / 2);
  });
});

describe("halfHeight", () => {
  const mid = WAVE_HEIGHT / 2;
  it("fills the half for the busiest bar, 3px short of the edge", () => expect(halfHeight(400, 400)).toBe(mid - 3));
  it("follows the square root below it, so a quiet bar still shows", () =>
    expect(halfHeight(100, 400)).toBeCloseTo((mid - 3) / 2));
  it("never draws a non-empty bar thinner than a dot", () => expect(halfHeight(1, 50_000)).toBe(2.5));
  it("draws nothing for an empty bar", () => expect(halfHeight(0, 80)).toBe(0));
});

describe("segments", () => {
  it("fills from the axis out: watched, downloaded, still to watch, hidden", () => {
    const segs = segments(bar("2024-03-01", "2024-03-31", { watched: 1, downloaded: 1, unwatched: 1, hidden: 1, total: 4 }), 40);
    expect(segs).toEqual([
      { kind: "watched", start: 0, length: 10 },
      { kind: "downloaded", start: 10, length: 10 },
      { kind: "unwatched", start: 20, length: 10 },
      { kind: "hidden", start: 30, length: 10 },
    ]);
  });

  it("gives an empty state no room", () => {
    const segs = segments(bar("2024-03-01", "2024-03-31", { watched: 1, hidden: 3, total: 4 }), 40);
    expect(segs.map((s) => s.kind)).toEqual(["watched", "hidden"]);
    expect(segments(bar("2024-03-01"), 40)).toEqual([]);
  });
});

describe("axisTicks", () => {
  const today = N("2026-10-19");
  const labels = (days: StatsDay[], width: number, rem = 10) => {
    const l = chooseLayout(days, today, width);
    const g = waveGeometry(l.slots, Math.min(width, l.slots * MAX_PITCH));
    return axisTicks(l, g, rem).map((t) => t.label);
  };

  it("names where the wave picks up after a break, and the stretch before it by its year", () => {
    const days = [day("2019-04-04"), day("2019-04-16", 4), ...daily("2025-02-05", "2026-10-19", 3)];
    const ticks = labels(days, 720);
    expect(ticks.slice(0, 2)).toEqual(["2019", "Feb 2025"]);
    expect(ticks).toContain("2026");
  });

  it("spaces the months evenly: every month, else every other, quarter or half year", () => {
    const ticks = labels(daily("2025-02-05", "2026-10-19", 2), 720);
    const months = ticks.filter((t) => /^[A-Z][a-z]{2}$/.test(t));
    const order = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    const steps = new Set(months.slice(1).map((m, i) => (order.indexOf(m) - order.indexOf(months[i]) + 12) % 12));
    expect(steps.size).toBeLessThanOrEqual(2); // one step, give or take a skip around a year label
  });

  it("puts years on round numbers once every year will not fit", () => {
    const ticks = labels(daily("2006-05-01", "2026-10-19", 9), 400);
    // The wave's own start ("2006") may stand off the step; the rest keep it.
    const years = ticks.map(Number).filter((y) => y > 2000).slice(1);
    const step = years[1] - years[0];
    expect([2, 5, 10]).toContain(step);
    for (const y of years) expect(y % step).toBe(0);
  });

  it("dates the Mondays of a library only days old", () => {
    expect(labels(daily("2026-10-01", "2026-10-19"), 720)).toEqual(["Oct 2026", "12", "19"]);
  });

  it("hangs a label inwards at either end rather than past the wave", () => {
    const l = chooseLayout(daily("2026-09-01", "2026-10-19"), today, 720);
    const g = waveGeometry(l.slots, 720);
    const ticks = axisTicks(l, g, 10);
    expect(ticks[0]).toMatchObject({ label: "Sep 2026", align: "start" });
  });

  it("never lets two labels crowd each other", () => {
    for (const width of [720, 500, 300]) {
      const l = chooseLayout(daily("2019-03-01", "2026-10-19", 4), today, width);
      const g = waveGeometry(l.slots, width);
      const ticks = axisTicks(l, g, 10);
      for (let i = 1; i < ticks.length; i++) expect(ticks[i].x).toBeGreaterThan(ticks[i - 1].x);
    }
  });
});

describe("words", () => {
  it("names a bar's span in the words its length calls for", () => {
    expect(barLabel(bar("2026-10-14"), "day")).toBe("14 Oct 2026");
    expect(barLabel(bar("2026-10-12", "2026-10-18"), "week")).toBe("12 – 18 Oct 2026");
    expect(barLabel(bar("2026-09-28", "2026-10-04"), "week")).toBe("28 Sep – 4 Oct 2026");
    expect(barLabel(bar("2025-12-29", "2026-01-04"), "week")).toBe("29 Dec 2025 – 4 Jan 2026");
    expect(barLabel(bar("2026-10-01", "2026-10-31"), "month")).toBe("October 2026");
    expect(barLabel(bar("2024-01-01", "2024-03-31"), "quarter")).toBe("Jan – Mar 2024");
    expect(barLabel(bar("2024-07-01", "2024-12-31"), "half")).toBe("Jul – Dec 2024");
    expect(barLabel(bar("2024-01-01", "2024-12-31"), "year")).toBe("2024");
  });

  it("names a break by the months it spans", () => {
    expect(breakLabel({ from: N("2019-04-17"), to: N("2025-02-04") })).toBe("Apr 2019 – Feb 2025");
    expect(breakLabel({ from: N("2024-03-02"), to: N("2024-10-30") })).toBe("Mar – Oct 2024");
  });

  it("counts videos", () => {
    expect(videosText(0)).toBe("No videos");
    expect(videosText(1)).toBe("1 video");
    expect(videosText(4656)).toBe("4,656 videos");
  });

  it("counts a downloaded video as still to watch, and says how many are on disk", () => {
    const rows = tipRows(bar("2024-03-01", "2024-03-31", { watched: 5, downloaded: 3, unwatched: 27, hidden: 4, total: 39 }));
    expect(rows).toEqual([
      { kind: "left", count: "30", label: "to watch", downloaded: "3 downloaded" },
      { kind: "watched", count: "5", label: "watched", downloaded: null },
      { kind: "hidden", count: "4", label: "hidden", downloaded: null },
    ]);
  });

  it("leaves out a row of zero, and the downloaded note when none are", () => {
    const rows = tipRows(bar("2024-03-01", "2024-03-31", { unwatched: 2, total: 2 }));
    expect(rows).toEqual([{ kind: "left", count: "2", label: "to watch", downloaded: null }]);
    expect(tipRows(bar("2024-03-01"))).toEqual([]);
  });

  it("names the series the videos are in", () => {
    expect(seriesCaption(4656, 393)).toBe("videos in 393 series");
    expect(seriesCaption(12, 1)).toBe("videos in 1 series");
    expect(seriesCaption(40, 0)).toBe("videos");
    expect(seriesCaption(1, 0)).toBe("video");
  });

  it("gives the runtime, with a + while some of it is unknown", () => {
    expect(runtimeText(17_154 * 3600, false)).toBe("17,154h");
    expect(runtimeText(17_154 * 3600, true)).toBe("17,154h+");
    expect(runtimeText(15000, true)).toBe("4h 10m+");
    expect(runtimeText(0, true)).toBe("—");
  });
});

describe("waveSummary", () => {
  const stats = (over: Partial<FeedStats>): FeedStats => ({
    videos: 4656, series: 393, toWatch: 3985, watched: 514, downloaded: 239, hidden: 157,
    runtimeSecs: 0, runtimeUnknown: false, newThisWeek: 86, today: "2026-10-19",
    days: [day("2019-04-04"), day("2026-10-19")], ...over,
  });

  it("gives the totals and the months they span", () =>
    expect(waveSummary(stats({}))).toBe(
      "4,656 videos, uploaded from April 2019 to October 2026: 3,985 to watch, 514 watched, 239 downloaded, 157 hidden",
    ));

  it("names a single month once, and no span without days", () => {
    expect(waveSummary(stats({ days: [day("2026-10-02"), day("2026-10-19")] }))).toContain("uploaded in October 2026:");
    expect(waveSummary(stats({ days: [] }))).toMatch(/^4,656 videos: /);
  });
});
