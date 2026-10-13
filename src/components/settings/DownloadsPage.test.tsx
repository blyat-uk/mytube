import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { screen, fireEvent, cleanup, waitFor, within } from "@testing-library/react";

vi.mock("@tauri-apps/api/event", async () => (await import("./testApi")).eventModule);
vi.mock("@tauri-apps/plugin-dialog", async () => (await import("./testApi")).dialogModule);
vi.mock("../../api", async () => (await import("./testApi")).apiModule);

import { api, backend, dialog, lastSaved, resetHarness, SETTINGS } from "./testApi";
import { renderSettings } from "./testRender";
import { summary } from "./DownloadsPage";
import type { SettingsCtx } from "./context";
import { DEFAULT_QUALITY } from "../../quality";
import { TEMPLATE_PRESETS } from "../../format";
import type { Quality, Settings, Video } from "../../types";

beforeEach(resetHarness);
afterEach(cleanup);

const group = (name: string) => screen.getByRole("radiogroup", { name });
const radio = (groupName: string, name: string | RegExp) =>
  within(group(groupName)).getByRole("radio", { name }) as HTMLInputElement;
const saves = () => api.saveSettings.mock.calls.length;
const withQuality = (q: Partial<Quality>): Settings => ({ ...SETTINGS, quality: { ...DEFAULT_QUALITY, ...q } });

const preview = () => document.querySelector(".st-dl-preview")!;

const VIDEO: Video = {
  id: "1VPfZ_XzisU", channel_id: "UCHnyfMqiRRG1u-2MsSQLbXA", channel_title: "Veritasium",
  title: "The Bizarre Behavior of Rotating Bodies", description: null, thumb_url: null,
  thumb_path: null, published_at: Date.UTC(2019, 8, 19, 15, 0) / 1000, sort_at: null,
  feed_rank: 0, added_manually: false, duration_secs: 896, view_count: 12_000_000,
  status: "ready", hidden: false, watched: false, watched_at: null, download_state: "none",
  download_error: null, file_path: null, downloaded_at: null, first_seen_at: 0,
  sibling_group: null,
  series_hidden: false,
};

describe("the page", () => {
  it("opens with its header", async () => {
    await renderSettings("downloads");
    expect(screen.getByRole("heading", { level: 2, name: "Downloads" })).toBeTruthy();
    expect(screen.getByText(/Where files land, what they are called/)).toBeTruthy();
  });
});

