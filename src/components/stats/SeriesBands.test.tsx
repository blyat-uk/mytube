import { describe, it, expect, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import SeriesBands from "./SeriesBands";
import type { Video } from "../../types";

afterEach(cleanup);

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

/** The mockup's series: twelve parts, the first seven watched, parts 2 and 8
 *  on disk -- handed over newest first, the way the grid shows them. */
function twelve(): Video[] {
  return Array.from({ length: 12 }, (_, i) => video({
    id: `p${String(i + 1).padStart(2, "0")}`,
    title: `The Blackwood Tapes - Part ${i + 1}`,
    published_at: 1_700_000_000 + i * 86_400,
    sort_at: 1_700_000_000 + i * 86_400,
    duration_secs: 12_000 + i * 60,
    watched: i < 7,
    download_state: i === 1 || i === 7 ? "done" : "none",
    thumb_url: `https://i.ytimg.com/vi/p${i + 1}/hqdefault.jpg`,
  })).reverse();
}

const bands = (c: HTMLElement) => Array.from(c.querySelectorAll<HTMLElement>(".sb-band"));

describe("SeriesBands", () => {
  it("draws nothing for no parts or one", () => {
    const { container, rerender } = render(<SeriesBands parts={[]} />);
    expect(container.innerHTML).toBe("");
    rerender(<SeriesBands parts={[video()]} />);
    expect(container.innerHTML).toBe("");
  });

  it("draws one band per part, in upload order however the grid sorted them", () => {
    const { container } = render(<SeriesBands parts={twelve()} />);
    const row = bands(container);
    expect(row).toHaveLength(12);
    expect(row.map((b) => b.dataset.part)).toEqual(Array.from({ length: 12 }, (_, i) => String(i + 1)));
    expect(row[0].style.gridColumn).toBe("1");
    expect(row[0].querySelector<HTMLElement>(".sb-fill")!.style.getPropertyValue("--img"))
      .toBe('url("https://i.ytimg.com/vi/p1/hqdefault.jpg")');
  });

  it("rails the watched and downloaded parts and rings the next one", () => {
    const { container } = render(<SeriesBands parts={twelve()} />);
    const row = bands(container);
    expect(row.filter((b) => b.classList.contains("is-watched")).map((b) => b.dataset.part))
      .toEqual(["1", "2", "3", "4", "5", "6", "7"]);
    expect(row.filter((b) => b.classList.contains("is-disk")).map((b) => b.dataset.part)).toEqual(["2", "8"]);
    expect(row.filter((b) => b.classList.contains("is-next")).map((b) => b.dataset.part)).toEqual(["8"]);
  });

  it("labels the next part by its place in upload order, with its runtime", () => {
    const { container } = render(<SeriesBands parts={twelve()} />);
    const label = container.querySelector(".sb-next")!;
    expect(label.textContent).toBe("Part 8 is next3h 27m");
    expect(label.querySelector("span")!.textContent).toBe("3h 27m");
  });

  it("has no next label once every part is watched", () => {
    const parts = twelve().map((v) => ({ ...v, watched: true }));
    const { container } = render(<SeriesBands parts={parts} />);
    expect(container.querySelector(".sb-next")).toBeNull();
    expect(screen.getByText("watched, all 12 parts")).toBeTruthy();
  });

  it("brackets a clean prefix, each bracket spanning its own parts' columns", () => {
    const { container } = render(<SeriesBands parts={twelve()} />);
    const [watched, left] = Array.from(container.querySelectorAll<HTMLElement>(".sb-brk"));
    expect(watched.classList.contains("is-watched")).toBe(true);
    expect(watched.style.gridColumn).toBe("1 / 8");
    expect(watched.textContent).toBe("23h 41mwatched, parts 1 to 7");
    expect(left.style.gridColumn).toBe("8 / -1");
    expect(left.textContent).toBe("17h 25mleft in 5 parts, 1 downloaded");
    expect(left.querySelector("em")!.textContent).toBe("1 downloaded");
    expect(container.querySelector(".sb-plain")).toBeNull();
  });

  it("shows the figures plain when a part was skipped and watched past", () => {
    const parts = twelve().map((v) => (v.id === "p03" ? { ...v, watched: false } : v));
    const { container } = render(<SeriesBands parts={parts} />);
    expect(container.querySelector(".sb-brk")).toBeNull();
    expect(container.querySelector(".sb-plain")).not.toBeNull();
    expect(screen.getByText("watched, 6 parts")).toBeTruthy();
    expect(screen.getByText(/left in 6 parts/)).toBeTruthy();
    expect(container.querySelector(".sb-next")!.textContent).toMatch(/^Part 3 is next/);
  });

  it("hatches a hidden part and passes over it for next", () => {
    const parts = twelve().map((v) => (v.id === "p08" ? { ...v, hidden: true } : v));
    const { container } = render(<SeriesBands parts={parts} />);
    const row = bands(container);
    expect(row[7].classList.contains("is-hidden")).toBe(true);
    expect(row.find((b) => b.classList.contains("is-next"))!.dataset.part).toBe("9");
  });

  it("summarises the series for a screen reader in place of the picture", () => {
    render(<SeriesBands parts={twelve()} />);
    expect(screen.getByRole("img").getAttribute("aria-label"))
      .toBe("12 parts, 7 watched, 2 downloaded, part 8 is next. 23h 41m watched, 17h 25m left.");
  });

  it("names a band's part and its state while the pointer is on it", () => {
    const { container } = render(<SeriesBands parts={twelve()} />);
    expect(container.querySelector(".sb-tip")).toBeNull();
    fireEvent.mouseEnter(bands(container)[1]);
    const tip = container.querySelector(".sb-tip")!;
    expect(screen.getByText("Part 2 of 12")).toBeTruthy();
    expect(tip.querySelector(".sb-tip-title")!.textContent).toBe("The Blackwood Tapes - Part 2");
    expect(Array.from(tip.querySelectorAll(".sb-state")).map((s) => s.textContent)).toEqual(["watched", "downloaded"]);
    expect(bands(container)[1].classList.contains("is-tip")).toBe(true);

    fireEvent.mouseEnter(bands(container)[7]);
    expect(screen.getByText("Part 8 of 12")).toBeTruthy();
    expect(Array.from(container.querySelectorAll(".sb-state")).map((s) => s.textContent)).toEqual(["next", "downloaded"]);

    fireEvent.mouseLeave(bands(container)[7], { relatedTarget: document.body });
    expect(container.querySelector(".sb-tip")).toBeNull();
  });

  it("keeps the tooltip up when the pointer moves straight to the next band", () => {
    const { container } = render(<SeriesBands parts={twelve()} />);
    const row = bands(container);
    fireEvent.mouseEnter(row[3]);
    fireEvent.mouseLeave(row[3], { relatedTarget: row[4] });
    expect(container.querySelector(".sb-tip")).not.toBeNull();
  });
});
