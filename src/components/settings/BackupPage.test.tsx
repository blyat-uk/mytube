import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import {
  api, backend, dialog, emit, ESTIMATE, resetHarness, SETTINGS,
} from "./testApi";
import { railItem, renderSettings } from "./testRender";
import { summary, todayStamp } from "./BackupPage";
import type { SettingsCtx } from "./context";
import type { ArchiveSummary, ImportReport } from "../../types";

beforeEach(resetHarness);
afterEach(cleanup);

const ARCHIVE_PATH = "/home/someone/mytube-export-2026-09-20.zip";

const ARCHIVE: ArchiveSummary = {
  format: 1, appVersion: "4.0.0", exportedAt: 1_790_000_000, exportedFrom: "workshop",
  includesThumbs: true, thumbCount: 40,
  downloadDir: "/home/someone/Videos", downloadDirExists: true,
  videoCount: 41, localOnlyChannels: 0, localOnlyVideos: 0,
  channels: [
    { channelId: "UCa", title: "Alpha", videoCount: 12, subscribed: true, member: false, alreadyHere: false },
    { channelId: "UCb", title: "Beta", videoCount: 29, subscribed: true, member: false, alreadyHere: true },
  ],
};

const report = (over: Partial<ImportReport> = {}): ImportReport => ({
  channelsAdded: 2, channelsUpdated: 0, videosAdded: 41, videosUpdated: 0,
  channelsRemoved: 0, videosRemoved: 0, downloadsRelinked: 0, thumbsWritten: 40,
  settingsApplied: false, downloadDirKept: false,
  ...over,
});

/** The Backup page, once the estimate has put a size on the tick. Returns the tick. */
async function renderPage() {
  await renderSettings("backup");
  return await screen.findByLabelText(/Include thumbnails/) as HTMLInputElement;
}

const exportButton = () => screen.getByRole("button", { name: "Export…" }) as HTMLButtonElement;
const importButton = () => screen.getByRole("button", { name: "Import…" }) as HTMLButtonElement;

/** Picks the archive and waits for its checklist. */
async function openArchive() {
  dialog.open.mockResolvedValueOnce(ARCHIVE_PATH);
  api.readArchive.mockResolvedValueOnce(ARCHIVE);
  fireEvent.click(importButton());
  return await screen.findByRole("dialog", { name: "Import from a MyTube archive" });
}

const toastTexts = () => [...document.querySelectorAll(".toast")].map((t) => t.textContent ?? "");
const toasted = (text: string) => waitFor(() => expect(toastTexts().join("\n")).toContain(text));

