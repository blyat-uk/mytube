import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";
import type { Video } from "../types";

vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

const listVideos = vi.fn(() => Promise.resolve(videos));
const setWatched = vi.fn((_id: string, _watched: boolean) => Promise.resolve());
const deleteDownload = vi.fn((_id: string) => Promise.resolve());

vi.mock("../api", () => ({
  api: {
    listVideos: (...a: unknown[]) => listVideos(...(a as [])),
    setWatched: (id: string, watched: boolean) => setWatched(id, watched),
    deleteDownload: (id: string) => deleteDownload(id),
  },
  thumbSrc: (v: { thumb_path: string | null }) => v.thumb_path ?? "",
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
    downloaded_at: null, first_seen_at: 0, sibling_group: null,
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
  setWatched.mockImplementation(() => Promise.resolve());
  deleteDownload.mockImplementation(() => Promise.resolve());
});
afterEach(cleanup);

/** Stands in for the shell's `.content`, the area a popped-out thumbnail must stay inside. */
let scrollArea: HTMLElement;

function renderView(cardSize = 260) {
  scrollArea = document.createElement("main");
  render(
    <ToastProvider>
      <DownloadsView reloadToken={0} cardSize={cardSize} scrollRef={{ current: scrollArea }} />
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

describe("Downloads row actions", () => {
  it("offers each row's actions as named icon buttons", async () => {
    videos = [
      video({ id: "a", title: "Running", download_state: "downloading" }),
      video({ id: "b", title: "Finished", download_state: "done" }),
      video({ id: "c", title: "Broken", download_state: "failed", file_path: null }),
    ];
    renderView();

    await screen.findByText("Finished");
    for (const name of ["Cancel download", "Play", "Mark as watched", "Delete file", "Retry download"]) {
      const button = screen.getByRole("button", { name });
      expect(button.classList.contains("icon-btn")).toBe(true);
      // No visible words: the glyph stands in, and the tooltip names it.
      expect(button.getAttribute("title")).toBeTruthy();
    }
  });

  it("marks a finished download watched, then asks whether to delete its file", async () => {
    videos = [video({ id: "a", title: "Finished", download_state: "done" })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Mark as watched" }));

    const dialog = await screen.findByRole("dialog", { name: "Delete the downloaded file?" });
    expect(within(dialog).getByText(/is marked as watched/)).toBeTruthy();
    expect(setWatched).toHaveBeenCalledWith("a", true);
    expect(deleteDownload).not.toHaveBeenCalled();

    fireEvent.click(within(dialog).getByRole("button", { name: "Delete file" }));

    await waitFor(() => expect(deleteDownload).toHaveBeenCalledWith("a"));
    await waitFor(() => expect(screen.queryByText("Finished")).toBeNull());
  });

  it("keeps the file, and the row, when the prompt is dismissed", async () => {
    videos = [video({ id: "a", title: "Finished", download_state: "done" })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Mark as watched" }));
    const dialog = await screen.findByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Cancel" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(deleteDownload).not.toHaveBeenCalled();
    expect(screen.getByText("Finished")).toBeTruthy();
    // The button now undoes what it just did.
    expect(screen.getByRole("button", { name: "Mark as unwatched" })).toBeTruthy();
  });

  it("unmarks a watched download without asking anything", async () => {
    videos = [video({ id: "a", title: "Seen", download_state: "done", watched: true, watched_at: 5 })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Mark as unwatched" }));

    await waitFor(() => expect(setWatched).toHaveBeenCalledWith("a", false));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(await screen.findByRole("button", { name: "Mark as watched" })).toBeTruthy();
  });

  it("puts the watched state back, and asks nothing, when the backend refuses", async () => {
    setWatched.mockImplementation(() => Promise.reject("database is locked"));
    videos = [video({ id: "a", title: "Finished", download_state: "done" })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Mark as watched" }));

    expect(await screen.findByText("database is locked")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Mark as watched" })).toBeTruthy();
    expect(screen.queryByRole("dialog")).toBeNull();
  });
});

describe("Downloads thumbnail pop-out", () => {
  const rect = (left: number, top: number, width: number, height: number) =>
    ({ left, top, width, height, right: left + width, bottom: top + height, x: left, y: top }) as DOMRect;

  it("hands the Subscriptions card size to the page", async () => {
    videos = [video({ id: "a", title: "Finished", thumb_path: "/t/a.jpg" })];
    renderView(640);

    await screen.findByText("Finished");
    expect(document.querySelector<HTMLElement>(".page")!.style.getPropertyValue("--card-w")).toBe("640px");
  });

  it("works out where the picture lands when the pointer reaches the thumbnail", async () => {
    videos = [video({ id: "a", title: "Near the bottom", thumb_path: "/t/a.jpg" })];
    renderView(640);

    await screen.findByText("Near the bottom");
    scrollArea.getBoundingClientRect = () => rect(0, 60, 1200, 800);
    Object.defineProperty(scrollArea, "clientWidth", { value: 1185 });
    Object.defineProperty(scrollArea, "clientHeight", { value: 800 });
    const thumb = document.querySelector<HTMLElement>(".dl-thumb")!;
    thumb.getBoundingClientRect = () => rect(50, 700, 128, 72);

    fireEvent.mouseEnter(thumb);

    expect(thumb.style.getPropertyValue("--pop-w")).toBe("640px");
    // The area ends at 860; 8px short of that, less 360px of picture, is 492.
    expect(thumb.style.getPropertyValue("--pop-y")).toBe("-208px");
  });

  it("leaves a row with no picture where it is", async () => {
    videos = [video({ id: "a", title: "Blank" })];
    renderView(640);

    await screen.findByText("Blank");
    expect(document.querySelector(".dl-thumb")!.classList.contains("can-pop")).toBe(false);
  });
});
