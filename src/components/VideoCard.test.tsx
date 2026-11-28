import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, act } from "@testing-library/react";
import VideoCard from "./VideoCard";
import { createProgressStore } from "../progress";
import type { DownloadState, Video } from "../types";

afterEach(cleanup);

function video(over: Partial<Video> = {}): Video {
  return {
    id: "J1WoNuemKOg", channel_id: "UC1", channel_title: "Veritasium",
    title: "Some video", description: null, thumb_url: null, thumb_path: null,
    published_at: 1_700_000_000, sort_at: null, feed_rank: 0,
    added_manually: false, duration_secs: 754, view_count: 1500,
    status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "none" as DownloadState, download_error: null,
    file_path: null, downloaded_at: null, first_seen_at: 0, sibling_group: null, series_hidden: false,
    part_links_refused: false, part_linked: false, series_auto_download: false,
    ...over,
  };
}

function renderCard(over: Partial<Video> = {}) {
  const onAction = vi.fn();
  render(
    <VideoCard
      video={video(over)}
      onAction={onAction}
      onContextMenu={vi.fn()}
    />,
  );
  return { onAction };
}

describe("VideoCard hover actions", () => {
  it("offers the primary action for the state, as an icon button", () => {
    renderCard({ download_state: "done", file_path: "/v.mkv" });
    fireEvent.click(screen.getByRole("button", { name: /^Play$/ }));
    expect(screen.getByRole("button", { name: /^Play$/ }).querySelector("svg")).toBeTruthy();
  });

  it("routes the primary button through onAction without also firing the card", () => {
    const { onAction } = renderCard();
    fireEvent.click(screen.getByRole("button", { name: /^Download$/ }));
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0]).toBe("download");
  });

  it("always offers a way out to YouTube", () => {
    const { onAction } = renderCard({ download_state: "downloading" });
    fireEvent.click(screen.getByRole("button", { name: /Open on YouTube/ }));
    expect(onAction).toHaveBeenCalledTimes(1);
    expect(onAction.mock.calls[0][0]).toBe("open");
  });

  it("keeps the queued badge cancellable", () => {
    const { onAction } = renderCard({ download_state: "queued" });
    fireEvent.click(screen.getByRole("button", { name: /Queued/ }));
    expect(onAction.mock.calls[0][0]).toBe("cancel");
  });
});

