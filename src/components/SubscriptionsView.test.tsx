import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act, getByRole } from "@testing-library/react";
import type { FeedStats, StatsFilter, Video, VideoFilter, VideoGroup } from "../types";

// Every listener the view registers, by event name, so a test can deliver a
// download's state change the way the backend does.
const listeners = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, cb: (e: { payload: unknown }) => void) => {
    listeners.set(name, cb);
    return Promise.resolve(() => {});
  },
}));

// The stats' own components are drawn by their own tests; here they only say
// what they were handed.
const waveformProps = vi.fn<(p: { stats: FeedStats }) => void>();
const bandsProps = vi.fn<(p: { parts: Video[] }) => void>();
vi.mock("./stats/FeedWaveform", () => ({
  default: (p: { stats: FeedStats }) => {
    waveformProps(p);
    return <div data-testid="waveform">{p.stats.videos} videos</div>;
  },
}));
vi.mock("./stats/SeriesBands", () => ({
  default: (p: { parts: Video[] }) => {
    bandsProps(p);
    return <div data-testid="bands">{p.parts.length} parts</div>;
  },
}));

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
const setSeriesAutoDownload =
  vi.fn((_leader: string, _enabled: boolean) => Promise.resolve<string[]>([]));
// Typed with the filter so a test can page through a library with it.
const listVideos = vi.fn<(f: VideoFilter) => Promise<Video[]>>(() => Promise.resolve(videos));
const listVideoGroups = vi.fn<(f: VideoFilter) => Promise<VideoGroup[]>>(() => Promise.resolve(groups));
const feedStats = vi.fn<(f: StatsFilter) => Promise<FeedStats>>(() => Promise.resolve(STATS));

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
    setSeriesAutoDownload: (leader: string, enabled: boolean) =>
      setSeriesAutoDownload(leader, enabled),
    feedStats: (filter: StatsFilter) => feedStats(filter),
    getSettings: () => Promise.resolve({ quality: DEFAULT_QUALITY }),
  },
  thumbSrc: () => "",
  errText: (e: unknown) => String(e),
}));

import SubscriptionsView from "./SubscriptionsView";
import { BUMP_DELAY } from "./stats/useFeedStats";
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
    part_links_refused: false, part_linked: false, series_auto_download: false,
    ...over,
  };
}

let videos: Video[] = [];
let groups: VideoGroup[] = [];

/** The feed's stats as the backend would answer, `videos` set to tell answers apart. */
function stats(videos = 42): FeedStats {
  return {
    videos, series: 3, toWatch: videos - 12, watched: 10, downloaded: 5, hidden: 2,
    runtimeSecs: 36_000, runtimeUnknown: false, newThisWeek: 1,
    today: "2026-10-19", days: [{ day: "2026-10-12", watched: 10, downloaded: 4, unwatched: 26, hidden: 2 }],
  };
}
const STATS = stats();

type ViewProps = Partial<React.ComponentProps<typeof SubscriptionsView>>;

/** No channel can be picked: the menu then offers no way to one, which keeps
 *  every other test's menu exactly what it was before that item existed. */
const NO_CHANNELS: ReadonlySet<string> = new Set();

/** No channel picked, held once as the shell holds its pick: a fresh array per
 *  render would be a new pick each time, and ask for the stats again. */
const EVERY_CHANNEL: string[] = [];

