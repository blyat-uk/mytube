/*
 * The Settings view's test double: one mocked backend shared by every page's
 * tests, so they all render the real shell (rail included) against the same
 * fixtures. Wire it up at the top of a test file with:
 *
 *   vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
 *   vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
 *   vi.mock("../../api", async () => (await import("./testApi")).apiModule);
 *
 * then `beforeEach(resetHarness)` and `afterEach(cleanup)`. Change what the
 * backend answers through `backend` (or `api.x.mockImplementation`), deliver an
 * event with `emit`, and read saves with `lastSaved()`.
 *
 * Never import `../../api` from here: this module *is* its mock.
 */
import { vi } from "vitest";
import { act } from "@testing-library/react";
import { DEFAULT_QUALITY } from "../../quality";
import type {
  ArchiveSummary, BrowserOption, BrowserScan, Channel, CookiesFileInfo, ImportReport,
  PlayerOption, Settings, SyncStatus, ToolStatus, TransferEstimate, Video, VersionInfo,
} from "../../types";

export const SETTINGS: Settings = {
  download_dir: "/home/someone/Videos", filename_template: "%(title)s.%(ext)s",
  player_command: "smplayer", max_concurrent_downloads: 2, poll_interval_minutes: 30,
  poll_on_startup: true, backfill_count: 30, card_size: 260,
  window_width: 1280, window_height: 880, window_x: null, window_y: null,
  window_maximized: false,
  cookies_browser: "auto", cookies_file: "", ytdlp_channel: "nightly", ytdlp_auto_update: true,
  check_app_updates: true, text_scale: 100, show_stats: false,
  ytdlp_path: "", ffmpeg_path: "", deno_path: "",
  quality: { ...DEFAULT_QUALITY },
  view: {
    channel_ids: [], search: "", hide_watched: false, downloaded_only: false,
    show_hidden: false, grouped: false, groups_only: false, in_progress: false, sort: "newest",
    subs_folded: false,
  },
};

export const ESTIMATE: TransferEstimate = {
  channelCount: 38, videoCount: 3168, thumbCount: 3225,
  // The developer's own cache, to the byte: 112 MB is what the tick must say.
  thumbBytes: 112 * 1024 * 1024,
};

export const PLAYERS: PlayerOption[] = [
  { id: "default", label: "System default", command: "" },
  { id: "mpv", label: "mpv", command: "mpv" },
  { id: "vlc", label: "VLC", command: "vlc" },
];

export const CHROME_NOTE =
  "yt-dlp cannot read this browser's cookies on Windows. Export a cookies file.";
export const browserOpt = (over: Partial<BrowserOption> & Pick<BrowserOption, "id" | "label">): BrowserOption => ({
  supported: true, blocked: false, signedIn: false, note: null, ...over,
});
export const BROWSERS: BrowserOption[] = [
  browserOpt({ id: "chrome", label: "Chrome", supported: false, signedIn: null, note: CHROME_NOTE }),
  browserOpt({ id: "firefox", label: "Firefox" }),
];
export const SCAN: BrowserScan = { browsers: BROWSERS, automatic: "firefox", accessHint: null };
export const FILE_INFO: CookiesFileInfo = {
  format: "JSON cookie export", cookies: 24, skipped: 0, youtube: true, signedIn: true,
};

export const tool = (over: Partial<ToolStatus> & Pick<ToolStatus, "kind">): ToolStatus => ({
  path: null, version: null, source: "missing", state: "ready", error: null, lastCheck: null,
  ...over,
});
export const TOOLS_OK: ToolStatus[] = [
  tool({
    kind: "ytdlp", source: "managed", version: "2026.09.20",
    path: "/home/someone/.local/share/mytube/bin/yt-dlp", lastCheck: 1_790_000_000,
  }),
  tool({ kind: "ffmpeg", source: "system", version: "7.1", path: "/usr/bin/ffmpeg" }),
  tool({ kind: "deno", source: "override", version: "2.5.1", path: "/opt/deno/deno" }),
];
export const TOOLS_BROKEN: ToolStatus[] = [
  TOOLS_OK[0],
  TOOLS_OK[1],
  tool({ kind: "deno", source: "missing", state: "error", error: "checksum mismatch for deno" }),
];

export const SYNC_OFF: SyncStatus = {
  enabled: false, code: null, machineName: "desktop", peers: [], lastSyncAt: null,
  pending: false, error: null, expired: false,
};

export const VERSION: VersionInfo = {
  current: "2.1.0", update: null, checkedAt: null, checksEnabled: true,
};

/** What the mocked backend answers. Tests assign to these; `resetHarness`
 *  puts every one back. */