describe("Saved to", () => {
  it("draws the folder as segments, its own name last and bold", async () => {
    await renderSettings("downloads");
    expect(document.querySelector(".st-dl-path-inner")!.textContent).toBe("home/someone/Videos");
    const dim = [...document.querySelectorAll(".st-dl-path-dim")].map((e) => e.textContent);
    expect(dim).toEqual(["home", "someone"]);
    // The drawing is hidden from a screen reader, which reads the path itself.
    expect(screen.getByText("/home/someone/Videos").className).toBe("sr-only");
  });

  it("keeps a Windows path's own separator", async () => {
    backend.settings = { ...SETTINGS, download_dir: "C:\\Users\\me\\Videos" };
    await renderSettings("downloads");
    expect(document.querySelector(".st-dl-path-inner")!.textContent).toBe("C:\\Users\\me\\Videos");
  });

  it("swaps in a focused field on Edit, and saves once on Enter", async () => {
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const field = screen.getByLabelText("Download folder") as HTMLInputElement;
    expect(document.activeElement).toBe(field);
    expect(field.value).toBe("/home/someone/Videos");

    fireEvent.change(field, { target: { value: "/mnt/media/YouTube" } });
    expect(saves()).toBe(0);
    fireEvent.keyDown(field, { key: "Enter" });

    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved()).toEqual({ ...SETTINGS, download_dir: "/mnt/media/YouTube" });
    expect(screen.queryByLabelText("Download folder")).toBeNull();
    expect(document.querySelector(".st-dl-path-inner")!.textContent).toBe("mnt/media/YouTube");
    // Back where the keyboard left off.
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Edit" }));
  });

  it("saves on blur, like every other text field", async () => {
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const field = screen.getByLabelText("Download folder");
    fireEvent.change(field, { target: { value: "/srv/videos" } });
    fireEvent.blur(field);
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().download_dir).toBe("/srv/videos");
    expect(screen.queryByLabelText("Download folder")).toBeNull();
  });

  it("puts the folder back on Escape and saves nothing", async () => {
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "Edit" }));
    const field = screen.getByLabelText("Download folder");
    fireEvent.change(field, { target: { value: "/oops" } });
    fireEvent.keyDown(field, { key: "Escape" });

    expect(screen.queryByLabelText("Download folder")).toBeNull();
    expect(screen.getByText("/home/someone/Videos")).toBeTruthy();
    // A blur arriving late, as the field goes, must not save the abandoned edit.
    fireEvent.blur(field);
    await new Promise((r) => setTimeout(r, 0));
    expect(saves()).toBe(0);
  });

  it("saves a folder picked with Choose… at once", async () => {
    dialog.open.mockImplementation(() => Promise.resolve("/mnt/media/YouTube"));
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));
    await waitFor(() => expect(saves()).toBe(1));
    expect(dialog.open).toHaveBeenCalledWith({
      directory: true, multiple: false, title: "Choose a download folder",
    });
    expect(lastSaved()).toEqual({ ...SETTINGS, download_dir: "/mnt/media/YouTube" });
  });

  it("saves nothing when the folder dialog is cancelled", async () => {
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));
    await waitFor(() => expect(dialog.open).toHaveBeenCalledTimes(1));
    await new Promise((r) => setTimeout(r, 0));
    expect(saves()).toBe(0);
  });

  it("says why when the folder dialog fails", async () => {
    dialog.open.mockImplementation(() => Promise.reject("No file chooser available"));
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "Choose…" }));
    expect(await screen.findByText("No file chooser available")).toBeTruthy();
    expect(saves()).toBe(0);
  });

  it("opens one folder dialog for a double click", async () => {
    let answer!: (p: string | null) => void;
    dialog.open.mockImplementation(() => new Promise((r) => { answer = r; }));
    await renderSettings("downloads");
    const choose = screen.getByRole("button", { name: "Choose…" });
    fireEvent.click(choose);
    fireEvent.click(choose);
    expect(dialog.open).toHaveBeenCalledTimes(1);
    answer(null);
  });
});

describe("Each video is saved as", () => {
  it("shows the sample when the library is empty, starting from the folder's name", async () => {
    await renderSettings("downloads");
    await waitFor(() => expect(api.listVideos).toHaveBeenCalled());
    expect(preview().textContent).toBe("Videos/Some video.mkv");
    expect(screen.queryByText("shown with your newest video")).toBeNull();
  });

  it("asks for exactly one video, the newest, whatever the feed is filtered to", async () => {
    await renderSettings("downloads");
    await waitFor(() => expect(api.listVideos).toHaveBeenCalledTimes(1));
    expect(api.listVideos).toHaveBeenCalledWith({
      channelIds: [], hideWatched: false, downloadedOnly: false, showHidden: false,
      groupsOnly: false, inProgress: false, search: null, siblingOf: null, sort: "newest",
      limit: 1, offset: 0,
    });
  });

  it("renders the template against the newest video, folders as chips", async () => {
    backend.videos = [VIDEO];
    backend.settings = { ...SETTINGS, filename_template: TEMPLATE_PRESETS[3].value };
    await renderSettings("downloads");
    await screen.findByText("shown with your newest video");
    expect(preview().textContent).toBe(
      "Videos/Veritasium/2019/The Bizarre Behavior of Rotating Bodies [1VPfZ_XzisU].mkv",
    );
    const chips = [...document.querySelectorAll(".st-dl-preview-dir")].map((e) => e.textContent);
    expect(chips).toEqual(["Veritasium", "2019"]);
    expect(document.querySelector(".st-dl-preview-file")!.textContent).toBe(
      "The Bizarre Behavior of Rotating Bodies [1VPfZ_XzisU].mkv",
    );
  });

  it("falls back to the sample when the library cannot be read", async () => {
    api.listVideos.mockImplementation(() => Promise.reject("database is locked"));
    backend.settings = { ...SETTINGS, filename_template: TEMPLATE_PRESETS[0].value };
    await renderSettings("downloads");
    await waitFor(() => expect(api.listVideos).toHaveBeenCalled());
    expect(preview().textContent).toBe("Videos/Some channel/Some video [abc123def45].mkv");
  });

  it("follows the quality's extension", async () => {
    backend.settings = withQuality({ mode: "audio", audio_format: "mp3" });
    await renderSettings("downloads");
    expect(preview().textContent).toBe("Videos/Some video.mp3");
    fireEvent.click(radio("Audio format", "Opus"));
    expect(preview().textContent).toBe("Videos/Some video.opus");
    fireEvent.click(radio("Mode", "Video"));
    expect(preview().textContent).toBe("Videos/Some video.mkv");
    // Two quick picks: queued saves collapse to what is newest when each runs.
    await waitFor(() => expect(lastSaved().quality.mode).toBe("video"));
    expect(lastSaved().quality.audio_format).toBe("opus");
  });

  it("follows the template as it is typed", async () => {
    await renderSettings("downloads");
    fireEvent.change(screen.getByLabelText("Custom template"), {
      target: { value: "%(uploader)s/%(id)s.%(ext)s" },
    });
    expect(preview().textContent).toBe("Videos/Some channel/abc123def45.mkv");
  });
});

