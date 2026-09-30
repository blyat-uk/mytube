import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";
import type {
  Channel, Settings, ToolStatus, VersionInfo, Video, ViewState, VideoFilter,
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

vi.mock("./api", () => ({
  api: {
    getSettings: () => getSettings(),
    listChannels: () => listChannels(),
    saveViewState: (...a: unknown[]) => saveViewState(...(a as [])),
    listVideos: (filter: VideoFilter) => listVideos(filter),
    listVideoGroups: (filter: VideoFilter) => listVideoGroups(filter),
    toolsStatus: () => toolsStatus(),
    openExternal: (url: string) => openExternal(url),
    appVersionInfo: () => appVersionInfo(),
  },
  thumbSrc: () => "",
  errText: (e: unknown) => String(e),
}));

import App from "./App";

function defaultView(over: Partial<ViewState> = {}): ViewState {
  return {
    channel_id: null, search: "", hide_watched: false, downloaded_only: false,
    show_hidden: false, grouped: false, groups_only: false, in_progress: false, sort: "newest",
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
    check_app_updates: true,
    ytdlp_path: "", ffmpeg_path: "", deno_path: "",
    quality: { ...DEFAULT_QUALITY },
    view: defaultView(view),
  };
}

let settings: Settings = settingsWith();

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
 * ordering guarantee between them. Before this feature `channelId` always
 * started null, so the channel-validity effect (in App.tsx, just above the
 * hydration effect) could never see a non-null one before `channels` was
 * populated. Restoring `channelId` from the saved view changes that: if
 * `getSettings` resolves first, the effect sees a restored `channelId`
 * against a still-unfetched `channels === []` and, without a guard, reads
 * that as "this channel was deleted."
 */
describe("restoring a channel filter while the channel list is still loading", () => {
  it("keeps a restored channel that the list, once it arrives, actually contains", async () => {
    settings = settingsWith({ channel_id: "UC1" });

    let resolveChannels!: (channels: Channel[]) => void;
    listChannels.mockImplementation(
      () => new Promise<Channel[]>((resolve) => { resolveChannels = resolve; }),
    );

    render(<App />);

    // The render gate waits only on getSettings, so the search box (and the
    // restored channelId behind it) exist well before listChannels resolves.
    await screen.findByLabelText("Search videos");
    // Real time, not a bare `act()` flush: a missing guard prunes
    // `channelId` through a cascade of renders (the newly-mounted
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
    expect(lastCall?.[0]).toMatchObject({ channelId: "UC1" });
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

/** One ready card, so the feed has something to open a series from. */
function feedVideo(): Video {
  return {
    id: "vid1", channel_id: "UC1", channel_title: "Veritasium", title: "Some video",
    description: null, thumb_url: null, thumb_path: null, published_at: 1_700_000_000,
    sort_at: null, feed_rank: 0, added_manually: false, duration_secs: 754,
    view_count: 1500, status: "ready", hidden: false, watched: false, watched_at: null,
    download_state: "none", download_error: null, file_path: null,
    downloaded_at: null, first_seen_at: 0, sibling_group: null,
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

describe("a newer MyTube release", () => {
  const RELEASE = "https://github.com/blyat-uk/mytube/releases/tag/v0.1.2";

  it("toasts it with a Download button that opens the release page", async () => {
    render(<App />);
    await waitFor(() => expect(listeners.has("app://update-available")).toBe(true));

    act(() => listeners.get("app://update-available")!({ payload: { version: "0.1.2", url: RELEASE } }));

    expect(await screen.findByText("MyTube 0.1.2 is available.")).toBeDefined();
    fireEvent.click(button("Download"));
    expect(openExternal).toHaveBeenCalledWith(RELEASE);
    // Acting on the toast is also done with it.
    await waitFor(() => expect(screen.queryByText("MyTube 0.1.2 is available.")).toBeNull());
  });
});

describe("the version in the logo", () => {
  const RELEASE = "https://github.com/blyat-uk/mytube/releases/tag/v2.2.0";

  it("shows this build's version beside the name", async () => {
    render(<App />);
    expect(await screen.findByText("2.1.0")).toBeDefined();
    expect(screen.getByText("MyTube")).toBeDefined();
  });

  it("turns into a link to the release page when a newer one is out", async () => {
    render(<App />);
    await screen.findByText("2.1.0");
    await waitFor(() => expect(listeners.has("app://version-info")).toBe(true));

    act(() => listeners.get("app://version-info")!({
      payload: { ...VERSION, update: { version: "2.2.0", url: RELEASE } },
    }));

    fireEvent.click(await screen.findByRole("button", { name: /Update available: MyTube 2\.2\.0/ }));
    expect(openExternal).toHaveBeenCalledWith(RELEASE);
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
