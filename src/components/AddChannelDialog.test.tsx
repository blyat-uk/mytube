import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";
import type { Channel } from "../types";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));

const setChannelMember =
  vi.fn<(id: string, member: boolean) => Promise<number>>(() => Promise.resolve(0));
const openExternal = vi.fn<(url: string) => Promise<void>>(() => Promise.resolve());
const removeChannel = vi.fn<(id: string) => Promise<void>>(() => Promise.resolve());
type Backlog = { includeWatched: boolean; includeHidden: boolean };
const setChannelAutoDownload =
  vi.fn<(id: string, enabled: boolean, backlog: Backlog | null) => Promise<number>>(
    () => Promise.resolve(0));
const autoDownloadBacklogCount =
  vi.fn<(id: string, includeWatched: boolean, includeHidden: boolean) => Promise<number>>(
    () => Promise.resolve(0));

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
    setChannelAutoDownload: (id: string, enabled: boolean, backlog: Backlog | null) =>
      setChannelAutoDownload(id, enabled, backlog),
    autoDownloadBacklogCount: (id: string, w: boolean, h: boolean) =>
      autoDownloadBacklogCount(id, w, h),
  },
  errText: (e: unknown) => String(e),
}));

import AddChannelDialog from "./AddChannelDialog";
import { ToastProvider } from "./Toast";

function channel(over: Partial<Channel>): Channel {
  const c = {
    id: "UC1", title: "AnimeCapped Manga", handle: null, url: "", thumb_path: null,
    subscribed: true, member: false, auto_download: false, added_at: 0, last_polled_at: null,
    terminated: false,
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
  setChannelAutoDownload.mockReset().mockResolvedValue(0);
  autoDownloadBacklogCount.mockReset().mockResolvedValue(0);
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

  it("removes once, however often the confirmation is pressed", async () => {
    let finish = () => {};
    removeChannel.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    renderDialog();
    fireEvent.click(rowButton("Remove Daily Comics"));
    const yes = rowButton("Remove Daily Comics and delete its videos");
    fireEvent.click(yes);
    fireEvent.click(yes);
    expect(removeChannel).toHaveBeenCalledTimes(1);
    expect(yes.disabled).toBe(true);
    expect(yes.getAttribute("aria-busy")).toBe("true");
    // Too late to keep it once the removal is out.
    expect(rowButton("Keep Daily Comics").disabled).toBe(true);

    await act(async () => { finish(); });
    await waitFor(() => expect(screen.queryByText("Remove and delete its videos?")).toBeNull());
  });

  it("backs out of the confirmation without removing anything", () => {
    renderDialog();
    fireEvent.click(rowButton("Remove Daily Comics"));
    fireEvent.click(rowButton("Keep Daily Comics"));
    expect(removeChannel).not.toHaveBeenCalled();
    expect(screen.queryByText("Remove and delete its videos?")).toBeNull();
  });
});

/**
 * A channel whose account YouTube terminated stays listed — its videos are
 * still in the library — but there is nothing left to do with it except look.
 */
