import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { RecSort, Recommendation } from "../../types";

const listRecommendations = vi.fn<() => Promise<Recommendation[]>>();
const dismissRecommendation = vi.fn<(id: string) => Promise<void>>();
const addChannel = vi.fn<(input: string) => Promise<unknown>>();
const openExternal = vi.fn<(url: string) => Promise<void>>();

vi.mock("../../api", () => ({
  api: {
    listRecommendations: () => listRecommendations(),
    dismissRecommendation: (id: string) => dismissRecommendation(id),
    addChannel: (input: string) => addChannel(input),
    openExternal: (url: string) => openExternal(url),
  },
  errText: (e: unknown) => String(e),
  fileSrc: (path: string) => `asset://localhost${path}`,
}));

import RecommendationList from "./RecommendationList";
import { ToastProvider } from "../Toast";

function uploads(prefix: string) {
  return [1, 2, 3, 4].map((i) => ({
    id: `${prefix}${i}xxxxxxxx`.slice(0, 11), title: `${prefix} upload ${i}`,
    publishedAt: 1_700_000_000 - i * 86_400, thumbPath: `/thumbs/recs/${prefix}${i}.jpg`,
  }));
}

const POTATO: Recommendation = {
  id: "UCfdAUuUI_gNd8BQmsJOyVdg", title: "Manga Potato", handle: "@MangaPotatoTOP",
  thumbPath: null, subscribers: 7910, videoCount: 11, popularity: 4, uploads: uploads("mp"),
};
const HYPER: Recommendation = {
  id: "UC40TIillrzMJe3cDWQju-Vg", title: "Hyper Recap", handle: null,
  thumbPath: null, subscribers: 2250, videoCount: 300, popularity: 1, uploads: uploads("hr"),
};

function show(over: { folded?: boolean; sort?: RecSort } = {}) {
  const onFold = vi.fn();
  const onSort = vi.fn();
  const onChanged = vi.fn();
  const view = (o: typeof over) => (
    <ToastProvider>
      <RecommendationList
        folded={o.folded ?? false} onFold={onFold} sort={o.sort ?? "videos"} onSort={onSort}
        refreshKey={0} onChanged={onChanged}
      />
    </ToastProvider>
  );
  const r = render(view(over));
  return { onFold, onSort, onChanged, rerender: (o: typeof over) => r.rerender(view(o)) };
}

const rows = () => screen.getAllByRole("listitem").map((li) => within(li).getAllByRole("heading")[0].textContent);

beforeEach(() => {
  listRecommendations.mockReset().mockResolvedValue([POTATO, HYPER]);
  dismissRecommendation.mockReset().mockResolvedValue(undefined);
  addChannel.mockReset().mockResolvedValue({});
  openExternal.mockReset().mockResolvedValue(undefined);
});

describe("RecommendationList", () => {
  it("lists each suggestion with its handle, subscribers and four latest uploads", async () => {
    show();
    const row = (await screen.findByRole("heading", { name: "Manga Potato" })).closest("li")!;
    expect(within(row).getByRole("button", { name: "@MangaPotatoTOP" })).toBeTruthy();
    expect(within(row).getByText(/7\.91K subscribers/)).toBeTruthy();
    // Inside labelled buttons, so the pictures themselves are unnamed.
    expect(row.querySelectorAll("img")).toHaveLength(4);
    expect(screen.getByRole("button", { name: "Recommendations 2" })).toBeTruthy();
  });

  it("opens the channel in the browser from its handle", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: "@MangaPotatoTOP" }));
    await waitFor(() => expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/@MangaPotatoTOP"));
  });

  it("opens a channel with no handle from its channel page link", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Channel page" }));
    await waitFor(() =>
      expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/channel/UC40TIillrzMJe3cDWQju-Vg"));
  });

  it("opens an upload on YouTube from its thumbnail", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Open mp upload 1 on YouTube" }));
    await waitFor(() => expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/watch?v=mp1xxxxxxxx"));
  });

  it("subscribes, and the channel leaves the list", async () => {
    const { onChanged } = show();
    fireEvent.click(await screen.findByRole("button", { name: "Subscribe to Manga Potato" }));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Manga Potato" })).toBeNull());
    expect(addChannel).toHaveBeenCalledWith("UCfdAUuUI_gNd8BQmsJOyVdg");
    expect(onChanged).toHaveBeenCalled();
  });

  it("keeps the row when subscribing fails", async () => {
    addChannel.mockRejectedValue("offline");
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Subscribe to Manga Potato" }));
    await waitFor(() => expect(addChannel).toHaveBeenCalled());
    await act(async () => {});
    expect(screen.getByRole("heading", { name: "Manga Potato" })).toBeTruthy();
  });

  it("dismisses a suggestion for good", async () => {
    show();
    fireEvent.click(await screen.findByRole("button", { name: "Not interested in Manga Potato" }));
    await waitFor(() => expect(screen.queryByRole("heading", { name: "Manga Potato" })).toBeNull());
    expect(dismissRecommendation).toHaveBeenCalledWith("UCfdAUuUI_gNd8BQmsJOyVdg");
    expect(screen.getByRole("button", { name: "Recommendations 1" })).toBeTruthy();
  });

  it("sorts by most videos by default, and by most popular when asked", async () => {
    const { onSort, rerender } = show();
    await screen.findByRole("heading", { name: "Manga Potato" });
    expect(rows()).toEqual(["Hyper Recap", "Manga Potato"]);
    expect(screen.getByRole("button", { name: "Most videos" }).getAttribute("aria-pressed")).toBe("true");
    fireEvent.click(screen.getByRole("button", { name: "Most popular" }));
    expect(onSort).toHaveBeenCalledWith("popular");
    rerender({ sort: "popular" });
    expect(rows()).toEqual(["Manga Potato", "Hyper Recap"]);
  });

  it("folds away, and says when there is nothing yet", async () => {
    listRecommendations.mockResolvedValue([]);
    const { onFold } = show({ folded: true });
    const fold = screen.getByRole("button", { name: /^Recommendations/ });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(fold);
    expect(onFold).toHaveBeenCalledWith(false);
    expect(await screen.findByText("Suggestions appear here as new videos arrive.")).toBeTruthy();
  });
});
