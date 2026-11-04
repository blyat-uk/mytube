import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, within, act } from "@testing-library/react";
import type { DownloadSeries, Video } from "../types";

vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

const listVideos = vi.fn(() => Promise.resolve(videos));
const setWatched = vi.fn((_id: string, _watched: boolean) => Promise.resolve());
const deleteDownload = vi.fn((_id: string) => Promise.resolve());
const cancelDownload = vi.fn((_id: string) => Promise.resolve());
const enqueueDownload = vi.fn((_id: string) => Promise.resolve());
const openInPlayer = vi.fn((_id: string) => Promise.resolve());
const openExternal = vi.fn((_url: string) => Promise.resolve());
const downloadSeries = vi.fn((_ids: string[]): Promise<DownloadSeries[]> => Promise.resolve([]));

vi.mock("../api", () => ({
  api: {
    listVideos: (...a: unknown[]) => listVideos(...(a as [])),
    setWatched: (id: string, watched: boolean) => setWatched(id, watched),
    deleteDownload: (id: string) => deleteDownload(id),
    cancelDownload: (id: string) => cancelDownload(id),
    enqueueDownload: (id: string) => enqueueDownload(id),
    openInPlayer: (id: string) => openInPlayer(id),
    openExternal: (url: string) => openExternal(url),
    downloadSeries: (ids: string[]) => downloadSeries(ids),
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
    downloaded_at: null, first_seen_at: 0, sibling_group: null, series_hidden: false,
    part_links_refused: false, part_linked: false, series_auto_download: false,
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
  cancelDownload.mockImplementation(() => Promise.resolve());
  enqueueDownload.mockImplementation(() => Promise.resolve());
  openInPlayer.mockImplementation(() => Promise.resolve());
  openExternal.mockImplementation(() => Promise.resolve());
  downloadSeries.mockImplementation(() => Promise.resolve([]));
});
afterEach(cleanup);

/** Stands in for the shell's `.content`, the area a popped-out thumbnail must stay inside. */
let scrollArea: HTMLElement;

function renderView(cardSize = 260, search = "", onOpenSeries = vi.fn()) {
  scrollArea = document.createElement("main");
  const tree = (q: string) => (
    <ToastProvider>
      <DownloadsView
        reloadToken={0} cardSize={cardSize} scrollRef={{ current: scrollArea }}
        search={q} onOpenSeries={onOpenSeries}
      />
    </ToastProvider>
  );
  const r = render(tree(search));
  return { onOpenSeries, search: (q: string) => r.rerender(tree(q)) };
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

  it("offers every row, in every section, a way to its YouTube page, last but for Delete file", async () => {
    videos = [
      video({ id: "a", title: "Running", download_state: "downloading" }),
      video({ id: "b", title: "Finished", download_state: "done" }),
      video({ id: "c", title: "Broken", download_state: "failed", file_path: null }),
    ];
    renderView();

    await screen.findByText("Finished");
    const rows = [...document.querySelectorAll(".dl-row")];
    expect(rows).toHaveLength(3);
    const finished = within(rows.find((r) => r.textContent?.includes("Finished")) as HTMLElement);
    const finishedButtons = finished.getAllByRole("button");
    expect(finishedButtons[finishedButtons.length - 1].textContent).toBe("Delete file");
    for (const row of rows) {
      const buttons = within(row as HTMLElement).getAllByRole("button");
      // Last in every row, except that a completed one ends on Delete file.
      const last = buttons[buttons.length - 1];
      const open = last.textContent === "Delete file" ? buttons[buttons.length - 2] : last;
      expect(open.textContent).toBe("Open on YouTube");
      expect(open.classList.contains("icon-btn")).toBe(true);
      expect(open.getAttribute("title")).toBeTruthy();
    }
  });

  it("opens the video's watch page in the browser", async () => {
    videos = [video({ id: "a b", title: "Finished", download_state: "done" })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Open on YouTube" }));
    await waitFor(() => expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/watch?v=a%20b"));
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

/**
 * Every row action goes out once, however hard it is clicked, and shows its
 * result before the backend answers -- putting the row back if it refuses.
 */
describe("Downloads row actions in flight", () => {
  it("takes a cancelled download off the list at once", async () => {
    let finish = () => {};
    cancelDownload.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    videos = [video({ id: "a", title: "Running", download_state: "downloading" })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Cancel download" }));
    await waitFor(() => expect(screen.queryByText("Running")).toBeNull());
    expect(cancelDownload).toHaveBeenCalledTimes(1);
    await act(async () => { finish(); });
  });

  it("puts a cancelled row back, and says why, when the cancel fails", async () => {
    cancelDownload.mockImplementation(() => Promise.reject("no such job"));
    videos = [video({ id: "a", title: "Running", download_state: "downloading" })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Cancel download" }));
    expect(await screen.findByText("no such job")).toBeTruthy();
    expect(titlesUnder("Active")).toEqual(["Running"]);
  });

  it("puts a retried row back under Failed when the queue refuses it", async () => {
    enqueueDownload.mockImplementation(() => Promise.reject("queue is closed"));
    videos = [video({
      id: "a", title: "Broken", download_state: "failed", file_path: null, download_error: "HTTP 403",
    })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: "Retry download" }));
    expect(await screen.findByText("queue is closed")).toBeTruthy();
    expect(titlesUnder("Failed")).toEqual(["Broken"]);
    expect(screen.getByText("HTTP 403")).toBeTruthy();
  });

  it("opens one player for a double-click, and none while the first is starting", async () => {
    let finish = () => {};
    openInPlayer.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    videos = [video({ id: "a", title: "Finished", download_state: "done" })];
    renderView();

    const play = await screen.findByRole<HTMLButtonElement>("button", { name: "Play" });
    fireEvent.click(play, { detail: 1 });
    fireEvent.click(play, { detail: 2 });
    expect(openInPlayer).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(play.getAttribute("aria-busy")).toBe("true"));
    expect(play.disabled).toBe(true);
    // The row's other file action waits too: deleting a file mid-launch is a race.
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Delete file" }).disabled).toBe(true);

    await act(async () => { finish(); });
    await waitFor(() => expect(play.disabled).toBe(false));
  });

  it("never locks a row's download actions while the browser is coming up", async () => {
    let finish = () => {};
    openExternal.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    videos = [video({ id: "a", title: "Finished", download_state: "done" })];
    renderView();

    const open = await screen.findByRole<HTMLButtonElement>("button", { name: "Open on YouTube" });
    fireEvent.click(open, { detail: 1 });
    fireEvent.click(open, { detail: 2 });
    expect(openExternal).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(open.disabled).toBe(true));
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Play" }).disabled).toBe(false);
    expect(screen.getByRole<HTMLButtonElement>("button", { name: "Delete file" }).disabled).toBe(false);

    await act(async () => { finish(); });
    await waitFor(() => expect(open.disabled).toBe(false));
  });

  it("deletes a file once, however often the button is pressed", async () => {
    videos = [video({ id: "a", title: "Finished", download_state: "done" })];
    renderView();

    const del = await screen.findByRole("button", { name: "Delete file" });
    fireEvent.click(del);
    fireEvent.click(del);
    await waitFor(() => expect(screen.queryByText("Finished")).toBeNull());
    expect(deleteDownload).toHaveBeenCalledTimes(1);
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

/**
 * A download in a series links into it: which series, which part, what has
 * come out since that is not downloaded yet, and Open series.
 */
describe("a download's series", () => {
  const SERIES: DownloadSeries = { videoId: "p2", stem: "The Lighthouse Files", position: 2, parts: 12, fresh: 3 };

  it("wears its series on rows that belong to one, and no link on the rest", async () => {
    videos = [video({ id: "p2", title: "Episode 2" }), video({ id: "lone", title: "A lone video" })];
    downloadSeries.mockImplementation(() => Promise.resolve([SERIES]));
    renderView();
    const link = await screen.findByRole("button", {
      name: "Open the series The Lighthouse Files: part 2 of 12, 3 new parts",
    });
    expect(link.textContent).toBe("The Lighthouse Filespart 2 of 12+3 newOpen series");
    expect(downloadSeries).toHaveBeenCalledWith(["lone", "p2"]);
    expect(screen.getAllByRole("button", { name: /^Open the series/ })).toHaveLength(1);
  });

  it("opens the series, led by that download", async () => {
    videos = [video({ id: "p2", title: "Episode 2" })];
    downloadSeries.mockImplementation(() => Promise.resolve([{ ...SERIES, fresh: 0 }]));
    const { onOpenSeries } = renderView();
    const link = await screen.findByRole("button", { name: "Open the series The Lighthouse Files: part 2 of 12" });
    expect(link.textContent).not.toContain("new");
    fireEvent.click(link);
    expect(onOpenSeries).toHaveBeenCalledWith(expect.objectContaining({ id: "p2" }), "The Lighthouse Files");
  });

  it("leaves the links off when they cannot be had", async () => {
    videos = [video({ id: "p2", title: "Episode 2" })];
    downloadSeries.mockImplementation(() => Promise.reject("no"));
    renderView();
    await screen.findByText("Episode 2");
    expect(screen.queryByRole("button", { name: /^Open the series/ })).toBeNull();
  });
});

/** The search over this tab: the feed's language, over titles, channels and series names. */
describe("searching downloads", () => {
  it("keeps the rows that match, by title, channel or series name, and marks the match", async () => {
    videos = [
      video({ id: "a", title: "Episode 4", channel_title: "Night Shift Radio" }),
      video({ id: "b", title: "Cooking with fire", channel_title: "Kitchen Basics" }),
      video({ id: "c", title: "Lighthouse at dawn", channel_title: "Kitchen Basics" }),
    ];
    downloadSeries.mockImplementation(() => Promise.resolve([
      { videoId: "a", stem: "The Lighthouse Files", position: 4, parts: 12, fresh: 0 },
    ]));
    const view = renderView();
    await screen.findByRole("button", { name: /^Open the series/ });

    view.search("lighthouse");
    expect(titlesUnder("Completed")).toEqual(["Episode 4", "Lighthouse at dawn"]);
    expect([...document.querySelectorAll(".dl-hit")].map((m) => m.textContent)).toEqual(["Lighthouse", "Lighthouse"]);

    view.search("kitchen -fire");
    expect(titlesUnder("Completed")).toEqual(["Lighthouse at dawn"]);
  });

  it("says when nothing matches", async () => {
    videos = [video({ id: "a", title: "Episode 4" })];
    const view = renderView();
    await screen.findByText("Episode 4");
    view.search("zzz");
    expect(screen.getByText("No download matches “zzz”")).toBeDefined();
  });
});
