import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  render, screen, fireEvent, cleanup, waitFor, act, within,
} from "@testing-library/react";
import type {
  AddKind, AddPlaylistResult, AddPreview, Channel, PlaylistEntry, Quality, Settings, Video,
  VideoPreview,
} from "../types";
import { DEFAULT_QUALITY } from "../quality";

vi.mock("@tauri-apps/plugin-dialog", () => ({ open: () => Promise.resolve(null) }));

const classifyAddInput = vi.fn<(input: string) => Promise<AddKind>>();
const previewAdd = vi.fn<(input: string) => Promise<AddPreview>>();
const addVideo = vi.fn<(input: string) => Promise<Video>>();
const addChannel = vi.fn<(input: string) => Promise<Channel>>();
const addPlaylist = vi.fn<(entries: PlaylistEntry[]) => Promise<AddPlaylistResult>>();
const enqueueDownload = vi.fn<(id: string, q?: Quality | null) => Promise<void>>();
const enqueueDownloads = vi.fn<(ids: string[], q?: Quality | null) => Promise<number>>();
const probeFormats = vi.fn<(id: string) => Promise<never>>();

vi.mock("../api", () => ({
  api: {
    classifyAddInput: (input: string) => classifyAddInput(input),
    previewAdd: (input: string) => previewAdd(input),
    addVideo: (input: string) => addVideo(input),
    addChannel: (input: string) => addChannel(input),
    addPlaylist: (entries: PlaylistEntry[]) => addPlaylist(entries),
    enqueueDownload: (id: string, q?: Quality | null) => enqueueDownload(id, q),
    enqueueDownloads: (ids: string[], q?: Quality | null) => enqueueDownloads(ids, q),
    probeFormats: (id: string) => probeFormats(id),
    getSettings: () => Promise.resolve({ quality: DEFAULT_QUALITY } as Settings),
    subscriptionStats: () => Promise.resolve([]),
    previewTakeoutCsv: () => Promise.resolve([]),
  },
  errText: (e: unknown) => String(e),
  fileSrc: (path: string) => `asset://localhost${path}`,
}));

import AddChannelDialog from "./AddChannelDialog";
import { ToastProvider } from "./Toast";

const FOLLOWED: Channel = {
  id: "UCarashi", title: "Arashi Recap", handle: "@ArashiRecap",
  url: "https://www.youtube.com/channel/UCarashi", thumb_path: null, subscribed: true,
  member: false, auto_download: false, added_at: 0, last_polled_at: null, terminated: false,
};

function videoPreview(over: Partial<VideoPreview> = {}): VideoPreview {
  return {
    id: "dI1xnt0sw5I",
    title: "He Touched MAX LEVEL for One Night",
    channelId: "UCarashi", channelTitle: "Arashi Recap",
    durationSecs: 18961, publishedAt: Math.floor(Date.now() / 1000) - 9 * 3600,
    thumbUrl: "https://i.ytimg.com/vi/dI1xnt0sw5I/hqdefault.jpg", thumbPath: null,
    inLibrary: false, downloadState: "none", membersOnly: false,
    ...over,
  };
}

function entry(n: number, over: Partial<PlaylistEntry> = {}): PlaylistEntry {
  return {
    id: `part${n}`, title: `Part ${n} of the story`, channelId: "UCcap", channelTitle: "AnimeCapped",
    durationSecs: 3600, thumbUrl: `https://i.ytimg.com/vi/part${n}/hqdefault.jpg`, thumbPath: null,
    inLibrary: false, downloadState: "none", membersOnly: false,
    ...over,
  };
}

/** Five parts: 1 downloaded, 2 in the library, 3 to 5 new. */
const PARTS = [
  entry(1, { inLibrary: true, downloadState: "done" }),
  entry(2, { inLibrary: true }),
  entry(3),
  entry(4),
  entry(5, { membersOnly: true }),
];

function playlistPreview(entries = PARTS): AddPreview {
  return {
    kind: "playlist",
    playlist: {
      id: "PLstory", title: "The whole story", channelId: "UCcap", channelTitle: "AnimeCapped",
      total: entries.length, entries, unavailable: 0, truncated: false,
    },
  };
}

const VIDEO_URL = "https://youtu.be/dI1xnt0sw5I";
const PLAYLIST_URL = "https://www.youtube.com/playlist?list=PLstory";

