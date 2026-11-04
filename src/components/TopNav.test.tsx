import { describe, it, expect, vi, afterEach } from "vitest";
import { act, render, screen, cleanup, fireEvent, within } from "@testing-library/react";
import { useState, type ComponentProps } from "react";
import TopNav, { type Tab } from "./TopNav";
import type { SeriesActions } from "./SubscriptionsView";
import { EVERY_CHANNEL } from "../channelPick";
import type { Channel, QuickFilter, SavedView } from "../types";

const CHANNELS: Channel[] = [
  {
    id: "UC1", title: "Chills Narrated", handle: null,
    url: "https://youtube.com/channel/UC1", thumb_path: null,
    subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
  },
  {
    id: "UC2", title: "Nexpo", handle: null,
    url: "https://youtube.com/channel/UC2", thumb_path: null,
    subscribed: true, member: true, auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
  },
];

type Props = ComponentProps<typeof TopNav>;

const SERIES = { name: "Building a CNC Router", parts: 7, runtime: "4h 10m", unknown: 0 };

/** Every filter starred, so each one has its control in the bar. */
const ALL_STARRED: QuickFilter[] = [
  "hide_watched", "downloaded_only", "show_hidden", "grouped", "in_progress",
  "auto_downloading", "length", "sort", "channel_ids",
];
const only = (...ids: string[]) => ({ ids, except: false });

function props(over: Partial<Props> = {}): Props {
  return {
    tab: "subscriptions",
    onTab: vi.fn(),
    channels: CHANNELS,
    channelPick: EVERY_CHANNEL,
    onChannelPick: vi.fn(),
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
    minLength: 0,
    onMinLength: vi.fn(),
    minParts: 0,
    onMinParts: vi.fn(),
    inProgress: false,
    onInProgress: vi.fn(),
    autoDownloading: false,
    onAutoDownloading: vi.fn(),
    sort: "newest",
    onSort: vi.fn(),
    starred: ALL_STARRED,
    onStarred: vi.fn(),
    savedViews: [],
    onSavedViews: vi.fn(),
    onApplyView: vi.fn(),
    downloadSearch: "",
    onDownloadSearch: vi.fn(),
    series: null,
    onExitSeries: vi.fn(),
    polling: false,
    onRefresh: vi.fn(),
    onAdd: vi.fn(),
    update: null,
    onOpenAbout: vi.fn(),
    ...over,
  };
}

function renderNav(over: Partial<Props> = {}) {
  const p = props(over);
  render(<TopNav {...p} />);
  return p;
}

const button = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement;
const channelButton = () =>
  screen.getByRole("button", { name: /^Filter by channel/ }) as HTMLButtonElement;
const openChannels = () => {
  fireEvent.click(channelButton());
  return screen.getByRole("dialog", { name: "Channels" });
};

afterEach(cleanup);

/**
 * In a series the backend deliberately ignores every filter — the point of the
 * view is the whole series. A control that still looks live but does nothing
 * reads as a broken filter, so they are disabled rather than ignored silently.
 * They keep their slots, though: the row must not reflow on the way in or out.
 */
describe("TopNav while a series is open", () => {
  const SELECTS = ["Search videos"];
  const TOGGLES = ["Unwatched", "Downloaded", "Include hidden", "Grouped"];

  it("disables the filters it cannot honour", () => {
    renderNav({ series: SERIES });
    for (const label of SELECTS) {
      expect((screen.getByLabelText(label) as HTMLInputElement).disabled).toBe(true);
    }
    for (const name of TOGGLES) expect(button(name).disabled).toBe(true);
    expect(channelButton().disabled).toBe(true);
  });

  it("closes the channel list when a series opens", () => {
    const p = props();
    const { rerender } = render(<TopNav {...p} />);
    openChannels();
    rerender(<TopNav {...p} series={SERIES} />);
    expect(screen.queryByRole("dialog", { name: "Channels" })).toBeNull();
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
    expect(channelButton().disabled).toBe(false);
  });
});

/**
 * A series opens a bar of its own under the row. The row itself never changes:
 * the sections stay, and the Subscriptions tab is the way back, so the bar
 * names the series and repeats nothing above it.
 */
describe("the series bar", () => {
  const bar = () => screen.queryByRole("region", { name: "Open series" });

  it("opens under the row and names the series, the sections still standing", () => {
    renderNav({ series: SERIES });
    expect(screen.getByLabelText("Sections")).not.toBeNull();
    expect(within(bar()!).getByText("Building a CNC Router")).not.toBeNull();
    expect(within(bar()!).getByText("7 parts · 4h 10m")).not.toBeNull();
    // Below the row, not in it.
    expect(document.querySelector(".topnav-row")!.contains(bar())).toBe(false);
  });

  it("says which channel the series is from, once known", () => {
    renderNav({ series: { ...SERIES, channel: "Chills Narrated" } });
    expect(within(bar()!).getByRole("status").textContent).toBe("7 parts · 4h 10m · Chills Narrated");
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
    renderNav({ series: { name: "Building a CNC Router", parts: 0, runtime: "", unknown: 0, channel: "Nexpo" } });
    expect(screen.queryByText(/parts/)).toBeNull();
    expect(screen.queryByText("no other parts found")).toBeNull();
    expect(within(bar()!).queryByRole("status")).toBeNull();
  });

  it("has no breadcrumb of its own: the Subscriptions tab above is the way back", () => {
    const p = renderNav({ series: SERIES });
    expect(within(bar()!).queryByRole("button", { name: "Subscriptions" })).toBeNull();
    expect(button("Subscriptions").getAttribute("aria-current")).toBe("page");
    fireEvent.click(button("Subscriptions"));
    expect(p.onExitSeries).toHaveBeenCalledTimes(1);
    expect(p.onTab).toHaveBeenCalledWith("subscriptions");
  });

  it("leaves the series by its ✕, and with any change of tab", () => {
    const p = renderNav({ series: SERIES });
    fireEvent.click(screen.getByRole("button", { name: "Leave this series" }));
    expect(p.onExitSeries).toHaveBeenCalledTimes(1);
    fireEvent.click(button("Downloads"));
    expect(p.onExitSeries).toHaveBeenCalledTimes(2);
    expect(p.onTab).toHaveBeenCalledWith("downloads");
  });

  it("is absent in the ordinary feed, where nothing about the row changes", () => {
    const p = renderNav();
    expect(bar()).toBeNull();
    expect(screen.queryByRole("button", { name: "Leave this series" })).toBeNull();
    fireEvent.click(button("Downloads"));
    expect(p.onExitSeries).not.toHaveBeenCalled();
  });
});

