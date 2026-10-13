import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act, getByRole } from "@testing-library/react";
import type { Video, VideoFilter, VideoGroup } from "../types";

vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

// jsdom ships no IntersectionObserver, and the grid's infinite scroll wants one.
// This one keeps its callbacks, so `pageIn()` below can reach the sentinel the
// way scrolling to the bottom of the grid does.
type Fired = (entries: { isIntersecting: boolean }[]) => void;
const sentinels: Fired[] = [];
class StubObserver {
  constructor(fired: Fired) { sentinels.push(fired); }
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("IntersectionObserver", StubObserver);

/** Scroll the sentinel into view, which is what fetches the next page. */
function pageIn() {
  act(() => { for (const fired of sentinels) fired([{ isIntersecting: true }]); });
}

const setWatched = vi.fn(() => Promise.resolve());
const deleteDownload = vi.fn(() => Promise.resolve());
const markSiblings = vi.fn(() => Promise.resolve(2));
const unlinkSiblings = vi.fn(() => Promise.resolve());
const enqueueDownload = vi.fn((_id: string, _q?: unknown) => Promise.resolve());
const probeFormats = vi.fn((_id: string) => Promise.resolve(rickroll()));
const cancelDownload = vi.fn((_id: string) => Promise.resolve());
const setVideoHidden = vi.fn((_id: string, _hidden: boolean) => Promise.resolve());
const setSeriesHidden = vi.fn((_leader: string, _hidden: boolean) => Promise.resolve<string[]>([]));
const enqueueDownloads = vi.fn((_ids: string[]) => Promise.resolve(0));
// Typed with the filter so a test can page through a library with it.
const listVideos = vi.fn<(f: VideoFilter) => Promise<Video[]>>(() => Promise.resolve(videos));
const listVideoGroups = vi.fn<(f: VideoFilter) => Promise<VideoGroup[]>>(() => Promise.resolve(groups));

vi.mock("../api", () => ({
  api: {
    listVideos: (...a: unknown[]) => listVideos(...(a as [VideoFilter])),
    listVideoGroups: (filter: VideoFilter) => listVideoGroups(filter),
    setWatched: (...a: unknown[]) => setWatched(...(a as [])),
    deleteDownload: (...a: unknown[]) => deleteDownload(...(a as [])),
    markSiblings: (...a: unknown[]) => markSiblings(...(a as [])),
    unlinkSiblings: (...a: unknown[]) => unlinkSiblings(...(a as [])),
    enqueueDownload: (id: string, q?: unknown) => enqueueDownload(id, q),
    probeFormats: (id: string) => probeFormats(id),
    cancelDownload: (id: string) => cancelDownload(id),
    setVideoHidden: (id: string, hidden: boolean) => setVideoHidden(id, hidden),
    setSeriesHidden: (leader: string, hidden: boolean) => setSeriesHidden(leader, hidden),
    enqueueDownloads: (ids: string[]) => enqueueDownloads(ids),
    getSettings: () => Promise.resolve({ quality: DEFAULT_QUALITY }),
  },
  thumbSrc: () => "",
  errText: (e: unknown) => String(e),
}));

import SubscriptionsView from "./SubscriptionsView";
import { DEFAULT_QUALITY } from "../quality";
import { rickroll } from "../quality.fixture";
import { ToastProvider } from "./Toast";

function video(over: Partial<Video> = {}): Video {
  return {
    id: "vid1", channel_id: "UC1", channel_title: "Veritasium", title: "Some video",
    description: null, thumb_url: null, thumb_path: null, published_at: 1_700_000_000,
    sort_at: null, feed_rank: 0, added_manually: false, duration_secs: 754,
    view_count: 1500, status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "done", download_error: null, file_path: "/videos/some.mkv",
    downloaded_at: null, first_seen_at: 0, sibling_group: null, series_hidden: false,
    ...over,
  };
}

let videos: Video[] = [];
let groups: VideoGroup[] = [];

type ViewProps = Partial<React.ComponentProps<typeof SubscriptionsView>>;

function renderView(over: ViewProps = {}) {
  const onFindSiblings = vi.fn();
  const onSeriesTally = vi.fn();
  // Stands in for the shell's `.content`: a real element, so a test can move
  // its offset and read back where the view put it.
  const scroller = document.createElement("div");
  document.body.appendChild(scroller);
  const scrollRef = { current: scroller };
  const view = (props: ViewProps) => (
    <ToastProvider>
      <SubscriptionsView
        channelIds={[]} search="" hideWatched={false} downloadedOnly={false}
        sort="newest" showHidden={false} grouped={false} groupsOnly={false} inProgress={false}
        siblingOf={null}
        onFindSiblings={onFindSiblings} onSeriesTally={onSeriesTally}
        cardSize={260} onCardSize={vi.fn()} reloadToken={0}
        channelCount={1} onAdd={vi.fn()} scrollRef={scrollRef}
        {...props}
      />
    </ToastProvider>
  );
  const { rerender } = render(view(over));
  return {
    onFindSiblings, onSeriesTally, scroller,
    /** Re-renders with the props changed, the rest of them as they were. */
    rerender: (next: ViewProps) => rerender(view({ ...over, ...next })),
  };
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
  sentinels.length = 0;
  vi.clearAllMocks();
  listVideos.mockImplementation(() => Promise.resolve(videos));
  listVideoGroups.mockImplementation(() => Promise.resolve(groups));
  probeFormats.mockImplementation(() => Promise.resolve(rickroll()));
  cancelDownload.mockImplementation(() => Promise.resolve());
  setVideoHidden.mockImplementation(() => Promise.resolve());
  setSeriesHidden.mockImplementation(() => Promise.resolve([]));
  enqueueDownloads.mockImplementation((ids) => Promise.resolve(ids.length));
  deleteDownload.mockImplementation(() => Promise.resolve());
  enqueueDownload.mockImplementation(() => Promise.resolve());
  markSiblings.mockImplementation(() => Promise.resolve(2));
});
afterEach(cleanup);

