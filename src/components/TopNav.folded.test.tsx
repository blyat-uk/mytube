import { describe, it, expect, vi, afterEach } from "vitest";
import { act, render, screen, cleanup, fireEvent } from "@testing-library/react";
import type { ComponentProps } from "react";
import type { Channel } from "../types";

// jsdom has no layout to measure, so the row is simply told it is folded.
vi.mock("../toolbarFit", () => ({ useToolbarFit: () => "folded" }));

import TopNav from "./TopNav";

const CHANNELS: Channel[] = [{
  id: "UC1", title: "Chills Narrated", handle: null, url: "", thumb_path: null,
  subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
}];

type Props = ComponentProps<typeof TopNav>;

function renderNav(over: Partial<Props> = {}) {
  const p: Props = {
    tab: "subscriptions", onTab: vi.fn(), channels: CHANNELS, channelId: null, onChannelId: vi.fn(),
    search: "", onSearch: vi.fn(), hideWatched: false, onHideWatched: vi.fn(),
    downloadedOnly: false, onDownloadedOnly: vi.fn(), showHidden: false, onShowHidden: vi.fn(),
    grouped: false, onGrouped: vi.fn(), groupsOnly: false, onGroupsOnly: vi.fn(),
    inProgress: false, onInProgress: vi.fn(), sort: "newest", onSort: vi.fn(),
    series: null, onExitSeries: vi.fn(), polling: false, onRefresh: vi.fn(), onAdd: vi.fn(),
    version: null, onOpenRelease: vi.fn(),
    ...over,
  };
  render(<TopNav {...p} />);
  return p;
}

const filtersButton = () => screen.getByRole("button", { name: /^Filters/ });
const openPanel = () => {
  fireEvent.click(filtersButton());
  return screen.getByRole("dialog", { name: "Filters" });
};

afterEach(cleanup);

describe("the folded filter row", () => {
  it("hides the full controls from the keyboard and screen readers", () => {
    renderNav();
    // The full controls stay in the layout (the fold animates them away) but
    // inert, so nothing reaches them.
    expect(screen.queryByRole("group", { name: "Filters" })).toBeNull();
    expect(screen.queryByRole("combobox", { name: "Sort order" })).toBeNull();
    expect(filtersButton()).toBeDefined();
  });

  it("wears the number of filters applied as a chip", () => {
    renderNav({ hideWatched: true, downloadedOnly: true, channelId: "UC1" });
    const btn = screen.getByRole("button", { name: "Filters, 3 applied" });
    expect(btn.querySelector(".count")?.textContent).toBe("3");
  });

  it("shows no chip when nothing is applied", () => {
    renderNav({ grouped: true, sort: "oldest" });
    expect(screen.getByRole("button", { name: "Filters" }).querySelector(".count")).toBeNull();
  });

  it("opens a panel with Show and Arrange, then Sort and Channel", () => {
    renderNav({ grouped: true });
    const panel = openPanel();
    const heads = Array.from(panel.querySelectorAll(".pop-head")).map((h) => h.textContent);
    expect(heads).toEqual(["Show", "Arrange", "Sort", "Channel"]);
    const cols = panel.querySelectorAll(".pop-col");
    expect(cols[0].textContent).toContain("Continue watching");
    expect(cols[0].textContent).toContain("Group siblings");
    expect(cols[1].textContent).toContain("Most parts first");
    expect(filtersButton().getAttribute("aria-expanded")).toBe("true");
  });

  it("applies each change as it is made and stays open", () => {
    const p = renderNav();
    openPanel();
    fireEvent.click(screen.getByRole("button", { name: "Unwatched" }));
    expect(p.onHideWatched).toHaveBeenCalledWith(true);
    fireEvent.click(screen.getByRole("radio", { name: "Longest first" }));
    expect(p.onSort).toHaveBeenCalledWith("length");
    fireEvent.change(screen.getByRole("combobox", { name: "Filter by channel" }), { target: { value: "UC1" } });
    expect(p.onChannelId).toHaveBeenCalledWith("UC1");
    expect(screen.getByRole("dialog", { name: "Filters" })).toBeDefined();
  });

  it("offers the series-only filters and sort only while grouped", () => {
    renderNav({ grouped: false });
    openPanel();
    expect(screen.queryByRole("button", { name: "Only groups" })).toBeNull();
    expect(screen.queryByRole("radio", { name: "Most parts first" })).toBeNull();
  });

  it("closes on Esc without leaving an open series, and hands focus back", () => {
    const p = renderNav({ series: { name: "Tapes", parts: 3, runtime: "", unknown: 0 } });
    openPanel();
    const seriesEsc = vi.fn();
    window.addEventListener("keydown", seriesEsc);
    act(() => { window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" })); });
    window.removeEventListener("keydown", seriesEsc);
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(seriesEsc).not.toHaveBeenCalled();
    expect(p.onExitSeries).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(filtersButton());
  });

  it("closes on a click outside it", () => {
    renderNav();
    openPanel();
    act(() => { document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true })); });
    expect(screen.queryByRole("dialog")).toBeNull();
  });

  it("keeps only the sort live while a series is open", () => {
    renderNav({ series: { name: "Tapes", parts: 3, runtime: "", unknown: 0 } });
    openPanel();
    expect((screen.getByRole("button", { name: "Unwatched" }) as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByRole("combobox", { name: "Filter by channel" }) as HTMLSelectElement).disabled).toBe(true);
    expect((screen.getByRole("radio", { name: "Oldest first" }) as HTMLButtonElement).disabled).toBe(false);
  });
});

describe("the folded search", () => {
  const box = () => screen.getByRole("searchbox", { name: "Search videos" }) as HTMLInputElement;

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

  it("leaves on Esc without leaving an open series", () => {
    const p = renderNav({ series: null });
    fireEvent.click(screen.getByRole("button", { name: "Open search" }));
    fireEvent.keyDown(box(), { key: "Escape" });
    expect(screen.queryByRole("searchbox")).toBeNull();
    expect(p.onExitSeries).not.toHaveBeenCalled();
  });
});
