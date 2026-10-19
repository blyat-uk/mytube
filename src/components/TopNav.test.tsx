import { describe, it, expect, vi, afterEach } from "vitest";
import { act, render, screen, cleanup, fireEvent } from "@testing-library/react";
import { useState, type ComponentProps } from "react";
import TopNav, { type Tab } from "./TopNav";
import type { SeriesActions } from "./SubscriptionsView";
import type { Channel } from "../types";

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

function props(over: Partial<Props> = {}): Props {
  return {
    tab: "subscriptions",
    onTab: vi.fn(),
    channels: CHANNELS,
    channelIds: [],
    onChannelIds: vi.fn(),
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
    groupsOnly: false,
    onGroupsOnly: vi.fn(),
    inProgress: false,
    onInProgress: vi.fn(),
    sort: "newest",
    onSort: vi.fn(),
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
 * An open series offers on its crumb what its card's menu offers: download
 * what is left of it, and hide it (or bring it back). The view runs them; the
 * nav only draws them from what it is handed.
 */
describe("the series actions on the crumb", () => {
  function actions(over: Partial<SeriesActions> = {}): SeriesActions {
    return {
      due: 3, marked: false, downloading: "idle", hiding: "idle",
      onDownload: vi.fn(), onHide: vi.fn(), onUnhide: vi.fn(),
      ...over,
    };
  }
  const group = () => screen.queryByRole("group", { name: "Series actions" });
  const names = () =>
    Array.from(group()!.querySelectorAll("button")).map((b) => b.getAttribute("aria-label"));

  it("offers the card menu's two, hide first, right after the crumb", () => {
    renderNav({ series: SERIES, seriesActions: actions() });
    expect(names()).toEqual(["Hide series", "Download 3 parts"]);
    // Beside the breadcrumb rather than inside it: the crumb is navigation.
    expect(screen.getByLabelText("Breadcrumb").nextElementSibling).toBe(group());
  });

  it("counts a single part left as one part, and offers no download once none is", () => {
    renderNav({ series: SERIES, seriesActions: actions({ due: 1 }) });
    expect(names()).toEqual(["Hide series", "Download 1 part"]);
    cleanup();

    renderNav({ series: SERIES, seriesActions: actions({ due: 0 }) });
    expect(names()).toEqual(["Hide series"]);
  });

  it("wears the menu's danger tone on Hide, and offers Un-hide instead while marked", () => {
    renderNav({ series: SERIES, seriesActions: actions() });
    expect(button("Hide series").classList.contains("is-danger")).toBe(true);
    cleanup();

    const a = actions({ marked: true });
    renderNav({ series: SERIES, seriesActions: a });
    expect(names()).toEqual(["Un-hide series", "Download 3 parts"]);
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

  it("goes with the crumb in the ordinary feed", () => {
    renderNav({ series: null, seriesActions: actions() });
    expect(group()).toBeNull();
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
    expect(row!.contains(channelButton())).toBe(true);
  });

  it("reads All channels, untinted, while nothing is picked", () => {
    renderNav();
    expect(channelButton().textContent).toBe("All channels");
    expect(channelButton().getAttribute("aria-label")).toBe("Filter by channel: All channels");
    expect(channelButton().classList.contains("is-on")).toBe(false);
  });

  it("wears the first picked name and how many more, every name in its tooltip", () => {
    renderNav({ channelIds: ["UC1", "UC2"] });
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
    const rows = Array.from(pop.querySelectorAll(".chan-row")).map((r) => r.textContent);
    expect(rows).toEqual(["All channels", "Chills Narrated", "Nexpo"]);
    expect(channelButton().getAttribute("aria-expanded")).toBe("true");
    // It opens to pick, so typing goes straight into the filter box.
    expect(document.activeElement).toBe(screen.getByRole("searchbox", { name: "Filter channels" }));
  });

  it("adds to the pick, switches to one with Only, and resets with All channels", () => {
    const p = renderNav({ channelIds: ["UC1"] });
    openChannels();
    fireEvent.click(button("Nexpo"));
    expect(p.onChannelIds).toHaveBeenLastCalledWith(["UC1", "UC2"]);
    fireEvent.click(button("Show only Nexpo"));
    expect(p.onChannelIds).toHaveBeenLastCalledWith(["UC2"]);
    fireEvent.click(button("All channels"));
    expect(p.onChannelIds).toHaveBeenLastCalledWith([]);
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

/** "Only groups" filters cards that exist only while grouped. */
describe("the only-groups filter", () => {
  it("is absent until siblings are grouped", () => {
    renderNav();
    expect(screen.queryByRole("button", { name: "Only groups" })).toBeNull();
  });

  it("joins the filters once they are, and toggles", () => {
    const p = renderNav({ grouped: true });
    const chip = screen.getByRole("button", { name: "Only groups" });
    expect(chip.getAttribute("aria-pressed")).toBe("false");
    fireEvent.click(chip);
    expect(p.onGroupsOnly).toHaveBeenCalledWith(true);
  });

  it("is switched off while a series is open, like the other filters", () => {
    renderNav({ grouped: true, groupsOnly: true, series: SERIES });
    const chip = screen.getByRole("button", { name: "Only groups" }) as HTMLButtonElement;
    expect(chip.disabled).toBe(true);
  });
});

/** "Continue watching" narrows to series, so like Only groups it exists only grouped. */
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

  it("goes with the tabs while a series is open", () => {
    renderNav({ update: UPDATE, series: SERIES });
    expect(chip()).toBeNull();
  });
});
