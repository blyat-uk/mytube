import { describe, it, expect, vi, afterEach, beforeEach } from "vitest";
import { act, render, screen, cleanup, fireEvent } from "@testing-library/react";
import type { ComponentProps } from "react";
import type { Fit } from "../toolbarFit";
import type { Channel } from "../types";

// jsdom has no layout to measure, so the row is simply told how much fits.
const fit = vi.hoisted(() => ({ current: { words: true, shown: 99, folded: false, tight: false } as Fit }));
vi.mock("../toolbarFit", () => ({ useToolbarFit: () => fit.current }));

import TopNav from "./TopNav";

const CHANNELS: Channel[] = [{
  id: "UC1", title: "Chills Narrated", handle: null, url: "", thumb_path: null,
  subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
}];

type Props = ComponentProps<typeof TopNav>;

function props(over: Partial<Props> = {}): Props {
  return {
    tab: "subscriptions", onTab: vi.fn(), channels: CHANNELS, channelIds: [], onChannelIds: vi.fn(),
    search: "", onSearch: vi.fn(), hideWatched: false, onHideWatched: vi.fn(),
    downloadedOnly: false, onDownloadedOnly: vi.fn(), showHidden: false, onShowHidden: vi.fn(),
    grouped: false, onGrouped: vi.fn(), groupsOnly: false, onGroupsOnly: vi.fn(),
    inProgress: false, onInProgress: vi.fn(), autoDownloading: false, onAutoDownloading: vi.fn(),
    sort: "newest", onSort: vi.fn(),
    starred: ["hide_watched", "downloaded_only", "sort", "channel_ids"], onStarred: vi.fn(),
    series: null, onExitSeries: vi.fn(), polling: false, onRefresh: vi.fn(), onAdd: vi.fn(),
    update: null, onOpenAbout: vi.fn(),
    ...over,
  };
}

function renderNav(over: Partial<Props> = {}) {
  const p = props(over);
  const r = render(<TopNav {...p} />);
  return { p, rerender: (next: Partial<Props> = {}) => r.rerender(<TopNav {...p} {...next} />) };
}

const slots = () => Array.from(document.querySelectorAll(".quick-slot")) as HTMLElement[];
const allFilters = () => screen.getByRole("button", { name: /^All filters/ });
const FOLDED: Fit = { words: false, shown: 0, folded: true, tight: false };

beforeEach(() => {
  fit.current = { words: true, shown: 99, folded: false, tight: false };
});
afterEach(cleanup);

