import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, cleanup, waitFor, act } from "@testing-library/react";
import type { Quality, Settings, Video, VideoFormats } from "../types";
import { DEFAULT_QUALITY } from "../quality";
import { rickroll } from "../quality.fixture";

const probeFormats = vi.fn<(id: string) => Promise<VideoFormats>>();
const enqueueDownload = vi.fn<(id: string, q?: Quality | null) => Promise<void>>(
  () => Promise.resolve(),
);
let quality: Quality = DEFAULT_QUALITY;

vi.mock("../api", () => ({
  api: {
    probeFormats: (id: string) => probeFormats(id),
    enqueueDownload: (id: string, q?: Quality | null) => enqueueDownload(id, q),
    getSettings: () => Promise.resolve({ quality } as Settings),
  },
  errText: (e: unknown) => String(e),
}));

import CustomDownloadDialog from "./CustomDownloadDialog";

const VIDEO = { id: "dQw4w9WgXcQ", title: "Never Gonna Give You Up" } as Video;

/** A promise the test settles by hand, to hold the probe in flight. */
function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((res, rej) => { resolve = res; reject = rej; });
  return { promise, resolve, reject };
}

function renderDialog() {
  const onQueued = vi.fn();
  const onClose = vi.fn();
  const view = render(<CustomDownloadDialog video={VIDEO} onQueued={onQueued} onClose={onClose} />);
  return { onQueued, onClose, ...view };
}

const downloadButton = () => screen.getByRole("button", { name: "Download" }) as HTMLButtonElement;

beforeEach(() => {
  vi.clearAllMocks();
  quality = DEFAULT_QUALITY;
  probeFormats.mockImplementation(() => Promise.resolve(rickroll()));
  enqueueDownload.mockImplementation(() => Promise.resolve());
});
afterEach(cleanup);

describe("CustomDownloadDialog", () => {
  it("shows a spinner, and no way to download, while the probe runs", async () => {
    const probe = deferred<VideoFormats>();
    probeFormats.mockImplementation(() => probe.promise);
    renderDialog();
    expect(screen.getByText(/Reading the formats/)).toBeTruthy();
    expect(downloadButton().disabled).toBe(true);
    expect(probeFormats).toHaveBeenCalledWith("dQw4w9WgXcQ");
    await act(async () => { probe.resolve(rickroll()); });
    expect(screen.queryByText(/Reading the formats/)).toBeNull();
  });

  it("downloads the exact tracks picked from what the video has", async () => {
    const { onQueued, onClose } = renderDialog();
    const res = await screen.findByLabelText("Resolution") as HTMLSelectElement;
    // 1080p is the third height; the codec stays AV1, which it has.
    fireEvent.change(res, { target: { value: "2" } });
    fireEvent.click(downloadButton());

    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(enqueueDownload).toHaveBeenCalledWith(
      "dQw4w9WgXcQ", { ...DEFAULT_QUALITY, format: "399+251" },
    );
    expect(onQueued).toHaveBeenCalledWith(VIDEO);
  });

  it("falls back to the Settings preferences when the probe fails", async () => {
    quality = { ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3" };
    probeFormats.mockImplementation(() => Promise.reject("Sign in to confirm you're not a bot"));
    const { onClose } = renderDialog();

    expect((await screen.findByRole("alert")).textContent).toContain(
      "Sign in to confirm you're not a bot",
    );
    const format = screen.getByLabelText("Audio format") as HTMLSelectElement;
    expect(format.options[format.selectedIndex].text).toBe("MP3");

    fireEvent.click(downloadButton());
    await waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(enqueueDownload).toHaveBeenCalledWith("dQw4w9WgXcQ", quality);
  });

  it("stays open with the reason when queueing fails", async () => {
    enqueueDownload.mockImplementation(() => Promise.reject("yt-dlp is not installed"));
    const { onClose, onQueued } = renderDialog();
    await screen.findByLabelText("Resolution");
    fireEvent.click(downloadButton());
    expect(await screen.findByText("yt-dlp is not installed")).toBeTruthy();
    expect(onClose).not.toHaveBeenCalled();
    expect(onQueued).not.toHaveBeenCalled();
    expect(downloadButton().disabled).toBe(false);
  });

  it("closes on Cancel and on Escape without queueing anything", async () => {
    const { onClose } = renderDialog();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    fireEvent.keyDown(window, { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(2);
    expect(enqueueDownload).not.toHaveBeenCalled();
    await screen.findByLabelText("Resolution");
  });

  it("lets a probe that finishes after the dialog closed land nowhere", async () => {
    const probe = deferred<VideoFormats>();
    probeFormats.mockImplementation(() => probe.promise);
    const { unmount } = renderDialog();
    unmount();
    // Nothing to see and nothing to do: the late answer must neither throw
    // nor be taken as a go-ahead.
    await act(async () => { probe.resolve(rickroll()); });
    await act(async () => {});
    expect(enqueueDownload).not.toHaveBeenCalled();
  });
});
