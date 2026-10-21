import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act, within } from "@testing-library/react";
import type {
  Channel, FeedStats, Settings, StatsFilter, ToolStatus, VersionInfo, Video, ViewState, VideoFilter,
} from "./types";
import { DEFAULT_QUALITY } from "./quality";

const VERSION: VersionInfo = {
  current: "2.1.0", update: null, checkedAt: 1_790_000_000, checksEnabled: true,
};

// Every listener the shell registers, by event name, so a test can deliver an
// event exactly when it wants to.
const listeners = new Map<string, (e: { payload: unknown }) => void>();
vi.mock("@tauri-apps/api/event", () => ({
  listen: (name: string, cb: (e: { payload: unknown }) => void) => {
    listeners.set(name, cb);
    return Promise.resolve(() => {});
  },
}));

// jsdom ships no IntersectionObserver, and the grid's infinite scroll wants one.
class NoopObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}
vi.stubGlobal("IntersectionObserver", NoopObserver);

const getSettings = vi.fn(() => Promise.resolve(settings));
// Typed with a Channel[] return so a per-test mockImplementation that
// resolves real channels (below) type-checks against the same signature.
const listChannels = vi.fn<() => Promise<Channel[]>>(() => Promise.resolve([]));
const saveViewState = vi.fn(() => Promise.resolve());
// Typed with a filter parameter so `.mock.calls[0][0]` below type-checks.
const listVideos = vi.fn<(filter: VideoFilter) => Promise<unknown[]>>(() => Promise.resolve([]));
const listVideoGroups = vi.fn<(filter: VideoFilter) => Promise<unknown[]>>(() => Promise.resolve([]));
const toolsStatus = vi.fn<() => Promise<ToolStatus[]>>(() => Promise.resolve([]));
const openExternal = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());
const appVersionInfo = vi.fn<() => Promise<VersionInfo>>(() => Promise.resolve(VERSION));
const syncNudge = vi.fn<() => Promise<void>>(() => Promise.resolve());
const feedStats = vi.fn<(f: StatsFilter) => Promise<FeedStats>>(() => Promise.resolve(STATS));
// Kept, as the backend keeps it: a second visit to Settings reads back what
// the first one saved.
const saveSettings = vi.fn((s: Settings) => {
  settings = s;
  return Promise.resolve();
});

// The waveform is drawn by its own tests; here it only says it is up.
vi.mock("./components/stats/FeedWaveform", () => ({
  default: ({ stats }: { stats: FeedStats }) => <div data-testid="waveform">{stats.videos} videos</div>,
}));

vi.mock("./api", () => {
  // What the Settings view reads on its way up, for the tests that open it.
  // They never answer: the rail then shows those lines loading, and nothing
  // here is about them. Declared in here because the factory is hoisted above
  // every top-level declaration in this file.
  const unanswered = () => new Promise<never>(() => {});
  return {
    api: {
      getSettings: () => getSettings(),
      listChannels: () => listChannels(),
      saveViewState: (...a: unknown[]) => saveViewState(...(a as [])),
      listVideos: (filter: VideoFilter) => listVideos(filter),
      listVideoGroups: (filter: VideoFilter) => listVideoGroups(filter),
      toolsStatus: () => toolsStatus(),
      openExternal: (url: string) => openExternal(url),
      appVersionInfo: () => appVersionInfo(),
      syncNudge: () => syncNudge(),
      subscriptionStats: () => Promise.resolve([]),
      feedStats: (f: StatsFilter) => feedStats(f),
      saveSettings: (s: Settings) => saveSettings(s),
      syncStatus: unanswered,
      detectBrowsers: unanswered,
      detectPlayers: unanswered,
      transferEstimate: unanswered,
    },
    thumbSrc: () => "",
    errText: (e: unknown) => String(e),
  };
});

import App from "./App";

function defaultView(over: Partial<ViewState> = {}): ViewState {
  return {
    channel_ids: [], search: "", hide_watched: false, downloaded_only: false,
    show_hidden: false, grouped: false, groups_only: false, in_progress: false,
    auto_downloading: false, sort: "newest", subs_folded: false,
    ...over,
  };
}