/**
 * An open series offers on its bar what its card's menu offers: download what
 * is left of it, and hide it (or bring it back). The view runs them; the nav
 * only draws them from what it is handed.
 */
describe("the series actions on the bar", () => {
  function actions(over: Partial<SeriesActions> = {}): SeriesActions {
    return {
      due: 3, marked: false, downloading: "idle", hiding: "idle", autoDownload: false,
      autoAvailable: true, autoing: "idle",
      onDownload: vi.fn(), onHide: vi.fn(), onUnhide: vi.fn(), onAutoDownload: vi.fn(),
      onStopAutoDownload: vi.fn(),
      ...over,
    };
  }
  const group = () => screen.queryByRole("group", { name: "Series actions" });
  const names = () =>
    Array.from(group()!.querySelectorAll("button")).map((b) => b.getAttribute("aria-label"));

  it("offers the card menu's three, hide first, on the series' own bar", () => {
    renderNav({ series: SERIES, seriesActions: actions() });
    expect(names()).toEqual(["Hide series", "Auto-download", "Download 3 parts"]);
    expect(screen.getByRole("region", { name: "Open series" }).contains(group())).toBe(true);
  });

  it("counts a single part left as one part, and offers no download once none is", () => {
    renderNav({ series: SERIES, seriesActions: actions({ due: 1 }) });
    expect(names()).toEqual(["Hide series", "Auto-download", "Download 1 part"]);
    cleanup();

    renderNav({ series: SERIES, seriesActions: actions({ due: 0 }) });
    expect(names()).toEqual(["Hide series", "Auto-download"]);
  });

  it("wears the menu's danger tone on Hide, and offers Un-hide instead while marked", () => {
    renderNav({ series: SERIES, seriesActions: actions() });
    expect(button("Hide series").classList.contains("is-danger")).toBe(true);
    cleanup();

    const a = actions({ marked: true });
    renderNav({ series: SERIES, seriesActions: a });
    expect(names()).toEqual(["Un-hide series", "Auto-download", "Download 3 parts"]);
    expect(button("Un-hide series").classList.contains("is-danger")).toBe(false);
    fireEvent.click(button("Un-hide series"));
    expect(a.onUnhide).toHaveBeenCalledTimes(1);
    expect(a.onHide).not.toHaveBeenCalled();
  });

  it("hands each press to the view", () => {
    const a = actions();
    renderNav({ series: SERIES, seriesActions: a });
    fireEvent.click(button("Download 3 parts"));
    fireEvent.click(button("Hide series"));
    expect(a.onDownload).toHaveBeenCalledTimes(1);
    expect(a.onHide).toHaveBeenCalledTimes(1);
  });

  it("switches auto-download on, and offers Stop in its place while it is on", () => {
    const a = actions();
    renderNav({ series: SERIES, seriesActions: a });
    fireEvent.click(button("Auto-download"));
    expect(a.onAutoDownload).toHaveBeenCalledTimes(1);
    cleanup();

    const on = actions({ autoDownload: true, autoing: "slow" });
    renderNav({ series: SERIES, seriesActions: on });
    expect(names()).toEqual(["Hide series", "Stop auto-download", "Download 3 parts"]);
    const stop = button("Stop auto-download");
    expect(stop.disabled).toBe(true);
    expect(stop.classList.contains("is-spinning")).toBe(true);
    cleanup();

    const idle = actions({ autoDownload: true });
    renderNav({ series: SERIES, seriesActions: idle });
    fireEvent.click(button("Stop auto-download"));
    expect(idle.onStopAutoDownload).toHaveBeenCalledTimes(1);
    expect(idle.onAutoDownload).not.toHaveBeenCalled();
  });

  it("leaves auto-download out where no part can arrive, but always offers Stop", () => {
    renderNav({ series: SERIES, seriesActions: actions({ autoAvailable: false }) });
    expect(names()).toEqual(["Hide series", "Download 3 parts"]);
    cleanup();

    renderNav({ series: SERIES, seriesActions: actions({ autoAvailable: false, autoDownload: true }) });
    expect(names()).toEqual(["Hide series", "Stop auto-download", "Download 3 parts"]);
  });

  /* Download takes itself away once nothing is left, so a double-click's
     second half would land on whatever slid into its place. */
  it("ignores the second half of a double-click", () => {
    const a = actions();
    renderNav({ series: SERIES, seriesActions: a });
    fireEvent.click(button("Hide series"), { detail: 1 });
    fireEvent.click(button("Hide series"), { detail: 2 });
    expect(a.onHide).toHaveBeenCalledTimes(1);
  });

  it("disables a button while its call is out, and spins it only once slow", () => {
    renderNav({ series: SERIES, seriesActions: actions({ downloading: "busy", hiding: "slow" }) });
    const download = button("Download 3 parts");
    expect(download.disabled).toBe(true);
    expect(download.classList.contains("is-spinning")).toBe(false);
    const hide = button("Hide series");
    expect(hide.disabled).toBe(true);
    expect(hide.classList.contains("is-spinning")).toBe(true);
    expect(hide.getAttribute("aria-busy")).toBe("true");
  });

  it("keeps its names and says what each does when the words fold away", () => {
    renderNav({ series: SERIES, seriesActions: actions() });
    // The words are there however narrow the window: CSS folds them, it never
    // removes them, and the name does not depend on them anyway.
    expect(button("Download 3 parts").textContent).toBe("Download 3 parts");
    expect(button("Download 3 parts").title).toBe("Download the 3 parts not downloaded yet, oldest first");
    expect(button("Hide series").title).toContain("any part that joins the series later");
  });

  it("offers nothing until the view has something to offer", () => {
    renderNav({ series: SERIES });
    expect(group()).toBeNull();
    cleanup();
    renderNav({ series: SERIES, seriesActions: null });
    expect(group()).toBeNull();
  });

  it("goes with the bar in the ordinary feed", () => {
    renderNav({ series: null, seriesActions: actions() });
    expect(group()).toBeNull();
  });
});

