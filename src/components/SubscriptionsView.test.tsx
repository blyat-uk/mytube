import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import type { Video, VideoGroup } from "../types";

vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

// jsdom ships no IntersectionObserver, and the grid's infinite scroll wants one.
class NoopObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("IntersectionObserver", NoopObserver);

const setWatched = vi.fn(() => Promise.resolve());
const deleteDownload = vi.fn(() => Promise.resolve());
const listVideos = vi.fn(() => Promise.resolve(videos));
const listVideoGroups = vi.fn(() => Promise.resolve(groups));

vi.mock("../api", () => ({
  api: {
    listVideos: (...a: unknown[]) => listVideos(...(a as [])),
    listVideoGroups: (...a: unknown[]) => listVideoGroups(...(a as [])),
    setWatched: (...a: unknown[]) => setWatched(...(a as [])),
    deleteDownload: (...a: unknown[]) => deleteDownload(...(a as [])),
  },
  thumbSrc: () => "",
  errText: (e: unknown) => String(e),
}));

import SubscriptionsView from "./SubscriptionsView";
import { ToastProvider } from "./Toast";

function video(over: Partial<Video> = {}): Video {
  return {
    id: "vid1", channel_id: "UC1", channel_title: "Veritasium", title: "Some video",
    description: null, thumb_url: null, thumb_path: null, published_at: 1_700_000_000,
    sort_at: null, feed_rank: 0, added_manually: false, duration_secs: 754,
    view_count: 1500, status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "done", download_error: null, file_path: "/videos/some.mkv",
    downloaded_at: null, first_seen_at: 0,
    ...over,
  };
}

let videos: Video[] = [];
let groups: VideoGroup[] = [];

type ViewProps = Partial<React.ComponentProps<typeof SubscriptionsView>>;

function renderView(over: ViewProps = {}) {
  const onFindSiblings = vi.fn();
  const onSeriesTally = vi.fn();
  render(
    <ToastProvider>
      <SubscriptionsView
        channelId={null} search="" hideWatched={false} downloadedOnly={false}
        sort="newest" showHidden={false} grouped={false} siblingOf={null}
        onFindSiblings={onFindSiblings} onSeriesTally={onSeriesTally}
        cardSize={260} onCardSize={vi.fn()} reloadToken={0}
        channelCount={1} onAdd={vi.fn()}
        {...over}
      />
    </ToastProvider>,
  );
  return { onFindSiblings, onSeriesTally };
}

/** Right-click the only card and read back the menu. */
async function openMenu() {
  const card = await screen.findByRole("article");
  fireEvent.contextMenu(card);
  return screen.getByRole("menu");
}

beforeEach(() => {
  videos = [video()];
  groups = [];
  vi.clearAllMocks();
  listVideos.mockImplementation(() => Promise.resolve(videos));
  listVideoGroups.mockImplementation(() => Promise.resolve(groups));
});
afterEach(cleanup);

describe("marking a downloaded video as watched", () => {
  it("marks it watched and then asks about the file", async () => {
    renderView();
    fireEvent.click(within(await openMenu(), "Mark as watched"));

    await waitFor(() => expect(setWatched).toHaveBeenCalledWith("vid1", true));
    expect(await screen.findByText("Delete the downloaded file?")).toBeTruthy();
    // Asking is not doing: nothing is removed until the choice is made.
    expect(deleteDownload).not.toHaveBeenCalled();
  });

  it("deletes the file on confirm and drops the downloaded styling", async () => {
    renderView();
    fireEvent.click(within(await openMenu(), "Mark as watched"));
    fireEvent.click(await screen.findByRole("button", { name: "Delete file" }));

    await waitFor(() => expect(deleteDownload).toHaveBeenCalledWith("vid1"));
    await waitFor(() =>
      expect(document.querySelector(".card")?.className).not.toContain("is-downloaded"),
    );
  });

  it("keeps the file when the dialog is dismissed", async () => {
    renderView();
    fireEvent.click(within(await openMenu(), "Mark as watched"));
    fireEvent.click(await screen.findByRole("button", { name: "Cancel" }));

    expect(deleteDownload).not.toHaveBeenCalled();
    expect(document.querySelector(".card")?.className).toContain("is-downloaded");
  });

  it("never asks when there is no file to delete", async () => {
    videos = [video({ download_state: "none", file_path: null })];
    renderView();
    fireEvent.click(within(await openMenu(), "Mark as watched"));

    await waitFor(() => expect(setWatched).toHaveBeenCalled());
    expect(screen.queryByText("Delete the downloaded file?")).toBeNull();
  });

  it("never asks when un-watching", async () => {
    videos = [video({ watched: true })];
    renderView();
    fireEvent.click(within(await openMenu(), "Mark as unwatched"));

    await waitFor(() => expect(setWatched).toHaveBeenCalledWith("vid1", false));
    expect(screen.queryByText("Delete the downloaded file?")).toBeNull();
  });
});