function settingsWith(view: Partial<ViewState> = {}): Settings {
  return {
    download_dir: "/videos", filename_template: "%(uploader)s/%(title)s [%(id)s].%(ext)s",
    player_command: "smplayer", max_concurrent_downloads: 5, poll_interval_minutes: 30,
    poll_on_startup: true, backfill_count: 30, card_size: 260,
    window_width: 1280, window_height: 840, window_x: null, window_y: null,
    window_maximized: false,
    cookies_browser: "auto", cookies_file: "", ytdlp_channel: "nightly", ytdlp_auto_update: true,
    check_app_updates: true, text_scale: 100, show_stats: false,
    ytdlp_path: "", ffmpeg_path: "", deno_path: "",
    quality: { ...DEFAULT_QUALITY },
    view: defaultView(view),
  };
}

let settings: Settings = settingsWith();

const STATS: FeedStats = {
  videos: 42, series: 3, toWatch: 30, watched: 10, downloaded: 5, hidden: 2,
  runtimeSecs: 36_000, runtimeUnknown: false, newThisWeek: 1,
  today: "2026-10-19", days: [{ day: "2026-10-12", watched: 10, downloaded: 4, unwatched: 26, hidden: 2 }],
};

/**
 * Waits real time, inside `act`. The shell's IPC mocks resolve on their own
 * schedule, and on a slow CI runner one of them can land in the middle of a
 * bare `setTimeout` wait -- a state update outside `act`, which React reports.
 * Locally everything has settled long before, so a bare wait looks fine here.
 */
const pause = (ms: number) => act(() => new Promise<void>((resolve) => setTimeout(resolve, ms)));

const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

beforeEach(() => {
  settings = settingsWith();
  vi.clearAllMocks();
  getSettings.mockImplementation(() => Promise.resolve(settings));
  listChannels.mockImplementation(() => Promise.resolve([]));
  listVideos.mockImplementation(() => Promise.resolve([]));
  listVideoGroups.mockImplementation(() => Promise.resolve([]));
  saveViewState.mockImplementation(() => Promise.resolve());
  toolsStatus.mockImplementation(() => Promise.resolve([]));
  appVersionInfo.mockImplementation(() => Promise.resolve(VERSION));
  syncNudge.mockImplementation(() => Promise.resolve());
  feedStats.mockImplementation(() => Promise.resolve(STATS));
  saveSettings.mockImplementation((s: Settings) => {
    settings = s;
    return Promise.resolve();
  });
  listeners.clear();
});

afterEach(cleanup);

describe("restoring the saved view on launch", () => {
  it("applies a saved search, chip and sort to the restored feed", async () => {
    settings = settingsWith({ search: "python", hide_watched: true, sort: "oldest" });
    render(<App />);

    // TopNav seeds its search box from the prop at mount, so this only reads
    // right if the shell withheld the first render until the view arrived.
    expect(await screen.findByLabelText("Search videos")).toHaveProperty("value", "python");
    expect(button("Unwatched").getAttribute("aria-pressed")).toBe("true");
    expect((screen.getByLabelText("Sort order") as HTMLSelectElement).value).toBe("oldest");

    await waitFor(() => expect(listVideos).toHaveBeenCalled());
    expect(listVideos.mock.calls[0][0]).toMatchObject({
      search: "python", hideWatched: true, sort: "oldest",
    });
  });
});

/**
 * `getSettings` and `listChannels` are two independent IPC calls with no
 * ordering guarantee between them. Restoring `channelIds` from the saved view
 * means that if `getSettings` resolves first, the channel-validity effect (in
 * App.tsx, just above the hydration effect) sees a restored pick against a
 * still-unfetched `channels === []` and, without a guard, reads that as
 * "these channels were deleted."
 */