async function renderDialog(channels: Channel[] = [FOLLOWED]) {
  const onChanged = vi.fn();
  const onClose = vi.fn();
  const onSubsFolded = vi.fn();
  await act(async () => {
    render(
      <ToastProvider>
        <AddChannelDialog
          open onClose={onClose} channels={channels} onChanged={onChanged}
          subsFolded onSubsFolded={onSubsFolded}
        />
      </ToastProvider>,
    );
  });
  return { onChanged, onClose, onSubsFolded };
}

const box = () =>
  screen.getByRole("textbox", { name: "Channel, video or playlist to add" }) as HTMLInputElement;

/** A paste is read at once, where typing waits for a pause. */
function paste(text: string) {
  fireEvent.paste(box());
  fireEvent.change(box(), { target: { value: text } });
}

const button = (name: string | RegExp) =>
  screen.getByRole("button", { name }) as HTMLButtonElement;
const findButton = async (name: string) =>
  (await screen.findByRole("button", { name })) as HTMLButtonElement;

beforeEach(() => {
  vi.clearAllMocks();
  classifyAddInput.mockImplementation((input) => Promise.resolve(
    /shorts\//.test(input) ? "short"
      : /[?&]v=|youtu\.be\//.test(input) ? "video"
        : /list=/.test(input) ? "playlist" : "channel",
  ));
  previewAdd.mockImplementation(() => Promise.resolve({ kind: "video", video: videoPreview(), playlistId: null }));
  addVideo.mockImplementation(() => Promise.resolve({ id: "dI1xnt0sw5I", title: "He Touched MAX LEVEL for One Night" } as Video));
  addChannel.mockImplementation(() => Promise.resolve({ ...FOLLOWED, id: "UCnew", title: "Someone New" }));
  addPlaylist.mockImplementation((entries) => Promise.resolve({
    added: entries.filter((e) => !e.inLibrary).map((e) => e.id),
    present: entries.filter((e) => e.inLibrary).map((e) => e.id),
    skippedShorts: [],
  }));
  enqueueDownload.mockImplementation(() => Promise.resolve());
  enqueueDownloads.mockImplementation((ids) => Promise.resolve(ids.length));
  probeFormats.mockImplementation(() => Promise.reject("offline"));
});
afterEach(cleanup);

describe("the input", () => {
  it("says what it takes until something is pasted", async () => {
    await renderDialog();
    expect(screen.getByText(
      "Paste a channel to subscribe to, or a video or playlist to add or download.",
    )).toBeTruthy();
  });

  it("names what a paste turned out to be", async () => {
    await renderDialog();
    paste(VIDEO_URL);
    expect(await screen.findByText("He Touched MAX LEVEL for One Night")).toBeTruthy();
    expect(screen.getByText("Video", { selector: ".add-kind" })).toBeTruthy();
  });

  it("waits for a pause in typing before it reads anything", async () => {
    await renderDialog();
    fireEvent.change(box(), { target: { value: "https://youtu.be/dI1" } });
    fireEvent.change(box(), { target: { value: VIDEO_URL } });
    await screen.findByText("He Touched MAX LEVEL for One Night");
    expect(classifyAddInput).toHaveBeenCalledTimes(1);
    expect(classifyAddInput).toHaveBeenCalledWith(VIDEO_URL);
  });

  it("refuses a Short in words, without asking YouTube about it", async () => {
    await renderDialog();
    paste("https://www.youtube.com/shorts/8Pae6Wy1abE");
    expect(await screen.findByText("Shorts are excluded from MyTube by design.")).toBeTruthy();
    expect(previewAdd).not.toHaveBeenCalled();
  });

  it("shows why a link could not be read, verbatim", async () => {
    previewAdd.mockImplementation(() => Promise.reject("The playlist does not exist."));
    await renderDialog();
    paste(PLAYLIST_URL);
    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("The playlist does not exist.");
    expect(alert.classList.contains("is-warn")).toBe(true);
  });

  it("drops an answer for text that has since been replaced", async () => {
    let lateVideo = (_p: AddPreview) => {};
    previewAdd
      .mockImplementationOnce(() => new Promise((res) => { lateVideo = res; }))
      .mockImplementationOnce(() => Promise.resolve(playlistPreview()));
    await renderDialog();
    paste(VIDEO_URL);
    await waitFor(() => expect(previewAdd).toHaveBeenCalledTimes(1));
    paste(PLAYLIST_URL);
    expect(await screen.findByText("The whole story")).toBeTruthy();
    await act(async () => {
      lateVideo({ kind: "video", video: videoPreview(), playlistId: null });
    });
    expect(screen.queryByText("He Touched MAX LEVEL for One Night")).toBeNull();
  });
});