describe("Backup & transfer · export", () => {
  it("quotes a measured size on the tick rather than a guess", async () => {
    await renderPage();
    await waitFor(() => expect(screen.getByText("Include thumbnails (112 MB)")).toBeDefined());
  });

  it("puts no size on the tick while the library is still being weighed", async () => {
    api.transferEstimate.mockImplementation(() => new Promise(() => {}));
    await renderPage();
    expect(screen.getByText("Include thumbnails")).toBeDefined();
  });

  /**
   * The tick is a choice about one archive, not a preference: routing it
   * through a save would write settings.json every time it was clicked and
   * carry the choice into every later export.
   */
  it("does not persist the thumbnails tick as a setting", async () => {
    const tick = await renderPage();
    expect(tick.checked).toBe(true);
    fireEvent.click(tick);
    expect(tick.checked).toBe(false);
    await waitFor(() => {});
    expect(api.saveSettings).not.toHaveBeenCalled();
  });

  it("writes to the file the user names, honouring the tick", async () => {
    const tick = await renderPage();
    fireEvent.click(tick);
    fireEvent.click(exportButton());
    await waitFor(() =>
      expect(api.exportConfig).toHaveBeenCalledWith(
        "/home/someone/mytube-export-2026-09-22.zip", false,
      ),
    );
    const opts = dialog.save.mock.calls[0][0] as { defaultPath: string };
    expect(opts.defaultPath).toMatch(/^mytube-export-\d{4}-\d{2}-\d{2}\.zip$/);
    expect(opts.defaultPath).toBe(`mytube-export-${todayStamp()}.zip`);
    await toasted("Exported to /home/someone/mytube-export-2026-09-22.zip.");
  });

  it("exports nothing when the save dialog is dismissed", async () => {
    dialog.save.mockResolvedValueOnce(null);
    await renderPage();
    fireEvent.click(exportButton());
    await waitFor(() => expect(dialog.save).toHaveBeenCalled());
    await waitFor(() => expect(exportButton().disabled).toBe(false));
    expect(api.exportConfig).not.toHaveBeenCalled();
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("shows the export's progress, and only the export's, while it writes", async () => {
    let finish!: () => void;
    api.exportConfig.mockImplementation(() => new Promise<void>((r) => { finish = r; }));
    const tick = await renderPage();
    fireEvent.click(exportButton());

    const bar = await screen.findByRole("progressbar", { name: "Export" });
    expect(screen.getByText("Starting…")).toBeTruthy();
    // Both directions are shut while one runs, and the tick with them.
    expect(exportButton().disabled).toBe(true);
    expect(importButton().disabled).toBe(true);
    expect(tick.disabled).toBe(true);

    await emit("transfer://progress", { phase: "import", done: 9, total: 10, current: "nope" });
    expect(screen.getByText("Starting…")).toBeTruthy();
    await emit("transfer://progress", { phase: "export", done: 3, total: 12, current: "thumbs/abc.jpg" });
    expect(screen.getByText("3 of 12 · thumbs/abc.jpg")).toBeTruthy();
    expect(bar.getAttribute("aria-valuenow")).toBe("25");

    finish();
    await waitFor(() => expect(screen.queryByRole("progressbar")).toBeNull());
    expect(exportButton().disabled).toBe(false);
    expect(importButton().disabled).toBe(false);
  });

  it("says why an export failed", async () => {
    api.exportConfig.mockImplementation(() => Promise.reject("disk full"));
    await renderPage();
    fireEvent.click(exportButton());
    await toasted("disk full");
    expect(exportButton().disabled).toBe(false);
    expect(screen.queryByRole("progressbar")).toBeNull();
  });

  it("names the library's size on the card", async () => {
    await renderPage();
    // The rail says the same on its second line; this is the card's own.
    expect(await screen.findByText("38 channels · 3,168 videos", { selector: "p" })).toBeTruthy();
  });
});

describe("Backup & transfer · import", () => {
  it("reads the chosen archive and shows its checklist, writing nothing yet", async () => {
    await renderPage();
    const dlg = await openArchive();
    expect(api.readArchive).toHaveBeenCalledWith(ARCHIVE_PATH);
    expect(within(dlg).getByText("Alpha")).toBeTruthy();
    expect(api.importConfig).not.toHaveBeenCalled();
    const opts = dialog.open.mock.calls[0][0] as { title: string; directory: boolean };
    expect(opts.title).toBe("Choose a MyTube export");
    expect(opts.directory).toBe(false);
  });

  it("reads nothing when the file chooser is dismissed", async () => {
    await renderPage();
    fireEvent.click(importButton());
    await waitFor(() => expect(dialog.open).toHaveBeenCalled());
    await waitFor(() => expect(importButton().disabled).toBe(false));
    expect(api.readArchive).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("says why an archive could not be read, and opens no checklist", async () => {
    dialog.open.mockResolvedValueOnce(ARCHIVE_PATH);
    api.readArchive.mockImplementation(() => Promise.reject("not a MyTube export"));
    await renderPage();
    fireEvent.click(importButton());
    await toasted("not a MyTube export");
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("imports the ticked channels and says what came in", async () => {
    api.importConfig.mockResolvedValue(report({ downloadsRelinked: 3 }));
    await renderPage();
    const dlg = await openArchive();
    fireEvent.click(within(dlg).getByRole("button", { name: "Import 2 channels" }));
    await waitFor(() =>
      expect(api.importConfig).toHaveBeenCalledWith(ARCHIVE_PATH, ["UCa", "UCb"], "merge", true),
    );
    await toasted("Imported 2 channels and 41 videos, and found 3 already downloaded.");
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
  });

  it("re-reads settings after an import that applied them, and weighs the library again", async () => {
    api.importConfig.mockResolvedValue(report({ settingsApplied: true }));
    await renderPage();
    const dlg = await openArchive();
    expect(api.getSettings).toHaveBeenCalledTimes(1);
    expect(api.transferEstimate).toHaveBeenCalledTimes(1);
    backend.settings = { ...SETTINGS, poll_interval_minutes: 120 };
    backend.estimate = { ...ESTIMATE, channelCount: 40, videoCount: 3209 };
    fireEvent.click(within(dlg).getByRole("button", { name: "Import 2 channels" }));

    await waitFor(() => expect(api.getSettings).toHaveBeenCalledTimes(2));
    await waitFor(() => expect(api.transferEstimate).toHaveBeenCalledTimes(2));
    // The shell now holds what the import wrote: the rail says so.
    const line = (label: string) =>
      document.getElementById(railItem(label).getAttribute("aria-describedby")!)!.textContent;
    await waitFor(() => expect(line("New videos")).toBe("Every 2 h · 30 back"));
    await waitFor(() => expect(line("Backup & transfer")).toBe("40 channels · 3,209 videos"));
    expect(screen.getByText("40 channels · 3,209 videos", { selector: "p" })).toBeTruthy();
  });

  it("leaves settings alone after an import that did not apply them", async () => {
    api.importConfig.mockResolvedValue(report());
    await renderPage();
    const dlg = await openArchive();
    fireEvent.click(within(dlg).getByRole("button", { name: "Import 2 channels" }));
    await waitFor(() => expect(api.transferEstimate).toHaveBeenCalledTimes(2));
    await toasted("Imported 2 channels and 41 videos.");
    expect(api.getSettings).toHaveBeenCalledTimes(1);
  });

  it("says twice what a Replace removed, and that the download folder was kept", async () => {
    api.importConfig.mockResolvedValue(report({
      channelsRemoved: 1, videosRemoved: 312, downloadDirKept: true,
    }));
    await renderPage();
    const dlg = await openArchive();
    fireEvent.click(within(dlg).getByRole("button", { name: "Import 2 channels" }));
    await toasted("Removed 312 video rows and 1 channel from the library. No files were deleted.");
    await toasted("Kept this machine's download folder; the archive's is not here.");
  });

  it("keeps the checklist open after a failed import, so it can be tried again", async () => {
    api.importConfig.mockImplementation(() => Promise.reject("database is locked"));
    await renderPage();
    const dlg = await openArchive();
    fireEvent.click(within(dlg).getByRole("button", { name: "Import 2 channels" }));
    await toasted("database is locked");
    expect(screen.getByRole("dialog", { name: "Import from a MyTube archive" })).toBeTruthy();
    await waitFor(() =>
      expect((within(dlg).getByRole("button", { name: "Import 2 channels" }) as HTMLButtonElement).disabled)
        .toBe(false),
    );
    expect(api.getSettings).toHaveBeenCalledTimes(1);
  });

  it("closes the checklist on Cancel without importing", async () => {
    await renderPage();
    const dlg = await openArchive();
    fireEvent.click(within(dlg).getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(api.importConfig).not.toHaveBeenCalled();
  });
});

describe("Backup & transfer · page", () => {
  it("points at the terminal commands", async () => {
    await renderPage();
    expect(screen.getByText("mytube export")).toBeTruthy();
    expect(screen.getByText("mytube import")).toBeTruthy();
    expect(screen.getByText(/Linux and macOS/)).toBeTruthy();
  });

  it("stamps the export with the local date", () => {
    expect(todayStamp(new Date(2026, 0, 5, 23, 59))).toBe("2026-01-05");
    expect(todayStamp(new Date(2026, 11, 31, 0, 1))).toBe("2026-12-31");
  });
});

describe("Backup & transfer · rail line", () => {
  const ctx = (estimate: SettingsCtx["estimate"]) => ({ estimate }) as SettingsCtx;

  it("counts the library, with thousands separated", () => {
    expect(summary(ctx(ESTIMATE))).toEqual({ text: "38 channels · 3,168 videos" });
    expect(summary(ctx({ ...ESTIMATE, channelCount: 1, videoCount: 1 })).text)
      .toBe("1 channel · 1 video");
  });

  it("says what the section is for until the library is weighed", () => {
    expect(summary(ctx(null))).toEqual({ text: "Export or import your library" });
  });
});