describe("the file name template", () => {
  const presetSettings = () => {
    backend.settings = { ...SETTINGS, filename_template: TEMPLATE_PRESETS[0].value };
  };

  it("shows a preset template as its preset, with the template itself beside it", async () => {
    presetSettings();
    await renderSettings("downloads");
    const names = within(group("File name template")).getAllByRole("radio")
      .map((r) => r.closest("label")!.textContent);
    expect(names).toEqual([...TEMPLATE_PRESETS.map((p) => p.label), "Custom…"]);
    expect(radio("File name template", "Channel folder").checked).toBe(true);
    expect(document.querySelector(".st-dl-tmpl-raw")!.textContent).toBe(TEMPLATE_PRESETS[0].value);
    expect(screen.queryByLabelText("Custom template")).toBeNull();
  });

  it("saves a preset the moment it is picked", async () => {
    presetSettings();
    await renderSettings("downloads");
    fireEvent.click(radio("File name template", "Flat"));
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved()).toEqual({ ...SETTINGS, filename_template: "%(title)s [%(id)s].%(ext)s" });
  });

  it("opens Custom for a template that matches no preset", async () => {
    await renderSettings("downloads");
    expect(radio("File name template", "Custom…").checked).toBe(true);
    expect((screen.getByLabelText("Custom template") as HTMLInputElement).value).toBe("%(title)s.%(ext)s");
    expect(screen.getByRole("group", { name: "Insert a field" })).toBeTruthy();
  });

  it("opens the field on Custom… without saving, holds typing until blur, and closes on a preset", async () => {
    presetSettings();
    await renderSettings("downloads");
    fireEvent.click(radio("File name template", "Custom…"));
    const field = screen.getByLabelText("Custom template") as HTMLInputElement;
    expect(field.value).toBe(TEMPLATE_PRESETS[0].value);
    expect(saves()).toBe(0);

    // Typing that happens to spell another preset keeps the field open.
    fireEvent.change(field, { target: { value: TEMPLATE_PRESETS[2].value } });
    expect(screen.getByLabelText("Custom template")).toBe(field);
    expect(radio("File name template", "Custom…").checked).toBe(true);
    expect(saves()).toBe(0);
    fireEvent.blur(field);
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().filename_template).toBe(TEMPLATE_PRESETS[2].value);

    fireEvent.click(radio("File name template", "Channel / year"));
    await waitFor(() => expect(saves()).toBe(2));
    expect(lastSaved().filename_template).toBe(TEMPLATE_PRESETS[3].value);
    expect(screen.queryByLabelText("Custom template")).toBeNull();
  });

  it("inserts a field at the caret, saves it, and keeps the caret after it", async () => {
    await renderSettings("downloads");
    const field = screen.getByLabelText("Custom template") as HTMLInputElement;
    field.focus();
    field.setSelectionRange(9, 9); // after "%(title)s"
    const chip = screen.getByRole("button", { name: "%(id)s" });
    fireEvent.mouseDown(chip);
    fireEvent.click(chip);
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().filename_template).toBe("%(title)s%(id)s.%(ext)s");
    await waitFor(() => expect(field.selectionStart).toBe(15));
    expect(document.activeElement).toBe(field);
  });

  it("remembers the caret when a chip is reached from the keyboard", async () => {
    await renderSettings("downloads");
    const field = screen.getByLabelText("Custom template") as HTMLInputElement;
    field.focus();
    field.setSelectionRange(0, 0);
    fireEvent.blur(field);
    const chip = screen.getByRole("button", { name: "%(uploader)s" });
    chip.focus();
    fireEvent.click(chip);
    await waitFor(() => expect(lastSaved().filename_template).toBe("%(uploader)s%(title)s.%(ext)s"));
  });

  it("appends a field to a template never clicked into", async () => {
    await renderSettings("downloads");
    fireEvent.click(screen.getByRole("button", { name: "%(id)s" }));
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().filename_template).toBe("%(title)s.%(ext)s%(id)s");
  });
});