/**
 * A card's action flips with its state, so a click that lands twice -- a
 * double-click, or an impatient second press while the first is out -- used to
 * download and then cancel. Every action now goes out once and shows its
 * result before the backend answers.
 */
describe("card actions in flight", () => {
  it("downloads once for a double-click, and never cancels", async () => {
    videos = [video({ download_state: "none", file_path: null })];
    renderView();
    const card = await screen.findByRole("article");

    fireEvent.click(card, { detail: 1 });
    fireEvent.click(card, { detail: 2 });

    await waitFor(() => expect(enqueueDownload).toHaveBeenCalledTimes(1));
    expect(cancelDownload).not.toHaveBeenCalled();
    expect(await screen.findByRole("button", { name: /Queued/ })).toBeTruthy();
  });

  it("ignores a second press while the first download is still going out", async () => {
    let finish = () => {};
    enqueueDownload.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    videos = [video({ download_state: "none", file_path: null })];
    renderView();
    const card = await screen.findByRole("article");

    fireEvent.click(card);
    // Shown as queued at once, but held: pressing the badge now would cancel
    // a download the backend has not even accepted yet.
    const badge = await screen.findByRole<HTMLButtonElement>("button", { name: /Queued/ });
    expect(badge.disabled).toBe(true);
    expect(card.getAttribute("aria-busy")).toBe("true");
    fireEvent.click(card);
    fireEvent.click(badge);
    expect(enqueueDownload).toHaveBeenCalledTimes(1);
    expect(cancelDownload).not.toHaveBeenCalled();

    await act(async () => { finish(); });
    await waitFor(() => expect(badge.disabled).toBe(false));
  });

  it("drops the ring at once on cancel", async () => {
    let finish = () => {};
    cancelDownload.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    videos = [video({ download_state: "downloading", file_path: null })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: /Cancel download/ }));
    const download = await screen.findByRole<HTMLButtonElement>("button", { name: "Download" });
    // Back to Download before the backend has answered -- and not pressable
    // until it has, or a quick second click would queue what was just stopped.
    expect(download.disabled).toBe(true);
    expect(cancelDownload).toHaveBeenCalledTimes(1);

    await act(async () => { finish(); });
    await waitFor(() => expect(download.disabled).toBe(false));
  });

  it("puts the download back when the cancel fails", async () => {
    cancelDownload.mockImplementation(() => Promise.reject("no such job"));
    videos = [video({ download_state: "downloading", file_path: null })];
    renderView();

    fireEvent.click(await screen.findByRole("button", { name: /Cancel download/ }));
    expect(await screen.findByText("no such job")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Cancel download/ })).toBeTruthy();
    expect(document.querySelector(".card")?.getAttribute("data-state")).toBe("downloading");
  });

  it("hides a card at once, and brings it back if the hide fails", async () => {
    setVideoHidden.mockImplementation(() => Promise.reject("database is locked"));
    videos = [video({ download_state: "none", file_path: null })];
    renderView();

    fireEvent.click(within(await openMenu(), "Hide from feed"));
    expect(await screen.findByText("database is locked")).toBeTruthy();
    expect(screen.getByRole("article")).toBeTruthy();
    expect(setVideoHidden).toHaveBeenCalledTimes(1);
  });

  it("keeps the grid up, dimmed, while a sibling mark regroups it", async () => {
    threeCards();
    renderView();
    const [a, b] = await cardsReady(3);
    // The refetch that follows the mark hangs, as a slow one would.
    listVideos.mockImplementation(() => new Promise<Video[]>(() => {}));

    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(b, { ctrlKey: true });
    fireEvent.contextMenu(a);
    fireEvent.click(within(screen.getByRole("menu"), "Mark 2 videos as siblings"));

    await waitFor(() =>
      expect(document.querySelector(".video-grid")?.className).toContain("is-refreshing"),
    );
    expect(cards()).toHaveLength(3);
  });

  it("sends a second, different mark while the first is still out", async () => {
    // The selection is cleared the moment a mark goes out, so a second mark
    // swallowed behind the first would be lost without a trace.
    videos = ["a", "b", "c"].map((id) =>
      video({ id, title: `Video ${id}`, download_state: "none", file_path: null }));
    renderView();
    const [a, b, c] = await cardsReady(3);
    markSiblings.mockImplementation(() => new Promise<number>(() => {}));

    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(b, { ctrlKey: true });
    fireEvent.contextMenu(a);
    fireEvent.click(within(screen.getByRole("menu"), "Mark 2 videos as siblings"));

    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(c, { ctrlKey: true });
    fireEvent.contextMenu(a);
    fireEvent.click(within(screen.getByRole("menu"), "Mark 2 videos as siblings"));

    expect(markSiblings).toHaveBeenCalledTimes(2);
    expect(markSiblings).toHaveBeenLastCalledWith(["a", "c"]);
  });
});