describe("a terminated channel", () => {
  const gone = channel({ id: "UC3", title: "Free manhwa", terminated: true });

  it("is struck through, and says why to a screen reader", () => {
    renderDialog([gone]);
    const name = screen.getByText(/Free manhwa/, { selector: ".channel-name" });
    expect(name.classList.contains("is-terminated")).toBe(true);
    expect(name.textContent).toContain("terminated by YouTube");
  });

  it("can still be opened on YouTube", async () => {
    renderDialog([gone]);
    fireEvent.click(rowButton("Open Free manhwa on YouTube"));
    await waitFor(() =>
      expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/channel/UC3"));
  });

  it("can be removed, still asking first", async () => {
    renderDialog([gone]);
    fireEvent.click(rowButton("Remove Free manhwa"));
    expect(removeChannel).not.toHaveBeenCalled();
    fireEvent.click(rowButton("Remove Free manhwa and delete its videos"));
    await waitFor(() => expect(removeChannel).toHaveBeenCalledWith("UC3"));
  });

  it("hides the members toggle but keeps its slot, so the glyphs still line up", () => {
    const { container } = render(
      <ToastProvider>
        <AddChannelDialog open onClose={vi.fn()} channels={[gone]} onChanged={vi.fn()} />
      </ToastProvider>,
    );
    expect(screen.queryByRole("button", { name: "Members — Free manhwa" })).toBeNull();
    const slot = container.querySelector(".channel-actions .is-placeholder") as HTMLButtonElement;
    expect(slot).not.toBeNull();
    expect(slot.disabled).toBe(true);
    expect(slot.tabIndex).toBe(-1);
    expect(container.querySelectorAll(".channel-actions .icon-btn")).toHaveLength(4);
  });

  it("hides the auto-download toggle too, keeping its slot", () => {
    const { container } = render(
      <ToastProvider>
        <AddChannelDialog open onClose={vi.fn()} channels={[gone]} onChanged={vi.fn()} />
      </ToastProvider>,
    );
    expect(screen.queryByRole("button", { name: "Auto-download — Free manhwa" })).toBeNull();
    expect(container.querySelectorAll(".channel-actions .is-placeholder")).toHaveLength(2);
  });

  it("leaves a live channel's row alone", () => {
    renderDialog([channel({ id: "UC2", title: "Daily Comics" })]);
    expect(screen.getByText("Daily Comics").classList.contains("is-terminated")).toBe(false);
    expect(rowButton("Remove Daily Comics").disabled).toBe(false);
  });
});

/**
 * Auto-download is opt-in per channel. Off is immediate and removes nothing;
 * on asks once whether the backlog already in the library comes too.
 */
describe("auto-download in the subscriptions list", () => {
  const autoChip = (title: string) => rowButton(`Auto-download — ${title}`);
  const AUTO = [
    channel({ id: "UC1", title: "AnimeCapped Manga", auto_download: true }),
    channel({ id: "UC2", title: "Daily Comics", auto_download: false }),
  ];

  it("shows which channels download on their own, naming each one", () => {
    renderDialog(AUTO);
    expect(autoChip("AnimeCapped Manga").getAttribute("aria-pressed")).toBe("true");
    expect(autoChip("Daily Comics").getAttribute("aria-pressed")).toBe("false");
  });

  it("turns off at once, with no prompt and no backlog", async () => {
    const { onChanged } = renderDialog(AUTO);
    fireEvent.click(autoChip("AnimeCapped Manga"));
    await waitFor(() =>
      expect(setChannelAutoDownload).toHaveBeenCalledWith("UC1", false, null));
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(await screen.findByText("Auto-download off for AnimeCapped Manga.")).toBeTruthy();
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("asks before turning on, and does nothing until confirmed", () => {
    renderDialog(AUTO);
    fireEvent.click(autoChip("Daily Comics"));
    expect(setChannelAutoDownload).not.toHaveBeenCalled();
    expect(screen.getByText("Auto-download Daily Comics")).toBeTruthy();
    const now = screen.getByRole("radio", { name: "From now on" }) as HTMLInputElement;
    expect(now.checked).toBe(true);
    // The count is only worth asking for once the backlog is in play.
    expect(autoDownloadBacklogCount).not.toHaveBeenCalled();
  });

  it("turns on from now on without a backlog", async () => {
    const { onChanged } = renderDialog(AUTO);
    fireEvent.click(autoChip("Daily Comics"));
    fireEvent.click(screen.getByRole("button", { name: "Turn on" }));
    await waitFor(() =>
      expect(setChannelAutoDownload).toHaveBeenCalledWith("UC2", true, null));
    expect(await screen.findByText("Auto-download on for Daily Comics.")).toBeTruthy();
    expect(screen.queryByRole("radiogroup")).toBeNull();
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("counts the backlog, recounts on every tick, and queues what it said", async () => {
    autoDownloadBacklogCount.mockImplementation((_id, w, h) =>
      Promise.resolve(27 + (w ? 10 : 0) + (h ? 100 : 0)));
    setChannelAutoDownload.mockResolvedValue(37);
    renderDialog(AUTO);
    fireEvent.click(autoChip("Daily Comics"));
    fireEvent.click(screen.getByRole("radio", { name: "Everything so far as well" }));

    await waitFor(() =>
      expect(autoDownloadBacklogCount).toHaveBeenLastCalledWith("UC2", false, false));
    expect(await screen.findByRole("button", { name: "Turn on & download 27" })).toBeTruthy();

    fireEvent.click(screen.getByRole("checkbox", { name: "Include watched" }));
    await waitFor(() =>
      expect(autoDownloadBacklogCount).toHaveBeenLastCalledWith("UC2", true, false));
    expect(await screen.findByRole("button", { name: "Turn on & download 37" })).toBeTruthy();

    fireEvent.click(screen.getByRole("checkbox", { name: "Include hidden" }));
    await waitFor(() =>
      expect(autoDownloadBacklogCount).toHaveBeenLastCalledWith("UC2", true, true));
    expect(await screen.findByRole("button", { name: "Turn on & download 137" })).toBeTruthy();
    fireEvent.click(screen.getByRole("checkbox", { name: "Include hidden" }));
    const confirm = await screen.findByRole("button", { name: "Turn on & download 37" });

    fireEvent.click(confirm);
    await waitFor(() =>
      expect(setChannelAutoDownload).toHaveBeenCalledWith(
        "UC2", true, { includeWatched: true, includeHidden: false }));
    expect(await screen.findByText("Auto-download on for Daily Comics. 37 videos queued.")).toBeTruthy();
  });

  it("keeps the newest count when an older one lands late", async () => {
    const pending: Array<(n: number) => void> = [];
    autoDownloadBacklogCount.mockImplementation(() => new Promise((res) => { pending.push(res); }));
    renderDialog(AUTO);
    fireEvent.click(autoChip("Daily Comics"));
    fireEvent.click(screen.getByRole("radio", { name: "Everything so far as well" }));
    await waitFor(() => expect(pending).toHaveLength(1));
    fireEvent.click(screen.getByRole("checkbox", { name: "Include watched" }));
    await waitFor(() => expect(pending).toHaveLength(2));

    pending[1](40);
    expect(await screen.findByRole("button", { name: "Turn on & download 40" })).toBeTruthy();
    await act(async () => { pending[0](5); });
    expect(screen.getByRole("button", { name: "Turn on & download 40" })).toBeTruthy();
  });

  it("with nothing to fetch, still reads plain Turn on", async () => {
    autoDownloadBacklogCount.mockResolvedValue(0);
    renderDialog(AUTO);
    fireEvent.click(autoChip("Daily Comics"));
    fireEvent.click(screen.getByRole("radio", { name: "Everything so far as well" }));
    await waitFor(() => expect(autoDownloadBacklogCount).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: "Turn on" }));
    await waitFor(() =>
      expect(setChannelAutoDownload).toHaveBeenCalledWith(
        "UC2", true, { includeWatched: false, includeHidden: false }));
  });

  it("cancels without calling anything", () => {
    renderDialog(AUTO);
    fireEvent.click(autoChip("Daily Comics"));
    fireEvent.click(screen.getByRole("button", { name: "Cancel auto-download for Daily Comics" }));
    expect(setChannelAutoDownload).not.toHaveBeenCalled();
    expect(screen.queryByRole("radiogroup")).toBeNull();
  });

  it("says it is working, and will not run twice", async () => {
    let finish = (_n: number) => {};
    setChannelAutoDownload.mockImplementation(() => new Promise((res) => { finish = res; }));
    renderDialog(AUTO);
    fireEvent.click(autoChip("AnimeCapped Manga"));
    await waitFor(() => expect(autoChip("AnimeCapped Manga").disabled).toBe(true));
    expect(autoChip("AnimeCapped Manga").getAttribute("aria-busy")).toBe("true");
    fireEvent.click(autoChip("AnimeCapped Manga"));
    expect(setChannelAutoDownload).toHaveBeenCalledTimes(1);
    expect(autoChip("Daily Comics").disabled).toBe(false);
    finish(0);
    await waitFor(() => expect(autoChip("AnimeCapped Manga").disabled).toBe(false));
  });
});
