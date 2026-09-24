import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import type {
  BrowserOption, PlayerOption, Settings, ToolStatus, TransferEstimate,
} from "../types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

const save = vi.fn<(opts: unknown) => Promise<string | null>>(
  () => Promise.resolve("/home/someone/mytube-export-2026-09-22.zip"),
);
const openDialog = vi.fn<(...args: unknown[]) => Promise<string | null>>(
  () => Promise.resolve(null),
);
vi.mock("@tauri-apps/plugin-dialog", () => ({
  open: (...args: unknown[]) => openDialog(...args),
  save: (opts: unknown) => save(opts),
}));

const SETTINGS: Settings = {
  download_dir: "/home/someone/Videos", filename_template: "%(title)s.%(ext)s",
  player_command: "smplayer", max_concurrent_downloads: 2, poll_interval_minutes: 30,
  poll_on_startup: true, backfill_count: 30, card_size: 260,
  window_width: 1280, window_height: 880, window_x: null, window_y: null,
  window_maximized: false,
  view: {
    channel_id: null, search: "", hide_watched: false, downloaded_only: false,
    show_hidden: false, grouped: false, sort: "newest",
  },
};

const ESTIMATE: TransferEstimate = {
  channelCount: 38, videoCount: 3168, thumbCount: 3225,
  // The developer's own cache, to the byte: 112 MB is what the tick must say.
  thumbBytes: 112 * 1024 * 1024,
};

const PLAYERS: PlayerOption[] = [
  { id: "default", label: "System default", command: "" },
  { id: "mpv", label: "mpv", command: "mpv" },
  { id: "vlc", label: "VLC", command: "vlc" },
];

const CHROME_NOTE = "Chrome's cookies can't be read on Windows. Use Firefox or a cookies.txt.";
const BROWSERS: BrowserOption[] = [
  { id: "firefox", label: "Firefox", supported: true, note: null },
  { id: "chrome", label: "Chrome", supported: false, note: CHROME_NOTE },
];

const tool = (over: Partial<ToolStatus> & Pick<ToolStatus, "kind">): ToolStatus => ({
  path: null, version: null, source: "missing", state: "ready", error: null, lastCheck: null,
  ...over,
});

const TOOLS_OK: ToolStatus[] = [
  tool({
    kind: "ytdlp", source: "managed", version: "2026.09.20",
    path: "/home/someone/.local/share/mytube/bin/yt-dlp", lastCheck: 1_790_000_000,
  }),
  tool({ kind: "ffmpeg", source: "system", version: "7.1", path: "/usr/bin/ffmpeg" }),
  tool({ kind: "deno", source: "override", version: "2.5.1", path: "/opt/deno/deno" }),
];

const TOOLS_BROKEN: ToolStatus[] = [
  TOOLS_OK[0],
  TOOLS_OK[1],
  tool({ kind: "deno", source: "missing", state: "error", error: "checksum mismatch for deno" }),
];

let settings: Settings = SETTINGS;
let players: PlayerOption[] = PLAYERS;
let browsers: BrowserOption[] = BROWSERS;
let tools: ToolStatus[] = TOOLS_OK;

const saveSettings = vi.fn<(s: Settings) => Promise<void>>(() => Promise.resolve());
const exportConfig = vi.fn<(p: string, t: boolean) => Promise<void>>(() => Promise.resolve());
const transferEstimate = vi.fn<() => Promise<TransferEstimate>>(() => Promise.resolve(ESTIMATE));
const toolsUpdateNow = vi.fn<() => Promise<ToolStatus[]>>(() => Promise.resolve(TOOLS_OK));

vi.mock("../api", () => ({
  api: {
    getSettings: () => Promise.resolve(settings),
    saveSettings: (s: Settings) => saveSettings(s),
    transferEstimate: () => transferEstimate(),
    exportConfig: (p: string, t: boolean) => exportConfig(p, t),
    readArchive: () => Promise.resolve(null),
    importConfig: () => Promise.resolve(null),
    detectPlayers: () => Promise.resolve(players),
    detectBrowsers: () => Promise.resolve(browsers),
    toolsStatus: () => Promise.resolve(tools),
    toolsUpdateNow: () => toolsUpdateNow(),
  },
  errText: (e: unknown) => String(e),
}));

