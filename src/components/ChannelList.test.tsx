import { afterEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { useState } from "react";
import ChannelList from "./ChannelList";
import { EVERY_CHANNEL, type ChannelPick } from "../channelPick";
import type { Channel } from "../types";

const ch = (id: string, title: string): Channel => ({
  id, title, handle: null, url: "", thumb_path: null, subscribed: true, member: false,
  auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
});
const CHANNELS = [ch("UC1", "Alpha"), ch("UC2", "Beta"), ch("UC3", "Gamma"), ch("UC4", "Delta Force")];

/** Holds the pick the way the shell does, so the list sees its own changes. */
function Harness({ start = EVERY_CHANNEL as ChannelPick, onPick = vi.fn(), disabled = false, autoFocus = false }) {
  const [pick, setPick] = useState(start);
  return (
    <ChannelList
      channels={CHANNELS}
      pick={pick}
      onPick={(next) => { onPick(next); setPick(next); }}
      disabled={disabled}
      autoFocus={autoFocus}
    />
  );
}

const rows = () => Array.from(
  screen.getByRole("group", { name: "Channels" }).querySelectorAll<HTMLButtonElement>(".chan-row"),
);
const names = () => rows().map((b) => b.querySelector(".chan-name")?.textContent);
const row = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;
const filterBox = () => screen.getByRole("searchbox", { name: "Filter channels" }) as HTMLInputElement;
const ticked = () => rows().slice(1).filter((b) => b.getAttribute("aria-pressed") === "true")
  .map((b) => b.querySelector(".chan-name")?.textContent);

afterEach(cleanup);

describe("ChannelList", () => {
  it("starts with every channel ticked, All channels the Select all box over them", () => {
    render(<Harness />);
    expect(names()).toEqual(["All channels", "Alpha", "Beta", "Gamma", "Delta Force"]);
    expect(row("All channels").getAttribute("aria-pressed")).toBe("true");
    expect(ticked()).toEqual(["Alpha", "Beta", "Gamma", "Delta Force"]);
    expect(screen.getByText("4 of 4 shown")).toBeDefined();
  });

  it("leaves out a channel or two by unticking them, and shows the rest", () => {
    const onPick = vi.fn();
    render(<Harness onPick={onPick} />);
    fireEvent.click(row("Gamma"));
    expect(onPick).toHaveBeenLastCalledWith({ ids: ["UC3"], except: true });
    fireEvent.click(row("Alpha"));
    expect(onPick).toHaveBeenLastCalledWith({ ids: ["UC1", "UC3"], except: true });
    expect(ticked()).toEqual(["Beta", "Delta Force"]);
    // Half-ticked while some are left out, and it says how many are shown.
    expect(row("All channels").getAttribute("aria-pressed")).toBe("mixed");
    expect(screen.getByText("2 of 4 shown")).toBeDefined();
    // Ticking one back leaves the other out still.
    fireEvent.click(row("Alpha"));
    expect(onPick).toHaveBeenLastCalledWith({ ids: ["UC3"], except: true });
  });

  it("ticks them all again from All channels", () => {
    const onPick = vi.fn();
    render(<Harness start={{ ids: ["UC1", "UC3"], except: true }} onPick={onPick} />);
    fireEvent.click(row("All channels"));
    expect(onPick).toHaveBeenLastCalledWith(EVERY_CHANNEL);
    // Every box ticked, the two it opened leaving out still listed first.
    expect(ticked()).toEqual(["Alpha", "Gamma", "Beta", "Delta Force"]);
  });

  it("lists what the pick named first, and leaves rows where they are while it is open", () => {
    render(<Harness start={{ ids: ["UC3"], except: true }} />);
    expect(names()).toEqual(["All channels", "Gamma", "Alpha", "Beta", "Delta Force"]);
    fireEvent.click(row("Gamma"));
    // Ticked back, Gamma stays at the top: the order is the one it opened with.
    expect(names()).toEqual(["All channels", "Gamma", "Alpha", "Beta", "Delta Force"]);
  });

  it("switches to one channel with Only, and ticks others in beside it", () => {
    const onPick = vi.fn();
    render(<Harness onPick={onPick} />);
    fireEvent.click(screen.getByRole("button", { name: "Show only Gamma" }));
    expect(onPick).toHaveBeenLastCalledWith({ ids: ["UC3"], except: false });
    expect(ticked()).toEqual(["Gamma"]);
    fireEvent.click(row("Alpha"));
    expect(onPick).toHaveBeenLastCalledWith({ ids: ["UC1", "UC3"], except: false });
  });

  it("never unticks the last channel shown", () => {
    const onPick = vi.fn();
    render(<Harness start={{ ids: ["UC3"], except: false }} onPick={onPick} />);
    fireEvent.click(row("Gamma"));
    expect(onPick).toHaveBeenLastCalledWith({ ids: ["UC3"], except: false });
    expect(ticked()).toEqual(["Gamma"]);
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