/** The starred filters sit in the one row with the sections. */
describe("the filters on the row", () => {
  it("keeps the channel filter beside the sort select", () => {
    renderNav();
    const row = screen.getByLabelText("Sort order").closest(".topnav-row");
    expect(row).not.toBeNull();
    expect(row!.contains(channelButton())).toBe(true);
  });

  it("reads All channels, untinted, while nothing is picked", () => {
    renderNav();
    expect(channelButton().textContent).toBe("All channels");
    expect(channelButton().getAttribute("aria-label")).toBe("Filter by channel: All channels");
    expect(channelButton().classList.contains("is-on")).toBe(false);
  });

  it("says which channel is left out, tinted like any filter that narrows", () => {
    renderNav({ channelPick: { ids: ["UC2"], except: true } });
    expect(channelButton().textContent).toBe("All but Nexpo");
    expect(channelButton().getAttribute("aria-label")).toBe("Filter by channel: all channels except Nexpo");
    expect(channelButton().classList.contains("is-on")).toBe(true);
  });

  it("wears the first picked name and how many more, every name in its tooltip", () => {
    renderNav({ channelPick: only("UC1", "UC2") });
    const btn = channelButton();
    expect(btn.querySelector(".channel-picker-name")?.textContent).toBe("Chills Narrated");
    expect(btn.querySelector(".channel-picker-more")?.textContent).toBe("+1");
    expect(btn.getAttribute("aria-label")).toBe("Filter by channel: Chills Narrated and 1 more");
    expect(btn.title).toBe("Chills Narrated\nNexpo");
    expect(btn.classList.contains("is-on")).toBe(true);
  });

  it("opens a checklist of every subscription plus an all-channels reset", () => {
    renderNav();
    const pop = openChannels();
    const rows = Array.from(pop.querySelectorAll(".chan-row .chan-name")).map((r) => r.textContent);
    expect(rows).toEqual(["All channels", "Chills Narrated", "Nexpo"]);
    expect(channelButton().getAttribute("aria-expanded")).toBe("true");
    // It opens to pick, so typing goes straight into the filter box.
    expect(document.activeElement).toBe(screen.getByRole("searchbox", { name: "Filter channels" }));
  });

  it("leaves a channel out, switches to one with Only, and resets with All channels", () => {
    const p = renderNav();
    openChannels();
    fireEvent.click(button("Nexpo"));
    expect(p.onChannelPick).toHaveBeenLastCalledWith({ ids: ["UC2"], except: true });
    fireEvent.click(button("Show only Nexpo"));
    expect(p.onChannelPick).toHaveBeenLastCalledWith(only("UC2"));
    fireEvent.click(button("All channels"));
    expect(p.onChannelPick).toHaveBeenLastCalledWith(EVERY_CHANNEL);
    // Ticks apply as they are made, so the list stays open for the next one.
    expect(screen.getByRole("dialog", { name: "Channels" })).toBeDefined();
  });

  it("closes the checklist on Esc, handing focus back, and on a click outside", () => {
    const p = renderNav({ series: null });
    openChannels();
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })); });
    expect(screen.queryByRole("dialog", { name: "Channels" })).toBeNull();
    expect(document.activeElement).toBe(channelButton());
    expect(p.onExitSeries).not.toHaveBeenCalled();

    openChannels();
    act(() => { document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); });
    expect(screen.queryByRole("dialog", { name: "Channels" })).toBeNull();
  });

  it("closes the checklist from its own button too", () => {
    renderNav();
    openChannels();
    fireEvent.click(channelButton());
    expect(screen.queryByRole("dialog", { name: "Channels" })).toBeNull();
  });

  /** The whole cluster belongs to the feed, so it leaves with it. */
  it("is absent on tabs the filters cannot affect", () => {
    renderNav({ tab: "downloads" });
    expect(screen.queryByRole("button", { name: /^Filter by channel/ })).toBeNull();
    for (const label of ["Search videos", "Sort order"]) {
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
   * Settings has no search box. Rather than have the shortcut do nothing
   * there, it means "find a video" and goes to the tab that can answer.
   * (Downloads has a box of its own, which takes it; see "the Downloads search".)
   */
  it("switches to Subscriptions from a tab that has no search box", () => {
    function Harness() {
      const [tab, setTab] = useState<Tab>("settings");
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
 * Length: at least so long, and (grouped) at least so many parts -- the way
 * to find the well-developed shows. Its chip wears its value and opens both
 * rows; "Only groups" is the parts row at 2.
 */
describe("the length filter", () => {
  const lengthChip = () => screen.getByRole("button", { name: /^Length:/ }) as HTMLButtonElement;
  const openLength = () => {
    fireEvent.click(lengthChip());
    return screen.getByRole("dialog", { name: "Length" });
  };

  it("has no Only groups switch any more", () => {
    renderNav({ grouped: true });
    expect(screen.queryByRole("button", { name: "Only groups" })).toBeNull();
  });

  it("wears its value, or Any length while it lets everything through", () => {
    renderNav({ grouped: true });
    expect(lengthChip().textContent).toBe("Any length");
    expect(lengthChip().classList.contains("is-on")).toBe(false);
    cleanup();
    renderNav({ grouped: true, minLength: 18000, minParts: 10 });
    expect(lengthChip().textContent).toBe("5h+ · 10+ parts");
    expect(lengthChip().getAttribute("aria-label")).toBe("Length: 5h+ · 10+ parts");
    expect(lengthChip().classList.contains("is-on")).toBe(true);
  });

  it("opens both rows and sets each, at least never at most", () => {
    const p = renderNav({ grouped: true });
    const pop = openLength();
    const total = within(within(pop).getByRole("radiogroup", { name: "Total length, at least" }));
    expect(total.getAllByRole("radio").map((r) => r.textContent)).toEqual(["Any", "1h+", "2h+", "5h+", "10h+"]);
    fireEvent.click(total.getByRole("radio", { name: "5h+" }));
    expect(p.onMinLength).toHaveBeenLastCalledWith(18000);
    const parts = within(within(pop).getByRole("radiogroup", { name: "Parts, at least" }));
    expect(parts.getAllByRole("radio").map((r) => r.textContent)).toEqual(["Any", "2+", "5+", "10+", "20+"]);
    fireEvent.click(parts.getByRole("radio", { name: "10+" }));
    expect(p.onMinParts).toHaveBeenLastCalledWith(10);
    // Each press applies at once, so the popover stays for the other row.
    expect(screen.getByRole("dialog", { name: "Length" })).toBeDefined();
  });

  it("offers the parts row only while grouped", () => {
    renderNav({ grouped: false });
    const pop = openLength();
    const parts = within(within(pop).getByRole("radiogroup", { name: "Parts, at least" }));
    for (const r of parts.getAllByRole("radio")) expect((r as HTMLButtonElement).disabled).toBe(true);
    expect(within(pop).getByText("Needs Group siblings")).toBeDefined();
  });

  it("is switched off while a series is open, like the other filters", () => {
    renderNav({ grouped: true, minLength: 3600, series: SERIES });
    expect(lengthChip().disabled).toBe(true);
  });

  it("closes on Esc, handing focus back", () => {
    renderNav({ grouped: true });
    openLength();
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })); });
    expect(screen.queryByRole("dialog", { name: "Length" })).toBeNull();
    expect(document.activeElement).toBe(lengthChip());
  });
});

/** "Continue watching" narrows to series, so it exists only grouped. */
describe("the continue-watching filter", () => {
  it("is absent until siblings are grouped", () => {
    renderNav();
    expect(screen.queryByRole("button", { name: "Continue watching" })).toBeNull();
  });

  it("joins the filters once they are, and toggles", () => {
    const p = renderNav({ grouped: true });
    const chip = screen.getByRole("button", { name: "Continue watching" });
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(chip);
    expect(p.onInProgress).toHaveBeenCalledWith(true);
  });

  it("shows itself pressed while on, and switches back off", () => {
    const p = renderNav({ grouped: true, inProgress: true });
    const chip = screen.getByRole("button", { name: "Continue watching" });
    expect(chip.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(chip);
    expect(p.onInProgress).toHaveBeenCalledWith(false);
  });

  it("is switched off while a series is open, like the other filters", () => {
    renderNav({ grouped: true, inProgress: true, series: SERIES });
    expect(button("Continue watching").disabled).toBe(true);
  });
});

/** "Auto-downloading" narrows to series set to auto-download: grouped only too. */
describe("the auto-downloading filter", () => {
  it("is absent until siblings are grouped", () => {
    renderNav();
    expect(screen.queryByRole("button", { name: "Auto-downloading" })).toBeNull();
  });

  it("joins the filters once they are, after Continue watching, and toggles", () => {
    const p = renderNav({ grouped: true });
    const bar = screen.getByRole("group", { name: "Starred filters" });
    const labels = Array.from(bar.querySelectorAll(".quick-chip:not(.length-chip)")).map((b) => b.getAttribute("aria-label"));
    expect(labels.slice(-2)).toEqual(["Continue watching", "Auto-downloading"]);
    const chip = screen.getByRole("button", { name: "Auto-downloading" });
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(chip);
    expect(p.onAutoDownloading).toHaveBeenCalledWith(true);
  });

  it("shows itself pressed while on, and is switched off while a series is open", () => {
    const p = renderNav({ grouped: true, autoDownloading: true });
    const chip = screen.getByRole("button", { name: "Auto-downloading" });
    expect(chip.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(chip);
    expect(p.onAutoDownloading).toHaveBeenCalledWith(false);
    cleanup();

    renderNav({ grouped: true, autoDownloading: true, series: SERIES });
    expect(button("Auto-downloading").disabled).toBe(true);
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

/** The logo is the red mark alone: no name beside it, and no version. */
describe("the logo", () => {
  it("is the mark at the head of the row, with no name or version beside it", () => {
    renderNav({ update: { version: "2.2.0", url: "https://example.org" } });
    expect(screen.queryByText("MyTube")).toBeNull();
    expect(screen.queryByText(/\d+\.\d+\.\d+/)).toBeNull();
    const row = document.querySelector(".topnav-row")!;
    expect(row.firstElementChild!.classList.contains("brand-mark")).toBe(true);
  });
});

/**
 * A newer release is a green chip beside the tab that leads to it. It opens
 * Settings at About rather than the release page: About is where installed and
 * available stand side by side, and where the link out already lives.
 */
describe("the update chip", () => {
  const UPDATE = { version: "2.2.0", url: "https://github.com/blyat-uk/mytube/releases/tag/v2.2.0" };
  const chip = () => screen.queryByRole("button", { name: /^Update available/ }) as HTMLButtonElement | null;

  it("is absent while no newer release is known", () => {
    renderNav();
    expect(chip()).toBeNull();
  });

  it("sits right after the Settings tab, on every tab", () => {
    for (const tab of ["subscriptions", "downloads", "settings"] as const) {
      renderNav({ tab, update: UPDATE });
      expect(chip()!.previousElementSibling).toBe(button("Settings"));
      expect(screen.getByLabelText("Sections").contains(chip())).toBe(true);
      cleanup();
    }
  });

  it("names the release and where it leads", () => {
    renderNav({ update: UPDATE });
    expect(chip()!.getAttribute("aria-label")).toBe("Update available: MyTube 2.2.0. Open About");
    expect(chip()!.title).toContain("MyTube 2.2.0");
  });

  /* Its words are there from the start, folded away: hover and focus only
     uncover them, so nothing about the row changes but what is drawn. */
  it("carries the words it spells out on hover", () => {
    renderNav({ update: UPDATE });
    expect(chip()!.textContent).toBe("Update available!");
  });

  it("opens About, not the release page", () => {
    const p = renderNav({ update: UPDATE });
    fireEvent.click(chip()!);
    expect(p.onOpenAbout).toHaveBeenCalledTimes(1);
  });

  /* The tabs no longer make way for a breadcrumb, so neither does the chip. */
  it("stays with the tabs while a series is open", () => {
    renderNav({ update: UPDATE, series: SERIES });
    expect(chip()!.previousElementSibling).toBe(button("Settings"));
  });
});

/**
 * The bar holds the filters the user starred and nothing else, in the order
 * the sidebar lists them. Everything else is one press away, behind All
 * filters, whose badge counts what is on there out of sight.
 */
describe("the starred filters", () => {
  const bar = () => screen.queryByRole("group", { name: "Starred filters" });
  const inBar = () =>
    Array.from(bar()?.querySelectorAll(".quick-slot > *") ?? []).map((el) => el.getAttribute("aria-label"));

  it("puts only what is starred in the bar, in the sidebar's order", () => {
    renderNav({ starred: ["downloaded_only", "sort"] });
    expect(inBar()).toEqual(["Downloaded", "Sort order"]);
    expect(screen.queryByRole("button", { name: "Unwatched" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Filter by channel/ })).toBeNull();
  });

  it("leaves the series-only filters out while ungrouped, starred or not", () => {
    renderNav({ starred: ["grouped", "in_progress"], grouped: false });
    expect(inBar()).toEqual(["Grouped"]);
    cleanup();
    renderNav({ starred: ["grouped", "in_progress"], grouped: true });
    expect(inBar()).toEqual(["Grouped", "Continue watching"]);
  });

  it("draws no group at all when nothing is starred, and keeps All filters", () => {
    renderNav({ starred: [] });
    expect(bar()).toBeNull();
    expect(button("All filters")).toBeDefined();
  });

  it("names a chip and says what it does, for when it is folded to its glyph", () => {
    renderNav({ starred: ["hide_watched"] });
    expect(button("Unwatched").title).toBe("Unwatched. Show only videos you have not watched");
  });

  it("counts on All filters only what is on and not in the bar", () => {
    renderNav({ starred: ["hide_watched"], hideWatched: true, downloadedOnly: true, channelPick: only("UC1") });
    const all = button("All filters, 2 more on");
    expect(all.querySelector(".count")?.textContent).toBe("2");
    expect(all.title).toBe("2 filters are on that the top bar is not showing");
  });

  it("shows no count when everything that is on is in sight", () => {
    renderNav({ starred: ["hide_watched"], hideWatched: true, grouped: true, sort: "oldest" });
    expect(button("All filters").querySelector(".count")).toBeNull();
  });
});

/**
 * Every filter, in a sidebar floating over the feed. Changes apply as they
 * are made, a star puts a filter in the bar, and it closes on Esc, ✕, All
 * filters again, or a press outside the nav.
 */
describe("the All filters sidebar", () => {
  const sidebar = () => screen.queryByRole("dialog", { name: "All filters" });
  const open = () => {
    fireEvent.click(screen.getByRole("button", { name: /^All filters/ }));
    return sidebar()!;
  };
  const side = () => within(sidebar()!);

  it("opens from All filters with Show, Series, Sort and Channels", () => {
    renderNav({ grouped: true });
    const panel = open();
    const heads = Array.from(panel.querySelectorAll(".side-sec-title")).map((h) => h.textContent);
    expect(heads).toEqual(["Views", "Show", "Series", "Length", "Sort", "Channels"]);
    expect(screen.getByRole("button", { name: /^All filters/ }).getAttribute("aria-expanded")).toBe("true");
    for (const name of ["Unwatched", "Downloaded", "Include hidden", "Group siblings",
      "Continue watching", "Auto-downloading"]) {
      expect(side().getByRole("button", { name })).toBeDefined();
    }
    expect(side().getByRole("radio", { name: "Most parts first" })).toBeDefined();
    expect(side().getByRole("button", { name: "Chills Narrated" })).toBeDefined();
    // Focus moves into it, so the keyboard carries on from the button.
    expect(document.activeElement).toBe(panel);
  });

  it("floats over the feed from the nav, rather than taking room from the grid", () => {
    renderNav();
    expect(screen.getByRole("banner").contains(open())).toBe(true);
  });

  it("applies each change as it is made and stays open", () => {
    const p = renderNav({ starred: [] });
    open();
    fireEvent.click(side().getByRole("button", { name: "Unwatched" }));
    expect(p.onHideWatched).toHaveBeenCalledWith(true);
    fireEvent.click(side().getByRole("radio", { name: "Longest first" }));
    expect(p.onSort).toHaveBeenCalledWith("length");
    fireEvent.click(side().getByRole("button", { name: "Chills Narrated" }));
    expect(p.onChannelPick).toHaveBeenCalledWith({ ids: ["UC1"], except: true });
    fireEvent.click(within(side().getByRole("radiogroup", { name: "Total length, at least" }))
      .getByRole("radio", { name: "2h+" }));
    expect(p.onMinLength).toHaveBeenCalledWith(7200);
    expect(sidebar()).not.toBeNull();
  });

  it("says which switches are on", () => {
    renderNav({ starred: [], downloadedOnly: true });
    open();
    expect(side().getByRole("button", { name: "Downloaded" }).getAttribute("aria-pressed")).toBe("true");
    expect(side().getByRole("button", { name: "Unwatched" }).getAttribute("aria-pressed")).toBe("false");
    expect(side().getByRole("radio", { name: "Newest first" }).getAttribute("aria-checked")).toBe("true");
  });

  it("stars a filter into the bar, and takes a star away, keeping the bar's order", () => {
    const p = renderNav({ starred: ["sort"] });
    open();
    const star = side().getByRole("button", { name: "Star Unwatched" });
    expect(star.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(star);
    expect(p.onStarred).toHaveBeenLastCalledWith(["hide_watched", "sort"]);

    const sortStar = side().getByRole("button", { name: "Star Sort" });
    expect(sortStar.getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(sortStar);
    expect(p.onStarred).toHaveBeenLastCalledWith([]);

    fireEvent.click(side().getByRole("button", { name: "Star Channels" }));
    expect(p.onStarred).toHaveBeenLastCalledWith(["sort", "channel_ids"]);
  });

  it("lights up a filter as it lands in the bar", () => {
    function Harness() {
      const [starred, setStarred] = useState<QuickFilter[]>([]);
      return <TopNav {...props({ starred, onStarred: setStarred })} />;
    }
    render(<Harness />);
    open();
    fireEvent.click(side().getByRole("button", { name: "Star Downloaded" }));
    const chip = within(screen.getByRole("group", { name: "Starred filters" }))
      .getByRole("button", { name: "Downloaded" });
    expect(chip.parentElement!.classList.contains("is-new")).toBe(true);
  });

  it("offers the series filters disabled until grouped, with their stars still live", () => {
    const p = renderNav({ grouped: false, starred: [] });
    open();
    expect((side().getByRole("button", { name: "Continue watching" }) as HTMLButtonElement).disabled).toBe(true);
    expect(side().getByText("The rest need Group siblings")).toBeDefined();
    expect(side().queryByRole("radio", { name: "Most parts first" })).toBeNull();
    fireEvent.click(side().getByRole("button", { name: "Star Continue watching" }));
    expect(p.onStarred).toHaveBeenLastCalledWith(["in_progress"]);
  });

  it("clears what narrows the list, leaving grouping and the order alone", () => {
    const p = renderNav({
      starred: [], grouped: true, sort: "oldest", hideWatched: true, inProgress: true,
      channelPick: only("UC2"), minLength: 3600, minParts: 5,
    });
    open();
    fireEvent.click(side().getByRole("button", { name: "Clear" }));
    expect(p.onHideWatched).toHaveBeenCalledWith(false);
    expect(p.onInProgress).toHaveBeenCalledWith(false);
    expect(p.onChannelPick).toHaveBeenCalledWith(EVERY_CHANNEL);
    expect(p.onMinLength).toHaveBeenCalledWith(0);
    expect(p.onMinParts).toHaveBeenCalledWith(0);
    expect(p.onGrouped).not.toHaveBeenCalled();
    expect(p.onSort).not.toHaveBeenCalled();
    expect(p.onDownloadedOnly).not.toHaveBeenCalled();
  });

  it("has nothing to clear when nothing narrows the list", () => {
    renderNav({ grouped: true });
    open();
    expect((side().getByRole("button", { name: "Clear" }) as HTMLButtonElement).disabled).toBe(true);
  });

  it("closes on Esc without leaving an open series, and hands focus back", () => {
    const p = renderNav({ series: SERIES });
    open();
    const seriesEsc = vi.fn();
    window.addEventListener("keydown", seriesEsc);
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })); });
    window.removeEventListener("keydown", seriesEsc);
    expect(sidebar()).toBeNull();
    expect(seriesEsc).not.toHaveBeenCalled();
    expect(p.onExitSeries).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(screen.getByRole("button", { name: /^All filters/ }));
  });

  it("closes from its ✕ and from All filters again", () => {
    renderNav();
    open();
    fireEvent.click(side().getByRole("button", { name: "Close all filters" }));
    expect(sidebar()).toBeNull();
    open();
    fireEvent.click(screen.getByRole("button", { name: /^All filters/ }));
    expect(sidebar()).toBeNull();
  });

  it("closes on a press outside the nav, but not on one inside it", () => {
    renderNav();
    open();
    act(() => { screen.getByLabelText("Search videos").dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); });
    expect(sidebar()).not.toBeNull();
    act(() => { document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); });
    expect(sidebar()).toBeNull();
  });

  it("keeps only the sort live while a series is open, and says why", () => {
    renderNav({ series: SERIES });
    open();
    expect(side().getByText(/only/i, { selector: ".side-note span" })).toBeDefined();
    expect((side().getByRole("button", { name: "Unwatched" }) as HTMLButtonElement).disabled).toBe(true);
    expect((side().getByRole("button", { name: "Chills Narrated" }) as HTMLButtonElement).disabled).toBe(true);
    expect((side().getByRole("searchbox", { name: "Filter channels" }) as HTMLInputElement).disabled).toBe(true);
    expect((side().getByRole("radio", { name: "Oldest first" }) as HTMLButtonElement).disabled).toBe(false);
    // A star is a choice about the bar, not this view.
    expect((side().getByRole("button", { name: "Star Unwatched" }) as HTMLButtonElement).disabled).toBe(false);
  });

  it("closes the bar's channel checklist, and the checklist closes it", () => {
    renderNav();
    openChannels();
    open();
    expect(screen.queryByRole("dialog", { name: "Channels" })).toBeNull();
    fireEvent.click(channelButton());
    expect(sidebar()).toBeNull();
  });

  it("is not there on the other tabs", () => {
    renderNav({ tab: "downloads" });
    expect(screen.queryByRole("button", { name: /^All filters/ })).toBeNull();
  });
});

/** The sections are the same everywhere: a glyph and a word each. */
describe("the sections", () => {
  it("wear their names on every tab and in a series", () => {
    for (const over of [{ tab: "downloads" as const }, { series: SERIES }]) {
      renderNav(over);
      const tabs = Array.from(screen.getByLabelText("Sections").querySelectorAll(".tab"))
        .map((t) => t.textContent);
      expect(tabs).toEqual(["Subscriptions", "Downloads", "Settings"]);
      cleanup();
    }
  });
});

/**
 * A saved view is a named set of filters: picked from the sidebar, or from its
 * chip once starred. It is showing while the filters on screen are its own.
 */
describe("saved views", () => {
  const LONG: SavedView = {
    id: "v1", name: "Long shows", starred: true, search: null,
    filters: {
      hide_watched: true, downloaded_only: false, show_hidden: false, grouped: true,
      in_progress: false, auto_downloading: false, min_length: 18000, min_parts: 5,
      sort: "length", channel_ids: [], channels_except: false,
    },
  };
  const READY: SavedView = {
    ...LONG, id: "v2", name: "Ready on disk", starred: false,
    filters: { ...LONG.filters, hide_watched: false, downloaded_only: true, min_length: 0, min_parts: 0, sort: "newest" },
  };
  const views = [LONG, READY];
  const sidebar = () => within(screen.getByRole("dialog", { name: "All filters" }));
  const openSide = () => fireEvent.click(screen.getByRole("button", { name: /^All filters/ }));
  /** The filters LONG sets, as props. */
  const showingLong = {
    hideWatched: true, grouped: true, minLength: 18000, minParts: 5, sort: "length" as const,
  };

  it("puts a starred view's chip in the bar ahead of the starred filters", () => {
    renderNav({ savedViews: views, starred: ["hide_watched"] });
    const bar = screen.getByRole("group", { name: "Starred filters" });
    const labels = Array.from(bar.querySelectorAll(".quick-slot > *")).map((el) => el.getAttribute("aria-label"));
    expect(labels).toEqual(["Long shows", "Unwatched"]);
  });

  it("applies a view from its chip, and lights it while its filters are on", () => {
    const p = renderNav({ savedViews: views, grouped: true });
    const chip = within(screen.getByRole("group", { name: "Starred filters" })).getByRole("button", { name: "Long shows" });
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(chip);
    expect(p.onApplyView).toHaveBeenCalledWith(LONG);
    cleanup();
    renderNav({ savedViews: views, ...showingLong });
    expect(within(screen.getByRole("group", { name: "Starred filters" }))
      .getByRole("button", { name: "Long shows" }).getAttribute("aria-pressed")).toBe("true");
  });

  it("lists every view in the sidebar with what it sets, and applies one", () => {
    const p = renderNav({ savedViews: views });
    openSide();
    const row = sidebar().getByRole("button", { name: "Ready on disk" });
    expect(row.getAttribute("aria-describedby")).toBeTruthy();
    expect(document.getElementById(row.getAttribute("aria-describedby")!)!.textContent)
      .toBe("Downloaded · Grouped · Newest first · All channels");
    fireEvent.click(row);
    expect(p.onApplyView).toHaveBeenCalledWith(READY);
  });

  it("saves the filters on screen under a name, with the search only if asked", () => {
    const p = renderNav({ savedViews: [], grouped: true, minLength: 7200, search: "tapes" });
    openSide();
    fireEvent.click(sidebar().getByRole("button", { name: "Save these filters as a view" }));
    const name = sidebar().getByRole("textbox", { name: "Name this view" });
    expect((sidebar().getByRole("button", { name: "Save view" }) as HTMLButtonElement).disabled).toBe(true);
    fireEvent.change(name, { target: { value: "  Weekend binge " } });
    fireEvent.click(sidebar().getByRole("checkbox", { name: /Include the search/ }));
    fireEvent.click(sidebar().getByRole("button", { name: "Save view" }));
    const saved = (p.onSavedViews as ReturnType<typeof vi.fn>).mock.calls[0][0] as SavedView[];
    expect(saved).toHaveLength(1);
    expect(saved[0]).toMatchObject({
      name: "Weekend binge", starred: false, search: "tapes",
      filters: { grouped: true, min_length: 7200, hide_watched: false },
    });
  });

  it("offers no save while the filters on screen are already a view", () => {
    renderNav({ savedViews: views, ...showingLong });
    openSide();
    expect((sidebar().getByRole("button", { name: "Save these filters as a view" }) as HTMLButtonElement).disabled)
      .toBe(true);
  });

  it("updates a view to the filters on screen, renames it, and deletes it", () => {
    const p = renderNav({ savedViews: views, grouped: true, downloadedOnly: true, sort: "oldest" });
    const changed = () => (p.onSavedViews as ReturnType<typeof vi.fn>).mock.lastCall![0] as SavedView[];
    openSide();
    fireEvent.click(sidebar().getByRole("button", { name: "More for Long shows" }));
    fireEvent.click(sidebar().getByRole("menuitem", { name: "Update to these filters" }));
    expect(changed()[0].filters).toMatchObject({ downloaded_only: true, sort: "oldest", min_length: 0 });

    fireEvent.click(sidebar().getByRole("button", { name: "More for Long shows" }));
    fireEvent.click(sidebar().getByRole("menuitem", { name: "Rename" }));
    const box = sidebar().getByRole("textbox", { name: "Rename Long shows" });
    fireEvent.change(box, { target: { value: "Epics" } });
    fireEvent.blur(box);
    expect(changed()[0].name).toBe("Epics");

    fireEvent.click(sidebar().getByRole("button", { name: "More for Ready on disk" }));
    fireEvent.click(sidebar().getByRole("menuitem", { name: "Delete view" }));
    expect(changed().map((v) => v.id)).toEqual(["v1"]);
  });

  it("stars a view into the bar and takes the star away", () => {
    const p = renderNav({ savedViews: views });
    openSide();
    fireEvent.click(sidebar().getByRole("button", { name: "Star Ready on disk" }));
    expect((p.onSavedViews as ReturnType<typeof vi.fn>).mock.lastCall![0][1].starred).toBe(true);
    fireEvent.click(sidebar().getByRole("button", { name: "Star Long shows" }));
    expect((p.onSavedViews as ReturnType<typeof vi.fn>).mock.lastCall![0][0].starred).toBe(false);
  });

  it("closes a rename on Esc without closing the sidebar", () => {
    renderNav({ savedViews: views });
    openSide();
    fireEvent.click(sidebar().getByRole("button", { name: "More for Long shows" }));
    fireEvent.click(sidebar().getByRole("menuitem", { name: "Rename" }));
    const box = sidebar().getByRole("textbox", { name: "Rename Long shows" });
    fireEvent.keyDown(box, { key: "Escape" });
    expect(sidebar().queryByRole("textbox", { name: "Rename Long shows" })).toBeNull();
    expect(screen.getByRole("dialog", { name: "All filters" })).toBeDefined();
  });

  it("cannot be applied while a series is open", () => {
    renderNav({ savedViews: views, series: SERIES });
    expect((within(screen.getByRole("group", { name: "Starred filters" }))
      .getByRole("button", { name: "Long shows" }) as HTMLButtonElement).disabled).toBe(true);
  });
});

/** The Downloads tab's row holds one search box and no filters. */
describe("the Downloads search", () => {
  it("is the only tool on the Downloads tab, and says what it searches", () => {
    const p = renderNav({ tab: "downloads" });
    const box = screen.getByRole("searchbox", { name: "Search downloads" }) as HTMLInputElement;
    expect(screen.queryByRole("button", { name: /^All filters/ })).toBeNull();
    expect(screen.queryByLabelText("Search videos")).toBeNull();
    fireEvent.change(box, { target: { value: "tapes" } });
    expect(p.onDownloadSearch).toHaveBeenCalledWith("tapes");
  });

  it("takes Ctrl+F on the Downloads tab, rather than going to Subscriptions", () => {
    const p = renderNav({ tab: "downloads", downloadSearch: "lighthouse" });
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "f", ctrlKey: true, cancelable: true })); });
    const box = screen.getByRole("searchbox", { name: "Search downloads" }) as HTMLInputElement;
    expect(document.activeElement).toBe(box);
    expect(p.onTab).not.toHaveBeenCalled();
  });
});