export const backend = {
  settings: SETTINGS,
  players: PLAYERS,
  scan: SCAN,
  tools: TOOLS_OK,
  sync: SYNC_OFF,
  version: VERSION,
  estimate: ESTIMATE,
  channels: [] as Channel[],
  videos: [] as Video[],
};

const fresh = () => ({
  settings: SETTINGS, players: PLAYERS, scan: SCAN, tools: TOOLS_OK, sync: SYNC_OFF,
  version: VERSION, estimate: ESTIMATE, channels: [] as Channel[], videos: [] as Video[],
});

/* Each mock is a `vi.fn` so a test can assert on it or swap its answer;
   `resetHarness` restores these default answers. */
function defaults() {
  return {
    getSettings: () => Promise.resolve(backend.settings),
    saveSettings: (_s: Settings) => Promise.resolve(),
    listChannels: () => Promise.resolve(backend.channels),
    listVideos: (_filter: unknown) => Promise.resolve(backend.videos),
    pollAll: () => Promise.resolve({ channels: 0, newVideos: 0, errors: [] } as unknown),
    transferEstimate: () => Promise.resolve(backend.estimate),
    exportConfig: (_path: string, _thumbs: boolean) => Promise.resolve(),
    readArchive: (_path: string) => Promise.resolve(null as unknown as ArchiveSummary),
    importConfig: (_path: string, _ids: string[], _mode: unknown, _apply: boolean) =>
      Promise.resolve(null as unknown as ImportReport),
    detectPlayers: () => Promise.resolve(backend.players),
    detectBrowsers: () => Promise.resolve(backend.scan),
    inspectCookiesFile: (_path: string) => Promise.resolve(FILE_INFO),
    openFullDiskAccess: () => Promise.resolve(),
    toolsStatus: () => Promise.resolve(backend.tools),
    toolsUpdateNow: () => Promise.resolve(TOOLS_OK),
    appVersionInfo: () => Promise.resolve(backend.version),
    checkAppUpdateNow: () => Promise.resolve({ ...backend.version, checkedAt: 1_790_000_000 }),
    openExternal: (_url: string) => Promise.resolve(),
    syncStatus: () => Promise.resolve(backend.sync),
    syncTurnOn: () => Promise.resolve(backend.sync),
    syncJoin: (_code: string) => Promise.resolve(backend.sync),
    syncLeave: () => Promise.resolve(SYNC_OFF),
    syncNow: () => Promise.resolve(false),
    syncNudge: () => Promise.resolve(),
  };
}

type Defaults = ReturnType<typeof defaults>;
type Mocked = { [K in keyof Defaults]: ReturnType<typeof vi.fn<Defaults[K]>> };

export const api = Object.fromEntries(
  Object.entries(defaults()).map(([k, impl]) => [k, vi.fn(impl)]),
) as unknown as Mocked;

export const apiModule = { api, errText: (e: unknown) => String(e) };

export const dialog = {
  open: vi.fn<(...args: unknown[]) => Promise<string | null>>(() => Promise.resolve(null)),
  save: vi.fn<(opts: unknown) => Promise<string | null>>(
    () => Promise.resolve("/home/someone/mytube-export-2026-09-22.zip"),
  ),
};
export const dialogModule = {
  open: (...args: unknown[]) => dialog.open(...args),
  save: (opts: unknown) => dialog.save(opts),
};

type Listener = (e: { payload: unknown }) => void;
const listeners = new Map<string, Set<Listener>>();
export const eventModule = {
  listen: (name: string, fn: Listener) => {
    let set = listeners.get(name);
    if (!set) listeners.set(name, (set = new Set()));
    set.add(fn);
    return Promise.resolve(() => { set!.delete(fn); });
  },
};

/** Delivers a backend event to every component listening for it. */
export async function emit(name: string, payload: unknown) {
  await act(async () => {
    for (const fn of [...(listeners.get(name) ?? [])]) fn({ payload });
  });
}

/** The settings object of the most recent save. */
export function lastSaved(): Settings {
  const calls = api.saveSettings.mock.calls;
  if (!calls.length) throw new Error("nothing was saved");
  return calls[calls.length - 1][0] as Settings;
}

export function resetHarness() {
  Object.assign(backend, fresh());
  const d = defaults() as Record<string, (...args: never[]) => unknown>;
  for (const [k, fn] of Object.entries(api)) {
    (fn as ReturnType<typeof vi.fn>).mockReset();
    (fn as ReturnType<typeof vi.fn>).mockImplementation(d[k] as never);
  }
  dialog.open.mockReset();
  dialog.open.mockImplementation(() => Promise.resolve(null));
  dialog.save.mockReset();
  dialog.save.mockImplementation(() => Promise.resolve("/home/someone/mytube-export-2026-09-22.zip"));
  listeners.clear();
}