function renderView(over: ViewProps = {}) {
  const onFindSiblings = vi.fn();
  const onSeriesTally = vi.fn();
  const onSeeChannel = vi.fn();
  // Stands in for the shell's `.content`: a real element, so a test can move
  // its offset and read back where the view put it.
  const scroller = document.createElement("div");
  document.body.appendChild(scroller);
  const scrollRef = { current: scroller };
  const view = (props: ViewProps) => (
    <ToastProvider>
      <SubscriptionsView
        channelIds={EVERY_CHANNEL} search="" hideWatched={false} downloadedOnly={false}
        sort="newest" showHidden={false} grouped={false} groupsOnly={false} inProgress={false}
        autoDownloading={false} siblingOf={null}
        onFindSiblings={onFindSiblings} onSeriesTally={onSeriesTally}
        cardSize={260} onCardSize={vi.fn()} reloadToken={0}
        channelCount={1} onAdd={vi.fn()} scrollRef={scrollRef}
        pickableChannels={NO_CHANNELS} onSeeChannel={onSeeChannel} showStats={false}
        {...props}
      />
    </ToastProvider>
  );
  const { rerender } = render(view(over));
  return {
    onFindSiblings, onSeriesTally, onSeeChannel, scroller,
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
  listeners.clear();
  vi.clearAllMocks();
  feedStats.mockImplementation(() => Promise.resolve(STATS));
  listVideos.mockImplementation(() => Promise.resolve(videos));
  listVideoGroups.mockImplementation(() => Promise.resolve(groups));
  probeFormats.mockImplementation(() => Promise.resolve(rickroll()));
  cancelDownload.mockImplementation(() => Promise.resolve());
  setVideoHidden.mockImplementation(() => Promise.resolve());
  setSeriesHidden.mockImplementation(() => Promise.resolve([]));
  setSeriesAutoDownload.mockImplementation(() => Promise.resolve([]));
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

/**
 * Grouped, the grid has already gathered every match behind a series card, so
 * on a lone card "Find siblings" could only ever open a series of one -- and
 * inside a series opened from that feed, the series already on screen.
 */
describe("Find siblings while grouped", () => {
  it("is not offered on a lone card in the grouped feed", async () => {
    groups = [{ videos: [video()], stem: null }];
    renderView({ grouped: true });
    expect(menuLabels(await openMenu())).not.toContain("Find siblings");
  });

  it("is not offered on a part of a series opened from the grouped feed", async () => {
    renderView({ grouped: true, siblingOf: video() });
    expect(menuLabels(await openMenu())).not.toContain("Find siblings");
  });

  it("is offered ungrouped, in the feed and inside a series alike", async () => {
    for (const siblingOf of [null, video()]) {
      renderView({ grouped: false, siblingOf });
      expect(menuLabels(await openMenu())).toContain("Find siblings");
      cleanup();
    }
  });

  it("leaves Unlink from siblings offered", async () => {
    groups = [{ videos: [video({ sibling_group: "g1" })], stem: null }];
    renderView({ grouped: true });
    expect(menuLabels(await openMenu())).toContain("Unlink from siblings");
  });
});

/**
 * "See all videos from <channel>" narrows the feed to the card's channel. The
 * shell does the narrowing; the view offers it only where the pick can hold
 * the channel and where taking it would change what is on screen.
 */
describe("See all videos from a card's channel", () => {
  const PICKABLE: ReadonlySet<string> = new Set(["UC1"]);
  const offered = (labels: string[]) => labels.some((l) => l.startsWith("See all videos from"));
  const seriesCard = () => document.querySelector<HTMLElement>(".series-card");
  async function openSeriesMenu() {
    await waitFor(() => expect(seriesCard()).not.toBeNull());
    fireEvent.contextMenu(seriesCard()!);
    return screen.getByRole("menu");
  }
  const twoParts = (over: Partial<Video> = {}): VideoGroup => ({
    videos: [
      video({ id: "p2", title: "Tapes 2", download_state: "none", file_path: null, ...over }),
      video({ id: "p1", title: "Tapes 1", download_state: "none", file_path: null, ...over }),
    ],
    stem: "Tapes",
  });

  it("names the channel and hands its id to the shell", async () => {
    const { onSeeChannel } = renderView({ pickableChannels: PICKABLE });
    fireEvent.click(within(await openMenu(), "See all videos from Veritasium"));
    expect(onSeeChannel).toHaveBeenCalledTimes(1);
    expect(onSeeChannel).toHaveBeenCalledWith("UC1");
  });

  it("says this channel when the channel has no name", async () => {
    videos = [video({ channel_title: "" })];
    renderView({ pickableChannels: PICKABLE });
    expect(menuLabels(await openMenu())).toContain("See all videos from this channel");
  });

  // Only subscriptions can be picked: the shell prunes any other id from the
  // pick the moment it lands, so the item would show every channel instead.
  it("is not offered on an uploader you do not subscribe to", async () => {
    videos = [video({ channel_id: "UCadhoc", added_manually: true })];
    renderView({ pickableChannels: PICKABLE });
    expect(offered(menuLabels(await openMenu()))).toBe(false);
  });

  it("is not offered when that channel alone is already the feed", async () => {
    renderView({ pickableChannels: PICKABLE, channelIds: ["UC1"] });
    expect(offered(menuLabels(await openMenu()))).toBe(false);
  });

  it("is offered while a search, a wider pick or an open series stands in the way", async () => {
    const cases: ViewProps[] = [
      { channelIds: [] },
      { channelIds: ["UC1", "UC2"] },
      { channelIds: ["UC1"], search: "tapes" },
      { channelIds: ["UC1"], siblingOf: video() },
    ];
    for (const over of cases) {
      renderView({ pickableChannels: PICKABLE, ...over });
      expect(menuLabels(await openMenu())).toContain("See all videos from Veritasium");
      cleanup();
    }
  });

  it("sits with the other ways to navigate, ahead of the hide", async () => {
    videos = [video({ sibling_group: "g1" })];
    renderView({ pickableChannels: PICKABLE });
    const labels = menuLabels(await openMenu());
    expect(labels.slice(labels.indexOf("See all videos from Veritasium"))).toEqual([
      "See all videos from Veritasium", "Find siblings", "Unlink from siblings", "Hide from feed",
    ]);
  });

  it("is offered on a series card, ahead of its hide, for the whole series' channel", async () => {
    groups = [twoParts()];
    const { onSeeChannel } = renderView({ grouped: true, pickableChannels: PICKABLE });
    const menu = await openSeriesMenu();
    expect(menuLabels(menu)).toEqual([
      "Download 2 parts", "Auto-download new parts", "See all videos from Veritasium",
      "Hide from feed",
    ]);
    fireEvent.click(within(menu, "See all videos from Veritasium"));
    expect(onSeeChannel).toHaveBeenCalledWith("UC1");
  });

  it("comes ahead of Un-hide series as well", async () => {
    groups = [twoParts({ hidden: true, series_hidden: true })];
    renderView({ grouped: true, showHidden: true, pickableChannels: PICKABLE });
    expect(menuLabels(await openSeriesMenu())).toEqual([
      "Download 2 parts", "Auto-download new parts", "See all videos from Veritasium",
      "Un-hide series",
    ]);
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
    expect(menuLabels(await openMenu())).toContain("Mark as watched");
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

/**
 * The crumb of an open series offers what the series card's menu offers, and
 * this view runs it exactly as it runs the menu: the same calls under the same
 * keys, the same marks shown before the answer -- and the series stays open.
 */
describe("what an open series offers the breadcrumb", () => {
  type Actions = Parameters<NonNullable<ViewProps["onSeriesActions"]>>[0];

  /** Three parts, newest first as the default sort hands them over. */
  function parts(...over: Partial<Video>[]): Video[] {
    return ["p3", "p2", "p1"].map((id, i) => video({
      id, title: `The Blackwood Tapes - Part ${3 - i}`, download_state: "none", file_path: null,
      ...over[i],
    }));
  }
  /** Opened on the middle part, so the anchor is not the first the list holds. */
  const anchor = () => video({ id: "p2", title: "The Blackwood Tapes - Part 2" });

  function openSeries(over: ViewProps = {}) {
    const onSeriesActions = vi.fn<(a: Actions) => void>();
    const view = renderView({
      siblingOf: anchor(), seriesName: "The Blackwood Tapes", onSeriesActions, ...over,
    });
    const last = (): Actions => {
      const calls = onSeriesActions.mock.calls;
      return calls.length > 0 ? calls[calls.length - 1][0] : null;
    };
    /** The actions, once the parts are in and there is something to offer. */
    const offered = async () => {
      await waitFor(() => expect(last()).not.toBeNull());
      return last()!;
    };
    return { ...view, onSeriesActions, last, offered };
  }

  /** Whether the view has offered anything at all. */
  const offeredAny = (spy: { mock: { calls: [Actions][] } }) => spy.mock.calls.some(([a]) => a !== null);

  // jsdom has no Web Animations, which the grid uses for cards coming and going.
  beforeEach(() => {
    Element.prototype.animate = vi.fn() as unknown as Element["animate"];
  });
  afterEach(() => {
    delete (Element.prototype as Partial<Element>).animate;
  });

  it("counts what Download would take as the card menu does, and says whether it is marked", async () => {
    videos = parts(
      { download_state: "failed", download_error: "HTTP Error 403" },
      { download_state: "done", file_path: "/videos/p2.mkv" },
      {},
    );
    const { offered } = openSeries();
    expect(await offered()).toMatchObject({
      due: 2, marked: false, downloading: "idle", hiding: "idle",
    });
  });

  it("reads the mark off any part, as the menu does", async () => {
    videos = parts({}, {}, { series_hidden: true });
    const { offered } = openSeries();
    expect((await offered()).marked).toBe(true);
  });

  it("offers to switch auto-download on only for a subscribed channel", async () => {
    videos = parts();
    const { offered } = openSeries();
    expect((await offered()).autoAvailable).toBe(false);
    cleanup();

    const subscribed = openSeries({ pickableChannels: new Set(["UC1"]) });
    expect((await subscribed.offered()).autoAvailable).toBe(true);
  });

  it("switches auto-download by the anchor and stays in the series", async () => {
    videos = parts({}, {}, { series_auto_download: true });
    const { offered, last } = openSeries();
    const a = await offered();
    expect(a).toMatchObject({ autoDownload: true, autoing: "idle" });
    act(() => a.onStopAutoDownload());

    expect(setSeriesAutoDownload).toHaveBeenCalledWith("p2", false);
    await waitFor(() => expect(last()!.autoDownload).toBe(false));
    expect(cards()).toHaveLength(3);
    act(() => last()!.onAutoDownload());
    expect(setSeriesAutoDownload).toHaveBeenLastCalledWith("p2", true);
    await waitFor(() => expect(last()!.autoDownload).toBe(true));
  });

  /** A lone card's own menu already covers a series of one. */
  it("offers nothing for a series of one", async () => {
    videos = [anchor()];
    const { onSeriesActions, onSeriesTally } = openSeries();
    await waitFor(() => expect(onSeriesTally).toHaveBeenCalled());
    expect(offeredAny(onSeriesActions)).toBe(false);
  });

  it("offers nothing in the ordinary feed", async () => {
    videos = parts();
    const { onSeriesActions } = openSeries({ siblingOf: null });
    await cardsReady(3);
    expect(offeredAny(onSeriesActions)).toBe(false);
  });

  /**
   * Until the series' first page lands, the grid is still the feed it was
   * opened from. A Download offered from that would queue the whole feed, and
   * a tally of it would count the wrong list.
   */
  it("says nothing about the feed behind it while the series loads", async () => {
    videos = shelf(5);
    const { onSeriesActions, onSeriesTally, rerender } = openSeries({ siblingOf: null });
    await cardsReady(5);

    let land = (_: Video[]) => {};
    listVideos.mockImplementation(() => new Promise<Video[]>((res) => { land = res; }));
    rerender({ siblingOf: anchor() });
    await waitFor(() => expect(lastFilter().siblingOf).toBe("p2"));
    expect(onSeriesTally).not.toHaveBeenCalled();
    expect(offeredAny(onSeriesActions)).toBe(false);

    await act(async () => { land(parts()); });
    await waitFor(() => expect(onSeriesTally).toHaveBeenLastCalledWith(
      expect.objectContaining({ parts: 3 }),
    ));
    await waitFor(() => expect(offeredAny(onSeriesActions)).toBe(true));
  });

  it("downloads through the menu's call, anchor first, and shows the parts queued", async () => {
    videos = parts({}, { download_state: "done", file_path: "/videos/p2.mkv" }, {});
    enqueueDownloads.mockImplementation(() => Promise.resolve(2));
    const { offered, last } = openSeries();
    const a = await offered();
    act(() => a.onDownload());

    // The anchor leads, as a card's leader would; the backend picks out what
    // it can queue and orders it oldest first.
    await waitFor(() => expect(enqueueDownloads).toHaveBeenCalledWith(["p2", "p3", "p1"]));
    expect(await screen.findByText("Queued 2 parts of “The Blackwood Tapes”.")).toBeTruthy();
    expect(cards().map((c) => c.dataset.state)).toEqual(["queued", "done", "queued"]);
    // Nothing left to take, so the crumb stops offering it.
    await waitFor(() => expect(last()!.due).toBe(0));
  });

  it("hides through the menu's call and stays in the series, its parts shown hidden", async () => {
    videos = parts();
    let answer = () => {};
    setSeriesHidden.mockImplementation(() => new Promise<string[]>((res) => {
      answer = () => res(["p2", "p3", "p1"]);
    }));
    const { offered, last } = openSeries();
    const a = await offered();
    act(() => a.onHide());

    // Hidden by the anchor, which leads the group the backend works out.
    expect(setSeriesHidden).toHaveBeenCalledWith("p2", true);
    // Shown at once, and held while the call is out.
    expect(last()).toMatchObject({ marked: true, hiding: "busy" });
    expect(cards()).toHaveLength(3);
    expect(cards().every((c) => c.className.includes("is-hidden-video"))).toBe(true);

    await act(async () => { answer(); });
    await waitFor(() => expect(last()!.hiding).toBe("idle"));
    expect(last()!.marked).toBe(true);
    expect(cards()).toHaveLength(3);
  });

  it("asks first when a part has a file, and removes every part's file when told to", async () => {
    videos = parts(
      { download_state: "done", file_path: "/videos/p3.mkv" },
      {},
      { download_state: "done", file_path: "/videos/p1.mkv" },
    );
    const { offered } = openSeries();
    const a = await offered();
    act(() => a.onHide());

    const dialog = await screen.findByRole("dialog", { name: "Hide this series?" });
    expect(setSeriesHidden).not.toHaveBeenCalled();
    // Named as the crumb names it, not by the anchor's own title.
    expect(dialog.textContent).toContain("All 3 parts of “The Blackwood Tapes”");
    fireEvent.click(getByRole(dialog, "button", { name: "Hide and remove files" }));

    // Every part's file -- not only the anchor's, which is the card the anchor
    // leads in a series view.
    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p2", true));
    expect(deleteDownload.mock.calls).toEqual([["p3"], ["p1"]]);
  });

  it("un-hides through the menu's call and clears the mark", async () => {
    videos = parts(
      { hidden: true, series_hidden: true },
      { hidden: true, series_hidden: true },
      { hidden: false, series_hidden: true },
    );
    const { offered, last } = openSeries();
    const a = await offered();
    act(() => a.onUnhide());

    await waitFor(() => expect(setSeriesHidden).toHaveBeenCalledWith("p2", false));
    await waitFor(() => expect(last()!.marked).toBe(false));
    expect(cards().some((c) => c.className.includes("is-hidden-video"))).toBe(false);
  });

  it("puts the mark back when the hide fails", async () => {
    videos = parts();
    setSeriesHidden.mockImplementation(() => Promise.reject("database is locked"));
    const { offered, last } = openSeries();
    const a = await offered();
    act(() => a.onHide());

    expect(await screen.findByText("database is locked")).toBeTruthy();
    await waitFor(() => expect(last()).toMatchObject({ marked: false, hiding: "idle" }));
    expect(cards().some((c) => c.className.includes("is-hidden-video"))).toBe(false);
  });

  it("refuses an Un-hide while its Hide is still out, as the menu does", async () => {
    videos = parts();
    setSeriesHidden.mockImplementation(() => new Promise<string[]>(() => {}));
    const { offered, last } = openSeries();
    const a = await offered();
    act(() => a.onHide());
    await waitFor(() => expect(last()!.marked).toBe(true));
    act(() => last()!.onUnhide());
    expect(setSeriesHidden).toHaveBeenCalledTimes(1);
  });

  /**
   * The crumb runs under the card menu's keys, so a download started from the
   * series card in the feed still holds the crumb's Download once the series
   * is opened, and the two can never both go out.
   */
  it("shares the card menu's keys across opening the series", async () => {
    groups = [{ videos: parts(), stem: "The Blackwood Tapes" }];
    enqueueDownloads.mockImplementation(() => new Promise<number>(() => {}));
    const { rerender, last } = openSeries({ siblingOf: null, grouped: true });
    await waitFor(() => expect(document.querySelector(".series-card")).not.toBeNull());
    fireEvent.contextMenu(document.querySelector(".series-card")!);
    fireEvent.click(within(screen.getByRole("menu"), "Download 3 parts"));
    expect(enqueueDownloads).toHaveBeenCalledTimes(1);

    // Opened on the card's leader; the database has not queued anything yet.
    videos = parts();
    rerender({ siblingOf: parts()[0] });
    await waitFor(() => expect(last()).not.toBeNull());
    expect(last()!.downloading).not.toBe("idle");
    act(() => last()!.onDownload());
    expect(enqueueDownloads).toHaveBeenCalledTimes(1);
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

  it("offers Unlink for a video its description linked", async () => {
    // No hand mark: a description put it on its card, and Unlink is how it
    // comes off.
    videos = [video({ id: "a", sibling_group: null, part_linked: true })];
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

/**
 * A series set to auto-download queues each part that joins it later, in the
 * poll that brings it in. The view only flips the switch and shows it: on the
 * card's menu, the card itself, the open series' crumb, and the filter.
 */
describe("auto-downloading a series", () => {
  function series(...parts: Partial<Video>[]): VideoGroup {
    return {
      videos: parts.map((over, i) => video({
        id: `p${parts.length - i}`, title: `The Blackwood Tapes - Part ${parts.length - i}`,
        download_state: "none", file_path: null, ...over,
      })),
      stem: "The Blackwood Tapes",
    };
  }
  const seriesCard = () => document.querySelector<HTMLElement>(".series-card");
  async function openSeriesMenu() {
    await waitFor(() => expect(seriesCard()).not.toBeNull());
    fireEvent.contextMenu(seriesCard()!);
    return screen.getByRole("menu");
  }
  const pill = () => seriesCard()?.querySelector(".pill-auto") ?? null;
  /** The series' channel is a subscription, so a new part can arrive. */
  const SUBSCRIBED: ReadonlySet<string> = new Set(["UC1"]);

  beforeEach(() => {
    Element.prototype.animate = vi.fn() as unknown as Element["animate"];
  });
  afterEach(() => {
    delete (Element.prototype as Partial<Element>).animate;
  });

  it("switches on from the card's menu by its leader, and the card wears it at once", async () => {
    groups = [series({}, {})];
    let answer = () => {};
    setSeriesAutoDownload.mockImplementation(() => new Promise<string[]>((res) => {
      answer = () => res(["p2", "p1"]);
    }));
    renderView({ grouped: true, pickableChannels: SUBSCRIBED });
    expect(pill()).toBeNull();
    fireEvent.click(within(await openSeriesMenu(), "Auto-download new parts"));

    expect(setSeriesAutoDownload).toHaveBeenCalledWith("p2", true);
    expect(pill()).not.toBeNull();
    await act(async () => { answer(); });
    expect(await screen.findByText("New parts of “The Blackwood Tapes” will download as they arrive."))
      .toBeTruthy();
    // Nothing already here is queued by it.
    expect(enqueueDownloads).not.toHaveBeenCalled();
    fireEvent.contextMenu(seriesCard()!);
    expect(menuLabels(screen.getByRole("menu"))).toContain("Stop auto-downloading new parts");
  });

  it("is not offered for a channel you do not follow, where no part can arrive", async () => {
    // A playlist added from the Add box: its uploader is never polled.
    groups = [series({}, {})];
    renderView({ grouped: true });
    expect(menuLabels(await openSeriesMenu())).not.toContain("Auto-download new parts");
  });

  it("offers Stop wherever the mark is, and the card comes back as it was if it fails", async () => {
    groups = [series({}, { series_auto_download: true })];
    setSeriesAutoDownload.mockImplementation(() => Promise.reject("database is locked"));
    renderView({ grouped: true });
    await waitFor(() => expect(pill()).not.toBeNull());
    fireEvent.click(within(await openSeriesMenu(), "Stop auto-downloading new parts"));

    expect(setSeriesAutoDownload).toHaveBeenCalledWith("p2", false);
    expect(await screen.findByText("database is locked")).toBeTruthy();
    expect(pill()).not.toBeNull();
  });

  it("takes the card out of the Auto-downloading list once it is stopped", async () => {
    groups = [series({ series_auto_download: true }, { series_auto_download: true })];
    renderView({ grouped: true, autoDownloading: true });
    await waitFor(() => expect(lastGroupsFilter().autoDownloading).toBe(true));
    fireEvent.click(within(await openSeriesMenu(), "Stop auto-downloading new parts"));
    await waitFor(() => expect(seriesCard()).toBeNull());
  });

  it("reads the feed again when Stop reached a part on another card", async () => {
    groups = [series({ series_auto_download: true }, { series_auto_download: true })];
    setSeriesAutoDownload.mockImplementation(() => Promise.resolve(["p2", "p1", "drifted"]));
    renderView({ grouped: true });
    await cardsReady(1);
    const fetched = listVideoGroups.mock.calls.length;
    fireEvent.click(within(await openSeriesMenu(), "Stop auto-downloading new parts"));
    await waitFor(() => expect(listVideoGroups.mock.calls.length).toBe(fetched + 1));
  });

  it("lets a lone card still carrying the mark stop it, and offers nothing on one without", async () => {
    videos = [video({ id: "a", series_auto_download: true }), video({ id: "b" })];
    renderView();
    const [a, b] = await cardsReady(2);
    fireEvent.contextMenu(b);
    expect(menuLabels(screen.getByRole("menu"))).not.toContain("Stop auto-downloading new parts");
    fireEvent.keyDown(window, { key: "Escape" });

    fireEvent.contextMenu(a);
    fireEvent.click(within(screen.getByRole("menu"), "Stop auto-downloading new parts"));
    await waitFor(() => expect(setSeriesAutoDownload).toHaveBeenCalledWith("a", false));
  });

  it("asks the backend for the auto-downloading cards only while grouped", async () => {
    groups = [series({}, {})];
    const { rerender } = renderView({ grouped: true, autoDownloading: true });
    await waitFor(() => expect(lastGroupsFilter().autoDownloading).toBe(true));
    rerender({ grouped: false, autoDownloading: true });
    await waitFor(() => expect(lastFilter().autoDownloading).toBe(false));
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

/** The same, for the grouped feed's fetch. */
function lastGroupsFilter(): VideoFilter {
  const calls = listVideoGroups.mock.calls;
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

/* ---------------- the stats over the grid ---------------- */

/**
 * Settings → Appearance → Show stats: the feed's waveform over the grid, an
 * open series' bands over its parts. Off, nothing is asked and nothing is
 * drawn; on, the feed's stats follow the pick, the search and every reload,
 * and a change made from a card asks again a beat later.
 */
describe("the stats over the grid", () => {
  // jsdom has no Web Animations, which the grid uses for cards coming and going.
  beforeEach(() => {
    Element.prototype.animate = vi.fn() as unknown as Element["animate"];
  });
  afterEach(() => {
    vi.useRealTimers();
    delete (Element.prototype as Partial<Element>).animate;
  });

  /** The shell's pick, held once: it keeps its identity there until it changes. */
  const PICK = ["UC1"];
  const lastStatsFilter = () => feedStats.mock.calls[feedStats.mock.calls.length - 1][0];
  /** What the bands were last handed. */
  const lastParts = () => bandsProps.mock.calls[bandsProps.mock.calls.length - 1][0].parts;
  const grid = () => document.querySelector(".video-grid");
  /** Lets every answer already given land. */
  const settle = () => act(async () => {});
  /** Time moves on, past a bump's wait. */
  const later = async (ms = BUMP_DELAY) => {
    act(() => { vi.advanceTimersByTime(ms); });
    await settle();
  };
  const fakeTimers = () => vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  const downloadState = (videoId: string, state: Video["download_state"]) =>
    act(() => listeners.get("download://state")!({
      payload: { videoId, state, filePath: state === "done" ? "/videos/x.mkv" : null, error: null },
    }));
  /** Right-click a card and pick an item from its menu. */
  const pick = (card: HTMLElement, label: string) => {
    fireEvent.contextMenu(card);
    fireEvent.click(within(screen.getByRole("menu"), label));
  };

  it("asks for nothing and draws nothing while it is off, whatever happens", async () => {
    videos = [video({ download_state: "none", file_path: null })];
    const { rerender } = renderView({ channelIds: PICK, search: "tapes" });
    const card = await screen.findByRole("article");
    // Nothing stands above the grid: the view is what it was without stats.
    expect(grid()!.previousElementSibling).toBeNull();

    pick(card, "Mark as watched");
    await waitFor(() => expect(setWatched).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(listeners.has("download://state")).toBe(true));
    downloadState("vid1", "done");
    rerender({ channelIds: PICK, search: "tapes", reloadToken: 1, siblingOf: video() });
    await waitFor(() => expect(lastFilter().siblingOf).toBe("vid1"));
    await act(() => new Promise<void>((r) => { setTimeout(r, BUMP_DELAY + 50); }));

    expect(feedStats).not.toHaveBeenCalled();
    expect(waveformProps).not.toHaveBeenCalled();
    expect(bandsProps).not.toHaveBeenCalled();
    expect(grid()!.previousElementSibling).toBeNull();
  });

  it("draws the waveform directly above the grid, asked with the pick and the trimmed search", async () => {
    renderView({ showStats: true, channelIds: PICK, search: "  tapes " });
    const wave = await screen.findByTestId("waveform");
    expect(wave.textContent).toBe("42 videos");
    expect(feedStats).toHaveBeenCalledTimes(1);
    expect(lastStatsFilter()).toEqual({ channelIds: ["UC1"], search: "tapes" });
    await screen.findByRole("article");
    expect(wave.nextElementSibling).toBe(grid());
  });

  it("asks again on a reload, a new pick and a new search", async () => {
    const { rerender } = renderView({ showStats: true });
    await screen.findByTestId("waveform");
    expect(lastStatsFilter()).toEqual({ channelIds: [], search: null });

    rerender({ reloadToken: 1 });
    await waitFor(() => expect(feedStats).toHaveBeenCalledTimes(2));

    rerender({ reloadToken: 1, channelIds: PICK });
    await waitFor(() => expect(feedStats).toHaveBeenCalledTimes(3));
    expect(lastStatsFilter()).toEqual({ channelIds: ["UC1"], search: null });

    rerender({ reloadToken: 1, channelIds: PICK, search: "tapes" });
    await waitFor(() => expect(feedStats).toHaveBeenCalledTimes(4));
    expect(lastStatsFilter()).toEqual({ channelIds: ["UC1"], search: "tapes" });
  });

  it("is not asked again by the Show toggles or the sort, which never move the counts", async () => {
    const { rerender } = renderView({ showStats: true });
    await screen.findByTestId("waveform");
    rerender({ hideWatched: true, downloadedOnly: true, showHidden: true, sort: "oldest" });
    await waitFor(() => expect(lastFilter()).toMatchObject({ hideWatched: true, sort: "oldest" }));
    await settle();
    expect(feedStats).toHaveBeenCalledTimes(1);
  });

  it("asks again a beat after a card is marked watched", async () => {
    videos = [video({ download_state: "none", file_path: null })];
    renderView({ showStats: true });
    await screen.findByTestId("waveform");
    const card = await screen.findByRole("article");

    fakeTimers();
    pick(card, "Mark as watched");
    await settle();
    expect(setWatched).toHaveBeenCalledTimes(1);
    await later(BUMP_DELAY - 1);
    expect(feedStats).toHaveBeenCalledTimes(1);
    await later(1);
    expect(feedStats).toHaveBeenCalledTimes(2);
    expect(lastStatsFilter()).toEqual({ channelIds: [], search: null });
  });

  it("answers a burst of three toggles with one request", async () => {
    videos = ["a", "b", "c"].map((id) =>
      video({ id, title: `Video ${id}`, download_state: "none", file_path: null }));
    renderView({ showStats: true });
    await screen.findByTestId("waveform");
    const [a, b, c] = await cardsReady(3);

    fakeTimers();
    for (const card of [a, b, c]) {
      pick(card, "Mark as watched");
      await settle();
    }
    expect(setWatched).toHaveBeenCalledTimes(3);
    await later(BUMP_DELAY * 3);
    expect(feedStats).toHaveBeenCalledTimes(2);
  });

  it("asks again after a hide, and after a sibling mark", async () => {
    threeCards();
    renderView({ showStats: true });
    await screen.findByTestId("waveform");
    const [a, b, far] = await cardsReady(3);

    fakeTimers();
    pick(far, "Hide from feed");
    await settle();
    expect(setVideoHidden).toHaveBeenCalledWith("far", true);
    await later();
    expect(feedStats).toHaveBeenCalledTimes(2);

    fireEvent.click(a, { ctrlKey: true });
    fireEvent.click(b, { ctrlKey: true });
    pick(a, "Mark 2 videos as siblings");
    await settle();
    expect(markSiblings).toHaveBeenCalledTimes(1);
    await later();
    expect(feedStats).toHaveBeenCalledTimes(3);
  });

  it("asks again when a file lands or goes, and not for a queue or a failure", async () => {
    renderView({ showStats: true });
    await screen.findByTestId("waveform");
    await waitFor(() => expect(listeners.has("download://state")).toBe(true));

    fakeTimers();
    downloadState("vid1", "queued");
    downloadState("vid1", "downloading");
    downloadState("vid1", "failed");
    await later(BUMP_DELAY * 2);
    expect(feedStats).toHaveBeenCalledTimes(1);

    downloadState("vid1", "done");
    await later();
    expect(feedStats).toHaveBeenCalledTimes(2);

    downloadState("vid1", "none");
    await later();
    expect(feedStats).toHaveBeenCalledTimes(3);
  });

  it("asks nothing for an open series, and hands the bands every part once its page is in", async () => {
    videos = ["p1", "p2", "p3"].map((id) => video({ id }));
    renderView({ showStats: true, siblingOf: video({ id: "p2" }) });
    const bands = await screen.findByTestId("bands");
    expect(bands.textContent).toBe("3 parts");
    expect(lastParts().map((v) => v.id)).toEqual(["p1", "p2", "p3"]);
    await screen.findAllByRole("article");
    expect(bands.nextElementSibling).toBe(grid());
    await settle();
    expect(feedStats).not.toHaveBeenCalled();
    expect(screen.queryByTestId("waveform")).toBeNull();
  });

  it("hands the bands a further page's parts as it comes in", async () => {
    const all = shelf(150);
    listVideos.mockImplementation((f) => Promise.resolve(all.slice(f.offset, f.offset + f.limit)));
    renderView({ showStats: true, siblingOf: all[0] });
    expect((await screen.findByTestId("bands")).textContent).toBe("100 parts");
    pageIn();
    await waitFor(() => expect(screen.getByTestId("bands").textContent).toBe("150 parts"));
    expect(lastParts()).toHaveLength(150);
  });

  it("keeps the feed's waveform up until the series' first page lands, then swaps in the bands", async () => {
    const { rerender } = renderView({ showStats: true });
    await screen.findByTestId("waveform");
    await screen.findByRole("article");

    let land!: (v: Video[]) => void;
    listVideos.mockImplementationOnce(() => new Promise((res) => { land = res; }));
    rerender({ siblingOf: video() });
    await waitFor(() => expect(lastFilter().siblingOf).toBe("vid1"));
    // The grid is still the feed, and so is what stands over it.
    expect(screen.getByTestId("waveform").textContent).toBe("42 videos");
    expect(bandsProps).not.toHaveBeenCalled();

    await act(async () => land([video(), video({ id: "p2" })]));
    expect(screen.getByTestId("bands").textContent).toBe("2 parts");
    expect(screen.queryByTestId("waveform")).toBeNull();
    expect(feedStats).toHaveBeenCalledTimes(1);
  });

  it("keeps the bands until the feed is back under them, with the waveform it was left with", async () => {
    const { rerender } = renderView({ showStats: true });
    await screen.findByTestId("waveform");
    rerender({ siblingOf: video() });
    await screen.findByTestId("bands");

    // Leaving asks for the feed's stats at once -- what was done inside the
    // series moved them -- but this answer never comes, and the feed's first
    // page is held.
    feedStats.mockImplementationOnce(() => new Promise<FeedStats>(() => {}));
    let land!: (v: Video[]) => void;
    listVideos.mockImplementationOnce(() => new Promise((res) => { land = res; }));
    rerender({ siblingOf: null });
    await waitFor(() => expect(feedStats).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId("bands")).toBeTruthy();
    expect(screen.queryByTestId("waveform")).toBeNull();

    await act(async () => land([video()]));
    expect(screen.getByTestId("waveform").textContent).toBe("42 videos");
    expect(screen.queryByTestId("bands")).toBeNull();
  });
});