describe("restoring a channel filter while the channel list is still loading", () => {
  it("keeps a restored channel that the list, once it arrives, actually contains", async () => {
    settings = settingsWith({ channel_ids: ["UC1"] });

    let resolveChannels!: (channels: Channel[]) => void;
    listChannels.mockImplementation(
      () => new Promise<Channel[]>((resolve) => { resolveChannels = resolve; }),
    );

    render(<App />);

    // The render gate waits only on getSettings, so the search box (and the
    // restored channelIds behind it) exist well before listChannels resolves.
    await screen.findByLabelText("Search videos");
    // Real time, not a bare `act()` flush: a missing guard prunes
    // `channelIds` through a cascade of renders (the newly-mounted
    // SubscriptionsView fetches with "UC1" before Shell's own effect corrects
    // it), and how much of that cascade a single `act()` pass settles is not
    // dependable. A real wait lets it fully play out one way or the other
    // before the channel list is ever introduced.
    await pause(100);

    // Only now does the channel list resolve, containing the very channel the
    // restored filter points at.
    act(() => resolveChannels([
      {
        id: "UC1", title: "Chills Narrated", handle: null,
        url: "https://youtube.com/channel/UC1", thumb_path: null,
        subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null,
        terminated: false,
      },
    ]));
    await pause(100);

    const lastCall = listVideos.mock.calls[listVideos.mock.calls.length - 1];
    expect(lastCall?.[0]).toMatchObject({ channelIds: ["UC1"] });
  });

  it("drops only the picked channels the list no longer has", async () => {
    settings = settingsWith({ channel_ids: ["UC1", "UCgone"] });
    listChannels.mockImplementation(() => Promise.resolve([
      {
        id: "UC1", title: "Chills Narrated", handle: null,
        url: "https://youtube.com/channel/UC1", thumb_path: null,
        subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null,
        terminated: false,
      },
    ]));

    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => {
      const lastCall = listVideos.mock.calls[listVideos.mock.calls.length - 1];
      expect(lastCall?.[0]).toMatchObject({ channelIds: ["UC1"] });
    });
  });
});

/**
 * Real timers throughout, not fake ones -- two fake-timer orderings were
 * tried and both misled. Installed before mount, hydration's own promise
 * chain never reached the DOM at all: `findByLabelText` ran out Vitest's
 * outer 5s test limit rather than `waitFor`'s own, which means the update was
 * genuinely stuck, not merely unpolled. Installed only after
 * `findByLabelText` resolves, it is too late: the writer effect's first run
 * fires the instant `hydrated` flips to true, inside that same promise chain,
 * so a real `setTimeout` from that run can already be pending before a fake
 * clock exists to see it -- and neither `advanceTimersByTime` nor even a fake
 * `clearTimeout` reaches a timer a real one scheduled. A real wait past the
 * 400ms window is the one thing that watches both runs of the effect on the
 * clock they actually run on.
 */
describe("saving the view back", () => {
  it("writes nothing on a launch that restores the defaults unchanged", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");

    // `hydrated` is itself a dependency of the writer effect even though none
    // of the seven filter values actually moved -- so this is the run that
    // would catch a missing "skip when unchanged" guard.
    await pause(500);

    expect(saveViewState).not.toHaveBeenCalled();
  });

  it("writes once, debounced, when a filter chip is toggled", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");

    fireEvent.click(button("Unwatched"));
    // Well short of the 400ms debounce -- close enough to the click to rule
    // out an immediate write, far enough from 400ms that a loaded machine
    // can't turn this into a flake -- nothing has been written yet.
    await pause(150);
    expect(saveViewState).not.toHaveBeenCalled();

    await waitFor(() => expect(saveViewState).toHaveBeenCalledTimes(1));
    expect(saveViewState).toHaveBeenCalledWith(defaultView({ hide_watched: true }));

    // `waitFor` only proves the call had landed by the time it settled --
    // that is "at least once", not "once". Give the writer effect a full
    // cycle beyond that and re-check the same count.
    await pause(500);
    expect(saveViewState).toHaveBeenCalledTimes(1);
  });

  /**
   * `getSettings` and the writer's 400ms debounce race on every launch.
   * `hydrated` exists to make sure hydration always wins: without it, the
   * writer's first run fires at t=400 on the component's own defaults --
   * clobbering the saved view on disk before hydration (arriving later
   * here) ever gets to restore it. Deferring `getSettings` past the
   * debounce, the way `listChannels` is deferred above, is what puts the
   * writer first in that race.
   */
  it("does not write before the saved view has been read back", async () => {
    let resolveSettings!: (s: Settings) => void;
    getSettings.mockImplementation(
      () => new Promise<Settings>((resolve) => { resolveSettings = resolve; }),
    );
    settings = settingsWith({ search: "python" });

    render(<App />);

    // Past the debounce, well before getSettings resolves.
    await pause(500);
    expect(saveViewState).not.toHaveBeenCalled();

    act(() => resolveSettings(settings));
    expect(await screen.findByLabelText("Search videos")).toHaveProperty("value", "python");
  });
});

