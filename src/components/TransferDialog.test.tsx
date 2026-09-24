import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, cleanup, within } from "@testing-library/react";
import TransferDialog from "./TransferDialog";
import type { ArchiveChannel, ArchiveSummary } from "../types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

const DOWNLOAD_DIR = "/home/someone/Videos/MyTube";

const channel = (over: Partial<ArchiveChannel>): ArchiveChannel => ({
  channelId: "UC0", title: "Untitled", videoCount: 0,
  subscribed: true, member: false, alreadyHere: false,
  ...over,
});

function archive(over: Partial<ArchiveSummary> = {}): ArchiveSummary {
  return {
    format: 1,
    appVersion: "0.1.0",
    exportedAt: 1_758_240_000,
    exportedFrom: "workshop",
    includesThumbs: true,
    thumbCount: 3225,
    downloadDir: DOWNLOAD_DIR,
    downloadDirExists: true,
    videoCount: 41,
    localOnlyChannels: 2,
    localOnlyVideos: 312,
    channels: [
      channel({ channelId: "UCa", title: "Alpha", videoCount: 12 }),
      channel({ channelId: "UCb", title: "Beta", videoCount: 18, alreadyHere: true }),
      channel({ channelId: "UCg", title: "Gamma", videoCount: 11, subscribed: false }),
    ],
    ...over,
  };
}

function renderDialog(over: Partial<ArchiveSummary> = {}) {
  const onImport = vi.fn();
  const onCancel = vi.fn();
  render(
    <TransferDialog
      summary={archive(over)}
      importing={false}
      onImport={onImport}
      onCancel={onCancel}
    />,
  );
  return { onImport, onCancel };
}

const tick = (name: RegExp) => screen.getByLabelText(name) as HTMLInputElement;
const importButton = () =>
  screen.getByRole("button", { name: /^Import \d+ channels?$/ }) as HTMLButtonElement;
/** The confirmation is a second dialog, named by its own question. */
const confirmation = () => screen.getByRole("dialog", { name: /Replace with the archive/ });

describe("TransferDialog", () => {
  it("starts with every channel in the archive ticked, and counts them on the button", () => {
    renderDialog();
    expect(tick(/Alpha/).checked).toBe(true);
    expect(tick(/Beta/).checked).toBe(true);
    expect(tick(/Gamma/).checked).toBe(true);
    expect(importButton().textContent).toBe("Import 3 channels");
    cleanup();
  });

  it("leaves an unticked channel out of what is imported", () => {
    const { onImport } = renderDialog();
    fireEvent.click(tick(/Beta/));
    expect(importButton().textContent).toBe("Import 2 channels");
    fireEvent.click(importButton());
    expect(onImport).toHaveBeenCalledWith(["UCa", "UCg"], "merge", true);
    cleanup();
  });

  it("imports straight away in Merge, which takes nothing away", () => {
    const { onImport } = renderDialog();
    fireEvent.click(importButton());
    expect(screen.queryByRole("dialog", { name: /Replace with the archive/ })).toBeNull();
    expect(onImport).toHaveBeenCalledWith(["UCa", "UCb", "UCg"], "merge", true);
    cleanup();
  });

  it("asks again before replacing, and imports nothing if that is cancelled", () => {
    const { onImport, onCancel } = renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(importButton());
    expect(onImport).not.toHaveBeenCalled();

    fireEvent.click(within(confirmation()).getByText("Cancel"));
    expect(onImport).not.toHaveBeenCalled();
    // Backing out of the question must not also close the checklist behind it.
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.queryByRole("dialog", { name: /Replace with the archive/ })).toBeNull();
    cleanup();
  });

  it("carries the replace mode through once the question is answered", () => {
    const { onImport } = renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(importButton());
    fireEvent.click(within(confirmation()).getByText("Replace"));
    expect(onImport).toHaveBeenCalledWith(["UCa", "UCb", "UCg"], "replace", true);
    cleanup();
  });

  /**
   * Replace removes rows and only rows. The promise has to be on screen both
   * where the mode is chosen and where it is confirmed, because either one on
   * its own is a sentence the user can act without ever reading.
   */
  it("promises in both places that replacing deletes nothing from disk", () => {
    renderDialog();
    expect(screen.getByText(/No file is ever deleted from disk/)).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(importButton());
    expect(
      within(confirmation()).getByText(
        /no downloaded file and no cached thumbnail is deleted from disk/,
      ),
    ).toBeDefined();
    cleanup();
  });

  it("quotes what a Replace would remove, in the chip and in the question", () => {
    renderDialog();
    expect(screen.getByText(/Removes 2 channels and 312 videos/)).toBeDefined();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(importButton());
    expect(within(confirmation()).getByText(/Removes 2 channels and 312 videos/)).toBeDefined();
    cleanup();
  });

  it("says in words, not a zero, when a Replace would remove nothing", () => {
    renderDialog({ localOnlyChannels: 0, localOnlyVideos: 0 });
    expect(screen.getByText(/Nothing here would be removed/)).toBeDefined();
    expect(screen.queryByText(/Removes 0/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(importButton());
    // Still worth asking: Replace overwrites every row with the archive's copy,
    // which is the one thing Merge will not do.
    const body = within(confirmation()).getByText(/Nothing would be removed/);
    expect(body.textContent).toMatch(/the one thing Merge will not do/);
    cleanup();
  });

  /**
   * The counts are measured against the whole archive, never the ticked subset.
   * Unticking a channel skips it; it does not mark it for deletion, and the
   * figure read at the top of the list has to still be true at the bottom.
   */
  it("does not move the removal counts as channels are unticked", () => {
    renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Replace" }));
    fireEvent.click(tick(/Beta/));
    fireEvent.click(tick(/Gamma/));
    expect(screen.getByText(/Removes 2 channels and 312 videos/)).toBeDefined();
    fireEvent.click(importButton());
    const body = within(confirmation()).getByText(/Removes 2 channels and 312 videos/);
    expect(body.textContent).toMatch(/Unticked channels are left exactly as they are/);
    cleanup();
  });

  it("names the archive's download folder only when this machine has no such folder", () => {
    renderDialog({ downloadDirExists: true });
    expect(screen.queryByText(/does not exist here/)).toBeNull();
    cleanup();

    renderDialog({ downloadDirExists: false });
    expect(screen.getByText(/does not exist here/)).toBeDefined();
    expect(screen.getByText(DOWNLOAD_DIR)).toBeDefined();
    cleanup();
  });

  it("applies settings by default, and carries a refusal through to the import", () => {
    const { onImport } = renderDialog({ downloadDirExists: false });
    const apply = tick(/Also apply settings/);
    expect(apply.checked).toBe(true);
    fireEvent.click(apply);
    // The folder note explains one of the settings about to be applied, so it
    // goes with them rather than warning about something no longer happening.
    expect(screen.queryByText(/does not exist here/)).toBeNull();
    fireEvent.click(importButton());
    expect(onImport).toHaveBeenCalledWith(["UCa", "UCb", "UCg"], "merge", false);
    cleanup();
  });

  it("tags a channel already here, and one that travels only as a parent row", () => {
    renderDialog();
    expect(screen.getByText("already here")).toBeDefined();
    expect(screen.getByText("not subscribed")).toBeDefined();
    cleanup();
  });
});
