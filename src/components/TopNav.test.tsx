import { describe, it, expect, vi, afterEach } from "vitest";
import { act, render, screen, cleanup, fireEvent } from "@testing-library/react";
import { useState, type ComponentProps } from "react";
import TopNav, { type Tab } from "./TopNav";
import type { Channel } from "../types";

const CHANNELS: Channel[] = [
  {
    id: "UC1", title: "Chills Narrated", handle: null,
    url: "https://youtube.com/channel/UC1", thumb_path: null,
    subscribed: true, added_at: 0, last_polled_at: null,
  },
  {
    id: "UC2", title: "Nexpo", handle: null,
    url: "https://youtube.com/channel/UC2", thumb_path: null,
    subscribed: true, added_at: 0, last_polled_at: null,
  },
];

type Props = ComponentProps<typeof TopNav>;

const SERIES = { name: "Building a CNC Router", parts: 7, runtime: "4h 10m", unknown: 0 };

function props(over: Partial<Props> = {}): Props {
  return {
    tab: "subscriptions",
    onTab: vi.fn(),
    channels: CHANNELS,
    channelId: null,
    onChannelId: vi.fn(),
    search: "",
    onSearch: vi.fn(),
    hideWatched: false,
    onHideWatched: vi.fn(),
    downloadedOnly: false,
    onDownloadedOnly: vi.fn(),
    showHidden: false,
    onShowHidden: vi.fn(),
    grouped: false,
    onGrouped: vi.fn(),
    sort: "newest",
    onSort: vi.fn(),
    series: null,
    onExitSeries: vi.fn(),
    polling: false,
    onRefresh: vi.fn(),
    onAdd: vi.fn(),
    ...over,
  };
}

function renderNav(over: Partial<Props> = {}) {
  const p = props(over);
  render(<TopNav {...p} />);
  return p;
}

const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;

afterEach(cleanup);

/**
 * In a series the backend deliberately ignores every filter — the point of the
 * view is the whole series. A control that still looks live but does nothing
 * reads as a broken filter, so they are disabled rather than ignored silently.
 * They keep their slots, though: the row must not reflow on the way in or out.
 */
describe("TopNav while a series is open", () => {
  const SELECTS = ["Search videos", "Filter by channel"];
  const TOGGLES = ["Unwatched", "Downloaded", "Include hidden", "Grouped"];

  it("disables the filters it cannot honour", () => {
    renderNav({ series: SERIES });
    for (const label of SELECTS) {
      expect((screen.getByLabelText(label) as HTMLInputElement).disabled).toBe(true);
    }
    for (const name of TOGGLES) expect(button(name).disabled).toBe(true);
  });

  it("leaves sort alone, which still applies", () => {
    renderNav({ series: SERIES });
    expect((screen.getByLabelText("Sort order") as HTMLSelectElement).disabled).toBe(false);
  });

  it("keeps every filter live in the ordinary feed", () => {
    renderNav();
    for (const label of [...SELECTS, "Sort order"]) {
      expect((screen.getByLabelText(label) as HTMLInputElement).disabled).toBe(false);
    }
    for (const name of TOGGLES) expect(button(name).disabled).toBe(false);
  });
});

/**
 * The series takes the place of the tab strip rather than adding a bar of its
 * own below the nav: it is where you are, not a notice about where you are.
 */
describe("the series breadcrumb", () => {
  it("replaces the sections and names the series", () => {
    renderNav({ series: SERIES });
    expect(screen.queryByLabelText("Sections")).toBeNull();
    expect(screen.getByText("Building a CNC Router")).not.toBeNull();
    expect(screen.getByText("7 parts · 4h 10m")).not.toBeNull();
  });

  it("marks a runtime that is still missing a part as approximate", () => {
    renderNav({ series: { ...SERIES, unknown: 2 } });
    expect(screen.getByText("7 parts · 4h 10m+")).not.toBeNull();
  });

  it("says so when the anchor turned out to be alone", () => {
    renderNav({ series: { ...SERIES, parts: 1, runtime: "21m" } });
    expect(screen.getByText("no other parts found")).not.toBeNull();
  });

  /** Nothing is known until the first page lands; silence beats a wrong count. */
  it("holds its tongue until the parts are counted", () => {
    renderNav({ series: { name: "Building a CNC Router", parts: 0, runtime: "", unknown: 0 } });
    expect(screen.queryByText(/parts/)).toBeNull();
    expect(screen.queryByText("no other parts found")).toBeNull();
  });

  it("leaves the series by the ✕ and by the section it came from", () => {
    const p = renderNav({ series: SERIES });
    fireEvent.click(screen.getByRole("button", { name: "Leave this series" }));
    expect(p.onExitSeries).toHaveBeenCalledTimes(1);

    fireEvent.click(screen.getByRole("button", { name: "Subscriptions" }));
    expect(p.onExitSeries).toHaveBeenCalledTimes(2);
  });

  it("is absent in the ordinary feed, where the sections stand", () => {
    renderNav();
    expect(screen.queryByRole("button", { name: "Leave this series" })).toBeNull();
    expect(screen.getByLabelText("Sections")).not.toBeNull();
  });
});