/**
 * A series shows every part whatever the filters say -- the backend's
 * `sibling_of` replaces them all -- so acting on a part must not take it off
 * the screen just because the feed behind the series would not show it.
 */
describe("acting on a part inside a series", () => {
  const part = () => video({ id: "p2", title: "Part 2", download_state: "none", file_path: null });

  it("keeps a part marked watched while Unwatched is on", async () => {
    videos = [part()];
    renderView({ hideWatched: true, siblingOf: video({ id: "p1" }) });
    fireEvent.click(within(await openMenu(), "Mark as watched"));

    await waitFor(() => expect(setWatched).toHaveBeenCalledWith("p2", true));
    expect(screen.getByRole("article")).toBeTruthy();
    expect(screen.getByRole("article").className).toContain("is-watched");
  });

  it("keeps a part hidden while Include hidden is off, shown as hidden", async () => {
    videos = [part()];
    renderView({ showHidden: false, siblingOf: video({ id: "p1" }) });
    fireEvent.click(within(await openMenu(), "Hide from feed"));

    await waitFor(() => expect(setVideoHidden).toHaveBeenCalledWith("p2", true));
    expect(screen.getByRole("article")).toBeTruthy();
  });

  it("still drops a watched card from the ordinary feed while Unwatched is on", async () => {
    videos = [part()];
    renderView({ hideWatched: true });
    fireEvent.click(within(await openMenu(), "Mark as watched"));

    await waitFor(() => expect(setWatched).toHaveBeenCalledWith("p2", true));
    await waitFor(() => expect(screen.queryByRole("article")).toBeNull());
  });
});

