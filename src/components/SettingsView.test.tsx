import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, render, screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./settings/testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./settings/testApi")).dialogModule);
vi.mock("../api", async () => (await import("./settings/testApi")).apiModule);

import {
  api, backend, emit, lastSaved, resetHarness, SYNC_OFF, TOOLS_BROKEN, VERSION,
} from "./settings/testApi";
import { railItem, renderSettings } from "./settings/testRender";
import SettingsView, { openSettingsSection } from "./SettingsView";
import { ToastProvider } from "./Toast";

beforeEach(resetHarness);
afterEach(cleanup);

const lineOf = (label: string) =>
  document.getElementById(railItem(label).getAttribute("aria-describedby")!)!.textContent;

describe("the rail", () => {
  it("lists every section in order of importance, under its tier", async () => {
    await renderSettings();
    const nav = screen.getByRole("navigation", { name: "Settings sections" });
    const names = within(nav).getAllByRole("button").map((b) => b.getAttribute("aria-label"));
    expect(names).toEqual([
      "Downloads", "YouTube sign-in", "Player", "New videos", "Sync", "Backup & transfer",
      "Appearance", "Tools", "About",
    ]);
    expect(nav.textContent).toMatch(/Essentials.*Your library.*Across computers.*Under the hood/);
  });

  it("opens Downloads first and marks it as the current page", async () => {
    await renderSettings();
    expect(railItem("Downloads").getAttribute("aria-current")).toBe("page");
    expect(screen.getByRole("heading", { level: 2, name: "Downloads" })).toBeTruthy();
  });

  it("switches the page when another section is picked", async () => {
    await renderSettings();
    fireEvent.click(railItem("Tools"));
    expect(screen.getByRole("heading", { level: 2, name: "Tools" })).toBeTruthy();
    expect(railItem("Tools").getAttribute("aria-current")).toBe("page");
    expect(railItem("Downloads").getAttribute("aria-current")).toBeNull();
  });

  it("reopens the section left open, after leaving the view", async () => {
    const first = render(<ToastProvider><SettingsView /></ToastProvider>);
    fireEvent.click(await screen.findByRole("button", { name: "Sync" }));
    first.unmount();
    render(<ToastProvider><SettingsView /></ToastProvider>);
    await waitFor(() => expect(railItem("Sync").getAttribute("aria-current")).toBe("page"));
    fireEvent.click(railItem("Downloads"));
  });

  it("walks the entries with the arrow keys", async () => {
    await renderSettings();
    railItem("Downloads").focus();
    fireEvent.keyDown(railItem("Downloads"), { key: "ArrowDown" });
    expect(document.activeElement).toBe(railItem("YouTube sign-in"));
    fireEvent.keyDown(document.activeElement!, { key: "End" });
    expect(document.activeElement).toBe(railItem("About"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(railItem("Downloads"));
  });

  it("reports each section's state on its second line", async () => {
    await renderSettings();
    await waitFor(() => expect(lineOf("Player")).toBe("smplayer"));
    expect(lineOf("Downloads")).toBe("Best · MKV · 2 at once");
    expect(lineOf("New videos")).toBe("Every 30 min · 30 back");
    await waitFor(() => expect(lineOf("Backup & transfer")).toBe("38 channels · 3,168 videos"));
    await waitFor(() => expect(lineOf("Tools")).toBe("yt-dlp, ffmpeg, deno · ready"));
    await waitFor(() => expect(lineOf("Sync")).toBe("Off"));
    await waitFor(() => expect(lineOf("About")).toBe("MyTube 2.1.0"));
    await waitFor(() => expect(lineOf("YouTube sign-in")).toMatch(/Firefox · not signed in/));
  });

  it("marks a section that needs the user, and follows the live state", async () => {
    backend.tools = TOOLS_BROKEN;
    await renderSettings();
    await waitFor(() => expect(railItem("Tools").className).toContain("is-error"));
    expect(lineOf("Tools")).toMatch(/deno failed to install/);
    await emit("tools://status", backend.tools.map((t) => ({ ...t, state: "ready", error: null })));
    await waitFor(() => expect(railItem("Tools").className).not.toContain("is-error"));
  });

  it("flags a newer release on About only while checks are on", async () => {
    backend.version = { ...VERSION, update: { version: "2.2.0", url: "https://example.org" } };
    await renderSettings();
    await waitFor(() => expect(lineOf("About")).toMatch(/2\.2\.0 is available/));
    await emit("app://version-info", { ...backend.version, checksEnabled: false });
    await waitFor(() => expect(lineOf("About")).toBe("MyTube 2.1.0"));
  });

  it("follows sync://status", async () => {
    await renderSettings();
    await waitFor(() => expect(lineOf("Sync")).toBe("Off"));
    await emit("sync://status", {
      ...SYNC_OFF, enabled: true, code: "MYT1-x", peers: [{ name: "laptop", lastWriteAt: null }],
    });
    expect(lineOf("Sync")).toBe("On · laptop");
  });
});

describe("loading", () => {
  it("says so when settings.json cannot be read", async () => {
    api.getSettings.mockImplementation(() => Promise.reject("nope"));
    render(<ToastProvider><SettingsView /></ToastProvider>);
    expect(await screen.findByText("Settings unavailable")).toBeTruthy();
  });
});

describe("saving", () => {
  it("sends one save at a time, so the newest value is the one on disk", async () => {
    // The first write stays out until the test lets it go.
    let release!: () => void;
    api.saveSettings.mockImplementationOnce(() => new Promise<void>((r) => { release = r; }));
    await renderSettings("downloads");
    const more = screen.getByRole("button", { name: "Increase concurrent downloads" });
    fireEvent.click(more);
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(1));
    fireEvent.click(more);
    fireEvent.click(more);
    // Nothing overtakes the write in flight...
    expect(api.saveSettings).toHaveBeenCalledTimes(1);
    expect(lastSaved().max_concurrent_downloads).toBe(3);
    await act(async () => { release(); });
    // ...and what follows it is the newest value, once.
    await waitFor(() => expect(api.saveSettings).toHaveBeenCalledTimes(2));
    expect(lastSaved().max_concurrent_downloads).toBe(5);
    await waitFor(() => expect(screen.getByText("Saved")).toBeTruthy());
  });
});

describe("switching sections", () => {
  it("keeps a page's work in flight while another section is looked at", async () => {
    // Turn on sync waits on the check window; the wait must survive a look at Tools.
    api.syncTurnOn.mockImplementation(() => new Promise(() => {}));
    await renderSettings("sync");
    const turnOn = await screen.findByRole("button", { name: /Turn on sync/ });
    fireEvent.click(turnOn);
    await waitFor(() => expect((turnOn as HTMLButtonElement).disabled).toBe(true));
    fireEvent.click(railItem("Tools"));
    expect(screen.getByRole("heading", { level: 2, name: "Tools" })).toBeTruthy();
    expect(screen.queryByRole("heading", { level: 2, name: "Sync" })).toBeNull();
    fireEvent.click(railItem("Sync"));
    expect((screen.getByRole("button", { name: /Turn on sync/ }) as HTMLButtonElement).disabled).toBe(true);
    expect(api.syncTurnOn).toHaveBeenCalledTimes(1);
  });
});

/**
 * The nav's update chip, the update toast and the tray all open Settings at
 * About, whether or not Settings is the tab on screen. The ask is written as the
 * section to show, so once seen it is simply where you are: a later visit lands
 * wherever you went next, never back on About.
 */
describe("opening a section from outside", () => {
  const settingsView = () => render(<ToastProvider><SettingsView /></ToastProvider>);
  const isOpen = (label: string) =>
    waitFor(() => expect(railItem(label).getAttribute("aria-current")).toBe("page"));

  it("opens it when Settings mounts next", async () => {
    openSettingsSection("about");
    settingsView();
    await isOpen("About");
    expect(screen.getByRole("heading", { level: 2, name: "About" })).toBeTruthy();
  });

  it("switches to it at once when Settings is already showing", async () => {
    await renderSettings("downloads");
    act(() => openSettingsSection("tools"));
    await isOpen("Tools");
    expect(screen.getByRole("heading", { level: 2, name: "Tools" })).toBeTruthy();
    expect(railItem("Downloads").getAttribute("aria-current")).toBeNull();
  });

  it("is not applied again on a later visit", async () => {
    openSettingsSection("about");
    const first = settingsView();
    await isOpen("About");
    fireEvent.click(railItem("Sync"));
    first.unmount();
    settingsView();
    await isOpen("Sync");
    expect(railItem("About").getAttribute("aria-current")).toBeNull();
  });
});