import SettingsView from "./SettingsView";
import { toolsReadyMessage } from "./ToolsSection";
import { ToastProvider } from "./Toast";

async function renderSettings() {
  render(
    <ToastProvider>
      <SettingsView />
    </ToastProvider>,
  );
  return await screen.findByLabelText(/Include thumbnails/) as HTMLInputElement;
}

beforeEach(() => {
  saveSettings.mockClear();
  exportConfig.mockClear();
  save.mockClear();
  openDialog.mockClear();
  transferEstimate.mockClear();
  toolsUpdateNow.mockClear();
  toolsUpdateNow.mockImplementation(() => Promise.resolve(TOOLS_OK));
  settings = SETTINGS;
  players = PLAYERS;
  browsers = BROWSERS;
  tools = TOOLS_OK;
});

afterEach(() => cleanup());

const lastSaved = () => saveSettings.mock.calls[saveSettings.mock.calls.length - 1][0];

/** The player select, once detection has filled it. */
async function playerSelect() {
  await renderSettings();
  const select = screen.getByLabelText("Player") as HTMLSelectElement;
  await waitFor(() => expect(select.disabled).toBe(false));
  return select;
}

async function cookiesSelect() {
  await renderSettings();
  const select = screen.getByLabelText("YouTube cookies") as HTMLSelectElement;
  await waitFor(() => expect(select.querySelector('option[value="firefox"]')).not.toBeNull());
  return select;
}

const optionTexts = (select: HTMLSelectElement) =>
  [...select.options].map((o) => o.textContent);

