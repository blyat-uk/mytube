import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import FeedWaveform from "./FeedWaveform";
import type { FeedStats, StatsDay } from "../../types";

/** Sixty months, Nov 2021 to this October, an upload on each 15th: month-long
 *  bars, 720px at 12px a bar, so bar i is x 12i..12i+12. */
function days(): StatsDay[] {
  const out: StatsDay[] = [];
  let y = 2021, m = 11;
  for (let i = 0; i < 60; i++) {
    out.push({ day: `${y}-${String(m).padStart(2, "0")}-15`, watched: 1, downloaded: 0, unwatched: 2, hidden: 0 });
    if (++m === 13) { m = 1; y++; }
  }
  // March 2024 is bar 28.
  out[28] = { day: "2024-03-15", watched: 5, downloaded: 3, unwatched: 27, hidden: 4 };
  return out;
}

function stats(over: Partial<FeedStats> = {}): FeedStats {
  return {
    videos: 4656, series: 393, toWatch: 3985, watched: 514, downloaded: 239, hidden: 157,
    runtimeSecs: 17_154 * 3600, runtimeUnknown: false, newThisWeek: 86, today: "2026-10-19", days: days(),
    ...over,
  };
}

// jsdom lays nothing out: give every box the wave's 720px, starting at x 0.
beforeEach(() => {
  vi.spyOn(Element.prototype, "getBoundingClientRect").mockReturnValue({
    x: 0, y: 0, left: 0, top: 0, width: 720, height: 96, right: 720, bottom: 96, toJSON: () => ({}),
  } as DOMRect);
});
afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

const wave = (c: HTMLElement) => c.querySelector(".wv-wave") as HTMLElement;

describe("FeedWaveform", () => {
  it("renders nothing for a library with no videos", () => {
    const { container } = render(<FeedWaveform stats={stats({ videos: 0, days: [] })} />);
    expect(container.firstChild).toBeNull();
  });

  it("stands the totals either side of the wave", () => {
    const { container } = render(<FeedWaveform stats={stats({ runtimeUnknown: true })} />);
    const [left, right] = Array.from(container.querySelectorAll(".wv-flank"));
    expect(left.textContent).toBe("4,656videos in 393 series");
    expect(right.textContent).toBe("17,154h+of runtime");
  });

  it("says just videos when none of them are in a series", () => {
    const { container } = render(<FeedWaveform stats={stats({ series: 0 })} />);
    expect(container.querySelector(".wv-flank span")?.textContent).toBe("videos");
  });

  it("names its four colours with a figure each, a zero included", () => {
    const { container } = render(<FeedWaveform stats={stats({ hidden: 0 })} />);
    const figs = Array.from(container.querySelectorAll(".wv-fig")).map((f) => f.textContent);
    expect(figs).toEqual(["3,985to watch", "514watched", "239downloaded", "0hidden"]);
  });

  it("wears the week's arrivals as a pill, and no pill for none", () => {
    render(<FeedWaveform stats={stats()} />);
    expect(screen.getByText("+86 this week")).toBeTruthy();
    cleanup();
    const { container } = render(<FeedWaveform stats={stats({ newThisWeek: 0 })} />);
    expect(container.querySelector(".wv-new")).toBeNull();
    expect(screen.queryByText(/this week/)).toBeNull();
  });

  it("summarises itself for a screen reader", () => {
    render(<FeedWaveform stats={stats()} />);
    expect(screen.getByRole("img").getAttribute("aria-label")).toBe(
      "4,656 videos, uploaded from November 2021 to October 2026: " +
      "3,985 to watch, 514 watched, 239 downloaded, 157 hidden",
    );
  });

  it("draws a bar a month for five years of uploads, and a year under each January", () => {
    const { container } = render(<FeedWaveform stats={stats()} />);
    expect(container.querySelectorAll(".wv-bars .wv-bar")).toHaveLength(60);
    const labels = Array.from(container.querySelectorAll(".wv-year")).map((y) => y.textContent);
    expect(labels.filter((t) => /^\d{4}$/.test(t ?? ""))).toEqual(["2022", "2023", "2024", "2025", "2026"]);
    expect(container.querySelector(".wv-break")).toBeNull();
  });

  it("cuts a quiet stretch out and says so under the pointer", () => {
    const d = [{ day: "2019-04-04", watched: 0, downloaded: 0, unwatched: 0, hidden: 5 }, ...days().slice(40)];
    const { container } = render(<FeedWaveform stats={stats({ days: d })} />);
    expect(container.querySelectorAll(".wv-bars .wv-break")).toHaveLength(1);
    // April 2019 is slot 0, the break the next three.
    fireEvent.pointerMove(wave(container), { clientX: 2 * 12 + 4 });
    const tip = screen.getByRole("tooltip");
    expect(tip.querySelector(".wv-tip-month")?.textContent).toBe("Apr 2019 – Mar 2025");
    expect(tip.querySelector(".wv-tip-count")?.textContent).toBe("Nothing uploaded");
  });

  it("shows the month under the pointer and its counts, and lets go on leave", () => {
    const { container } = render(<FeedWaveform stats={stats()} />);
    expect(screen.queryByRole("tooltip")).toBeNull();

    fireEvent.pointerMove(wave(container), { clientX: 28 * 12 + 4 });
    const tip = screen.getByRole("tooltip");
    expect(tip.querySelector(".wv-tip-month")?.textContent).toBe("March 2024");
    expect(tip.querySelector(".wv-tip-count")?.textContent).toBe("39 videos");
    const rows = Array.from(tip.querySelectorAll("li")).map((li) => li.textContent);
    expect(rows).toEqual(["30 to watch, 3 downloaded", "5 watched", "4 hidden"]);
    expect(container.querySelector(".wv")?.classList.contains("is-hovering")).toBe(true);

    fireEvent.pointerMove(wave(container), { clientX: 719 });
    expect(screen.getByRole("tooltip").querySelector(".wv-tip-month")?.textContent).toBe("October 2026");
    expect(Array.from(screen.getByRole("tooltip").querySelectorAll("li")).map((li) => li.textContent))
      .toEqual(["2 to watch", "1 watched"]);

    fireEvent.pointerLeave(wave(container));
    expect(screen.queryByRole("tooltip")).toBeNull();
    expect(container.querySelector(".wv")?.classList.contains("is-hovering")).toBe(false);
  });

  it("grows its bars in only once, not on a later mount", () => {
    render(<FeedWaveform stats={stats()} />);
    cleanup();
    const { container } = render(<FeedWaveform stats={stats()} />);
    expect(container.querySelector(".wv-bars")?.classList.contains("is-entering")).toBe(false);
  });
});