/**
 * Every filter now shares the one row with the sections, so the split that
 * matters is which side of the divider a control sits on — not which bar.
 */
describe("the filters on the row", () => {
  it("keeps the channel filter beside the sort select", () => {
    renderNav();
    const row = screen.getByLabelText("Sort order").closest(".topnav-row");
    expect(row).not.toBeNull();
    expect(row!.contains(screen.getByLabelText("Filter by channel"))).toBe(true);
  });

  it("offers every subscription plus an all-channels reset", () => {
    renderNav();
    const select = screen.getByLabelText("Filter by channel") as HTMLSelectElement;
    expect([...select.options].map((o) => o.text))
      .toEqual(["All channels", "Chills Narrated", "Nexpo"]);
  });

  it("emits the picked channel id, and null for all channels", () => {
    const p = renderNav({ channelId: "UC1" });
    const select = screen.getByLabelText("Filter by channel") as HTMLSelectElement;
    expect(select.value).toBe("UC1");

    fireEvent.change(select, { target: { value: "UC2" } });
    expect(p.onChannelId).toHaveBeenLastCalledWith("UC2");

    fireEvent.change(select, { target: { value: "" } });
    expect(p.onChannelId).toHaveBeenLastCalledWith(null);
  });

  /** The whole cluster belongs to the feed, so it leaves with it. */
  it("is absent on tabs the filters cannot affect", () => {
    renderNav({ tab: "downloads" });
    for (const label of ["Filter by channel", "Search videos", "Sort order"]) {
      expect(screen.queryByLabelText(label)).toBeNull();
    }
    for (const name of ["Unwatched", "Downloaded", "Include hidden", "Grouped"]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
  });

  it("toggles each filter without touching the others", () => {
    const p = renderNav({ hideWatched: true });
    fireEvent.click(button("Unwatched"));
    expect(p.onHideWatched).toHaveBeenCalledWith(false);

    fireEvent.click(button("Downloaded"));
    expect(p.onDownloadedOnly).toHaveBeenCalledWith(true);

    fireEvent.click(button("Include hidden"));
    expect(p.onShowHidden).toHaveBeenCalledWith(true);

    fireEvent.click(button("Grouped"));
    expect(p.onGrouped).toHaveBeenCalledWith(true);
  });

  it("says which toggles are on, for anything that cannot see the colour", () => {
    renderNav({ hideWatched: true, grouped: true });
    expect(button("Unwatched").getAttribute("aria-pressed")).toBe("true");
    expect(button("Downloaded").getAttribute("aria-pressed")).toBe("false");
    expect(button("Grouped").getAttribute("aria-pressed")).toBe("true");
  });
});

describe("the Ctrl+F shortcut", () => {
  function ctrlF() {
    const e = new KeyboardEvent("keydown", { key: "f", ctrlKey: true, cancelable: true });
    act(() => { window.dispatchEvent(e); });
    return e;
  }

  const searchBox = () => screen.getByLabelText("Search videos") as HTMLInputElement;

  it("focuses the search box and selects what is already there", () => {
    renderNav({ search: "cats" });
    const input = searchBox();
    expect(document.activeElement).not.toBe(input);

    const e = ctrlF();
    expect(document.activeElement).toBe(input);
    // Selected, so the next keystroke replaces the old query rather than
    // appending to it.
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, "cats".length]);
    // The webview's own find bar must not also open.
    expect(e.defaultPrevented).toBe(true);
  });

  /**
   * The box only exists on Subscriptions now. Rather than have the shortcut do
   * nothing on the other two tabs, it means "find a video" everywhere and goes
   * to the tab that can answer.
   */
  it("switches to Subscriptions from a tab that has no search box", () => {
    function Harness() {
      const [tab, setTab] = useState<Tab>("downloads");
      return <TopNav {...props({ tab, onTab: setTab, search: "cats" })} />;
    }
    render(<Harness />);
    expect(screen.queryByLabelText("Search videos")).toBeNull();

    ctrlF();
    const input = searchBox();
    expect(document.activeElement).toBe(input);
    expect([input.selectionStart, input.selectionEnd]).toEqual([0, "cats".length]);
  });

  it("leaves focus alone when the box is disabled by an open series", () => {
    renderNav({ series: SERIES });
    ctrlF();
    expect(document.activeElement).not.toBe(searchBox());
  });

  it("does not pull focus out of an open modal", () => {
    renderNav();
    const modal = document.createElement("div");
    modal.className = "modal-backdrop";
    document.body.appendChild(modal);

    ctrlF();
    expect(document.activeElement).not.toBe(searchBox());

    modal.remove();
  });

  it("ignores the key on its own and with the wrong modifiers", () => {
    renderNav();
    for (const init of [
      { key: "f" },
      { key: "f", altKey: true },
      { key: "F", ctrlKey: true, shiftKey: true },
    ]) {
      act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { ...init, cancelable: true })); });
      expect(document.activeElement).not.toBe(searchBox());
    }
  });
});

