import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import VideoCard from "./VideoCard";
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
    file_path: null, downloaded_at: null, first_seen_at: 0,
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