/**
 * "Continue watching" is a filter over series, so it lives and dies with the
 * Grouped chip exactly as "Only groups" does.
 */
describe("the continue-watching filter", () => {
  it("restores from the saved view and is sent to the grouped feed", async () => {
    settings = settingsWith({ grouped: true, in_progress: true });
    render(<App />);
    await screen.findByLabelText("Search videos");
    expect(button("Continue watching").getAttribute("aria-pressed")).toBe("true");
    await waitFor(() => expect(listVideoGroups).toHaveBeenCalled());
    expect(listVideoGroups.mock.calls[0][0]).toMatchObject({ inProgress: true });
  });

  it("is saved into the view when switched on", async () => {
    settings = settingsWith({ grouped: true });
    render(<App />);
    await screen.findByLabelText("Search videos");

    fireEvent.click(button("Continue watching"));
    await waitFor(() => expect(saveViewState).toHaveBeenCalledTimes(1));
    expect(saveViewState).toHaveBeenCalledWith(defaultView({ grouped: true, in_progress: true }));
  });

  it("is cleared, with Only groups, when Group siblings goes off", async () => {
    settings = settingsWith({ grouped: true, groups_only: true, in_progress: true });
    render(<App />);
    await screen.findByLabelText("Search videos");

    fireEvent.click(button("Grouped"));
    await waitFor(() => expect(saveViewState).toHaveBeenCalledTimes(1));
    expect(saveViewState).toHaveBeenCalledWith(defaultView());

    // Grouping again must not bring the filter back on by itself. Async `act`
    // so the refetch this click starts has answered before the test ends --
    // otherwise it lands after the last line, outside any `act`.
    await act(async () => { fireEvent.click(button("Grouped")); });
    expect(button("Continue watching").getAttribute("aria-pressed")).toBe("false");
  });
});

/** "Auto-downloading" is a filter over series too, kept and cleared the same way. */
describe("the auto-downloading filter", () => {
  it("restores from the saved view and is sent to the grouped feed", async () => {
    settings = settingsWith({ grouped: true, auto_downloading: true });
    render(<App />);
    await screen.findByLabelText("Search videos");
    expect(button("Auto-downloading").getAttribute("aria-pressed")).toBe("true");
    await waitFor(() => expect(listVideoGroups).toHaveBeenCalled());
    expect(listVideoGroups.mock.calls[0][0]).toMatchObject({ autoDownloading: true });
  });

  it("is saved into the view when switched on", async () => {
    settings = settingsWith({ grouped: true });
    render(<App />);
    await screen.findByLabelText("Search videos");

    fireEvent.click(button("Auto-downloading"));
    await waitFor(() => expect(saveViewState).toHaveBeenCalledTimes(1));
    expect(saveViewState).toHaveBeenCalledWith(defaultView({ grouped: true, auto_downloading: true }));
  });

  it("is cleared when Group siblings goes off", async () => {
    settings = settingsWith({ grouped: true, auto_downloading: true });
    render(<App />);
    await screen.findByLabelText("Search videos");

    fireEvent.click(button("Grouped"));
    await waitFor(() => expect(saveViewState).toHaveBeenCalledTimes(1));
    expect(saveViewState).toHaveBeenCalledWith(defaultView());
    await act(async () => { fireEvent.click(button("Grouped")); });
    expect(button("Auto-downloading").getAttribute("aria-pressed")).toBe("false");
  });

  it("reads a view saved before the chip existed as off, and writes nothing back", async () => {
    settings = settingsWith({ grouped: true });
    delete (settings.view as Partial<ViewState>).auto_downloading;
    render(<App />);
    await screen.findByLabelText("Search videos");
    expect(button("Auto-downloading").getAttribute("aria-pressed")).toBe("false");
    await act(async () => { await new Promise((r) => setTimeout(r, 500)); });
    expect(saveViewState).not.toHaveBeenCalled();
  });
});

/**
 * Group siblings is on for someone who has never saved a view. The backend's
 * ViewState default says so too; this is the shell's own fallback, the one a
 * failed settings read leaves in place.
 */