describe("Quality", () => {
  const ladderSteps = () => within(group("Resolution")).getAllByRole("radio") as HTMLInputElement[];
  const stepClass = () => ladderSteps().map((r) => {
    const c = r.closest("label")!.classList;
    return c.contains("is-on") ? "on" : c.contains("is-in") ? "in" : "-";
  });

  it("shows the defaults", async () => {
    await renderSettings("downloads");
    expect(radio("Mode", "Video").checked).toBe(true);
    expect(radio("Resolution", "Best").checked).toBe(true);
    expect(radio("Codec", "Any").checked).toBe(true);
    expect(radio("Frame rate", "Any").checked).toBe(true);
    expect(radio("Container", "MKV").checked).toBe(true);
    expect(screen.queryByRole("radiogroup", { name: "Audio format" })).toBeNull();
    expect(screen.getByText("best available")).toBeTruthy();
  });

  it("shows the saved preferences", async () => {
    backend.settings = withQuality({ max_height: 1080, container: "mp4", fps: "prefer60" });
    await renderSettings("downloads");
    expect(radio("Resolution", "1080p").checked).toBe(true);
    expect(radio("Container", "MP4").checked).toBe(true);
    expect(radio("Frame rate", "Prefer 60").checked).toBe(true);
    expect(screen.getByText("up to 1080p")).toBeTruthy();
    expect(screen.getByText("smaller videos download as they are")).toBeTruthy();
  });

  it("draws the defaults for a settings object with no quality block", async () => {
    const { quality: _, ...rest } = SETTINGS;
    backend.settings = rest as Settings;
    await renderSettings("downloads");
    expect(radio("Mode", "Video").checked).toBe(true);
    expect(radio("Resolution", "Best").checked).toBe(true);
  });

  it("saves a pick the moment it changes, with the rest of the settings", async () => {
    await renderSettings("downloads");
    fireEvent.click(radio("Codec", /^AV1/));
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved()).toEqual({ ...SETTINGS, quality: { ...DEFAULT_QUALITY, vcodec: "av1" } });
  });

  it("hands every pick back as a whole Quality", async () => {
    await renderSettings("downloads");
    fireEvent.click(radio("Resolution", "1080p"));
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().quality).toEqual({ ...DEFAULT_QUALITY, max_height: 1080 });
    fireEvent.click(radio("Codec", /^H\.264/));
    await waitFor(() => expect(saves()).toBe(2));
    expect(lastSaved().quality).toEqual({ ...DEFAULT_QUALITY, max_height: 1080, vcodec: "h264" });
    fireEvent.click(radio("Frame rate", "At most 30"));
    fireEvent.click(radio("Container", "MP4"));
    // Two picks in one burst: the queue may write them as one.
    await waitFor(() => expect(lastSaved().quality.container).toBe("mp4"));
    expect(saves()).toBeGreaterThanOrEqual(3);
    expect(lastSaved().quality).toEqual({
      ...DEFAULT_QUALITY, max_height: 1080, vcodec: "h264", fps: "max30", container: "mp4",
    });
  });

  it("names each codec with its quiet note", async () => {
    await renderSettings("downloads");
    const notes = within(group("Codec")).getAllByRole("radio")
      .map((r) => r.closest("label")!.querySelector(".st-opt-note")?.textContent ?? null);
    expect(notes).toEqual([null, "smallest", null, "plays everywhere"]);
  });

  it("swaps the video controls for the audio format in audio mode", async () => {
    await renderSettings("downloads");
    fireEvent.click(radio("Mode", "Audio only"));
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().quality).toEqual({ ...DEFAULT_QUALITY, mode: "audio" });
    expect(screen.queryByRole("radiogroup", { name: "Resolution" })).toBeNull();
    expect(screen.queryByRole("radiogroup", { name: "Codec" })).toBeNull();
    expect(radio("Audio format", /^Original/).checked).toBe(true);

    fireEvent.click(radio("Audio format", "MP3"));
    await waitFor(() => expect(saves()).toBe(2));
    expect(lastSaved().quality).toEqual({ ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3" });
    expect(screen.getByText(/anything but Original is converted by ffmpeg/)).toBeTruthy();
  });

  it("says preferences are not requirements, in video mode", async () => {
    await renderSettings("downloads");
    expect(screen.getByText("Preferences, not requirements: a video without them still downloads.")).toBeTruthy();
  });

  describe("the resolution ladder", () => {
    it("is a radio group, smallest first", async () => {
      await renderSettings("downloads");
      expect(ladderSteps().map((r) => r.closest("label")!.textContent)).toEqual([
        "360p", "480p", "720p", "1080p", "1440p", "2160p", "Best",
      ]);
    });

    it("tints the steps under the ceiling and lights the ceiling itself", async () => {
      backend.settings = withQuality({ max_height: 1080 });
      await renderSettings("downloads");
      expect(stepClass()).toEqual(["in", "in", "in", "on", "-", "-", "-"]);
    });

    it("lights every step for Best", async () => {
      await renderSettings("downloads");
      expect(stepClass()).toEqual(["in", "in", "in", "in", "in", "in", "on"]);
    });

    it("moves with a pick, and saves it", async () => {
      await renderSettings("downloads");
      fireEvent.click(radio("Resolution", "360p"));
      expect(stepClass()).toEqual(["on", "-", "-", "-", "-", "-", "-"]);
      await waitFor(() => expect(saves()).toBe(1));
      expect(lastSaved().quality.max_height).toBe(360);
      expect(screen.getByText("up to 360p")).toBeTruthy();
    });
  });

  describe("the raw format", () => {
    const disclosure = () => screen.getByRole("button", { name: "Raw -f" });

    it("starts folded, and opens on its disclosure", async () => {
      await renderSettings("downloads");
      expect(disclosure().getAttribute("aria-expanded")).toBe("false");
      expect(screen.queryByLabelText("Format (-f)")).toBeNull();
      fireEvent.click(disclosure());
      expect(disclosure().getAttribute("aria-expanded")).toBe("true");
      const field = screen.getByLabelText("Format (-f)") as HTMLInputElement;
      expect(field.placeholder).toBe("bv*+ba/b");
      expect(field.getAttribute("aria-describedby")).toBeTruthy();
      expect(document.getElementById(field.getAttribute("aria-describedby")!)!.textContent)
        .toMatch(/bv\*\[height<=720\]\+ba\/b/);
    });

    it("opens by itself when a raw format is already set", async () => {
      backend.settings = withQuality({ format: "bv+ba" });
      await renderSettings("downloads");
      expect(disclosure().getAttribute("aria-expanded")).toBe("true");
      expect((screen.getByLabelText("Format (-f)") as HTMLInputElement).value).toBe("bv+ba");
    });

    it("saves on blur, not per keystroke, and disables what it overrides", async () => {
      await renderSettings("downloads");
      fireEvent.click(disclosure());
      const field = screen.getByLabelText("Format (-f)");
      fireEvent.change(field, { target: { value: "bv*[height<=720]+ba/b" } });
      expect(saves()).toBe(0);

      expect(ladderSteps().every((r) => r.disabled)).toBe(true);
      expect(within(group("Codec")).getAllByRole("radio").every((r) => (r as HTMLInputElement).disabled)).toBe(true);
      expect(within(group("Frame rate")).getAllByRole("radio").every((r) => (r as HTMLInputElement).disabled)).toBe(true);
      // Merging and the mode still apply to whatever the raw format picks.
      expect(radio("Container", "MP4").disabled).toBe(false);
      expect(radio("Mode", "Audio only").disabled).toBe(false);
      expect(screen.getByText("replaced by the raw -f below")).toBeTruthy();

      fireEvent.blur(field);
      await waitFor(() => expect(saves()).toBe(1));
      expect(lastSaved().quality).toEqual({ ...DEFAULT_QUALITY, format: "bv*[height<=720]+ba/b" });
    });

    it("gives the controls back once it is emptied", async () => {
      backend.settings = withQuality({ format: "18" });
      await renderSettings("downloads");
      expect(radio("Resolution", "720p").disabled).toBe(true);
      fireEvent.change(screen.getByLabelText("Format (-f)"), { target: { value: "  " } });
      expect(radio("Resolution", "720p").disabled).toBe(false);
    });
  });
});

