import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import TakeoutDialog from "./TakeoutDialog";
import type { TakeoutRow } from "../types";

vi.mock("@tauri-apps/api/event", () => ({
  listen: () => Promise.resolve(() => {}),
}));

const rows: TakeoutRow[] = [
  { channelId: "UCaaaaaaaaaaaaaaaaaaaaaa", title: "Alpha", alreadySubscribed: false },
  { channelId: "UCbbbbbbbbbbbbbbbbbbbbbb", title: "Beta", alreadySubscribed: false },
  { channelId: "UCcccccccccccccccccccccc", title: "Gamma", alreadySubscribed: true },
];

/**
 * Regression for a real bug: TakeoutDialog was rendered inside
 * AddChannelDialog's backdrop, whose onClick closes the whole Add dialog. The
 * checklist stopped onMouseDown but not onClick, so every click inside it —
 * checkboxes, Select all — bubbled out and closed everything instantly.
 */
describe("TakeoutDialog inside another modal's backdrop", () => {
  function renderNested() {
    const outerClose = vi.fn();
    const onImport = vi.fn();
    const onCancel = vi.fn();
    render(
      <div className="modal-backdrop" onClick={outerClose} onMouseDown={outerClose}>
        <TakeoutDialog rows={rows} importing={false} onImport={onImport} onCancel={onCancel} />
      </div>,
    );
    return { outerClose, onImport, onCancel };
  }

  it("does not close the surrounding dialog when a control inside is clicked", () => {
    const { outerClose, onCancel } = renderNested();
    fireEvent.click(screen.getByText("Select all"));
    fireEvent.click(screen.getByText("None"));
    expect(outerClose).not.toHaveBeenCalled();
    expect(onCancel).not.toHaveBeenCalled();
    cleanup();
  });

  it("lets checkboxes toggle without closing anything", () => {
    const { outerClose } = renderNested();
    const alpha = screen.getByLabelText(/Alpha/) as HTMLInputElement;
    expect(alpha.checked).toBe(true);
    fireEvent.click(alpha);
    expect((screen.getByLabelText(/Alpha/) as HTMLInputElement).checked).toBe(false);
    expect(outerClose).not.toHaveBeenCalled();
    cleanup();
  });

  it("Select all and None change how many will be imported", () => {
    renderNested();
    fireEvent.click(screen.getByText("None"));
    expect(screen.getByText("Import 0")).toBeDefined();
    fireEvent.click(screen.getByText("Select all"));
    // Gamma is already subscribed, so only Alpha and Beta are importable.
    expect(screen.getByText("Import 2")).toBeDefined();
    cleanup();
  });

  it("still cancels when the backdrop itself is clicked", () => {
    const { onCancel } = renderNested();
    fireEvent.mouseDown(screen.getByRole("dialog").parentElement!);
    expect(onCancel).toHaveBeenCalled();
    cleanup();
  });
});