describe("VideoCard with a call in flight", () => {
  it("refuses clicks on the card and its primary button while pending", () => {
    const onAction = vi.fn();
    render(
      <VideoCard video={video()} pending onAction={onAction} onContextMenu={vi.fn()} />,
    );
    const button = screen.getByRole<HTMLButtonElement>("button", { name: /^Download$/ });
    expect(button.disabled).toBe(true);
    expect(button.getAttribute("aria-busy")).toBe("true");
    fireEvent.click(screen.getByRole("article"));
    expect(onAction).not.toHaveBeenCalled();
    // Leaving for YouTube is never held up by a download call.
    fireEvent.click(screen.getByRole("button", { name: /Open on YouTube/ }));
    expect(onAction).toHaveBeenCalledWith("open", expect.anything());
  });

  it("spins only once the call is slow, keeping the name for screen readers", () => {
    const { rerender } = render(
      <VideoCard video={video()} pending onAction={vi.fn()} onContextMenu={vi.fn()} />,
    );
    const button = () => screen.getByRole("button", { name: /^Download$/ });
    expect(button().className).not.toContain("is-spinning");
    rerender(<VideoCard video={video()} pending slow onAction={vi.fn()} onContextMenu={vi.fn()} />);
    expect(button().className).toContain("is-spinning");
  });

  it("drops the second click of a double-click", () => {
    const { onAction } = renderCard();
    const card = screen.getByRole("article");
    fireEvent.click(card, { detail: 1 });
    fireEvent.click(card, { detail: 2 });
    expect(onAction).toHaveBeenCalledTimes(1);
  });

  it("reads its progress from the store, by its own id", () => {
    const store = createProgressStore();
    render(
      <VideoCard
        video={video({ download_state: "downloading" })}
        progress={store}
        onAction={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );
    act(() => store.set({ videoId: "someone-else", percent: 90, speed: "", eta: "" }));
    expect(screen.getByText("0%")).toBeTruthy();
    act(() => store.set({ videoId: "J1WoNuemKOg", percent: 42, speed: "3MiB/s", eta: "00:10" }));
    expect(screen.getByText("42%")).toBeTruthy();
    expect(screen.getByText(/3MiB\/s/)).toBeTruthy();
  });
});

describe("VideoCard downloaded state", () => {
  it("turns the whole card green, with the word kept for screen readers", () => {
    const { container } = render(
      <VideoCard
        video={video({ download_state: "done", file_path: "/v.mkv" })}
        onAction={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );
    expect(container.querySelector(".card")?.className).toContain("is-downloaded");
    expect(screen.getByText("Downloaded").className).toBe("sr-only");
  });

  it("stays neutral when the file is gone", () => {
    const { container } = render(
      <VideoCard
        video={video({ download_state: "done", file_path: null })}
        onAction={vi.fn()}
        onContextMenu={vi.fn()}
      />,
    );
    expect(container.querySelector(".card")?.className).not.toContain("is-downloaded");
  });
});

describe("VideoCard hover bar", () => {
  const DONE = { download_state: "done" as DownloadState, file_path: "/v.mkv" };
  const tileNames = () =>
    Array.from(document.querySelectorAll(".card-tile")).map((t) => t.getAttribute("aria-label"));

  it("holds four tiles in slots that never move: action, watched, YouTube, remove", () => {
    renderCard(DONE);
    expect(tileNames()).toEqual(["Play", "Mark as watched", "Open on YouTube", "Delete downloaded file"]);
    cleanup();
    renderCard();
    expect(tileNames()).toEqual(["Download", "Mark as watched", "Open on YouTube", "Hide from feed"]);
  });

  it("routes each tile through onAction without also firing the card", () => {
    const { onAction } = renderCard(DONE);
    fireEvent.click(screen.getByRole("button", { name: "Mark as watched" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete downloaded file" }));
    expect(onAction.mock.calls.map((c) => c[0])).toEqual(["watched", "delete-file"]);
  });

  it("marks only the permanent tile as dangerous, and the main one as main", () => {
    renderCard(DONE);
    expect(screen.getByRole("button", { name: "Delete downloaded file" }).className).toContain("is-danger");
    expect(screen.getByRole("button", { name: "Play" }).className).toContain("is-main");
    cleanup();
    renderCard();
    expect(screen.getByRole("button", { name: "Hide from feed" }).className).not.toContain("is-danger");
  });

  // Cancel sits in the main slot but must never be the bright, easy tile.
  it("never makes Cancel the main tile", () => {
    renderCard({ download_state: "queued" });
    expect(screen.getByRole("button", { name: "Cancel download" }).className).not.toContain("is-main");
  });

  it("holds the main and remove tiles for a call in flight, but not watched or YouTube", () => {
    render(<VideoCard video={video(DONE)} pending onAction={vi.fn()} onContextMenu={vi.fn()} />);
    const disabled = (name: string) =>
      screen.getByRole<HTMLButtonElement>("button", { name }).disabled;
    expect(disabled("Play")).toBe(true);
    expect(disabled("Delete downloaded file")).toBe(true);
    expect(disabled("Mark as watched")).toBe(false);
    expect(disabled("Open on YouTube")).toBe(false);
  });

  it("shows the progress ring as progress, leaving Cancel to the tile", () => {
    renderCard({ download_state: "downloading" });
    expect(screen.getByRole("progressbar", { name: "Download progress" })).toBeTruthy();
    expect(screen.getAllByRole("button", { name: /Cancel download/ })).toHaveLength(1);
  });
});

describe("VideoCard asking about its file", () => {
  const DONE = { download_state: "done" as DownloadState, file_path: "/v.mkv" };
  function renderAsking(asking: "file" | "watched", over: Partial<Video> = {}) {
    const onAction = vi.fn();
    const onAnswer = vi.fn();
    render(
      <VideoCard
        video={video({ ...DONE, ...over })}
        asking={asking}
        onAnswer={onAnswer}
        onAction={onAction}
        onContextMenu={vi.fn()}
      />,
    );
    return { onAction, onAnswer };
  }

  it("asks on the card, naming the video, with Keep holding the focus", () => {
    renderAsking("watched", { watched: true });
    const asked = screen.getByRole("group", { name: "Delete the downloaded file?" });
    expect(asked.textContent).toContain("Delete the file? Marked watched.");
    expect(asked.textContent).toContain("Some video");
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Keep" }));
  });

  it("answers Keep and Delete file, never running the card's own action", () => {
    const { onAction, onAnswer } = renderAsking("file");
    fireEvent.click(screen.getByRole("group", { name: "Delete the downloaded file?" }));
    fireEvent.click(screen.getByRole("button", { name: "Keep" }));
    fireEvent.click(screen.getByRole("button", { name: "Delete file" }));
    expect(onAnswer.mock.calls.map((c) => c[1])).toEqual([false, true]);
    expect(onAction).not.toHaveBeenCalled();
  });

  it("says when the video has not been watched", () => {
    renderAsking("file");
    expect(screen.getByRole("group").textContent).toContain("Delete the file? Not watched yet.");
  });

  it("goes as Keep when the pointer leaves", () => {
    const { onAnswer } = renderAsking("file");
    fireEvent.mouseLeave(screen.getByRole("article"));
    expect(onAnswer).toHaveBeenCalledWith(expect.anything(), false);
  });

  it("goes as Keep when nobody answers, unless the pointer comes to it", () => {
    vi.useFakeTimers();
    try {
      const { onAnswer } = renderAsking("file");
      act(() => { vi.advanceTimersByTime(5999); });
      expect(onAnswer).not.toHaveBeenCalled();
      act(() => { vi.advanceTimersByTime(1); });
      expect(onAnswer).toHaveBeenCalledWith(expect.anything(), false);

      cleanup();
      const held = renderAsking("file");
      fireEvent.mouseEnter(screen.getByRole("article"));
      act(() => { vi.advanceTimersByTime(60_000); });
      expect(held.onAnswer).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });
});