describe("Also in the file", () => {
  const subtitles = () => screen.getByRole("switch", { name: "Include subtitles" }) as HTMLInputElement;
  const chapters = () => screen.getByRole("switch", { name: "Include chapters" }) as HTMLInputElement;
  const hint = (el: HTMLElement) =>
    document.getElementById(el.getAttribute("aria-describedby")!)!.textContent;
  const SUBS_HINT = "Every language the uploader provided — YouTube's automatic captions are never added.";

  it("sits directly under Quality", async () => {
    await renderSettings("downloads");
    const page = screen.getByRole("region", { name: "Downloads" });
    expect(within(page).getAllByRole("heading", { level: 3 }).map((h) => h.textContent))
      .toEqual(["Quality", "Also in the file", "At once"]);
  });

  it("starts with subtitles off and chapters on, each saying what it adds", async () => {
    await renderSettings("downloads");
    expect(subtitles().checked).toBe(false);
    expect(subtitles().disabled).toBe(false);
    expect(hint(subtitles())).toBe(SUBS_HINT);
    expect(chapters().checked).toBe(true);
    expect(chapters().disabled).toBe(false);
    expect(hint(chapters())).toBe(
      "Chapter markers from the video's description, so your player can skip between them.",
    );
  });

  it("saves each switch at once, the rest of the quality as it was", async () => {
    backend.settings = withQuality({ max_height: 1080, container: "mp4" });
    await renderSettings("downloads");
    fireEvent.click(subtitles());
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved()).toEqual({
      ...SETTINGS, quality: { ...DEFAULT_QUALITY, max_height: 1080, container: "mp4", subtitles: true },
    });
    expect(subtitles().checked).toBe(true);

    fireEvent.click(chapters());
    await waitFor(() => expect(saves()).toBe(2));
    expect(lastSaved().quality).toEqual({
      ...DEFAULT_QUALITY, max_height: 1080, container: "mp4", subtitles: true, chapters: false,
    });
    expect(chapters().checked).toBe(false);
  });

  it("is off and disabled in audio mode, says why, and keeps the stored choice", async () => {
    backend.settings = withQuality({ mode: "audio", subtitles: true });
    await renderSettings("downloads");
    expect(subtitles().checked).toBe(false);
    expect(subtitles().disabled).toBe(true);
    expect(hint(subtitles())).toBe("Audio files can't hold subtitles.");
    // An audio file still takes chapters.
    expect(chapters().disabled).toBe(false);

    // Drawn off is not saved off: another pick writes the choice back untouched.
    fireEvent.click(radio("Audio format", "MP3"));
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().quality).toEqual({
      ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3", subtitles: true,
    });

    // Back to video, it applies again, and is on.
    fireEvent.click(radio("Mode", "Video"));
    expect(subtitles().disabled).toBe(false);
    expect(subtitles().checked).toBe(true);
    expect(hint(subtitles())).toBe(SUBS_HINT);
    await waitFor(() => expect(lastSaved().quality.mode).toBe("video"));
    expect(lastSaved().quality.subtitles).toBe(true);
  });

  it("saves nothing for a click on the disabled switch", async () => {
    backend.settings = withQuality({ mode: "audio", subtitles: true });
    await renderSettings("downloads");
    fireEvent.click(subtitles());
    await new Promise((r) => setTimeout(r, 0));
    expect(saves()).toBe(0);
  });

  it("is off and disabled for a raw single-stream -f, and comes back for a merge", async () => {
    backend.settings = withQuality({ format: "18", subtitles: true });
    await renderSettings("downloads");
    expect(subtitles().checked).toBe(false);
    expect(subtitles().disabled).toBe(true);
    expect(hint(subtitles())).toBe("Needs a merged MKV or MP4 — the raw -f picks a single stream.");
    expect(chapters().disabled).toBe(false);

    // Follows the field as it is typed, before the blur saves it.
    fireEvent.change(screen.getByLabelText("Format (-f)"), { target: { value: "399+251" } });
    expect(subtitles().disabled).toBe(false);
    expect(subtitles().checked).toBe(true);
    expect(hint(subtitles())).toBe(SUBS_HINT);
  });

  it("is enabled for a raw merge like 399+251", async () => {
    backend.settings = withQuality({ format: "399+251" });
    await renderSettings("downloads");
    expect(subtitles().disabled).toBe(false);
    fireEvent.click(subtitles());
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved().quality).toEqual({ ...DEFAULT_QUALITY, format: "399+251", subtitles: true });
  });
});