describe("marking a downloaded video as watched", () => {
  it("marks it watched and then asks about the file", async () => {
    renderView();
    fireEvent.click(within(await openMenu(), "Mark as watched"));

    await waitFor(() => expect(setWatched).toHaveBeenCalledWith("vid1", true));
    expect(await screen.findByText("Delete the downloaded file?")).toBeTruthy();
    // It was watched a moment ago, and the copy has to know that.
    expect(screen.getByText(/is marked as watched/)).toBeTruthy();
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

describe("Download (custom)…", () => {
  it("sits directly below Download, wherever Download is offered", async () => {
    for (const download_state of ["none", "failed"] as const) {
      videos = [video({ download_state, file_path: null })];
      renderView();
      const labels = menuLabels(await openMenu());
      expect(labels[labels.indexOf("Download") + 1]).toBe("Download (custom)…");
      cleanup();
    }
  });

  it("is not offered for a video already downloaded or on its way", async () => {
    for (const download_state of ["done", "queued", "downloading"] as const) {
      videos = [video({ download_state })];
      renderView();
      expect(menuLabels(await openMenu())).not.toContain("Download (custom)…");
      cleanup();
    }
  });

  it("opens the dialog, and queues the exact pick with the card marked queued", async () => {
    videos = [video({ download_state: "none", file_path: null })];
    renderView();
    fireEvent.click(within(await openMenu(), "Download (custom)…"));

    const dialog = await screen.findByRole("dialog", { name: "Custom download" });
    expect(probeFormats).toHaveBeenCalledWith("vid1");
    await screen.findByLabelText("Resolution");
    // The card has a Download button of its own, underneath the dialog.
    fireEvent.click(getByRole(dialog, "button", { name: "Download" }));

    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(enqueueDownload).toHaveBeenCalledWith(
      "vid1", { ...DEFAULT_QUALITY, format: "401+251" },
    );
    expect(document.querySelector(".card")?.getAttribute("data-state")).toBe("queued");
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

  it("asks for only groups when that filter is on", async () => {
    renderView({ grouped: true, groupsOnly: true });
    await waitFor(() => expect(listVideoGroups).toHaveBeenCalled());
    expect(listVideoGroups.mock.calls[0][0]).toMatchObject({ groupsOnly: true });
  });

  it("drops only-groups with the grouping it filters", async () => {
    renderView({ grouped: false, groupsOnly: true });
    await waitFor(() => expect(listVideos).toHaveBeenCalled());
    expect(listVideos.mock.calls[0][0]).toMatchObject({ groupsOnly: false });
  });

  it("asks for only started series when continue-watching is on", async () => {
    renderView({ grouped: true, inProgress: true });
    await waitFor(() => expect(listVideoGroups).toHaveBeenCalled());
    expect(listVideoGroups.mock.calls[0][0]).toMatchObject({ inProgress: true });
  });

  it("drops continue-watching with the grouping it filters", async () => {
    renderView({ grouped: false, inProgress: true });
    await waitFor(() => expect(listVideos).toHaveBeenCalled());
    expect(listVideos.mock.calls[0][0]).toMatchObject({ inProgress: false });
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

/* ---------------- marking siblings by hand ---------------- */

/** Two cards on one channel, plus one on another. */
function threeCards() {
  videos = [
    video({ id: "a", title: "The Blackwood Tapes Pt 3", download_state: "none",
            file_path: null }),
    video({ id: "b", title: "EVERYTHING CHANGED", download_state: "none", file_path: null }),
    video({ id: "far", channel_id: "UC2", channel_title: "Elsewhere", title: "Other",
            download_state: "none", file_path: null }),
  ];
}

const cards = () => Array.from(document.querySelectorAll<HTMLElement>(".card"));

/** The grid after its first fetch has landed. */
async function cardsReady(n: number) {
  await waitFor(() => expect(cards().length).toBe(n));
  return cards();
}

/** A DataTransfer stand-in: jsdom's drag events carry none of their own. */
function dt() {
  return { effectAllowed: "", dropEffect: "", setData: vi.fn(), getData: () => "" };
}

describe("marking siblings by hand", () => {
  beforeEach(threeCards);

  it("ctrl+click picks a card out without running its action", async () => {
    renderView();
    const [a] = await cardsReady(3);
    fireEvent.click(a, { ctrlKey: true });

    expect(a.className).toContain("is-selected");
    // The card's primary action is a download; selecting must not start one.
    expect(markSiblings).not.toHaveBeenCalled();
  });

  it("offers the marking item only once two are picked", async () => {
    renderView();
    const [a, b] = await cardsReady(3);

    fireEvent.click(a, { ctrlKey: true });
    fireEvent.contextMenu(a);
    expect(menuLabels(screen.getByRole("menu"))).not.toContain("Mark 1 videos as siblings");
    fireEvent.keyDown(window, { key: "Escape" });

    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(b, { ctrlKey: true });
    fireEvent.contextMenu(a);
    fireEvent.click(within(screen.getByRole("menu"), "Mark 2 videos as siblings"));

    await waitFor(() => expect(markSiblings).toHaveBeenCalledWith(["a", "b"]));
  });

  it("clears the selection on Escape", async () => {
    renderView();
    const [a] = await cardsReady(3);
    fireEvent.click(a, { ctrlKey: true });
    expect(a.className).toContain("is-selected");

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(cards()[0].className).not.toContain("is-selected"));
  });

  it("a plain click drops the selection and lets the card act as usual", async () => {
    renderView();
    const [a, b] = await cardsReady(3);
    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(b);

    await waitFor(() => expect(cards()[0].className).not.toContain("is-selected"));
    expect(markSiblings).not.toHaveBeenCalled();
  });

  it("dropping one card on another marks the two", async () => {
    renderView();
    const [a, b] = await cardsReady(3);
    const data = dt();
    fireEvent.dragStart(a, { dataTransfer: data });
    fireEvent.dragOver(b, { dataTransfer: data });
    expect(cards()[1].className).toContain("is-drop-target");

    fireEvent.drop(b, { dataTransfer: data });
    // A drop can be a slip of the mouse, so it asks before marking anything.
    const dialog = await screen.findByRole("dialog", { name: "Mark these two as siblings?" });
    expect(markSiblings).not.toHaveBeenCalled();

    fireEvent.click(getByRole(dialog, "button", { name: "Mark as siblings" }));
    await waitFor(() => expect(markSiblings).toHaveBeenCalledWith(["a", "b"]));
  });

  it("cancelling the drop's confirmation marks nothing", async () => {
    renderView();
    const [a, b] = await cardsReady(3);
    const data = dt();
    fireEvent.dragStart(a, { dataTransfer: data });
    fireEvent.dragOver(b, { dataTransfer: data });
    fireEvent.drop(b, { dataTransfer: data });

    const dialog = await screen.findByRole("dialog", { name: "Mark these two as siblings?" });
    fireEvent.click(getByRole(dialog, "button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(markSiblings).not.toHaveBeenCalled();
  });

  it("offers the drag a move, since Wayland has no link action", async () => {
    renderView();
    const [a] = await cardsReady(3);
    const data = dt();
    fireEvent.dragStart(a, { dataTransfer: data });
    expect(data.effectAllowed).toBe("move");
  });

  it("refuses a drop from another channel, and never lights the card up", async () => {
    renderView();
    const [a, , far] = await cardsReady(3);
    const data = dt();
    fireEvent.dragStart(a, { dataTransfer: data });
    fireEvent.dragOver(far, { dataTransfer: data });
    expect(cards()[2].className).not.toContain("is-drop-target");

    fireEvent.drop(far, { dataTransfer: data });
    expect(await screen.findByText("Siblings must come from the same channel.")).toBeTruthy();
    expect(markSiblings).not.toHaveBeenCalled();
  });

  it("dragging a picked card brings the whole selection", async () => {
    renderView();
    const [a, b, far] = await cardsReady(3);
    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(far, { ctrlKey: true });

    const data = dt();
    fireEvent.dragStart(a, { dataTransfer: data });
    fireEvent.drop(b, { dataTransfer: data });
    // Two channels in one link: refused rather than half-applied.
    expect(await screen.findByText("Siblings must come from the same channel.")).toBeTruthy();
    expect(markSiblings).not.toHaveBeenCalled();
  });

  it("offers Unlink only for a video carrying a mark", async () => {
    videos = [video({ id: "a", sibling_group: null })];
    renderView();
    expect(menuLabels(await openMenu())).not.toContain("Unlink from siblings");
    fireEvent.keyDown(window, { key: "Escape" });
    cleanup();

    videos = [video({ id: "a", sibling_group: "g1" })];
    renderView();
    fireEvent.click(within(await openMenu(), "Unlink from siblings"));
    await waitFor(() => expect(unlinkSiblings).toHaveBeenCalledWith("a"));
  });

  it("wears a Linked pill so a hand-built group is visible", async () => {
    videos = [video({ id: "a", sibling_group: "g1" })];
    renderView();
    expect(await screen.findByText("Linked")).toBeTruthy();
  });
});

/**
 * A series card stands for every one of its parts, so its menu acts on all of
 * them or on none: never on the leader alone, which is what a lone video's
 * items would do from here.
 */
describe("a series card's right-click menu", () => {
  /** A series, newest part leading as the backend hands it over, one part per
   *  argument. Nothing downloaded unless a part says so. */
  function series(...parts: Partial<Video>[]): VideoGroup {
    return {
      videos: parts.map((over, i) => video({
        id: `p${parts.length - i}`,
        title: `The Blackwood Tapes - Part ${parts.length - i}`,
        download_state: "none",
        file_path: null,
        ...over,
      })),
      stem: "The Blackwood Tapes",
    };
  }
  const fresh = () => series({}, {}, {});
  const solo = (): VideoGroup => ({ videos: [video({ id: "solo" })], stem: null });
  const seriesCard = () => document.querySelector<HTMLElement>(".series-card");

  /** Right-click the series card once it is up, and read back the menu. A
   *  series card is one button rather than an article, so `openMenu` misses it. */
  async function openSeriesMenu() {
    await waitFor(() => expect(seriesCard()).not.toBeNull());
    fireEvent.contextMenu(seriesCard()!);
    return screen.getByRole("menu");
  }

  // jsdom has no Web Animations, and a card put back after a failed hide is an
  // insertion, which the grid animates in.
  beforeEach(() => {
    Element.prototype.animate = vi.fn() as unknown as Element["animate"];
  });
  afterEach(() => {
    delete (Element.prototype as Partial<Element>).animate;
  });

  it("offers to download the parts not yet down, and to hide the series", async () => {
    groups = [series(
      { download_state: "failed", download_error: "HTTP Error 403" },
      { download_state: "done", file_path: "/videos/p2.mkv" },
      {},
    )];
    renderView({ grouped: true });
    // A failed part is retried; the downloaded one is left alone.
    expect(menuLabels(await openSeriesMenu())).toEqual(["Download 2 parts", "Hide from feed"]);
  });

  it("counts a single part left to download as one part", async () => {
    groups = [series({ download_state: "done", file_path: "/videos/p2.mkv" }, {})];
    renderView({ grouped: true });
    expect(menuLabels(await openSeriesMenu())).toContain("Download 1 part");
  });

  it("offers no download once every part is downloaded or on its way", async () => {
    groups = [series(
      { download_state: "done", file_path: "/videos/p3.mkv" },
      { download_state: "downloading" },
      { download_state: "queued" },
    )];
    renderView({ grouped: true });
    expect(menuLabels(await openSeriesMenu())).toEqual(["Hide from feed"]);
  });

  it("queues every part and shows the ones it could take as queued", async () => {
    groups = [series({}, { download_state: "done", file_path: "/videos/p2.mkv" }, {})];
    enqueueDownloads.mockImplementation(() => Promise.resolve(2));
    renderView({ grouped: true });
    fireEvent.click(within(await openSeriesMenu(), "Download 2 parts"));

    // Every part goes, in the card's order: the backend picks out what it can
    // queue and puts it oldest first, so neither is the menu's job.
    await waitFor(() => expect(enqueueDownloads).toHaveBeenCalledWith(["p3", "p2", "p1"]));
    expect(await screen.findByText("Queued 2 parts of “The Blackwood Tapes”.")).toBeTruthy();
    // Shown queued before any state event arrives, so there is nothing left
    // for a second Download to take.
    fireEvent.contextMenu(seriesCard()!);
    expect(menuLabels(screen.getByRole("menu"))).toEqual(["Hide from feed"]);
  });

  it("puts the parts back when the download cannot be queued", async () => {
    groups = [fresh()];
    enqueueDownloads.mockImplementation(() => Promise.reject("database is locked"));
    renderView({ grouped: true });
    fireEvent.click(within(await openSeriesMenu(), "Download 3 parts"));

    expect(await screen.findByText("database is locked")).toBeTruthy();
    fireEvent.contextMenu(seriesCard()!);
    expect(menuLabels(screen.getByRole("menu"))).toContain("Download 3 parts");
  });

  it("reads the card again when the backend queued fewer parts than it showed", async () => {
    groups = [fresh()];
    // One part was already on its way as far as the database knew.
    enqueueDownloads.mockImplementation(() => Promise.resolve(2));
    renderView({ grouped: true });
    await cardsReady(1);
    const fetched = listVideoGroups.mock.calls.length;
    fireEvent.click(within(await openSeriesMenu(), "Download 3 parts"));
    await waitFor(() => expect(listVideoGroups.mock.calls.length).toBe(fetched + 1));
  });

  it("does not read the card again when every part it showed was queued", async () => {
    groups = [fresh()];
    renderView({ grouped: true });
    await cardsReady(1);
    const fetched = listVideoGroups.mock.calls.length;
    fireEvent.click(within(await openSeriesMenu(), "Download 3 parts"));
    expect(await screen.findByText("Queued 3 parts of “The Blackwood Tapes”.")).toBeTruthy();
    expect(listVideoGroups.mock.calls.length).toBe(fetched);
  });

  it("reads the feed again when the hide reached a part on another card", async () => {
    groups = [fresh(), solo()];
    // The sweep after the hide caught a drifted part leading a card of its own.
    setSeriesHidden.mockImplementation(() => Promise.resolve(["p3", "p2", "p1", "drifted"]));
    renderView({ grouped: true });
    await cardsReady(2);
    const fetched = listVideoGroups.mock.calls.length;
    fireEvent.click(within(await openSeriesMenu(), "Hide from feed"));
    await waitFor(() => expect(listVideoGroups.mock.calls.length).toBe(fetched + 1));
  });

  it("hides a series with no files at once, taking the whole card", async () => {
    groups = [fresh(), solo()];
    renderView({ grouped: true });
    await cardsReady(2);
    fireEvent.contextMenu(seriesCard()!);
    fireEvent.click(within(screen.getByRole("menu"), "Hide from feed"));

    // Hidden by its leader: the backend works the rest of the series out.
    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p3", true));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(deleteDownload).not.toHaveBeenCalled();
    await waitFor(() => expect(seriesCard()).toBeNull());
    expect(cards()).toHaveLength(1);
    expect(await screen.findByText(/^Hidden “The Blackwood Tapes”\./)).toBeTruthy();
  });

  it("brings the card back, where it was, when the hide fails", async () => {
    groups = [solo(), fresh()];
    setSeriesHidden.mockImplementation(() => Promise.reject("database is locked"));
    renderView({ grouped: true });
    await cardsReady(2);
    fireEvent.contextMenu(seriesCard()!);
    fireEvent.click(within(screen.getByRole("menu"), "Hide from feed"));

    expect(await screen.findByText("database is locked")).toBeTruthy();
    expect(cards().map((c) => c.dataset.videoId)).toEqual(["solo", "p3"]);
  });

  it("asks first when a part has a file, and keeps it unless told otherwise", async () => {
    groups = [series({}, { download_state: "done", file_path: "/videos/p2.mkv" }, {})];
    renderView({ grouped: true });
    fireEvent.click(within(await openSeriesMenu(), "Hide from feed"));

    const dialog = await screen.findByRole("dialog", { name: "Hide this series?" });
    // Asking is not doing.
    expect(setSeriesHidden).not.toHaveBeenCalled();
    // It says what is being hidden -- the series by name, and what is to come.
    expect(dialog.textContent)
      .toContain("All 3 parts of “The Blackwood Tapes”, and any part that joins it later");
    expect(dialog.textContent).toContain("The downloaded file is kept");
    // One file, so the choices say so, as a lone video's dialog does.
    fireEvent.click(getByRole(dialog, "button", { name: "Hide, keep file" }));

    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p3", true));
    expect(deleteDownload).not.toHaveBeenCalled();
    await waitFor(() => expect(seriesCard()).toBeNull());
  });

  it("removes every downloaded part's file first when asked to", async () => {
    groups = [series(
      { download_state: "done", file_path: "/videos/p3.mkv" },
      {},
      { download_state: "done", file_path: "/videos/p1.mkv" },
    )];
    renderView({ grouped: true });
    fireEvent.click(within(await openSeriesMenu(), "Hide from feed"));
    const dialog = await screen.findByRole("dialog", { name: "Hide this series?" });
    fireEvent.click(getByRole(dialog, "button", { name: "Hide and remove files" }));

    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p3", true));
    expect(deleteDownload.mock.calls).toEqual([["p3"], ["p1"]]);
    // The files go before the hide, as a lone video's does.
    expect(Math.max(...deleteDownload.mock.invocationCallOrder))
      .toBeLessThan(setSeriesHidden.mock.invocationCallOrder[0]);
  });

  it("keeps a hidden series on screen, dimmed, while Include hidden is on", async () => {
    groups = [fresh()];
    renderView({ grouped: true, showHidden: true });
    fireEvent.click(within(await openSeriesMenu(), "Hide from feed"));

    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p3", true));
    expect(seriesCard()!.className).toContain("is-hidden-video");
    fireEvent.contextMenu(seriesCard()!);
    expect(menuLabels(screen.getByRole("menu"))).toContain("Un-hide series");
  });

  it("offers to un-hide a series carrying the mark, and clears it", async () => {
    groups = [series(
      { hidden: true, series_hidden: true },
      { hidden: true, series_hidden: true },
      // Un-hidden by hand inside the series: the part keeps its mark.
      { hidden: false, series_hidden: true },
    )];
    renderView({ grouped: true, showHidden: true });
    const labels = menuLabels(await openSeriesMenu());
    expect(labels).toContain("Un-hide series");
    expect(labels).not.toContain("Hide from feed");
    fireEvent.click(within(screen.getByRole("menu"), "Un-hide series"));

    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p3", false));
    fireEvent.contextMenu(seriesCard()!);
    expect(menuLabels(screen.getByRole("menu"))).toContain("Hide from feed");
  });

  it("puts the mark back when the un-hide fails", async () => {
    groups = [series(
      { hidden: true, series_hidden: true },
      { hidden: true, series_hidden: true },
    )];
    setSeriesHidden.mockImplementation(() => Promise.reject("database is locked"));
    renderView({ grouped: true, showHidden: true });
    fireEvent.click(within(await openSeriesMenu(), "Un-hide series"));

    expect(await screen.findByText("database is locked")).toBeTruthy();
    expect(seriesCard()!.className).toContain("is-hidden-video");
    fireEvent.contextMenu(seriesCard()!);
    expect(menuLabels(screen.getByRole("menu"))).toContain("Un-hide series");
  });

  it("still leads with the marking item when cards are picked", async () => {
    groups = [fresh(), solo()];
    renderView({ grouped: true });
    const [card, lone] = await cardsReady(2);

    fireEvent.click(card, { ctrlKey: true });
    fireEvent.click(lone, { ctrlKey: true });
    fireEvent.contextMenu(card);
    expect(menuLabels(screen.getByRole("menu")))
      .toEqual(["Mark 2 videos as siblings", "Download 3 parts", "Hide from feed"]);
  });
});

/* ---------------- keeping your place in the feed ---------------- */

/** A library big enough to page, so "the pages it had" means more than one. */
function shelf(n: number): Video[] {
  return Array.from({ length: n }, (_, i) =>
    video({ id: `v${i}`, title: `Video ${i}`, download_state: "none", file_path: null }));
}

/** The filter the most recent fetch went out with. */
function lastFilter(): VideoFilter {
  const calls = listVideos.mock.calls;
  return calls[calls.length - 1][0];
}

describe("keeping your place in the feed", () => {
  beforeEach(() => {
    const all = shelf(250);
    listVideos.mockImplementation((f) =>
      Promise.resolve(all.slice(f.offset, f.offset + f.limit)));
  });

  /** Two pages in, scrolled down. Returns the view and its scroll container. */
  async function twoPagesIn(at: number) {
    const view = renderView();
    await cardsReady(100);
    pageIn();
    await cardsReady(200);
    view.scroller.scrollTop = at;
    return view;
  }

  it("comes back to where you were when a series is left", async () => {
    const { scroller, rerender } = await twoPagesIn(4200);

    rerender({ siblingOf: video({ id: "v7" }) });
    await waitFor(() => expect(lastFilter().siblingOf).toBe("v7"));
    // What the browser does once the grid collapses to a handful of parts.
    // jsdom lays nothing out, so the clamp is applied by hand.
    scroller.scrollTop = 0;

    rerender({ siblingOf: null });
    // Both pages back in one request: an offset past the end of a single page
    // would land at the bottom of it instead of where you were.
    await waitFor(() => expect(lastFilter().limit).toBe(200));
    await waitFor(() => expect(scroller.scrollTop).toBe(4200));
  });

  it("stays put when a poll refetches the same feed", async () => {
    const { scroller, rerender } = await twoPagesIn(3100);

    rerender({ reloadToken: 1 });
    scroller.scrollTop = 0;

    await waitFor(() => expect(lastFilter().limit).toBe(200));
    await waitFor(() => expect(scroller.scrollTop).toBe(3100));
  });

  it("starts a list it has not shown before at the top", async () => {
    const { scroller, rerender } = await twoPagesIn(3100);

    rerender({ channelIds: ["UC1"] });
    await waitFor(() => expect(lastFilter().channelIds).toEqual(["UC1"]));

    // A different feed, so the pages the old one had mean nothing here.
    expect(lastFilter().limit).toBe(100);
    await waitFor(() => expect(scroller.scrollTop).toBe(0));
  });
});