describe("the default grouping", () => {
  it("starts grouped when no saved view could be read, and writes nothing back", async () => {
    getSettings.mockImplementation(() => Promise.reject("no settings"));
    render(<App />);
    await screen.findByLabelText("Search videos");
    expect(button("Grouped").getAttribute("aria-pressed")).toBe("true");
    await waitFor(() => expect(listVideoGroups).toHaveBeenCalled());
    expect(listVideos).not.toHaveBeenCalled();

    await pause(500);
    expect(saveViewState).not.toHaveBeenCalled();
  });
});

/**
 * The Add dialog's subscription list folds away, and the shell remembers it
 * with the rest of the view -- which is what brings it back folded on the next
 * launch.
 */
describe("the Add dialog's folded list", () => {
  it("opens the dialog folded when it was left folded", async () => {
    settings = settingsWith({ subs_folded: true });
    render(<App />);
    await screen.findByLabelText("Search videos");
    await act(async () => { fireEvent.click(button("Add")); });
    expect(button("Subscriptions 0").getAttribute("aria-expanded")).toBe("false");
  });

  it("writes the fold into the saved view", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");
    await act(async () => { fireEvent.click(button("Add")); });
    fireEvent.click(button("Subscriptions 0"));
    await waitFor(() => expect(saveViewState).toHaveBeenCalledTimes(1));
    expect(saveViewState).toHaveBeenCalledWith(defaultView({ subs_folded: true }));
  });
});

/** One ready card, so the feed has something to open a series from. */
function feedVideo(): Video {
  return {
    id: "vid1", channel_id: "UC1", channel_title: "Veritasium", title: "Some video",
    description: null, thumb_url: null, thumb_path: null, published_at: 1_700_000_000,
    sort_at: null, feed_rank: 0, added_manually: false, duration_secs: 754,
    view_count: 1500, status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "none", download_error: null, file_path: null,
    downloaded_at: null, first_seen_at: 0, sibling_group: null, series_hidden: false,
    part_links_refused: false, part_linked: false, series_auto_download: false,
  };
}

/**
 * The scroll container is the shell's, and the view that puts an offset back
 * into it is a child holding a ref to it -- so this covers the one thing the
 * view's own tests cannot: that the ref reaches `.content` itself.
 */
describe("keeping the feed's place across a series visit", () => {
  it("puts the shell's scroll container back where it was", async () => {
    listVideos.mockImplementation(() => Promise.resolve([feedVideo()]));
    render(<App />);

    const card = await screen.findByRole("article");
    const content = document.querySelector(".content") as HTMLElement;
    content.scrollTop = 900;

    fireEvent.contextMenu(card);
    fireEvent.click(screen.getByRole("menuitem", { name: "Find siblings" }));
    await waitFor(() => {
      const calls = listVideos.mock.calls;
      expect(calls[calls.length - 1][0]).toMatchObject({ siblingOf: "vid1" });
    });
    // What the browser does once the grid collapses to the parts of one series.
    // jsdom lays nothing out, so the clamp is applied by hand.
    content.scrollTop = 0;

    // Escape leaves the series, the same as the breadcrumb's own button.
    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(content.scrollTop).toBe(900));
  });
});

/**
 * A card's "See all videos from <channel>" is the shell's to carry out: the
 * channel pick, the search box and an open series all live up here.
 */
