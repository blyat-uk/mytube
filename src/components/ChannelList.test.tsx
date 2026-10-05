import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import ChannelList from "./ChannelList";
import type { Channel } from "../types";

const ch = (id: string, title: string): Channel => ({
  id, title, handle: null, url: "", thumb_path: null, subscribed: true, member: false,
  auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
});
const CHANNELS = [ch("UC1", "Alpha"), ch("UC2", "Beta"), ch("UC3", "Gamma"), ch("UC4", "Delta Force")];

/** Holds the pick the way the shell does, so the list sees its own changes. */
function Harness({ start = [] as string[], onPick = vi.fn(), disabled = false, autoFocus = false }) {
  const [picked, setPicked] = useState(start);
  return (
    <ChannelList
      channels={CHANNELS}
      picked={picked}
      onPick={(ids) => { onPick(ids); setPicked(ids); }}
      disabled={disabled}
      autoFocus={autoFocus}
    />
  );
}

const rows = () => Array.from(
  screen.getByRole("group", { name: "Channels" }).querySelectorAll<HTMLButtonElement>(".chan-row"),
);
const names = () => rows().map((b) => b.textContent);
const row = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;
const filterBox = () => screen.getByRole("searchbox", { name: "Filter channels" }) as HTMLInputElement;

afterEach(cleanup);

describe("ChannelList", () => {
  it("offers All channels, then every channel, with All channels on when nothing is picked", () => {
    render(<Harness />);
    expect(names()).toEqual(["All channels", "Alpha", "Beta", "Gamma", "Delta Force"]);
    expect(row("All channels").getAttribute("aria-pressed")).toBe("true");
    expect(row("Alpha").getAttribute("aria-pressed")).toBe("false");
  });

  it("lists what was picked first, and leaves rows where they are while it is open", () => {
    const onPick = vi.fn();
    render(<Harness start={["UC3"]} onPick={onPick} />);
    expect(names()).toEqual(["All channels", "Gamma", "Alpha", "Beta", "Delta Force"]);
    fireEvent.click(row("Gamma"));
    expect(onPick).toHaveBeenLastCalledWith([]);
    // Unticked, Gamma stays at the top: the order is the one it opened with.
    expect(names()).toEqual(["All channels", "Gamma", "Alpha", "Beta", "Delta Force"]);
    expect(row("Gamma").getAttribute("aria-pressed")).toBe("false");
  });

  it("ticks channels into a sorted pick and unticks them again", () => {
    const onPick = vi.fn();
    render(<Harness onPick={onPick} />);
    fireEvent.click(row("Gamma"));
    fireEvent.click(row("Alpha"));
    expect(onPick).toHaveBeenLastCalledWith(["UC1", "UC3"]);
    expect(row("All channels").getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(row("Gamma"));
    expect(onPick).toHaveBeenLastCalledWith(["UC1"]);
  });

  it("switches to one channel with Only, and back to every channel with All channels", () => {
    const onPick = vi.fn();
    render(<Harness start={["UC1", "UC2"]} onPick={onPick} />);
    fireEvent.click(screen.getByRole("button", { name: "Show only Gamma" }));
    expect(onPick).toHaveBeenLastCalledWith(["UC3"]);
    fireEvent.click(row("All channels"));
    expect(onPick).toHaveBeenLastCalledWith([]);
  });

  it("narrows to channels whose name holds the filter, in any case", () => {
    render(<Harness />);
    fireEvent.change(filterBox(), { target: { value: "ta" } });
    // All channels is not a channel, so a filter leaves it out.
    expect(names()).toEqual(["Beta", "Delta Force"]);
    fireEvent.change(filterBox(), { target: { value: "zzz" } });
    expect(names()).toEqual([]);
    expect(screen.getByText("No channel matches “zzz”")).toBeDefined();
  });

  it("moves between rows with the arrow keys, from the filter box and back to it", () => {
    render(<Harness />);
    filterBox().focus();
    fireEvent.keyDown(filterBox(), { key: "ArrowDown" });
    expect(document.activeElement).toBe(row("All channels"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(row("Alpha"));
    // From an Only button, the arrows still step row to row.
    screen.getByRole("button", { name: "Show only Alpha" }).focus();
    fireEvent.keyDown(document.activeElement!, { key: "ArrowDown" });
    expect(document.activeElement).toBe(row("Beta"));
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    fireEvent.keyDown(document.activeElement!, { key: "ArrowUp" });
    expect(document.activeElement).toBe(filterBox());
  });

  it("focuses the filter box on opening only when asked", () => {
    render(<Harness autoFocus />);
    expect(document.activeElement).toBe(filterBox());
    cleanup();
    render(<Harness />);
    expect(document.activeElement).not.toBe(filterBox());
  });

  it("disables every control when the filter cannot apply", () => {
    render(<Harness disabled />);
    expect(filterBox().disabled).toBe(true);
    for (const b of screen.getAllByRole("button")) expect((b as HTMLButtonElement).disabled).toBe(true);
  });
});
