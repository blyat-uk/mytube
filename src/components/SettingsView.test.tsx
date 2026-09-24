import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import type { Settings, TransferEstimate } from "../types";

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

const saveSettings = vi.fn<(s: Settings) => Promise<void>>(() => Promise.resolve());
const exportConfig = vi.fn<(p: string, t: boolean) => Promise<void>>(() => Promise.resolve());
const transferEstimate = vi.fn<() => Promise<TransferEstimate>>(() => Promise.resolve(ESTIMATE));

vi.mock("../api", () => ({
  api: {
    getSettings: () => Promise.resolve(SETTINGS),
    saveSettings: (s: Settings) => saveSettings(s),
    transferEstimate: () => transferEstimate(),
    exportConfig: (p: string, t: boolean) => exportConfig(p, t),
    readArchive: () => Promise.resolve(null),
    importConfig: () => Promise.resolve(null),
  },
  errText: (e: unknown) => String(e),
}));

import SettingsView from "./SettingsView";
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
  transferEstimate.mockClear();
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