describe("the right-click menu's delete-file entry", () => {
  it("offers to delete a file left behind by a watched video", async () => {
    videos = [video({ watched: true })];
    renderView();
    const menu = await openMenu();
    fireEvent.click(within(menu, "Delete downloaded file"));

    expect(await screen.findByText(/is marked as watched/)).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Delete file" }));
    await waitFor(() => expect(deleteDownload).toHaveBeenCalledWith("vid1"));
  });

  // Deciding not to watch something leaves a file behind exactly as watching it
  // does, so the entry is not gated on watched.
  it("offers it for an unwatched video too", async () => {
    renderView();
    const menu = await openMenu();
    fireEvent.click(within(menu, "Delete downloaded file"));

    // The copy has to stop claiming the video was watched, and say what is
    // being thrown away.
    expect(await screen.findByText(/has not been watched/)).toBeTruthy();
    fireEvent.click(await screen.findByRole("button", { name: "Delete file" }));
    await waitFor(() => expect(deleteDownload).toHaveBeenCalledWith("vid1"));
  });

  it("hides the entry when the file is already gone, watched or not", async () => {
    for (const watched of [true, false]) {
      videos = [video({ watched, download_state: "none", file_path: null })];
      renderView();
      expect(menuLabels(await openMenu())).not.toContain("Delete downloaded file");
      cleanup();
    }
  });
});

function menuLabels(menu: HTMLElement): string[] {
  return Array.from(menu.querySelectorAll("[role=menuitem]")).map((n) => n.textContent ?? "");
}

function within(menu: HTMLElement, label: string): HTMLElement {
  const hit = Array.from(menu.querySelectorAll("[role=menuitem]")).find(
    (n) => n.textContent === label,
  );
  if (!hit) throw new Error(`no menu item "${label}" in: ${menuLabels(menu).join(", ")}`);
  return hit as HTMLElement;
}

describe("the grouped feed", () => {
  const series: VideoGroup = {
    videos: [
      video({ id: "p3", title: "The Blackwood Tapes - Part 3" }),
      video({ id: "p2", title: "The Blackwood Tapes - Part 2" }),
      video({ id: "p1", title: "The Blackwood Tapes" }),
    ],
    stem: "The Blackwood Tapes",
  };

  it("asks the grouping command instead of the flat one", async () => {
    groups = [series];
    renderView({ grouped: true });
    await waitFor(() => expect(listVideoGroups).toHaveBeenCalled());
    expect(listVideos).not.toHaveBeenCalled();
  });

  it("draws a series as one card carrying its shared name and part count", async () => {
    groups = [series];
    renderView({ grouped: true });
    expect((await screen.findByRole("heading")).textContent).toBe("The Blackwood Tapes");
    expect(screen.getByText("3")).toBeTruthy();
    expect(document.querySelectorAll(".card").length).toBe(1);
  });

  it("opens the series on its leader when the card is clicked", async () => {
    groups = [series];
    const { onFindSiblings } = renderView({ grouped: true });
    fireEvent.click(await screen.findByRole("button", { name: /The Blackwood Tapes/ }));
    expect(onFindSiblings).toHaveBeenCalledTimes(1);
    expect(onFindSiblings.mock.calls[0][0].id).toBe("p3");
    // The shared name travels with it: the breadcrumb wears it, and the
    // leader's own title carries a part number the series as a whole has not.
    expect(onFindSiblings.mock.calls[0][1]).toBe("The Blackwood Tapes");
  });

  it("leaves a lone video as an ordinary card, right-click menu and all", async () => {
    groups = [{ videos: [video()], stem: null }];
    renderView({ grouped: true });
    expect(menuLabels(await openMenu())).toContain("Find siblings");
  });

  // A series view is a flat list of its parts. Grouping there would collapse
  // the very series the view exists to open.
  it("stands down while a series view is open", async () => {
    renderView({ grouped: true, siblingOf: video() });
    await waitFor(() => expect(listVideos).toHaveBeenCalled());
    expect(listVideoGroups).not.toHaveBeenCalled();
  });
});

/**
 * The nav's breadcrumb says what the open series holds. Only this view knows —
 * it is the one that fetched the parts — so the count and the runtime are
 * handed up as each page settles.
 */
describe("what an open series reports to the breadcrumb", () => {
  it("counts the parts and adds up their runtimes", async () => {
    videos = [
      video({ id: "p1", duration_secs: 600 }),
      video({ id: "p2", duration_secs: 1200 }),
      video({ id: "p3", duration_secs: 600 }),
    ];
    const { onSeriesTally } = renderView({ siblingOf: video({ id: "p1" }) });
    await waitFor(() => expect(onSeriesTally).toHaveBeenCalled());
    expect(onSeriesTally).toHaveBeenLastCalledWith({ parts: 3, runtime: "40m", unknown: 0 });
  });

  /** A part with no duration yet would silently understate the total. */
  it("carries out how many runtimes are still missing", async () => {
    videos = [
      video({ id: "p1", duration_secs: 3600 }),
      video({ id: "p2", duration_secs: null }),
    ];
    const { onSeriesTally } = renderView({ siblingOf: video({ id: "p1" }) });
    await waitFor(() => expect(onSeriesTally).toHaveBeenCalled());
    expect(onSeriesTally).toHaveBeenLastCalledWith({ parts: 2, runtime: "1h", unknown: 1 });
  });

  it("reports a lone anchor as one part, which is an answer of its own", async () => {
    videos = [video({ id: "p1", duration_secs: 754 })];
    const { onSeriesTally } = renderView({ siblingOf: video({ id: "p1" }) });
    await waitFor(() => expect(onSeriesTally).toHaveBeenCalled());
    expect(onSeriesTally).toHaveBeenLastCalledWith({ parts: 1, runtime: "12m", unknown: 0 });
  });

  it("says nothing at all in the ordinary feed", async () => {
    videos = [video()];
    const { onSeriesTally } = renderView();
    await waitFor(() => expect(listVideos).toHaveBeenCalled());
    expect(onSeriesTally).not.toHaveBeenCalled();
  });
});
