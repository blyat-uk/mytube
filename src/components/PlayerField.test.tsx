import { describe, it, expect, vi, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup } from "@testing-library/react";
import PlayerField from "./PlayerField";

vi.mock("../api", () => ({
  api: {
    detectPlayers: vi.fn().mockResolvedValue([
      { id: "default", label: "System default", command: "" },
      { id: "mpv", label: "mpv", command: "mpv" },
    ]),
  },
}));

afterEach(cleanup);

describe("PlayerField", () => {
  /** A pick's own state change can render before the parent hands the picked
   *  command back. The field must not read that in-between render — old custom
   *  value, nothing focused — as a reason to stay open. */
  it("closes the text field after a pick even when the new value arrives a render later", async () => {
    const props = { onEdit: vi.fn(), onBlur: vi.fn(), onPick: vi.fn() };
    const { rerender } = render(<PlayerField value="smplayer" {...props} />);
    const select = (await screen.findByRole("combobox")) as HTMLSelectElement;
    await screen.findByRole("option", { name: "mpv" });
    expect(screen.getByLabelText("Player command")).toBeTruthy();

    fireEvent.change(select, { target: { value: "mpv" } });
    expect(props.onPick).toHaveBeenCalledWith("mpv");
    rerender(<PlayerField value="mpv" {...props} />);

    expect(select.value).toBe("mpv");
    expect(screen.queryByLabelText("Player command")).toBeNull();
  });
});