describe("Player", () => {
  it("offers every detected player, and Custom for a command none of them is", async () => {
    const select = await playerSelect();
    expect(optionTexts(select)).toEqual(["System default", "mpv", "VLC", "Custom command…"]);
    expect(select.value).toBe("__custom");
    const custom = screen.getByLabelText("Player command") as HTMLInputElement;
    expect(custom.value).toBe("smplayer");
  });

  it("shows a detected player as itself, with no text field", async () => {
    settings = { ...SETTINGS, player_command: "vlc" };
    const select = await playerSelect();
    expect(select.value).toBe("vlc");
    expect(screen.queryByLabelText("Player command")).toBeNull();
  });

  it("reads an empty command as the system default", async () => {
    settings = { ...SETTINGS, player_command: "" };
    const select = await playerSelect();
    expect(select.value).toBe("default");
  });

  it("commits a detected player the moment it is chosen", async () => {
    const select = await playerSelect();
    fireEvent.change(select, { target: { value: "mpv" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().player_command).toBe("mpv");
    expect(screen.queryByLabelText("Player command")).toBeNull();
  });

  it("keeps a custom command on blur, like every other text field", async () => {
    const select = await playerSelect();
    expect(select.value).toBe("__custom");
    const custom = screen.getByLabelText("Player command") as HTMLInputElement;
    fireEvent.change(custom, { target: { value: "mpv --fullscreen" } });
    expect(saveSettings).not.toHaveBeenCalled();
    fireEvent.blur(custom);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().player_command).toBe("mpv --fullscreen");
  });

  /** Choosing Custom over a detected player must open the field without
   *  snapping straight back to the player whose command it still holds. */
  it("opens the text field when Custom is chosen over a detected player", async () => {
    settings = { ...SETTINGS, player_command: "vlc" };
    const select = await playerSelect();
    fireEvent.change(select, { target: { value: "__custom" } });
    expect(select.value).toBe("__custom");
    expect((screen.getByLabelText("Player command") as HTMLInputElement).value).toBe("vlc");
    expect(saveSettings).not.toHaveBeenCalled();
  });
});

describe("YouTube cookies", () => {
  it("names what Automatic resolves to", async () => {
    const select = await cookiesSelect();
    expect(optionTexts(select)[0]).toBe("Automatic (Firefox)");
  });

  it("says Automatic finds nothing on a machine with no Firefox", async () => {
    browsers = [BROWSERS[1]];
    await renderSettings();
    const select = screen.getByLabelText("YouTube cookies") as HTMLSelectElement;
    await waitFor(() => expect(optionTexts(select)[0]).toBe("Automatic (none found)"));
  });

  it("disables a browser yt-dlp cannot read here and says why", async () => {
    const select = await cookiesSelect();
    const chrome = select.querySelector('option[value="chrome"]') as HTMLOptionElement;
    expect(chrome.disabled).toBe(true);
    expect(screen.getByText(CHROME_NOTE, { exact: false })).toBeDefined();
    const firefox = select.querySelector('option[value="firefox"]') as HTMLOptionElement;
    expect(firefox.disabled).toBe(false);
  });

  it("shows the cookies.txt file when one is set, since it wins", async () => {
    settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.txt" };
    const select = await cookiesSelect();
    expect(select.value).toBe("__file");
    expect(screen.getByText("/home/someone/cookies.txt")).toBeDefined();
  });

  it("clears the cookies.txt file when a browser is chosen", async () => {
    settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.txt" };
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "firefox" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().cookies_browser).toBe("firefox");
    expect(lastSaved().cookies_file).toBe("");
  });

  it("stores None as an empty browser", async () => {
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().cookies_browser).toBe("");
  });

  it("picks a cookies.txt through the file dialog", async () => {
    openDialog.mockResolvedValueOnce("/home/someone/yt-cookies.txt");
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "__file" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const opts = openDialog.mock.calls[0][0] as { filters: { extensions: string[] }[] };
    expect(opts.filters[0].extensions).toEqual(["txt"]);
    expect(lastSaved().cookies_file).toBe("/home/someone/yt-cookies.txt");
  });

  it("changes nothing when the file dialog is dismissed", async () => {
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "__file" } });
    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(saveSettings).not.toHaveBeenCalled();
    expect(select.value).toBe("auto");
  });

  /** A hand-written spec like `chrome:Profile 1` is legal and must not be
   *  shown as something else, or silently rewritten on the next save. */
  it("keeps a hand-written browser spec on screen as itself", async () => {
    settings = { ...SETTINGS, cookies_browser: "firefox:work" };
    const select = await cookiesSelect();
    expect(select.value).toBe("firefox:work");
  });
});