describe("a channel", () => {
  it("offers to subscribe, and Enter does it", async () => {
    previewAdd.mockImplementation(() => Promise.resolve({ kind: "channel", subscribedId: null }));
    const { onChanged } = await renderDialog();
    paste("@SomeoneNew");
    await waitFor(() => expect(button("Subscribe").disabled).toBe(false));
    fireEvent.submit(box());
    await waitFor(() => expect(addChannel).toHaveBeenCalledWith("@SomeoneNew"));
    expect(await screen.findByText("Subscribed to Someone New.")).toBeTruthy();
    expect(box().value).toBe("");
    expect(onChanged).toHaveBeenCalled();
  });

  it("says when you already follow it, and takes you to it in the list", async () => {
    previewAdd.mockImplementation(() => Promise.resolve({ kind: "channel", subscribedId: "UCarashi" }));
    const { onSubsFolded } = await renderDialog();
    paste("@ArashiRecap");
    expect(await screen.findByText(/You're already subscribed to/)).toBeTruthy();
    expect(screen.queryByRole("button", { name: "Subscribe" })).toBeNull();
    // Enter has nothing to do.
    fireEvent.submit(box());
    expect(addChannel).not.toHaveBeenCalled();

    fireEvent.click(button("Show in list"));
    expect(onSubsFolded).toHaveBeenCalledWith(false);
    expect(button(/^Arashi Recap/).getAttribute("aria-expanded")).toBe("true");
  });
});

describe("a video", () => {
  it("adds without downloading, and Enter does the same", async () => {
    const { onChanged } = await renderDialog();
    paste(VIDEO_URL);
    await screen.findByText("He Touched MAX LEVEL for One Night");
    fireEvent.submit(box());
    await waitFor(() => expect(addVideo).toHaveBeenCalledWith(VIDEO_URL));
    expect(await screen.findByText("Added “He Touched MAX LEVEL for One Night”.")).toBeTruthy();
    expect(enqueueDownload).not.toHaveBeenCalled();
    expect(box().value).toBe("");
    expect(onChanged).toHaveBeenCalled();
  });

  it("downloads with the Settings quality", async () => {
    await renderDialog();
    paste(VIDEO_URL);
    await screen.findByText("He Touched MAX LEVEL for One Night");
    fireEvent.click(button("Download"));
    await waitFor(() => expect(enqueueDownload).toHaveBeenCalledWith("dI1xnt0sw5I", undefined));
    expect(addVideo).toHaveBeenCalledWith(VIDEO_URL);
    expect(await screen.findByText("Downloading “He Touched MAX LEVEL for One Night”.")).toBeTruthy();
  });

  it("sends one action at a time, however fast the buttons are pressed", async () => {
    let finish = (_v: Video) => {};
    addVideo.mockImplementation(() => new Promise((res) => { finish = res; }));
    await renderDialog();
    paste(VIDEO_URL);
    await screen.findByText("He Touched MAX LEVEL for One Night");
    fireEvent.click(button("Add"));
    fireEvent.click(button("Download"));
    fireEvent.click(button("Add"));
    expect(addVideo).toHaveBeenCalledTimes(1);
    expect(button("Download").disabled).toBe(true);
    await act(async () => { finish({ id: "dI1xnt0sw5I", title: "t" } as Video); });
    expect(enqueueDownload).not.toHaveBeenCalled();
  });

  it("says when it is already in the library, or downloaded, or on its way", async () => {
    await renderDialog();
    previewAdd.mockImplementation(() => Promise.resolve({
      kind: "video", video: videoPreview({ inLibrary: true }), playlistId: null,
    }));
    paste(VIDEO_URL);
    expect((await findButton("In your library")).disabled).toBe(true);
    expect(button("Download").disabled).toBe(false);

    previewAdd.mockImplementation(() => Promise.resolve({
      kind: "video", video: videoPreview({ inLibrary: true, downloadState: "done" }), playlistId: null,
    }));
    paste(`${VIDEO_URL}?t=1`);
    expect((await findButton("Downloaded")).disabled).toBe(true);
    expect(button("Download (custom)…").disabled).toBe(true);

    previewAdd.mockImplementation(() => Promise.resolve({
      kind: "video", video: videoPreview({ inLibrary: true, downloadState: "queued" }), playlistId: null,
    }));
    paste(`${VIDEO_URL}?t=2`);
    expect((await findButton("Downloading…")).disabled).toBe(true);

    // A failed one keeps Download: pressing it again is the retry.
    previewAdd.mockImplementation(() => Promise.resolve({
      kind: "video", video: videoPreview({ inLibrary: true, downloadState: "failed" }), playlistId: null,
    }));
    paste(`${VIDEO_URL}?t=3`);
    await waitFor(() => expect(button("Download").disabled).toBe(false));
  });

  it("marks a members-only video", async () => {
    previewAdd.mockImplementation(() => Promise.resolve({
      kind: "video", video: videoPreview({ membersOnly: true }), playlistId: null,
    }));
    await renderDialog();
    paste(VIDEO_URL);
    expect(await screen.findByText("Members-only")).toBeTruthy();
  });

  it("offers the whole playlist a part came from", async () => {
    previewAdd.mockImplementation((input) => Promise.resolve(
      input.includes("playlist?") ? playlistPreview()
        : { kind: "video", video: videoPreview(), playlistId: "PLstory" },
    ));
    await renderDialog();
    paste("https://www.youtube.com/watch?v=dI1xnt0sw5I&list=PLstory");
    fireEvent.click(await screen.findByRole("button", {
      name: "Part of a playlist — add the whole playlist instead",
    }));
    expect(box().value).toBe(PLAYLIST_URL);
    expect(await screen.findByText("The whole story")).toBeTruthy();
  });

  it("opens the custom download over this dialog, and Escape closes only that", async () => {
    const { onClose, onChanged } = await renderDialog();
    paste(VIDEO_URL);
    await screen.findByText("He Touched MAX LEVEL for One Night");
    fireEvent.click(button("Download (custom)…"));
    const custom = await screen.findByRole("dialog", { name: "Custom download" });
    // Added already, so the feed has it whichever way the dialog ends.
    expect(addVideo).toHaveBeenCalledWith(VIDEO_URL);
    expect(onChanged).toHaveBeenCalled();
    await within(custom).findByRole("alert");

    fireEvent.keyDown(window, { key: "Escape" });
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Custom download" })).toBeNull());
    expect(onClose).not.toHaveBeenCalled();
    // Read again: the card now knows the video is in the library.
    await waitFor(() => expect(previewAdd).toHaveBeenCalledTimes(2));

    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("downloads the custom way once the stacked dialog says so", async () => {
    await renderDialog();
    paste(VIDEO_URL);
    await screen.findByText("He Touched MAX LEVEL for One Night");
    fireEvent.click(button("Download (custom)…"));
    const custom = await screen.findByRole("dialog", { name: "Custom download" });
    await within(custom).findByRole("alert");
    fireEvent.click(within(custom).getByRole("button", { name: "Download" }));
    await waitFor(() => expect(enqueueDownload).toHaveBeenCalledWith("dI1xnt0sw5I", DEFAULT_QUALITY));
    expect(await screen.findByText("Downloading “He Touched MAX LEVEL for One Night”.")).toBeTruthy();
    expect(box().value).toBe("");
  });
});

describe("a playlist", () => {
  beforeEach(() => {
    previewAdd.mockImplementation(() => Promise.resolve(playlistPreview()));
  });

  it("sums up what is new and what is already here, with the parts folded away", async () => {
    await renderDialog();
    paste(PLAYLIST_URL);
    const card = await screen.findByRole("group", { name: "Playlist: The whole story" });
    expect(card.textContent).toContain("3 new");
    expect(card.textContent).toContain("2 in your library");
    expect(card.textContent).toContain("1 downloaded");
    expect(card.textContent).toContain("1 members-only");
    expect(card.textContent).toContain("AnimeCapped · 5 videos · 5h");
    expect(button("Add 3 new")).toBeTruthy();
    expect(button("Download 4")).toBeTruthy();
    const disclose = button("Show the 5 parts");
    expect(disclose.getAttribute("aria-expanded")).toBe("false");
    // Closed is not built: no checklist until it is asked for.
    expect(screen.queryAllByRole("checkbox")).toHaveLength(0);
  });

  it("counts only the ticked parts, and adds only those", async () => {
    const { onChanged } = await renderDialog();
    paste(PLAYLIST_URL);
    fireEvent.click(await screen.findByRole("button", { name: "Show the 5 parts" }));
    fireEvent.click(screen.getByRole("checkbox", { name: /Part 3 of the story/ }));
    expect(button("Add 2 new")).toBeTruthy();
    expect(button("Download 3")).toBeTruthy();
    expect(button(/Hide the 5 parts/).textContent).toContain("4 ticked");

    fireEvent.click(button("Add 2 new"));
    await waitFor(() => expect(addPlaylist).toHaveBeenCalledTimes(1));
    expect(addPlaylist.mock.calls[0][0].map((e) => e.id)).toEqual(["part1", "part2", "part4", "part5"]);
    expect(await screen.findByText("Added 2 videos.")).toBeTruthy();
    expect(enqueueDownloads).not.toHaveBeenCalled();
    expect(onChanged).toHaveBeenCalled();
    expect(box().value).toBe("");
  });

  it("ticks all or none at once", async () => {
    await renderDialog();
    paste(PLAYLIST_URL);
    fireEvent.click(await screen.findByRole("button", { name: "Show the 5 parts" }));
    const all = screen.getByRole("checkbox", { name: "All" }) as HTMLInputElement;
    expect(all.checked).toBe(true);
    fireEvent.click(all);
    expect(button("Add").disabled).toBe(true);
    expect(button("Download").disabled).toBe(true);
    fireEvent.click(screen.getByRole("checkbox", { name: /Part 4 of the story/ }));
    expect(all.indeterminate).toBe(true);
    fireEvent.click(all);
    expect(button("Add 3 new")).toBeTruthy();
  });

  it("downloads every ticked part not on disk, after adding them", async () => {
    await renderDialog();
    paste(PLAYLIST_URL);
    fireEvent.click(await screen.findByRole("button", { name: "Download 4" }));
    await waitFor(() => expect(enqueueDownloads).toHaveBeenCalled());
    expect(addPlaylist.mock.calls[0][0]).toHaveLength(5);
    expect(enqueueDownloads).toHaveBeenCalledWith(["part2", "part3", "part4", "part5"], undefined);
    expect(await screen.findByText("Queued 4 videos.")).toBeTruthy();
  });

  it("leaves out the Shorts it refused, and says so", async () => {
    addPlaylist.mockImplementation(() => Promise.resolve({
      added: ["part3", "part5"], present: ["part1", "part2"], skippedShorts: ["part4"],
    }));
    await renderDialog();
    paste(PLAYLIST_URL);
    fireEvent.click(await screen.findByRole("button", { name: "Download 4" }));
    await waitFor(() => expect(enqueueDownloads).toHaveBeenCalledWith(["part2", "part3", "part5"], undefined));
    expect(await screen.findByText("Queued 3 videos, 1 Short skipped.")).toBeTruthy();
  });

  it("says when every part is already in the library", async () => {
    previewAdd.mockImplementation(() => Promise.resolve(playlistPreview(
      PARTS.map((e) => ({ ...e, inLibrary: true })),
    )));
    await renderDialog();
    paste(PLAYLIST_URL);
    expect((await findButton("All in your library")).disabled).toBe(true);
    // Enter has nothing to add.
    fireEvent.submit(box());
    expect(addPlaylist).not.toHaveBeenCalled();
  });

  it("notes when the playlist runs past what was read", async () => {
    previewAdd.mockImplementation(() => Promise.resolve({
      kind: "playlist",
      playlist: {
        id: "PLlong", title: "Long", channelId: null, channelTitle: null,
        total: 1204, entries: PARTS, unavailable: 0, truncated: true,
      },
    }));
    await renderDialog();
    paste(PLAYLIST_URL);
    expect(await screen.findByText("Showing the first 5 of 1,204")).toBeTruthy();
  });

  it("downloads one custom quality for every part, from a form with no probe", async () => {
    await renderDialog();
    paste(PLAYLIST_URL);
    fireEvent.click(await screen.findByRole("button", { name: "Download (custom)…" }));
    const custom = await screen.findByRole("dialog", { name: "Custom download" });
    expect(within(custom).getByText(/One quality for all 4 videos/)).toBeTruthy();
    fireEvent.click(await within(custom).findByRole("button", { name: "Download 4" }));
    await waitFor(() => expect(enqueueDownloads).toHaveBeenCalledWith(
      ["part2", "part3", "part4", "part5"], DEFAULT_QUALITY,
    ));
    expect(probeFormats).not.toHaveBeenCalled();
    expect(await screen.findByText("Queued 4 videos.")).toBeTruthy();
  });
});
