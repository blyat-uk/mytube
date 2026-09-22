import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor } from "@testing-library/react";
import type { Channel } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));

const setChannelMember =
  vi.fn<(id: string, member: boolean) => Promise<number>>(() => Promise.resolve(0));
const openExternal = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());
const removeChannel = vi.fn<(id: string) => Promise<void>>(() => Promise.resolve());

vi.mock("../api", () => ({
  api: {
    classifyAddInput: () => Promise.resolve("channel"),
    addChannel: () => Promise.resolve({}),
    addVideo: () => Promise.resolve({}),
    removeChannel: (id: string) => removeChannel(id),
    previewTakeoutCsv: () => Promise.resolve([]),
    importTakeoutCsv: () => Promise.resolve({ added: 0, skipped: 0, failed: [] }),
    setChannelMember: (id: string, member: boolean) => setChannelMember(id, member),
    openExternal: (url: string) => openExternal(url),
  },
  errText: (e: unknown) => String(e),
}));

import AddChannelDialog from "./AddChannelDialog";
import { ToastProvider } from "./Toast";

function channel(over: Partial<Channel>): Channel {
  const c = {
    id: "UC1", title: "AnimeCapped Manga", handle: null, url: "", thumb_path: null,
    subscribed: true, member: false, added_at: 0, last_polled_at: null,
    ...over,
  };
  // The URL follows the id, as it does in the backend — otherwise every row in
  // a list would carry the first channel's link and a mix-up would still pass.
  return { ...c, url: c.url || `https://www.youtube.com/channel/${c.id}` };
}

const CHANNELS = [
  channel({ id: "UC1", title: "AnimeCapped Manga", member: true }),
  channel({ id: "UC2", title: "Daily Comics", member: false }),
];

function renderDialog(channels: Channel[] = CHANNELS) {
  const onChanged = vi.fn();
  render(
    <ToastProvider>
      <AddChannelDialog open onClose={vi.fn()} channels={channels} onChanged={onChanged} />
    </ToastProvider>,
  );
  return { onChanged };
}

/** The rows act through glyphs, so every button is found by its accessible name. */
const rowButton = (name: string) =>
  screen.getByRole("button", { name }) as HTMLButtonElement;
const memberChip = (title: string) => rowButton(`Members — ${title}`);

// A block body, not an expression: Vitest takes a value returned from a hook as
// a teardown callback, and returning the mock itself would have it *called*
// after every test.
beforeEach(() => {
  setChannelMember.mockReset().mockResolvedValue(0);
  openExternal.mockReset().mockResolvedValue(undefined);
  removeChannel.mockReset().mockResolvedValue(undefined);
});
afterEach(cleanup);

/**
 * The subscription list is the only place that shows every channel at once,
 * which is what makes membership readable at a glance rather than one channel
 * at a time.
 */
describe("membership in the subscriptions list", () => {
  it("lights the channels already joined and leaves the rest dark", () => {
    renderDialog();
    expect(memberChip("AnimeCapped Manga").getAttribute("aria-pressed")).toBe("true");
    expect(memberChip("Daily Comics").getAttribute("aria-pressed")).toBe("false");
  });

  it("joins a channel and says how many members-only videos that found", async () => {
    setChannelMember.mockResolvedValue(35);
    const { onChanged } = renderDialog();

    fireEvent.click(memberChip("Daily Comics"));

    await waitFor(() => expect(setChannelMember).toHaveBeenCalledWith("UC2", true));
    expect(await screen.findByText(/35 members-only videos added/)).toBeTruthy();
    // The feed has to pick them up without a manual refresh.
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("leaves a channel without touching what it already collected", async () => {
    renderDialog();
    fireEvent.click(memberChip("AnimeCapped Manga"));
    await waitFor(() => expect(setChannelMember).toHaveBeenCalledWith("UC1", false));
    expect(await screen.findByText(/Videos already collected stay/)).toBeTruthy();
  });

  /**
   * Joining reads a listing hundreds of entries deep and a watch page per
   * video it finds, so the row has to say it is working — and refuse a second
   * click, which would run the whole pass again.
   */
  it("says it is working, and will not run twice", async () => {
    let finish = (_n: number) => {};
    setChannelMember.mockImplementation(() => new Promise((res) => { finish = res; }));
    renderDialog();

    fireEvent.click(memberChip("Daily Comics"));
    await waitFor(() => expect(memberChip("Daily Comics").disabled).toBe(true));
    // The label is a glyph now, so "working" is the spinner and aria-busy.
    expect(memberChip("Daily Comics").getAttribute("aria-busy")).toBe("true");
    fireEvent.click(memberChip("Daily Comics"));
    expect(setChannelMember).toHaveBeenCalledTimes(1);

    // The other rows stay usable: one channel's listing is not the list's.
    expect(memberChip("AnimeCapped Manga").disabled).toBe(false);

    finish(0);
    await waitFor(() => expect(memberChip("Daily Comics").disabled).toBe(false));
    expect(await screen.findByText(/No members-only videos found/)).toBeTruthy();
  });
});

/**
 * This list is the only place a whole channel — rather than one of its videos —
 * is on screen, so it is where "take me to it on YouTube" belongs.
 */
describe("channel row actions", () => {
  it("opens a channel on YouTube", async () => {
    renderDialog();
    fireEvent.click(rowButton("Open Daily Comics on YouTube"));
    await waitFor(() =>
      expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/channel/UC2"));
  });

  it("asks before removing, and removes only on the second click", async () => {
    renderDialog();
    fireEvent.click(rowButton("Remove Daily Comics"));
    expect(removeChannel).not.toHaveBeenCalled();
    expect(screen.getByText("Remove and delete its videos?")).toBeTruthy();

    fireEvent.click(rowButton("Remove Daily Comics and delete its videos"));
    await waitFor(() => expect(removeChannel).toHaveBeenCalledWith("UC2"));
  });

  it("backs out of the confirmation without removing anything", () => {
    renderDialog();
    fireEvent.click(rowButton("Remove Daily Comics"));
    fireEvent.click(rowButton("Keep Daily Comics"));
    expect(removeChannel).not.toHaveBeenCalled();
    expect(screen.queryByText("Remove and delete its videos?")).toBeNull();
  });
});