describe("a card's See all videos from its channel", () => {
  const subscription = (id: string, title: string): Channel => ({
    id, title, handle: null, url: `https://youtube.com/channel/${id}`, thumb_path: null,
    subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null,
    terminated: false,
  });
  const lastFilter = () => listVideos.mock.calls[listVideos.mock.calls.length - 1][0];

  it("narrows the feed to that channel, empties the search box and leaves the series", async () => {
    settings = settingsWith({
      channel_ids: ["UC1", "UC2"], search: "some", hide_watched: true, sort: "oldest",
    });
    listChannels.mockImplementation(() => Promise.resolve([
      subscription("UC1", "Veritasium"), subscription("UC2", "Chills Narrated"),
    ]));
    listVideos.mockImplementation(() => Promise.resolve([feedVideo()]));
    render(<App />);

    fireEvent.contextMenu(await screen.findByRole("article"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Find siblings" }));
    await screen.findByRole("navigation", { name: "Breadcrumb" });

    fireEvent.contextMenu(await screen.findByRole("article"));
    fireEvent.click(
      await screen.findByRole("menuitem", { name: "See all videos from Veritasium" }),
    );

    await waitFor(() => expect(screen.queryByRole("navigation", { name: "Breadcrumb" })).toBeNull());
    expect(screen.getByLabelText("Search videos")).toHaveProperty("value", "");
    await waitFor(() => expect(lastFilter()).toMatchObject({
      channelIds: ["UC1"], search: null, siblingOf: null,
      // How the feed is read stays as it was.
      hideWatched: true, sort: "oldest",
    }));
    // Past the box's debounce: the query it held is not sent back up after it.
    await pause(350);
    expect(lastFilter()).toMatchObject({ channelIds: ["UC1"], search: null });
    expect(screen.getByLabelText("Search videos")).toHaveProperty("value", "");
  });

  it("is not offered on a video from a channel you do not subscribe to", async () => {
    listChannels.mockImplementation(() => Promise.resolve([subscription("UC2", "Chills Narrated")]));
    listVideos.mockImplementation(() => Promise.resolve([feedVideo()]));
    render(<App />);
    await waitFor(() => expect(listChannels).toHaveBeenCalled());
    await pause(50);

    fireEvent.contextMenu(await screen.findByRole("article"));
    expect(screen.getByRole("menuitem", { name: "Find siblings" })).toBeTruthy();
    expect(screen.queryByRole("menuitem", { name: /^See all videos from/ })).toBeNull();
  });
});

/**
 * `tools_status` (the seed) and `tools://status` (the events) race at launch:
 * provisioning starts before the webview does. Whichever lands first, a tool
 * that went from installing to ready while the shell was listening gets its
 * toast, and a launch where everything was already ready gets none.
 */
describe("the tools-ready toast", () => {
  const ytdlp = (state: ToolStatus["state"]): ToolStatus[] => [{
    kind: "ytdlp", path: null, version: null, source: "managed", state,
    error: null, lastCheck: null,
  }];

  /** Resolves the seed only when the test says so. */
  function holdSeed() {
    let resolve!: (list: ToolStatus[]) => void;
    toolsStatus.mockImplementation(() => new Promise((r) => { resolve = r; }));
    return (list: ToolStatus[]) => act(() => resolve(list));
  }

  const emit = (list: ToolStatus[]) =>
    act(() => listeners.get("tools://status")!({ payload: list }));

  it("still toasts when the ready event beats a seed read while installing", async () => {
    const seed = holdSeed();
    render(<App />);
    await waitFor(() => expect(listeners.has("tools://status")).toBe(true));
    await waitFor(() => expect(toolsStatus).toHaveBeenCalled());

    emit(ytdlp("ready"));
    seed(ytdlp("installing"));

    expect(await screen.findByText("yt-dlp is ready.")).toBeDefined();
  });

  it("toasts when the seed lands first and the install finishes after", async () => {
    const seed = holdSeed();
    render(<App />);
    await waitFor(() => expect(listeners.has("tools://status")).toBe(true));
    await waitFor(() => expect(toolsStatus).toHaveBeenCalled());

    seed(ytdlp("installing"));
    await pause(0);
    emit(ytdlp("ready"));

    expect(await screen.findByText("yt-dlp is ready.")).toBeDefined();
  });

  it("stays quiet on a launch where the tools were already ready", async () => {
    const seed = holdSeed();
    render(<App />);
    await waitFor(() => expect(listeners.has("tools://status")).toBe(true));
    await waitFor(() => expect(toolsStatus).toHaveBeenCalled());

    emit(ytdlp("ready"));
    seed(ytdlp("ready"));
    await pause(50);
    emit(ytdlp("ready"));
    await pause(50);

    expect(screen.queryByText("yt-dlp is ready.")).toBeNull();
  });
});

/**
 * Every way MyTube says a newer release is out -- the nav's chip, the toast,
 * the tray's menu item -- leads to Settings at About, where the installed and
 * the available version stand side by side and the release page is one click
 * further. None of them opens the release page itself.
 */
describe("a newer MyTube release", () => {
  const RELEASE = "https://github.com/blyat-uk/mytube/releases/tag/v2.2.0";
  const UPDATE = { version: "2.2.0", url: RELEASE };
  const chip = () => screen.queryByRole("button", { name: /^Update available: MyTube 2\.2\.0/ });

  /** Settings is the tab on screen, opened at About. */
  async function aboutShown() {
    await waitFor(() => expect(button("Settings").getAttribute("aria-current")).toBe("page"));
    const rail = await screen.findByRole("navigation", { name: "Settings sections" });
    expect(rail).toBeDefined();
    expect(button("About").getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("heading", { level: 2, name: "About" })).toBeDefined();
  }

  it("toasts it with a View button that opens About", async () => {
    render(<App />);
    await waitFor(() => expect(listeners.has("app://update-available")).toBe(true));

    act(() => listeners.get("app://update-available")!({ payload: UPDATE }));

    expect(await screen.findByText("MyTube 2.2.0 is available.")).toBeDefined();
    fireEvent.click(button("View"));
    await aboutShown();
    expect(openExternal).not.toHaveBeenCalled();
    // Acting on the toast is also done with it.
    await waitFor(() => expect(screen.queryByText("MyTube 2.2.0 is available.")).toBeNull());
  });

  it("puts no chip in the nav, and no version, while this build is the newest", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => expect(appVersionInfo).toHaveBeenCalled());
    await pause(20);
    expect(chip()).toBeNull();
    expect(screen.queryByText("2.1.0")).toBeNull();
    expect(screen.queryByText("MyTube")).toBeNull();
  });

  it("puts a chip beside Settings once a check finds one, which opens About", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => expect(listeners.has("app://version-info")).toBe(true));

    act(() => listeners.get("app://version-info")!({ payload: { ...VERSION, update: UPDATE } }));

    fireEvent.click(await screen.findByRole("button", { name: /^Update available: MyTube 2\.2\.0/ }));
    await aboutShown();
    expect(openExternal).not.toHaveBeenCalled();
  });

  it("opens About when the tray's update item asks", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => expect(listeners.has("app://open-about")).toBe(true));

    act(() => listeners.get("app://open-about")!({ payload: null }));
    await aboutShown();
  });

  /* Settings under a series' breadcrumb would leave no tabs to get back by. */
  it("leaves an open series to show About", async () => {
    listVideos.mockImplementation(() => Promise.resolve([feedVideo()]));
    render(<App />);
    fireEvent.contextMenu(await screen.findByRole("article"));
    fireEvent.click(screen.getByRole("menuitem", { name: "Find siblings" }));
    await screen.findByRole("navigation", { name: "Breadcrumb" });

    act(() => listeners.get("app://open-about")!({ payload: null }));
    await aboutShown();
    expect(screen.queryByRole("navigation", { name: "Breadcrumb" })).toBeNull();
  });

  it("keeps a check that lands before the first read over that older read", async () => {
    let resolve!: (v: VersionInfo) => void;
    appVersionInfo.mockImplementation(() => new Promise((r) => { resolve = r; }));
    render(<App />);
    await waitFor(() => expect(listeners.has("app://version-info")).toBe(true));

    act(() => listeners.get("app://version-info")!({
      payload: { ...VERSION, update: { version: "2.2.0", url: RELEASE } },
    }));
    await act(async () => { resolve(VERSION); });

    expect(screen.getByRole("button", { name: /Update available: MyTube 2\.2\.0/ })).toBeDefined();
  });
});

