import { useState } from "react";
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render, screen, fireEvent, cleanup, waitFor, act, within,
} from "@testing-library/react";
import type { Channel, ChannelStats } from "../types";

const pickFile = vi.fn<() => Promise<string | null>>(() => Promise.resolve(null));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => pickFile() }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

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
const subscriptionStats = vi.fn<() => Promise<ChannelStats[]>>(() => Promise.resolve([]));

vi.mock("../api", () => ({
  api: {
    classifyAddInput: () => Promise.resolve("channel"),
    previewAdd: () => Promise.resolve({ kind: "channel", subscribedId: null }),
    addChannel: () => Promise.resolve({}),
    addVideo: () => Promise.resolve({}),
    removeChannel: (id: string) => removeChannel(id),
    previewTakeoutCsv: () => Promise.resolve([
      { channelId: "UCaaaaaaaaaaaaaaaaaaaaaa", title: "Alpha", alreadySubscribed: false },
    ]),
    importTakeoutCsv: () => Promise.resolve({ added: 0, skipped: 0, failed: [] }),
    setChannelMember: (id: string, member: boolean) => setChannelMember(id, member),
    openExternal: (url: string) => openExternal(url),
    setChannelAutoDownload: (id: string, enabled: boolean, backlog: Backlog | null) =>
      setChannelAutoDownload(id, enabled, backlog),
    autoDownloadBacklogCount: (id: string, w: boolean, h: boolean) =>
      autoDownloadBacklogCount(id, w, h),
    subscriptionStats: () => subscriptionStats(),
  },
  errText: (e: unknown) => String(e),
  fileSrc: (path: string) => `asset://localhost${path}`,
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

function stats(channelId: string, over: Partial<ChannelStats> = {}): ChannelStats {
  return {
    channelId, videos: 0, downloaded: 0, watched: 0, hidden: 0, runtimeSecs: 0,
    latestUpload: null, followerCount: null, ...over,
  };
}

const CHANNELS = [
  channel({ id: "UC1", title: "AnimeCapped Manga", member: true }),
  channel({ id: "UC2", title: "Daily Comics", member: false }),
];

/** Renders the dialog and lets its first fetch (the counts) land inside act. */
async function renderDialog(channels: Channel[] = CHANNELS, { folded = false } = {}) {
  const onChanged = vi.fn();
  const onClose = vi.fn();
  const onSubsFolded = vi.fn();
  let view!: ReturnType<typeof render>;
  await act(async () => {
    view = render(
      <ToastProvider>
        <AddChannelDialog
          open
          onClose={onClose}
          channels={channels}
          onChanged={onChanged}
          subsFolded={folded}
          onSubsFolded={onSubsFolded}
        />
      </ToastProvider>,
    );
  });
  return { onChanged, onClose, onSubsFolded, container: view.container };
}

/** A row's header is the button that opens it, named by the channel first. */
const rowHead = (title: string) =>
  screen.getByRole("button", { name: new RegExp(`^${title}`) }) as HTMLButtonElement;
const panel = (title: string) => screen.getByRole("region", { name: title });
const inPanel = (title: string, name: string | RegExp) =>
  within(panel(title)).getByRole("button", { name }) as HTMLButtonElement;
function openRow(title: string) {
  fireEvent.click(rowHead(title));
  return panel(title);
}

// A block body, not an expression: Vitest takes a value returned from a hook as
// a teardown callback, and returning the mock itself would have it *called*
// after every test.
beforeEach(() => {
  setChannelMember.mockReset().mockResolvedValue(0);
  openExternal.mockReset().mockResolvedValue(undefined);
  removeChannel.mockReset().mockResolvedValue(undefined);
  setChannelAutoDownload.mockReset().mockResolvedValue(0);
  autoDownloadBacklogCount.mockReset().mockResolvedValue(0);
  subscriptionStats.mockReset().mockResolvedValue([]);
  pickFile.mockReset().mockResolvedValue(null);
});
afterEach(cleanup);

describe("importing a Takeout CSV", () => {
  it("lays the checklist over the dialog, and Escape closes only the checklist", async () => {
    pickFile.mockResolvedValue("/home/me/subscriptions.csv");
    const { onClose } = await renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Import CSV" }));
    expect(await screen.findByRole("dialog", { name: "Choose subscriptions to import" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Choose subscriptions to import" })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

/**
 * One row open at a time: the list stays a list, and the opened channel is
 * the one its panel's buttons act on.
 */
describe("opening a channel", () => {
  it("opens one row at a time, saying so to a screen reader", async () => {
    await renderDialog();
    expect(rowHead("AnimeCapped Manga").getAttribute("aria-expanded")).toBe("false");
    openRow("AnimeCapped Manga");
    expect(rowHead("AnimeCapped Manga").getAttribute("aria-expanded")).toBe("true");
    openRow("Daily Comics");
    expect(rowHead("Daily Comics").getAttribute("aria-expanded")).toBe("true");
    expect(rowHead("AnimeCapped Manga").getAttribute("aria-expanded")).toBe("false");
    // And the open one closes again.
    fireEvent.click(rowHead("Daily Comics"));
    expect(rowHead("Daily Comics").getAttribute("aria-expanded")).toBe("false");
  });

  it("counts each channel's videos on its row, and lays the rest out in its panel", async () => {
    const day = Date.UTC(2026, 7, 30, 12) / 1000;
    subscriptionStats.mockResolvedValue([
      stats("UC1", {
        videos: 334, downloaded: 19, watched: 58, hidden: 2, runtimeSecs: 980 * 3600,
        latestUpload: null, followerCount: 83_500,
      }),
    ]);
    await renderDialog([
      channel({ id: "UC1", title: "AnimeCapped Manga", handle: "@AnimeCapped25", added_at: day }),
    ]);
    expect(rowHead("AnimeCapped Manga").textContent).toContain("334 videos · 19");
    const p = openRow("AnimeCapped Manga");
    expect(rowHead("AnimeCapped Manga").textContent).toContain("@AnimeCapped25 · 83.5K subscribers");
    expect(p.textContent).toContain("watched · 17%");
    expect(p.textContent).toContain("980h");
    expect(p.textContent).toContain("30 Aug 2026");
    // Never polled yet.
    expect(p.textContent).toContain("Never");
  });

  it("reads a channel the counts do not mention as all zeros", async () => {
    subscriptionStats.mockResolvedValue([stats("UC1", { videos: 3 })]);
    await renderDialog();
    expect(rowHead("Daily Comics").textContent).toContain("0 videos · 0");
  });

  it("leaves out the second line when neither handle nor subscriber count is known", async () => {
    await renderDialog([channel({ id: "UC2", title: "Daily Comics" })]);
    openRow("Daily Comics");
    expect(rowHead("Daily Comics").querySelector(".add-ch-sub")).toBeNull();
  });

  it("counts again after a change made in the dialog", async () => {
    await renderDialog();
    expect(subscriptionStats).toHaveBeenCalledTimes(1);
    openRow("Daily Comics");
    fireEvent.click(inPanel("Daily Comics", "Join members"));
    await waitFor(() => expect(subscriptionStats).toHaveBeenCalledTimes(2));
  });
});

/**
 * Channel pictures arrive with the first poll after an update, and a file can
 * always go missing; until then a channel wears its initials.
 */
describe("channel pictures", () => {
  it("shows the cached picture when there is one", async () => {
    const { container } = await renderDialog([
      channel({ id: "UC1", title: "AnimeCapped Manga", thumb_path: "/c/thumbs/channels/UC1.jpg" }),
    ]);
    const img = container.querySelector("img.add-pic") as HTMLImageElement;
    expect(img.getAttribute("src")).toBe("asset://localhost/c/thumbs/channels/UC1.jpg");
  });

  it("falls back to the channel's initials with no picture, or one that will not load", async () => {
    const { container } = await renderDialog([
      channel({ id: "UC1", title: "AnimeCapped Manga", thumb_path: "/gone.jpg" }),
      channel({ id: "UC2", title: "100 MILLION DOLLARS RECAP" }),
    ]);
    expect(container.querySelector(".add-pic.is-initials")?.textContent).toBe("1M");
    fireEvent.error(container.querySelector("img.add-pic") as HTMLImageElement);
    const discs = [...container.querySelectorAll(".add-pic.is-initials")].map((e) => e.textContent);
    expect(discs).toEqual(["AM", "1M"]);
    expect(container.querySelector("img.add-pic")).toBeNull();
  });
});

/**
 * The list folds away, and the shell remembers how it was left: the dialog is
 * mostly opened to paste a link.
 */
describe("folding the list", () => {
  it("asks the shell to fold it, and the fold button says which way it is", async () => {
    const { onSubsFolded } = await renderDialog();
    const fold = screen.getByRole("button", { name: /^Subscriptions/ });
    expect(fold.getAttribute("aria-expanded")).toBe("true");
    fireEvent.click(fold);
    expect(onSubsFolded).toHaveBeenCalledWith(true);
  });

  it("opens folded when it was left folded, with nothing in it reachable", async () => {
    const { container } = await renderDialog(CHANNELS, { folded: true });
    const fold = screen.getByRole("button", { name: /^Subscriptions/ });
    expect(fold.getAttribute("aria-expanded")).toBe("false");
    expect(container.querySelector(".add-subs-body")?.hasAttribute("inert")).toBe(true);
    // The filter and Import go with the list.
    expect(container.querySelector(".add-subs-tools")?.getAttribute("aria-hidden")).toBe("true");
  });

  it("goes both ways under a shell that keeps the state", async () => {
    const remembered = vi.fn();
    function Shell() {
      const [folded, setFolded] = useState(false);
      return (
        <ToastProvider>
          <AddChannelDialog
            open onClose={vi.fn()} channels={CHANNELS} onChanged={vi.fn()}
            subsFolded={folded}
            onSubsFolded={(f) => { remembered(f); setFolded(f); }}
          />
        </ToastProvider>
      );
    }
    await act(async () => { render(<Shell />); });
    const fold = () => screen.getByRole("button", { name: /^Subscriptions/ });
    fireEvent.click(fold());
    expect(fold().getAttribute("aria-expanded")).toBe("false");
    fireEvent.click(fold());
    expect(fold().getAttribute("aria-expanded")).toBe("true");
    expect(remembered.mock.calls).toEqual([[true], [false]]);
  });
});

describe("filtering the list", () => {
  const MANY = [
    channel({ id: "UC1", title: "AnimeCapped Manga", handle: "@AnimeCapped25" }),
    channel({ id: "UC2", title: "Daily Comics", handle: "@dailycomics" }),
    channel({ id: "UC3", title: "Arashi Recap", handle: null }),
  ];
  const filterBox = () => screen.getByRole("textbox", { name: "Filter channels" });

  it("matches the name or the handle, in any case", async () => {
    await renderDialog(MANY);
    fireEvent.change(filterBox(), { target: { value: "RECAP" } });
    expect(screen.queryByRole("button", { name: /^Arashi Recap/ })).not.toBeNull();
    expect(screen.queryByRole("button", { name: /^Daily Comics/ })).toBeNull();
    fireEvent.change(filterBox(), { target: { value: "@anime" } });
    expect(screen.queryByRole("button", { name: /^AnimeCapped Manga/ })).not.toBeNull();
    expect(screen.queryByRole("button", { name: /^Arashi Recap/ })).toBeNull();
    fireEvent.change(filterBox(), { target: { value: "nothing like it" } });
    expect(screen.getByText("No channel matches “nothing like it”.")).toBeTruthy();
  });

  it("clears on Escape without closing the dialog, which the next Escape does", async () => {
    const { onClose } = await renderDialog(MANY);
    fireEvent.change(filterBox(), { target: { value: "daily" } });
    fireEvent.keyDown(filterBox(), { key: "Escape" });
    expect((filterBox() as HTMLInputElement).value).toBe("");
    expect(screen.queryByRole("button", { name: /^Arashi Recap/ })).not.toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    fireEvent.keyDown(filterBox(), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});

/**
 * The subscription list is the only place that shows every channel at once,
 * which is what makes membership readable at a glance rather than one channel
 * at a time: the star rides on the row, and the panel holds the toggle.
 */
describe("membership", () => {
  it("stars the channels already joined, and the panel's toggle agrees", async () => {
    await renderDialog();
    expect(rowHead("AnimeCapped Manga").textContent).toContain("member");
    expect(rowHead("Daily Comics").textContent).not.toContain("member");
    openRow("AnimeCapped Manga");
    expect(inPanel("AnimeCapped Manga", "Member").getAttribute("aria-pressed")).toBe("true");
    openRow("Daily Comics");
    expect(inPanel("Daily Comics", "Join members").getAttribute("aria-pressed")).toBe("false");
  });

  it("joins a channel and says how many members-only videos that found", async () => {
    setChannelMember.mockResolvedValue(35);
    const { onChanged } = await renderDialog();
    openRow("Daily Comics");
    fireEvent.click(inPanel("Daily Comics", "Join members"));

    await waitFor(() => expect(setChannelMember).toHaveBeenCalledWith("UC2", true));
    expect(await screen.findByText(/35 members-only videos added/)).toBeTruthy();
    // The feed has to pick them up without a manual refresh.
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("leaves a channel without touching what it already collected", async () => {
    await renderDialog();
    openRow("AnimeCapped Manga");
    fireEvent.click(inPanel("AnimeCapped Manga", "Member"));
    await waitFor(() => expect(setChannelMember).toHaveBeenCalledWith("UC1", false));
    expect(await screen.findByText(/Videos already collected stay/)).toBeTruthy();
  });

  /**
   * Joining reads a listing hundreds of entries deep and a watch page per
   * video it finds, so the button has to say it is working — and refuse a
   * second click, which would run the whole pass again.
   */
  it("says it is working, and will not run twice", async () => {
    let finish = (_n: number) => {};
    setChannelMember.mockImplementation(() => new Promise((res) => { finish = res; }));
    await renderDialog();
    openRow("Daily Comics");
    const join = () => inPanel("Daily Comics", "Join members");

    fireEvent.click(join());
    await waitFor(() => expect(join().disabled).toBe(true));
    expect(join().getAttribute("aria-busy")).toBe("true");
    fireEvent.click(join());
    expect(setChannelMember).toHaveBeenCalledTimes(1);

    // Another channel stays usable: one channel's listing is not the list's.
    openRow("AnimeCapped Manga");
    expect(inPanel("AnimeCapped Manga", "Member").disabled).toBe(false);

    finish(0);
    expect(await screen.findByText(/No members-only videos found/)).toBeTruthy();
  });
});

/**
 * This list is the only place a whole channel — rather than one of its videos —
 * is on screen, so it is where "take me to it on YouTube" belongs.
 */
describe("a channel's actions", () => {
  it("opens a channel on YouTube", async () => {
    await renderDialog();
    openRow("Daily Comics");
    fireEvent.click(inPanel("Daily Comics", "Open on YouTube"));
    await waitFor(() =>
      expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/channel/UC2"));
  });

  it("asks before removing, naming what goes with it, and removes only when told", async () => {
    subscriptionStats.mockResolvedValue([stats("UC2", { videos: 207 })]);
    const { onChanged } = await renderDialog();
    openRow("Daily Comics");
    fireEvent.click(inPanel("Daily Comics", "Remove"));
    expect(removeChannel).not.toHaveBeenCalled();
    const ask = within(panel("Daily Comics")).getByRole("group", { name: "Remove Daily Comics?" });
    expect(ask.textContent).toContain("Remove Daily Comics and delete its 207 videos?");
    // The question stands where the actions were.
    expect(within(panel("Daily Comics")).queryByRole("button", { name: "Open on YouTube" })).toBeNull();

    fireEvent.click(within(ask).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(removeChannel).toHaveBeenCalledWith("UC2"));
    expect(await screen.findByText("Removed Daily Comics and its videos.")).toBeTruthy();
    expect(onChanged).toHaveBeenCalled();
  });

  it("removes once, however often the confirmation is pressed", async () => {
    let finish = () => {};
    removeChannel.mockImplementation(() => new Promise<void>((res) => { finish = res; }));
    await renderDialog();
    openRow("Daily Comics");
    fireEvent.click(inPanel("Daily Comics", "Remove"));
    const ask = within(panel("Daily Comics")).getByRole("group");
    const yes = within(ask).getByRole("button", { name: "Remove" }) as HTMLButtonElement;
    fireEvent.click(yes);
    fireEvent.click(yes);
    expect(removeChannel).toHaveBeenCalledTimes(1);
    expect(yes.disabled).toBe(true);
    expect(yes.getAttribute("aria-busy")).toBe("true");
    // Too late to keep it once the removal is out.
    expect((within(ask).getByRole("button", { name: "Keep" }) as HTMLButtonElement).disabled)
      .toBe(true);

    await act(async () => { finish(); });
    await waitFor(() => expect(screen.queryByRole("group", { name: "Remove Daily Comics?" })).toBeNull());
  });

  it("backs out of the confirmation without removing anything", async () => {
    await renderDialog();
    openRow("Daily Comics");
    fireEvent.click(inPanel("Daily Comics", "Remove"));
    fireEvent.click(inPanel("Daily Comics", "Keep"));
    expect(removeChannel).not.toHaveBeenCalled();
    expect(screen.queryByRole("group", { name: "Remove Daily Comics?" })).toBeNull();
    expect(inPanel("Daily Comics", "Open on YouTube")).toBeTruthy();
  });
});

/**
 * A channel whose account YouTube terminated stays listed — its videos are
 * still in the library — but there is nothing left to do with it except look,
 * open it and remove it.
 */
describe("a terminated channel", () => {
  const gone = channel({ id: "UC3", title: "Free manhwa", terminated: true });

  it("is struck through, and says why to a screen reader", async () => {
    const { container } = await renderDialog([gone]);
    expect(container.querySelector(".add-ch")?.classList.contains("is-terminated")).toBe(true);
    expect(rowHead("Free manhwa").textContent).toContain("terminated by YouTube");
  });

  it("offers only Open and Remove, and says its videos stay", async () => {
    await renderDialog([gone]);
    const p = openRow("Free manhwa");
    const names = within(p).getAllByRole("button").map((b) => b.textContent);
    expect(names).toEqual(["Open on YouTube", "Remove"]);
    expect(p.textContent).toContain("YouTube has terminated this account");
    expect(p.textContent).toContain("stay in your library");
  });

  it("can still be opened on YouTube, and removed after asking", async () => {
    await renderDialog([gone]);
    openRow("Free manhwa");
    fireEvent.click(inPanel("Free manhwa", "Open on YouTube"));
    await waitFor(() =>
      expect(openExternal).toHaveBeenCalledWith("https://www.youtube.com/channel/UC3"));
    fireEvent.click(inPanel("Free manhwa", "Remove"));
    expect(removeChannel).not.toHaveBeenCalled();
    fireEvent.click(within(screen.getByRole("group", { name: "Remove Free manhwa?" }))
      .getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(removeChannel).toHaveBeenCalledWith("UC3"));
  });

  it("leaves a live channel's row alone", async () => {
    const { container } = await renderDialog([channel({ id: "UC2", title: "Daily Comics" })]);
    expect(container.querySelector(".add-ch")?.classList.contains("is-terminated")).toBe(false);
  });
});

/**
 * Auto-download is opt-in per channel. Off is immediate and removes nothing;
 * on asks once, inside the panel, whether the backlog already in the library
 * comes too.
 */
describe("auto-download", () => {
  const AUTO = [
    channel({ id: "UC1", title: "AnimeCapped Manga", auto_download: true }),
    channel({ id: "UC2", title: "Daily Comics", auto_download: false }),
  ];
  const autoToggle = (title: string) => inPanel(title, "Auto-download");

  it("marks the channels that download on their own, on the row and in the panel", async () => {
    await renderDialog(AUTO);
    expect(rowHead("AnimeCapped Manga").textContent).toContain("auto-download on");
    expect(rowHead("Daily Comics").textContent).not.toContain("auto-download on");
    openRow("AnimeCapped Manga");
    expect(autoToggle("AnimeCapped Manga").getAttribute("aria-pressed")).toBe("true");
    openRow("Daily Comics");
    expect(autoToggle("Daily Comics").getAttribute("aria-pressed")).toBe("false");
  });

  it("turns off at once, with no prompt and no backlog", async () => {
    const { onChanged } = await renderDialog(AUTO);
    openRow("AnimeCapped Manga");
    fireEvent.click(autoToggle("AnimeCapped Manga"));
    await waitFor(() =>
      expect(setChannelAutoDownload).toHaveBeenCalledWith("UC1", false, null));
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(await screen.findByText("Auto-download off for AnimeCapped Manga.")).toBeTruthy();
    await waitFor(() => expect(onChanged).toHaveBeenCalled());
  });

  it("asks before turning on, inside the panel, and does nothing until confirmed", async () => {
    await renderDialog(AUTO);
    const p = openRow("Daily Comics");
    fireEvent.click(autoToggle("Daily Comics"));
    expect(setChannelAutoDownload).not.toHaveBeenCalled();
    expect(within(p).getByText("Auto-download Daily Comics")).toBeTruthy();
    const now = within(p).getByRole("radio", { name: "From now on" }) as HTMLInputElement;
    expect(now.checked).toBe(true);
    // The count is only worth asking for once the backlog is in play.
    expect(autoDownloadBacklogCount).not.toHaveBeenCalled();
  });

  it("turns on from now on without a backlog", async () => {
    const { onChanged } = await renderDialog(AUTO);
    openRow("Daily Comics");
    fireEvent.click(autoToggle("Daily Comics"));
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
    await renderDialog(AUTO);
    openRow("Daily Comics");
    fireEvent.click(autoToggle("Daily Comics"));
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
    await renderDialog(AUTO);
    openRow("Daily Comics");
    fireEvent.click(autoToggle("Daily Comics"));
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
    await renderDialog(AUTO);
    openRow("Daily Comics");
    fireEvent.click(autoToggle("Daily Comics"));
    fireEvent.click(screen.getByRole("radio", { name: "Everything so far as well" }));
    await waitFor(() => expect(autoDownloadBacklogCount).toHaveBeenCalled());
    fireEvent.click(await screen.findByRole("button", { name: "Turn on" }));
    await waitFor(() =>
      expect(setChannelAutoDownload).toHaveBeenCalledWith(
        "UC2", true, { includeWatched: false, includeHidden: false }));
  });

  it("cancels without calling anything, and Escape backs out of the prompt only", async () => {
    const { onClose } = await renderDialog(AUTO);
    openRow("Daily Comics");
    fireEvent.click(autoToggle("Daily Comics"));
    fireEvent.click(screen.getByRole("button", { name: "Cancel auto-download for Daily Comics" }));
    expect(setChannelAutoDownload).not.toHaveBeenCalled();
    expect(screen.queryByRole("radiogroup")).toBeNull();

    fireEvent.click(autoToggle("Daily Comics"));
    fireEvent.keyDown(screen.getByRole("radio", { name: "From now on" }), { key: "Escape" });
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
  });

  it("says it is working, and will not run twice", async () => {
    let finish = (_n: number) => {};
    setChannelAutoDownload.mockImplementation(() => new Promise((res) => { finish = res; }));
    await renderDialog(AUTO);
    openRow("AnimeCapped Manga");
    fireEvent.click(autoToggle("AnimeCapped Manga"));
    await waitFor(() => expect(autoToggle("AnimeCapped Manga").disabled).toBe(true));
    expect(autoToggle("AnimeCapped Manga").getAttribute("aria-busy")).toBe("true");
    fireEvent.click(autoToggle("AnimeCapped Manga"));
    expect(setChannelAutoDownload).toHaveBeenCalledTimes(1);
    finish(0);
    await waitFor(() => expect(autoToggle("AnimeCapped Manga").disabled).toBe(false));
  });
});
