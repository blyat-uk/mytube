import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, cleanup, waitFor } from "@testing-library/react";
import type { Video } from "../types";

vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

const listVideos = vi.fn(() => Promise.resolve(videos));

vi.mock("../api", () => ({
  api: { listVideos: (...a: unknown[]) => listVideos(...(a as [])) },
  thumbSrc: () => "",
  errText: (e: unknown) => String(e),
}));

import DownloadsView from "./DownloadsView";
import { ToastProvider } from "./Toast";

function video(over: Partial<Video>): Video {
  return {
    id: "vid", channel_id: "UC1", channel_title: "Veritasium", title: "Some video",
    description: null, thumb_url: null, thumb_path: null, published_at: 1_700_000_000,
    sort_at: 1_700_000_000, feed_rank: 0, added_manually: false, duration_secs: 754,
    view_count: 1500, status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "done", download_error: null, file_path: "/videos/some.mkv",
    downloaded_at: null, first_seen_at: 0,
    ...over,
  };
}

let videos: Video[] = [];

/** Titles listed under the named section heading, in the order they render. */
function titlesUnder(heading: string): string[] {
  const section = [...document.querySelectorAll(".dl-section")].find((s) =>
    s.querySelector(".section-title")?.textContent?.startsWith(heading),
  );
  return [...(section?.querySelectorAll(".dl-title") ?? [])].map((n) => n.textContent ?? "");
}

beforeEach(() => {
  vi.clearAllMocks();
  listVideos.mockImplementation(() => Promise.resolve(videos));
});
afterEach(cleanup);

function renderView() {
  render(
    <ToastProvider>
      <DownloadsView reloadToken={0} />
    </ToastProvider>,
  );
}

describe("Downloads ordering", () => {
  it("lists completed downloads by fetch time, not by release date", async () => {
    videos = [
      // The newer upload, but it has been sitting on disk for a month.
      video({ id: "a", title: "Fetched a month ago", sort_at: 9_000, downloaded_at: 1_000 }),
      // An old video pulled down this morning.
      video({ id: "b", title: "Fetched today", sort_at: 1_000, downloaded_at: 9_000 }),
    ];
    renderView();

    await waitFor(() =>
      expect(titlesUnder("Completed")).toEqual(["Fetched today", "Fetched a month ago"]),
    );
  });

  it("sinks a download with no recorded fetch time to the bottom", async () => {
    videos = [
      video({ id: "a", title: "Undated", sort_at: 9_000, downloaded_at: null }),
      video({ id: "b", title: "Dated", sort_at: 1_000, downloaded_at: 5 }),
    ];
    renderView();

    await waitFor(() => expect(titlesUnder("Completed")).toEqual(["Dated", "Undated"]));
  });

  it("reads the active queue oldest first, the order it will actually run", async () => {
    videos = [
      video({ id: "b", title: "Queued second", download_state: "queued", downloaded_at: 2_000 }),
      video({ id: "a", title: "Queued first", download_state: "downloading", downloaded_at: 1_000 }),
    ];
    renderView();

    await waitFor(() =>
      expect(titlesUnder("Active")).toEqual(["Queued first", "Queued second"]),
    );
  });

  it("asks the backend for the fetch ordering, so the sweep window is the recent downloads", async () => {
    videos = [video({ id: "a", downloaded_at: 1 })];
    renderView();

    await waitFor(() => expect(listVideos).toHaveBeenCalledTimes(2));
    for (const [filter] of listVideos.mock.calls as unknown as [{ sort: string }][]) {
      expect(filter.sort).toBe("downloaded");
    }
  });
});
