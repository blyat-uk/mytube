import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import type {
  BrowserOption, BrowserScan, CookiesFileInfo, PlayerOption, Settings, ToolStatus,
  TransferEstimate,
} from "../types";
import { DEFAULT_QUALITY } from "../quality";

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
  cookies_browser: "auto", cookies_file: "", ytdlp_channel: "nightly", ytdlp_auto_update: true,
  check_app_updates: true,
  ytdlp_path: "", ffmpeg_path: "", deno_path: "",
  quality: { ...DEFAULT_QUALITY },
  view: {
    channel_id: null, search: "", hide_watched: false, downloaded_only: false,
    show_hidden: false, grouped: false, groups_only: false, in_progress: false, sort: "newest",
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

const CHROME_NOTE = "yt-dlp cannot read this browser's cookies on Windows. Export a cookies file.";
const browserOpt = (over: Partial<BrowserOption> & Pick<BrowserOption, "id" | "label">) => ({
  supported: true, blocked: false, signedIn: false, note: null, ...over,
});
const BROWSERS: BrowserOption[] = [
  browserOpt({ id: "chrome", label: "Chrome", supported: false, signedIn: null, note: CHROME_NOTE }),
  browserOpt({ id: "firefox", label: "Firefox" }),
];
const SCAN: BrowserScan = { browsers: BROWSERS, automatic: "firefox", accessHint: null };
const FILE_INFO: CookiesFileInfo = {
  format: "JSON cookie export", cookies: 24, skipped: 0, youtube: true, signedIn: true,
};

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
let scan: BrowserScan = SCAN;
let tools: ToolStatus[] = TOOLS_OK;

const saveSettings = vi.fn<(s: Settings) => Promise<void>>(() => Promise.resolve());
const exportConfig = vi.fn<(p: string, t: boolean) => Promise<void>>(() => Promise.resolve());
const transferEstimate = vi.fn<() => Promise<TransferEstimate>>(() => Promise.resolve(ESTIMATE));
const toolsUpdateNow = vi.fn<() => Promise<ToolStatus[]>>(() => Promise.resolve(TOOLS_OK));
const inspectCookiesFile = vi.fn<(path: string) => Promise<CookiesFileInfo>>(
  () => Promise.resolve(FILE_INFO),
);

vi.mock("../api", () => ({
  api: {
    getSettings: () => Promise.resolve(settings),
    saveSettings: (s: Settings) => saveSettings(s),
    transferEstimate: () => transferEstimate(),
    exportConfig: (p: string, t: boolean) => exportConfig(p, t),
    readArchive: () => Promise.resolve(null),
    importConfig: () => Promise.resolve(null),
    detectPlayers: () => Promise.resolve(players),
    detectBrowsers: () => Promise.resolve(scan),
    inspectCookiesFile: (path: string) => inspectCookiesFile(path),
    toolsStatus: () => Promise.resolve(tools),
    toolsUpdateNow: () => toolsUpdateNow(),
    appVersionInfo: () => Promise.resolve({
      current: "2.1.0", update: null, checkedAt: null, checksEnabled: true,
    }),
    checkAppUpdateNow: () => Promise.resolve({
      current: "2.1.0", update: null, checkedAt: 1_790_000_000, checksEnabled: true,
    }),
    openExternal: () => Promise.resolve(),
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
  scan = SCAN;
  tools = TOOLS_OK;
  inspectCookiesFile.mockReset();
  inspectCookiesFile.mockImplementation(() => Promise.resolve(FILE_INFO));
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

describe("Download quality", () => {
  const pickOption = (select: HTMLSelectElement, text: string) => {
    const i = optionTexts(select).indexOf(text);
    fireEvent.change(select, { target: { value: String(i) } });
  };

  it("shows the saved preferences", async () => {
    settings = { ...SETTINGS, quality: { ...DEFAULT_QUALITY, max_height: 1080, container: "mp4" } };
    await renderSettings();
    expect(screen.getByRole("heading", { name: "Download quality" })).toBeTruthy();
    const res = screen.getByLabelText("Resolution") as HTMLSelectElement;
    expect(res.options[res.selectedIndex].text).toBe("Up to 1080p");
    const box = screen.getByLabelText("Container") as HTMLSelectElement;
    expect(box.options[box.selectedIndex].text).toBe("MP4");
  });

  it("saves a select the moment it changes, with the rest of the settings", async () => {
    await renderSettings();
    pickOption(screen.getByLabelText("Codec") as HTMLSelectElement, "AV1 (smallest files)");
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved()).toEqual({ ...SETTINGS, quality: { ...DEFAULT_QUALITY, vcodec: "av1" } });
  });

  it("saves the raw format on blur, not per keystroke", async () => {
    await renderSettings();
    const raw = screen.getByLabelText("Format (-f)");
    fireEvent.change(raw, { target: { value: "bv*[height<=720]+ba/b" } });
    expect(saveSettings).not.toHaveBeenCalled();
    expect((screen.getByLabelText("Resolution") as HTMLSelectElement).disabled).toBe(true);
    fireEvent.blur(raw);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().quality.format).toBe("bv*[height<=720]+ba/b");
  });

  it("draws the defaults for a settings object with no quality block", async () => {
    const { quality: _, ...rest } = SETTINGS;
    settings = rest as Settings;
    await renderSettings();
    const mode = screen.getByLabelText("Mode") as HTMLSelectElement;
    expect(mode.options[mode.selectedIndex].text).toBe("Video");
  });
});

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

  /** Typing that happens to spell a detected command must not swap the field
   *  for that player: the input would unmount mid-edit, never fire its blur,
   *  and the select would show a choice settings.json does not hold. */
  it.each([
    ["backspaced to a detected player", "mpv"],
    ["cleared to the system default", ""],
  ])("keeps the text field when the command is %s, and saves it on blur", async (_, typed) => {
    settings = { ...SETTINGS, player_command: "mpv --fullscreen" };
    const select = await playerSelect();
    const custom = screen.getByLabelText("Player command") as HTMLInputElement;
    fireEvent.focus(custom);
    fireEvent.change(custom, { target: { value: typed } });

    const still = screen.getByLabelText("Player command") as HTMLInputElement;
    expect(still).toBe(custom);
    expect(still.value).toBe(typed);
    expect(select.value).toBe("__custom");

    fireEvent.blur(still);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().player_command).toBe(typed);
  });

  it("closes the text field again once a player is picked from the list", async () => {
    const select = await playerSelect();
    fireEvent.change(select, { target: { value: "vlc" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(select.value).toBe("vlc");
    expect(screen.queryByLabelText("Player command")).toBeNull();
  });

  it("links the hint to the select for a screen reader", async () => {
    settings = { ...SETTINGS, player_command: "vlc" };
    const select = await playerSelect();
    const described = select.getAttribute("aria-describedby");
    expect(described).toBeTruthy();
    expect(document.getElementById(described!)?.textContent).toBe("Opens each download with vlc.");
  });
});

describe("YouTube cookies", () => {
  it("names the browser Automatic resolves to, whichever it is", async () => {
    const select = await cookiesSelect();
    expect(optionTexts(select)[0]).toBe("Automatic (Firefox)");
    cleanup();
    scan = {
      ...SCAN,
      browsers: [...BROWSERS, browserOpt({ id: "brave", label: "Brave", signedIn: true })],
      automatic: "brave",
    };
    const again = await cookiesSelect();
    expect(optionTexts(again)[0]).toBe("Automatic (Brave)");
    expect(optionTexts(again)).toContain("Brave (signed in)");
    const hint = document.getElementById(again.getAttribute("aria-describedby")!)!;
    expect(hint.textContent).toContain("Uses Brave, which is signed in to YouTube");
  });

  it("expects no browser in particular when Automatic has nothing to use", async () => {
    scan = { browsers: [BROWSERS[0]], automatic: null, accessHint: null };
    await renderSettings();
    const select = screen.getByLabelText("YouTube cookies") as HTMLSelectElement;
    await waitFor(() => expect(optionTexts(select)[0]).toBe("Automatic (none usable)"));
    cleanup();

    scan = { browsers: [], automatic: null, accessHint: null };
    await renderSettings();
    const empty = screen.getByLabelText("YouTube cookies") as HTMLSelectElement;
    await waitFor(() => expect(optionTexts(empty)[0]).toBe("Automatic (none found)"));
    const hint = document.getElementById(empty.getAttribute("aria-describedby")!)!;
    expect(hint.textContent).toMatch(/^No browser found, so downloads run signed out/);
    // Nothing on the field presumes a browser the machine does not have.
    expect(empty.closest(".field")!.textContent).not.toMatch(/Firefox|Chrome|Safari/);
  });

  it("says when the browser Automatic uses is not signed in to YouTube", async () => {
    const select = await cookiesSelect();
    const hint = document.getElementById(select.getAttribute("aria-describedby")!)!;
    expect(hint.textContent).toBe(
      "Uses Firefox, the browser used last, but none MyTube can read is signed in to YouTube, so " +
      "members-only and age-restricted videos will fail to download.",
    );
  });

  it("disables a browser yt-dlp cannot read here and says why", async () => {
    const select = await cookiesSelect();
    const chrome = select.querySelector('option[value="chrome"]') as HTMLOptionElement;
    expect(chrome.disabled).toBe(true);
    expect(chrome.textContent).toBe("Chrome (not supported here)");
    expect(screen.getByText(CHROME_NOTE, { exact: false })).toBeDefined();
    const firefox = select.querySelector('option[value="firefox"]') as HTMLOptionElement;
    expect(firefox.disabled).toBe(false);
  });

  it("marks a browser macOS keeps MyTube out of as no access, and says how to allow it", async () => {
    const hintText = "Browser missing, or marked “no access”? … Full Disk Access …";
    scan = {
      browsers: [browserOpt({
        id: "firefox", label: "Firefox", supported: false, blocked: true, signedIn: null,
        note: "macOS is not letting MyTube read it.",
      })],
      automatic: null,
      accessHint: hintText,
    };
    const select = await cookiesSelect();
    const firefox = select.querySelector('option[value="firefox"]') as HTMLOptionElement;
    expect(firefox.disabled).toBe(true);
    expect(firefox.textContent).toBe("Firefox (no access)");
    expect(screen.getByText("Firefox: macOS is not letting MyTube read it.")).toBeDefined();
    expect(screen.getByText(hintText)).toBeDefined();
  });

  it("shows the cookies file when one is set, since it wins, with what it holds", async () => {
    settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.json" };
    const select = await cookiesSelect();
    expect(select.value).toBe("__file");
    expect(screen.getByText("/home/someone/cookies.json")).toBeDefined();
    expect(inspectCookiesFile).toHaveBeenCalledWith("/home/someone/cookies.json");
    const hint = document.getElementById(select.getAttribute("aria-describedby")!)!;
    await waitFor(() => expect(hint.textContent).toBe(
      "JSON cookie export, 24 cookies, signed in to YouTube. It is used instead of any browser.",
    ));
  });

  it("says why a cookies file already set can no longer be used", async () => {
    inspectCookiesFile.mockImplementation(
      () => Promise.reject("The cookies file /home/someone/cookies.txt no longer exists."),
    );
    settings = { ...SETTINGS, cookies_browser: "auto", cookies_file: "/home/someone/cookies.txt" };
    const select = await cookiesSelect();
    const hint = document.getElementById(select.getAttribute("aria-describedby")!)!;
    await waitFor(() => expect(hint.textContent)
      .toBe("The cookies file /home/someone/cookies.txt no longer exists."));
  });

  it("clears the cookies file when a browser is chosen", async () => {
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

  it("picks a cookies file of any kind through the file dialog, checked first", async () => {
    openDialog.mockResolvedValueOnce("/home/someone/yt-cookies.json");
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "__file" } });
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    const opts = openDialog.mock.calls[0][0] as { filters?: unknown };
    expect(opts.filters).toBeUndefined();
    expect(inspectCookiesFile).toHaveBeenCalledWith("/home/someone/yt-cookies.json");
    expect(lastSaved().cookies_file).toBe("/home/someone/yt-cookies.json");
  });

  it("refuses a file it cannot read, says why, and saves nothing", async () => {
    const why = "notes.txt can't be used as a cookies file: it is not a Netscape cookies.txt, " +
      "a JSON cookie export, or a list of name=value pairs.";
    inspectCookiesFile.mockImplementation(() => Promise.reject(why));
    openDialog.mockResolvedValueOnce("/home/someone/notes.txt");
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "__file" } });
    expect(await screen.findByText(why)).toBeDefined();
    expect(saveSettings).not.toHaveBeenCalled();
    expect(select.value).toBe("auto");
  });

  it("changes nothing when the file dialog is dismissed", async () => {
    const select = await cookiesSelect();
    fireEvent.change(select, { target: { value: "__file" } });
    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(saveSettings).not.toHaveBeenCalled();
    expect(inspectCookiesFile).not.toHaveBeenCalled();
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

  /** An update check that fails leaves a managed yt-dlp working at the version
   *  it has: said quietly under the row, with no alert and no Retry. */
  it("shows a working tool's error as a note, not an alert", async () => {
    tools = [
      { ...TOOLS_OK[0], error: "Update check failed: HTTP 503" },
      TOOLS_OK[1],
      TOOLS_OK[2],
    ];
    await renderSettings();
    const note = await screen.findByText("Update check failed: HTTP 503");
    expect(note.className).toBe("tool-note");
    expect(note.closest(".tool-row")?.classList.contains("is-error")).toBe(false);
    expect(screen.queryByRole("button", { name: "Retry" })).toBeNull();
  });

  it("keeps the red alert for a tool that is actually broken", async () => {
    tools = TOOLS_BROKEN;
    await renderSettings();
    const alert = await screen.findByText("checksum mismatch for deno");
    expect(alert.className).toBe("tool-error");
    expect(alert.closest(".tool-row")?.classList.contains("is-error")).toBe(true);
  });

  it("announces an update to a yt-dlp that was already managed", async () => {
    tools = [{ ...TOOLS_OK[0], version: "2026.09.10" }, TOOLS_OK[1], TOOLS_OK[2]];
    await renderSettings();
    await screen.findByText("2026.09.10");
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    expect(await screen.findByText("yt-dlp updated to 2026.09.20.")).toBeDefined();
  });

  /** A system yt-dlp is replaced by a managed install, which the shell already
   *  toasts as "yt-dlp is ready." — saying "updated to" as well is one event
   *  told twice. */
  it("says nothing of its own when the check installs a managed yt-dlp", async () => {
    tools = [
      tool({ kind: "ytdlp", source: "system", version: "2025.01.01", path: "/usr/bin/yt-dlp" }),
      TOOLS_OK[1],
      TOOLS_OK[2],
    ];
    await renderSettings();
    await screen.findByText("2025.01.01");
    fireEvent.click(screen.getByRole("button", { name: "Check for updates" }));
    await waitFor(() => expect(toolsUpdateNow).toHaveBeenCalledTimes(1));
    // The row taking the answer is the sign the call has fully settled.
    expect(await screen.findByText("Managed by MyTube")).toBeDefined();
    expect(screen.queryByText(/yt-dlp updated to/)).toBeNull();
    expect(screen.queryByText("yt-dlp is up to date.")).toBeNull();
  });

  it("heads the section with a real heading", async () => {
    await renderSettings();
    expect(screen.getByRole("heading", { name: "Tools" })).toBeDefined();
    expect(screen.getByRole("region", { name: "Tools" })).toBeDefined();
    expect(screen.getByRole("heading", { name: "Backup & transfer" })).toBeDefined();
  });

  it("links the update channel's hint to its select", async () => {
    await renderSettings();
    const channel = screen.getByLabelText("yt-dlp channel") as HTMLSelectElement;
    const described = channel.getAttribute("aria-describedby");
    expect(document.getElementById(described!)?.textContent).toMatch(/reach nightly first/);
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

describe("checking for MyTube releases", () => {
  it("is on by default and saves when switched off", async () => {
    await renderSettings();
    const check = screen.getByLabelText(/Check GitHub for new MyTube releases/) as HTMLInputElement;
    expect(check.checked).toBe(true);
    fireEvent.click(check);
    await waitFor(() => expect(saveSettings).toHaveBeenCalledTimes(1));
    expect(lastSaved().check_app_updates).toBe(false);
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
