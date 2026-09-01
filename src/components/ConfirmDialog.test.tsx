import { describe, it, expect, vi } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";
import ConfirmDialog from "./ConfirmDialog";
// Vite hands the stylesheet over as text; there is no @types/node here to read
// it off disk with.
import stylesheet from "../App.css?raw";

describe("ConfirmDialog", () => {
  function renderDialog() {
    const onChoose = vi.fn();
    const onCancel = vi.fn();
    render(
      <ConfirmDialog
        title="Delete the downloaded file?"
        body="“Some Video” is marked as watched. Its file is still on disk."
        choices={[{ label: "Delete file", value: "delete", danger: true }]}
        onChoose={onChoose}
        onCancel={onCancel}
      />,
    );
    return { onChoose, onCancel };
  }

  it("stacks a title, a body and an action row", () => {
    renderDialog();
    const dialog = screen.getByRole("dialog");
    expect(dialog.querySelector(".modal-title")?.textContent)
      .toBe("Delete the downloaded file?");
    expect(dialog.querySelector(".confirm-body")).not.toBeNull();
    expect([...dialog.querySelectorAll(".confirm-actions .btn")].map((b) => b.textContent))
      .toEqual(["Cancel", "Delete file"]);
    cleanup();
  });

  /**
   * The dialog's root is `modal confirm`, so any rule written for a bare
   * `.confirm` lands on it too. One once did — `display: flex` on an unrelated
   * inline confirm row in the channel list — and laid the title, body and
   * buttons out as three centred columns. jsdom applies no stylesheet, so the
   * only way to catch a recurrence is to read the stylesheet.
   */
  it("shares its class name with no other rule in the stylesheet", () => {
    const css = stylesheet.replace(/\/\*[\s\S]*?\*\//g, "");
    const offenders = [...css.matchAll(/(^|[\s,{}])\.confirm(?![\w-])[^{]*\{/g)]
      .map((m) => m[0].trim());
    expect(offenders).toEqual([]);
    cleanup();
  });

  it("cancels on Escape and on a backdrop press, not on a press inside", () => {
    const { onCancel } = renderDialog();

    fireEvent.mouseDown(screen.getByRole("dialog"));
    expect(onCancel).not.toHaveBeenCalled();

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onCancel).toHaveBeenCalledTimes(1);

    fireEvent.mouseDown(document.querySelector(".modal-backdrop")!);
    expect(onCancel).toHaveBeenCalledTimes(2);
    cleanup();
  });
});