describe("a row short of room", () => {
  it("folds the starred toggles to their glyphs, keeping their names", () => {
    fit.current = { words: false, shown: 4, folded: false, tight: false };
    renderNav();
    expect(screen.getByRole("group", { name: "Starred filters" }).classList.contains("is-glyphs")).toBe(true);
    // The words are still there for CSS to fold; the names never depended on them.
    expect(screen.getByRole("button", { name: "Unwatched" }).textContent).toBe("Unwatched");
  });

  it("folds the last starred filters into All filters, out of reach", () => {
    fit.current = { words: false, shown: 2, folded: false, tight: false };
    renderNav();
    expect(slots().map((s) => s.classList.contains("is-folded"))).toEqual([false, false, true, true]);
    for (const s of slots().slice(2)) {
      expect(s.hasAttribute("inert")).toBe(true);
      expect(s.getAttribute("aria-hidden")).toBe("true");
    }
    expect(screen.queryByRole("combobox", { name: "Sort order" })).toBeNull();
    expect(screen.queryByRole("button", { name: /^Filter by channel/ })).toBeNull();
    expect(screen.getByRole("button", { name: "Downloaded" })).toBeDefined();
  });

  it("counts a folded filter that is on, as it does one never starred", () => {
    fit.current = { words: false, shown: 2, folded: false, tight: false };
    renderNav({ hideWatched: true, channelIds: ["UC1"], showHidden: true });
    // Unwatched is in sight; Channels is folded away and Include hidden was
    // never starred.
    expect(allFilters().getAttribute("aria-label")).toBe("All filters, 2 more on");
  });

  it("closes the channel checklist when its button folds away", () => {
    const { rerender } = renderNav();
    fireEvent.click(screen.getByRole("button", { name: /^Filter by channel/ }));
    expect(screen.getByRole("dialog", { name: "Channels" })).toBeDefined();
    fit.current = { words: false, shown: 2, folded: false, tight: false };
    rerender();
    expect(screen.queryByRole("dialog", { name: "Channels" })).toBeNull();
  });

  it("keeps All filters and Add spelled out until the very last step", () => {
    fit.current = FOLDED;
    renderNav();
    expect(allFilters().classList.contains("is-tight")).toBe(false);
    expect(screen.getByRole("button", { name: "Add" }).classList.contains("is-tight")).toBe(false);
    cleanup();

    fit.current = { ...FOLDED, tight: true };
    renderNav();
    expect(allFilters().classList.contains("is-tight")).toBe(true);
    expect(screen.getByRole("button", { name: "Add" }).classList.contains("is-tight")).toBe(true);
  });

  it("counts every filter that is on once all of them have folded", () => {
    fit.current = FOLDED;
    renderNav({ hideWatched: true, downloadedOnly: true, channelIds: ["UC1"] });
    expect(allFilters().getAttribute("aria-label")).toBe("All filters, 3 more on");
    expect(allFilters().querySelector(".count")?.textContent).toBe("3");
  });

  it("never folds the sections", () => {
    fit.current = { ...FOLDED, tight: true };
    renderNav();
    const tabs = Array.from(screen.getByLabelText("Sections").querySelectorAll(".tab")).map((t) => t.textContent);
    expect(tabs).toEqual(["Subscriptions", "Downloads", "Settings"]);
  });
});

describe("the folded search", () => {
  const box = () => screen.getByRole("searchbox", { name: "Search videos" }) as HTMLInputElement;

  beforeEach(() => {
    fit.current = FOLDED;
  });

  it("is a magnifier that opens into a focused box", () => {
    renderNav();
    expect(screen.queryByRole("searchbox")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Open search" }));
    expect(document.activeElement).toBe(box());
  });

  it("opens and focuses on Ctrl+F", () => {
    renderNav();
    const e = new KeyboardEvent("keydown", { key: "f", ctrlKey: true, cancelable: true });
    act(() => { window.dispatchEvent(e); });
    expect(e.defaultPrevented).toBe(true);
    expect(document.activeElement).toBe(box());
  });

  it("folds back to the magnifier when left empty", () => {
    renderNav();
    fireEvent.click(screen.getByRole("button", { name: "Open search" }));
    fireEvent.blur(box());
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(screen.getByRole("button", { name: "Open search" })).toBeDefined();
  });

  it("stays open while it holds a query, so the search in force is in sight", () => {
    renderNav({ search: "blackwood" });
    expect(box().value).toBe("blackwood");
    fireEvent.blur(box());
    expect(box()).toBeDefined();
  });

  // The shell clears the search itself when a card narrows the feed to its
  // channel. The box keeps a copy of its own for the debounce, so it has to
  // follow -- and, empty, fold back to the magnifier.
  it("empties and folds back when the search is cleared from above", async () => {
    const { p, rerender } = renderNav({ search: "blackwood" });
    expect(box().value).toBe("blackwood");
    rerender({ search: "" });
    expect(screen.queryByRole("searchbox")).toBeNull();
    // Past the debounce: the old query is not sent back up after it.
    await act(() => new Promise<void>((resolve) => setTimeout(resolve, 300)));
    expect(p.onSearch).not.toHaveBeenCalled();
  });

  it("leaves on Esc without leaving an open series", () => {
    const { p } = renderNav({ series: null });
    fireEvent.click(screen.getByRole("button", { name: "Open search" }));
    fireEvent.keyDown(box(), { key: "Escape" });
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(p.onExitSeries).not.toHaveBeenCalled();
  });
});