/**
 * Ranking cards by how many parts they hold only means anything once the cards
 * *are* groups. Ungrouped the backend treats it as plain newest-first, so an
 * always-present option would sit in the picker doing nothing.
 */
describe("the most-parts sort option", () => {
  const values = () =>
    [...(screen.getByLabelText("Sort order") as HTMLSelectElement).options].map((o) => o.value);

  it("is absent until siblings are grouped", () => {
    renderNav();
    expect(values()).toEqual(["newest", "oldest", "channel", "length"]);
  });

  it("joins the picker once they are", () => {
    renderNav({ grouped: true });
    expect(values()).toEqual(["newest", "oldest", "channel", "length", "parts"]);
  });

  /**
   * Longest-first is the other way round: a lone video has a runtime of its
   * own, so the option means something with or without grouping and stays put.
   */
  it("does not take longest-first with it, which works either way", () => {
    renderNav();
    expect(values()).toContain("length");
    cleanup();

    const p = renderNav({ grouped: true });
    expect(values()).toContain("length");
    fireEvent.change(screen.getByLabelText("Sort order"), { target: { value: "length" } });
    expect(p.onSort).toHaveBeenCalledWith("length");
  });

  it("emits the picked order", () => {
    const p = renderNav({ grouped: true });
    fireEvent.change(screen.getByLabelText("Sort order"), { target: { value: "parts" } });
    expect(p.onSort).toHaveBeenCalledWith("parts");
  });
});

/**
 * Ctrl+R and Ctrl+N stand in for the two buttons at the end of the nav, so
 * they answer to the same rules those buttons do: both are reachable from
 * every tab, a running poll greys the refresh out, and an open dialog owns
 * the keyboard.
 */
describe("the action shortcuts", () => {
  function press(key: string, init: KeyboardEventInit = { ctrlKey: true }) {
    const e = new KeyboardEvent("keydown", { key, cancelable: true, ...init });
    act(() => { window.dispatchEvent(e); });
    return e;
  }

  /** Stand up the backdrop every dialog in the app renders, for one press. */
  function withModal(run: () => void) {
    const modal = document.createElement("div");
    modal.className = "modal-backdrop";
    document.body.appendChild(modal);
    try { run(); } finally { modal.remove(); }
  }

  describe("Ctrl+R", () => {
    it("checks for new videos, exactly as the button does", () => {
      const p = renderNav();
      const e = press("r");
      expect(p.onRefresh).toHaveBeenCalledTimes(1);
      // Without this the webview reloads the page instead, throwing away the
      // grid's scroll position and every page it has fetched.
      expect(e.defaultPrevented).toBe(true);
    });

    it("works on a tab that has no filter row", () => {
      const p = renderNav({ tab: "settings" });
      press("r");
      expect(p.onRefresh).toHaveBeenCalledTimes(1);
    });

    it("does nothing while a poll is already running", () => {
      const p = renderNav({ polling: true });
      const e = press("r");
      expect(p.onRefresh).not.toHaveBeenCalled();
      // The button is disabled here, but the key must still swallow the reload.
      expect(e.defaultPrevented).toBe(true);
    });

    it("leaves the keyboard to an open dialog", () => {
      const p = renderNav();
      withModal(() => press("r"));
      expect(p.onRefresh).not.toHaveBeenCalled();
    });

    it("ignores the key on its own and with the wrong modifiers", () => {
      const p = renderNav();
      press("r", {});
      press("r", { altKey: true });
      press("R", { ctrlKey: true, shiftKey: true });
      expect(p.onRefresh).not.toHaveBeenCalled();
    });
  });

  describe("Ctrl+N", () => {
    it("opens the add dialog", () => {
      const p = renderNav();
      const e = press("n");
      expect(p.onAdd).toHaveBeenCalledTimes(1);
      expect(e.defaultPrevented).toBe(true);
    });

    it("works on a tab that has no filter row", () => {
      const p = renderNav({ tab: "downloads" });
      press("n");
      expect(p.onAdd).toHaveBeenCalledTimes(1);
    });

    /* Otherwise it would stack a second dialog on the one already up -- or
       fire while a channel name is being typed into it. */
    it("does nothing while a dialog is already open", () => {
      const p = renderNav();
      withModal(() => press("n"));
      expect(p.onAdd).not.toHaveBeenCalled();
    });

    it("ignores the key on its own and with the wrong modifiers", () => {
      const p = renderNav();
      press("n", {});
      press("n", { altKey: true });
      press("N", { ctrlKey: true, shiftKey: true });
      expect(p.onAdd).not.toHaveBeenCalled();
    });
  });

  // A shortcut nobody knows about is not a shortcut, and the search box has
  // said "Ctrl+F to focus" since it was added.
  it("advertises itself on the button that owns it", () => {
    renderNav();
    expect(button("Refresh subscriptions").title).toContain("Ctrl+R");
    expect(button("Add").title).toContain("Ctrl+N");
  });
});
