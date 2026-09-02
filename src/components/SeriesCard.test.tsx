import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import SeriesCard from "./SeriesCard";
import type { Video, VideoGroup } from "../types";

afterEach(cleanup);

function video(over: Partial<Video> = {}): Video {
  return {
    id: "p1", channel_id: "UC1", channel_title: "Chills Narrated",
    title: "The Blackwood Tapes", description: null, thumb_url: null, thumb_path: null,
    published_at: 1_700_000_000, sort_at: null, feed_rank: 0,
    added_manually: false, duration_secs: 754, view_count: 1500,
    status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "none", download_error: null,
    file_path: null, downloaded_at: null, first_seen_at: 0, sibling_group: null,
    ...over,
  };
}

/** A three-part series, newest part leading, as the backend hands it over. */
function group(over: Partial<VideoGroup> = {}): VideoGroup {
  return {
    videos: [
      video({ id: "p3", title: "The Blackwood Tapes - Part 3" }),
      video({ id: "p2", title: "The Blackwood Tapes - Part 2" }),
      video({ id: "p1", title: "The Blackwood Tapes" }),
    ],
    stem: "The Blackwood Tapes",
    ...over,
  };
}

function renderCard(over: Partial<VideoGroup> = {}) {
  const onOpen = vi.fn();
  render(<SeriesCard group={group(over)} onOpen={onOpen} onContextMenu={vi.fn()} />);
  return { onOpen };
}

describe("SeriesCard", () => {
  it("wears the shared series name rather than any one part's title", () => {
    renderCard();
    expect(screen.getByRole("heading").textContent).toBe("The Blackwood Tapes");
  });

  it("falls back to the leading part's title when the parts share no name", () => {
    renderCard({ stem: null });
    expect(screen.getByRole("heading").textContent).toBe("The Blackwood Tapes - Part 3");
  });

  it("counts every part, including the ones the filters would have hidden", () => {
    renderCard();
    expect(screen.getByText("3")).toBeTruthy();
    expect(screen.getByText("parts")).toBeTruthy();
  });

  it("opens the series on the leader, which is the anchor the view expects", () => {
    const { onOpen } = renderCard();
    fireEvent.click(screen.getByRole("button"));
    expect(onOpen).toHaveBeenCalledTimes(1);
    expect(onOpen.mock.calls[0][0].id).toBe("p3");
  });

  it("opens from the keyboard too, since the whole card is the control", () => {
    const { onOpen } = renderCard();
    fireEvent.keyDown(screen.getByRole("button"), { key: "Enter" });
    expect(onOpen).toHaveBeenCalledTimes(1);
  });

  /* No download or play button: a card standing for seven parts has no one
     part to act on, and the parts are reachable by opening it. */
  it("offers no per-video action of its own", () => {
    renderCard();
    expect(screen.queryByRole("button", { name: /Download|Play/ })).toBeNull();
  });
});

describe("SeriesCard watch progress", () => {
  /** The three-part series, with `count` of its parts marked watched. */
  function withWatched(count: number): VideoGroup {
    const g = group();
    return {
      ...g,
      videos: g.videos.map((v, i) => ({ ...v, watched: i >= g.videos.length - count })),
    };
  }

  it("tallies the parts watched and the parts still to go", () => {
    render(<SeriesCard group={withWatched(2)} onOpen={vi.fn()} onContextMenu={vi.fn()} />);
    expect(screen.getByText("2 watched")).toBeTruthy();
    expect(screen.getByText("1 unwatched")).toBeTruthy();
  });

  it("fills the bar by the share of the series behind you", () => {
    render(<SeriesCard group={withWatched(1)} onOpen={vi.fn()} onContextMenu={vi.fn()} />);
    const fill = document.querySelector(".series-progress-fill") as HTMLElement;
    expect(fill.style.width).toBe(`${(1 / 3) * 100}%`);
  });

  it("dims the whole card once every part has been watched", () => {
    render(<SeriesCard group={withWatched(3)} onOpen={vi.fn()} onContextMenu={vi.fn()} />);
    expect(document.querySelector(".series-card")!.className).toContain("is-watched");
  });

  it("stays undimmed while any one part is unwatched", () => {
    render(<SeriesCard group={withWatched(2)} onOpen={vi.fn()} onContextMenu={vi.fn()} />);
    expect(document.querySelector(".series-card")!.className).not.toContain("is-watched");
  });

  // A zero is not something to chase, so it drops out of the accent colour.
  it("only accents the count still to watch while there is one", () => {
    render(<SeriesCard group={withWatched(2)} onOpen={vi.fn()} onContextMenu={vi.fn()} />);
    expect(screen.getByText("1 unwatched").className).toContain("is-due");
    cleanup();
    render(<SeriesCard group={withWatched(3)} onOpen={vi.fn()} onContextMenu={vi.fn()} />);
    expect(screen.getByText("0 unwatched").className).not.toContain("is-due");
  });
});

/**
 * The panel is the series' fact box, so the runtime it shows is the series'
 * runtime — every part added up, not the leading part's own.
 */
describe("SeriesCard total runtime", () => {
  const runtime = () => screen.getByTestId("series-runtime");

  it("adds every part up", () => {
    // 20 + 30 + 10 minutes.
    renderCard({
      videos: [
        video({ id: "p3", duration_secs: 1200 }),
        video({ id: "p2", duration_secs: 1800 }),
        video({ id: "p1", duration_secs: 600 }),
      ],
    });
    expect(runtime().textContent).toBe("1h");
  });

  it("marks a total it knows to be short, rather than quietly understating it", () => {
    renderCard({
      videos: [
        video({ id: "p3", duration_secs: 1200 }),
        video({ id: "p2", duration_secs: null }),
        video({ id: "p1", duration_secs: 600 }),
      ],
    });
    expect(runtime().textContent).toBe("30m+");
    expect(runtime().title).toContain("1 has no runtime yet");
  });

  it("says nothing at all when no part has a runtime", () => {
    renderCard({
      videos: [
        video({ id: "p2", duration_secs: null }),
        video({ id: "p1", duration_secs: null }),
      ],
    });
    expect(screen.queryByTestId("series-runtime")).toBeNull();
  });

  it("reads the total out with the rest of the card", () => {
    renderCard({
      videos: [
        video({ id: "p2", duration_secs: 1800 }),
        video({ id: "p1", duration_secs: 1800 }),
      ],
    });
    expect(screen.getByRole("button").getAttribute("aria-label"))
      .toContain("1h total");
  });
});