describe("Tools", () => {
  it("names where each tool comes from", async () => {
    await renderSettings();
    expect(await screen.findByText("Managed by MyTube")).toBeDefined();
    expect(screen.getByText("System")).toBeDefined();
    expect(screen.getByText("Custom path (settings.json)")).toBeDefined();
    expect(screen.getByText("2026.09.20")).toBeDefined();
    expect(screen.getByText("/usr/bin/ffmpeg")).toBeDefined();
  });

  it("shows a failed install's error and offers Retry only then", async () => {
    tools = TOOLS_BROKEN;
    await renderSettings();
    expect(await screen.findByText("Not installed")).toBeDefined();
    expect(screen.getByText("checksum mismatch for deno")).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(toolsUpdateNow).toHaveBeenCalledTimes(1));
    // The fresh status the command answered with replaces the broken row.
    await waitFor(() => expect(screen.queryByText("checksum mismatch for deno")).toBeNull());
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("has no Retry while every tool is fine", async () => {
    await renderSettings();
    await screen.findByText("Managed by MyTube");
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("checks for updates and reports a refusal", async () => {
    toolsUpdateNow.mockImplementationOnce(() =>
      Promise.reject("busy: a download is using yt-dlp"),
    );
    await renderSettings();
    await screen.findByText("Managed by MyTube");
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByText("busy: a download is using yt-dlp")).toBeDefined();
  });

  it("saves the update channel and the auto-update switch", async () => {
    await renderSettings();
    const channel = screen.getByLabelText("yt-dlp channel") as HTMLSelectElement;
    expect(channel.value).toBe("nightly");
    fireEvent.change(channel, { target: { value: "stable" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().ytdlp_channel).toBe("stable");

    const auto = screen.getByLabelText(/Update yt-dlp automatically/) as HTMLInputElement;
    expect(auto.checked).toBe(true);
    fireEvent.click(auto);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(2));
    expect(lastSaved().ytdlp_auto_update).toBe(false);
  });
});

/**
 * No control writes the three path overrides, so the only way to lose one is a
 * save that rebuilt the object from the fields it knows about.
 */
it("carries hand-edited tool paths through a save untouched", async () => {
  settings = {
    ...SETTINGS, ytdlp_path: "/opt/yt-dlp", ffmpeg_path: "C:\\ffmpeg\\bin\\ffmpeg.exe",
    deno_path: "/opt/deno",
  };
  const select = await playerSelect();
  fireEvent.change(select, { target: { value: "mpv" } });
  await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
  expect(lastSaved()).toMatchObject({
    ytdlp_path: "/opt/yt-dlp", ffmpeg_path: "C:\\ffmpeg\\bin\\ffmpeg.exe", deno_path: "/opt/deno",
  });
});

describe("toolsReadyMessage", () => {
  const states = (...pairs: [ToolStatus["kind"], ToolStatus["state"]][]) =>
    new Map(pairs);

  it("announces one tool that finished installing", () => {
    const next = [tool({ kind: "ytdlp", state: "ready" })];
    expect(toolsReadyMessage(states(["ytdlp", "installing"]), next)).toBe("yt-dlp is ready.");
  });

  it("lists several in one sentence", () => {
    const next = [
      tool({ kind: "ytdlp", state: "ready" }),
      tool({ kind: "ffmpeg", state: "ready" }),
      tool({ kind: "deno", state: "ready" }),
    ];
    const prev = states(["ytdlp", "installing"], ["ffmpeg", "installing"], ["deno", "installing"]);
    expect(toolsReadyMessage(prev, next)).toBe("yt-dlp, ffmpeg and deno are ready.");
  });

  /** An update is housekeeping the user never asked to hear about; only a
   *  first install changes what the app can do. */
  it("stays quiet for an update, a failure, and a tool already ready", () => {
    const next = [
      tool({ kind: "ytdlp", state: "ready" }),
      tool({ kind: "ffmpeg", state: "error" }),
      tool({ kind: "deno", state: "ready" }),
    ];
    const prev = states(["ytdlp", "updating"], ["ffmpeg", "installing"], ["deno", "ready"]);
    expect(toolsReadyMessage(prev, next)).toBeNull();
  });
});

describe("Backup & transfer", () => {
  it("quotes a measured size on the tick rather than a guess", async () => {
    await renderSettings();
    expect(screen.getByText("Include thumbnails (112 MB)")).toBeDefined();
    cleanup();
  });

  /**
   * The tick is a choice about one archive, not a preference: routing it
   * through `commit()` would write settings.json every time it was clicked and
   * carry the choice into every later export.
   */
  it("does not persist the thumbnails tick as a setting", async () => {
    const tick = await renderSettings();
    fireEvent.click(tick);
    expect(tick.checked).toBe(false);
    expect(saveSettings).not.toHaveBeenCalled();
    cleanup();
  });

  it("writes to the file the user names, honouring the tick", async () => {
    const tick = await renderSettings();
    fireEvent.click(tick);
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    await waitFor(() =>
      expect(exportConfig).toHaveBeenCalledWith(
        "/home/someone/mytube-export-2026-09-22.zip", false,
      ),
    );
    const opts = save.mock.calls[0][0] as { defaultPath: string };
    expect(opts.defaultPath).toMatch(/^mytube-export-\d{4}-\d{2}-\d{2}\.zip$/);
    cleanup();
  });

  it("exports nothing when the save dialog is dismissed", async () => {
    save.mockResolvedValueOnce(null);
    await renderSettings();
    fireEvent.click(screen.getByRole("button", { name: "Export…" }));
    await waitFor(() => expect(save).toHaveBeenCalled());
    expect(exportConfig).not.toHaveBeenCalled();
    cleanup();
  });
});