describe("At once", () => {
  const lit = () => document.querySelectorAll(".st-dl-lanes i.is-on").length;
  const increase = () => screen.getByRole("button", { name: "Increase concurrent downloads" }) as HTMLButtonElement;
  const decrease = () => screen.getByRole("button", { name: "Decrease concurrent downloads" }) as HTMLButtonElement;

  it("lights a lane per download, out of sixteen", async () => {
    await renderSettings("downloads");
    expect(document.querySelectorAll(".st-dl-lanes i")).toHaveLength(16);
    expect(lit()).toBe(2);
    expect(screen.getByRole("group", { name: "concurrent downloads" }).textContent).toContain("2");
  });

  it("saves each press", async () => {
    await renderSettings("downloads");
    fireEvent.click(increase());
    await waitFor(() => expect(saves()).toBe(1));
    expect(lastSaved()).toEqual({ ...SETTINGS, max_concurrent_downloads: 3 });
    expect(lit()).toBe(3);
    fireEvent.click(decrease());
    fireEvent.click(decrease());
    // A burst of presses: the newest value is what lands, however many writes
    // the queue needed for it.
    await waitFor(() => expect(lastSaved().max_concurrent_downloads).toBe(1));
    expect(saves()).toBeGreaterThanOrEqual(2);
  });

  it("stops at one and at sixteen", async () => {
    backend.settings = { ...SETTINGS, max_concurrent_downloads: 1 };
    await renderSettings("downloads");
    expect(decrease().disabled).toBe(true);
    expect(increase().disabled).toBe(false);
    cleanup();

    backend.settings = { ...SETTINGS, max_concurrent_downloads: 16 };
    await renderSettings("downloads");
    expect(increase().disabled).toBe(true);
    expect(lit()).toBe(16);
  });
});