/**
 * Sync's two touch points with the shell: another computer's changes refetch
 * the views exactly as an import does, and the moments someone is looking --
 * the window coming forward, a poll landing in front of them -- ask for a pull.
 */
describe("cloud sync", () => {
  const POLLED = { channelsPolled: 3, newVideos: 0, shortsRejected: 0, errors: [] };

  // Only this spy: restoring every mock would also take down test-setup's
  // console.error spy, which is what fails a test on an update outside act().
  let visibility: { mockRestore: () => void } | null = null;
  const pretend = (state: DocumentVisibilityState) => {
    visibility = vi.spyOn(document, "visibilityState", "get").mockReturnValue(state);
  };
  afterEach(() => {
    visibility?.mockRestore();
    visibility = null;
  });

  it("refetches the channels and the feed when a sync changes the library, and toasts nothing", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => expect(listeners.has("sync://finished")).toBe(true));
    await waitFor(() => expect(listVideos).toHaveBeenCalled());
    await pause(50);
    const feeds = listVideos.mock.calls.length;
    const lists = listChannels.mock.calls.length;

    act(() => listeners.get("sync://finished")!({ payload: null }));

    await waitFor(() => expect(listVideos.mock.calls.length).toBeGreaterThan(feeds));
    expect(listChannels.mock.calls.length).toBeGreaterThan(lists);
    expect(screen.queryByText(/new video/)).toBeNull();
  });

  it("asks for a sync when the window gains focus", async () => {
    render(<App />);
    await screen.findByLabelText("Search videos");
    expect(syncNudge).not.toHaveBeenCalled();

    act(() => { window.dispatchEvent(new Event("focus")); });
    await waitFor(() => expect(syncNudge).toHaveBeenCalledTimes(1));
  });

  it("never toasts a failed nudge", async () => {
    syncNudge.mockImplementation(() => Promise.reject("Sync is off."));
    render(<App />);
    await screen.findByLabelText("Search videos");

    act(() => { window.dispatchEvent(new Event("focus")); });
    await waitFor(() => expect(syncNudge).toHaveBeenCalledTimes(1));
    await pause(20);
    expect(screen.queryByText("Sync is off.")).toBeNull();
  });

  it("asks for a sync after a poll while the window is visible", async () => {
    pretend("visible");
    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => expect(listeners.has("poll://finished")).toBe(true));

    act(() => listeners.get("poll://finished")!({ payload: POLLED }));
    await waitFor(() => expect(syncNudge).toHaveBeenCalledTimes(1));
  });

  it("stays quiet after a poll behind a hidden window", async () => {
    pretend("hidden");
    render(<App />);
    await screen.findByLabelText("Search videos");
    await waitFor(() => expect(listeners.has("poll://finished")).toBe(true));

    act(() => listeners.get("poll://finished")!({ payload: POLLED }));
    // The poll's own refetch is the sign the handler has run.
    await waitFor(() => expect(listChannels.mock.calls.length).toBeGreaterThan(1));
    await pause(20);
    expect(syncNudge).not.toHaveBeenCalled();
  });
});

/**
 * Show stats is switched in Settings, which is unmounted whenever the feed is
 * up, and the shell reads settings.json once: so the switch reaches the feed
 * through the saves Settings reports, never through a fresh read.
 */
describe("the feed's stats", () => {
  /** Settings, opened at Appearance, and its Show stats switch. */
  async function statsSwitch() {
    fireEvent.click(button("Settings"));
    const rail = await screen.findByRole("navigation", { name: "Settings sections" });
    fireEvent.click(within(rail).getByRole("button", { name: "Appearance" }));
    return await screen.findByRole("switch", { name: "Show stats" }) as HTMLInputElement;
  }

  it("asks for none, and draws none, while the setting is off", async () => {
    listVideos.mockImplementation(() => Promise.resolve([feedVideo()]));
    render(<App />);
    await screen.findByRole("article");
    await pause(50);
    expect(feedStats).not.toHaveBeenCalled();
    expect(screen.queryByTestId("waveform")).toBeNull();
  });

  it("draws them from launch when the setting is on", async () => {
    settings = { ...settingsWith({ channel_ids: ["UC1"], search: "tapes" }), show_stats: true };
    render(<App />);
    expect((await screen.findByTestId("waveform")).textContent).toBe("42 videos");
    expect(feedStats).toHaveBeenCalledWith({ channelIds: ["UC1"], search: "tapes" });
  });

  it("follows the switch from Settings back to the feed, without a restart", async () => {
    listVideos.mockImplementation(() => Promise.resolve([feedVideo()]));
    render(<App />);
    await screen.findByRole("article");

    const on = await statsSwitch();
    expect(on.checked).toBe(false);
    fireEvent.click(on);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(saveSettings.mock.calls[0][0]).toMatchObject({ show_stats: true });

    fireEvent.click(button("Subscriptions"));
    expect((await screen.findByTestId("waveform")).textContent).toBe("42 videos");
    // Read once by the shell at launch and once by Settings as it opened --
    // never again by the shell on the way back.
    expect(getSettings).toHaveBeenCalledTimes(2);

    const off = await statsSwitch();
    expect(off.checked).toBe(true);
    fireEvent.click(off);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    const asked = feedStats.mock.calls.length;

    fireEvent.click(button("Subscriptions"));
    await screen.findByRole("article");
    await pause(50);
    expect(screen.queryByTestId("waveform")).toBeNull();
    expect(feedStats).toHaveBeenCalledTimes(asked);
  });
});