describe("summary", () => {
  const line = (s: Partial<Settings>) => summary({ settings: { ...SETTINGS, ...s } } as SettingsCtx).text;

  it("names the ceiling, the codec when there is one, the container and the count", () => {
    expect(line({})).toBe("Best · MKV · 2 at once");
    expect(line({ quality: { ...DEFAULT_QUALITY, max_height: 1080, vcodec: "av1" }, max_concurrent_downloads: 3 }))
      .toBe("1080p AV1 · MKV · 3 at once");
    expect(line({ quality: { ...DEFAULT_QUALITY, vcodec: "h264", container: "mp4" } }))
      .toBe("Best H.264 · MP4 · 2 at once");
  });

  it("names the audio format in audio mode", () => {
    expect(line({ quality: { ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3" } }))
      .toBe("Audio · MP3 · 2 at once");
  });

  it("says a raw format is in charge", () => {
    expect(line({ quality: { ...DEFAULT_QUALITY, format: "bv+ba", max_height: 720 } }))
      .toBe("Custom format · 2 at once");
  });

  it("reads a settings object with no quality block as the defaults", () => {
    const { quality: _, ...rest } = SETTINGS;
    expect(summary({ settings: rest as Settings } as SettingsCtx).text).toBe("Best · MKV · 2 at once");
  });

  it("is never an error: nothing on this page needs the user", () => {
    const l = summary({ settings: SETTINGS } as SettingsCtx);
    expect(l.tone).toBeUndefined();
    expect(l.flag).toBeUndefined();
  });
});
