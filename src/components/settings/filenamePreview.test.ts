import { describe, it, expect } from "vitest";
import { DEFAULT_QUALITY } from "../../quality";
import { TEMPLATE_PRESETS } from "../../format";
import {
  SAMPLE_VIDEO, folderName, folderSegments, previewExt, previewResolution, renderTemplate,
  sanitizeValue,
  splitRendered, strftime, templateFields, uploadDate, type TemplateFields,
} from "./filenamePreview";

/*
 * Every expectation marked "yt-dlp:" was produced by yt-dlp itself
 * (`YoutubeDL({"windowsfilenames": False}).evaluate_outtmpl(t, info, sanitize=True)`,
 * yt-dlp 2026.x) against the same info as FIELDS, so the renderer is checked
 * against the program it imitates rather than against a reading of its docs.
 */
const FIELDS: TemplateFields = {
  id: "abc123def45", title: "Some video", uploader: "Some channel", channel: "Some channel",
  upload_date: "20260818", timestamp: 1787063400, duration: 754, view_count: 48210,
  resolution: "1920x1080", playlist_index: null, ext: "mkv",
};
const r = (t: string, f: TemplateFields = FIELDS) => renderTemplate(t, f);

describe("renderTemplate", () => {
  it("fills plain string fields", () => {
    // yt-dlp:
    expect(r("%(title)s.%(ext)s")).toBe("Some video.mkv");
    expect(r("%(uploader)s/%(title)s [%(id)s].%(ext)s")).toBe(
      "Some channel/Some video [abc123def45].mkv",
    );
  });

  it("renders an unknown or empty field as NA, the way yt-dlp does", () => {
    // yt-dlp:
    expect(r("%(nope)s %(nope)d")).toBe("NA NA");
    expect(r("%(title)s", { ...FIELDS, title: "" })).toBe("NA");
    expect(r("%(title)s", { ...FIELDS, title: undefined })).toBe("NA");
  });

  it("prints NA bare, dropping the width and precision it would have had", () => {
    // yt-dlp: the whole format becomes a plain `s`.
    expect(r("%(playlist_index)03d - %(title)s")).toBe("NA - Some video");
    expect(r("%(title).3s%(nope).3s")).toBe("SomNA");
  });

  it("applies printf flags, width and precision to numbers", () => {
    // yt-dlp:
    expect(r("%(duration)05d|%(duration)-6d|%(duration)+d|%(view_count) d")).toBe(
      "00754|754   |+754| 48210",
    );
    expect(r("%(duration).4d")).toBe("0754");
    expect(r("%(duration).2f %(duration)e %(duration)g")).toBe("754.00 7.540000e+02 754");
    expect(r("%(duration)10.3e")).toBe(" 7.540e+02");
    expect(r("%(view_count).3g %(timestamp)g")).toBe("4.82e+04 1.78706e+09");
    expect(r("%(view_count)G")).toBe("48210");
  });

  it("zero-pads a playlist index when there is one", () => {
    expect(r("%(playlist_index)03d", { playlist_index: 7 })).toBe("007");
  });

  it("truncates and pads strings", () => {
    // yt-dlp:
    expect(r("%(upload_date>%Y)s/%(title).4s")).toBe("2026/Some");
    expect(r("%(title)05s")).toBe("Some video"); // `0` means nothing to a string
    expect(r("[%(title)12s|%(title)-12s]")).toBe("[  Some video|Some video  ]");
  });

  it("counts characters, not UTF-16 units, when truncating", () => {
    expect(r("%(title).2s", { title: "😀😀😀" })).toBe("😀😀");
  });

  it("formats dates from upload_date and from a timestamp, in UTC", () => {
    // yt-dlp:
    expect(r("%(upload_date>%Y-%m-%d)s")).toBe("2026-08-18");
    expect(r("%(upload_date>%y)s")).toBe("26");
    expect(r("%(timestamp>%Y-%m-%d %H-%M-%S)s")).toBe("2026-08-18 14-30-00");
    expect(r("%(timestamp>%s)s")).toBe("1787063400");
  });

  it("renders a date of a field that is not a date as NA", () => {
    expect(r("%(title>%Y)s")).toBe("NA");
    expect(r("%(nope>%Y)s")).toBe("NA");
  });

  it("never lets a value make a folder: a slash in it is swapped for a lookalike", () => {
    // yt-dlp, with --no-windows-filenames as MyTube runs it off Windows:
    expect(r("%(title)s", { title: "AC/DC: Back in Black?" })).toBe("AC⧸DC: Back in Black?");
    expect(r("%(upload_date>%Y/%m)s")).toBe("2026⧸08");
    expect(r("%(nope|a/b)s")).toBe("a⧸b");
  });

  it("takes |default, ,alternate and &replacement", () => {
    // yt-dlp:
    expect(r("%(nope|fallback)s")).toBe("fallback");
    expect(r("%(nope,uploader)s")).toBe("Some channel");
    expect(r("%(nope,also,id)s")).toBe("abc123def45");
    expect(r("%(title&yes {})s")).toBe("yes Some video");
    expect(r("%(nope&yes|no)s")).toBe("no");
  });

  it("turns %% into % and leaves a % that starts nothing as it is", () => {
    // yt-dlp:
    expect(r("100%% sure %(title)s")).toBe("100% sure Some video");
    expect(r("%%(title)s")).toBe("%(title)s");
    expect(r("50%off %(title)s")).toBe("50%off Some video");
    expect(r("trailing %")).toBe("trailing %");
  });

  it("handles the rarer conversions", () => {
    // yt-dlp:
    expect(r("%(title)r %(duration)r")).toBe("'Some video' 754");
    expect(r("%(duration)c")).toBe("7");
    expect(r("%(title)j")).toBe('"Some video"');
  });

  it("does no maths: what it cannot evaluate is NA rather than a guess", () => {
    // yt-dlp would print 694.0 here; NA is the honest preview of a template
    // the preview does not understand.
    expect(r("%(duration-60)s")).toBe("NA");
  });

  it("renders every preset against the sample", () => {
    const f = templateFields(SAMPLE_VIDEO, DEFAULT_QUALITY);
    expect(TEMPLATE_PRESETS.map((p) => renderTemplate(p.value, f))).toEqual([
      "Some channel/Some video [abc123def45].mkv",
      "Some channel/20260818 - Some video [abc123def45].mkv",
      "Some video [abc123def45].mkv",
      "Some channel/2026/Some video [abc123def45].mkv",
      "20260818 - Some channel - Some video.mkv",
    ]);
  });
});

describe("strftime", () => {
  it("reads a number as a UTC timestamp and a string as YYYYMMDD", () => {
    expect(strftime(1787063400, "%d.%m.%Y %H:%M:%S")).toBe("18.08.2026 14:30:00");
    expect(strftime("20260818", "%Y%m%d")).toBe("20260818");
  });

  it("is no date for anything else", () => {
    expect(strftime("2026-08-18", "%Y")).toBeNull();
    expect(strftime(null, "%Y")).toBeNull();
  });

  it("keeps %% and leaves a directive it does not know as typed", () => {
    expect(strftime("20260818", "%Y%%%Q")).toBe("2026%%Q");
  });
});

describe("templateFields", () => {
  it("takes the date in UTC, so a late upload west of Greenwich is the next day", () => {
    // 2026-08-18 23:30 in New York is 03:30 on the 19th in UTC.
    expect(uploadDate(Date.UTC(2026, 7, 19, 3, 30) / 1000)).toBe("20260819");
  });

  it("names the uploader as the channel too, and a missing date as NA", () => {
    const f = templateFields({ ...SAMPLE_VIDEO, published_at: null }, DEFAULT_QUALITY);
    expect(f.channel).toBe("Some channel");
    expect(renderTemplate("%(upload_date)s", f)).toBe("NA");
  });

  it("is in no playlist", () => {
    expect(renderTemplate("%(playlist_index)s", templateFields(SAMPLE_VIDEO, DEFAULT_QUALITY))).toBe("NA");
  });
});

describe("previewExt", () => {
  it("is the container for video", () => {
    expect(previewExt(DEFAULT_QUALITY)).toBe("mkv");
    expect(previewExt({ ...DEFAULT_QUALITY, container: "mp4" })).toBe("mp4");
  });

  it("is the converted format for audio, and m4a for the original stream", () => {
    expect(previewExt({ ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3" })).toBe("mp3");
    expect(previewExt({ ...DEFAULT_QUALITY, mode: "audio", audio_format: "opus" })).toBe("opus");
    expect(previewExt({ ...DEFAULT_QUALITY, mode: "audio", audio_format: "original" })).toBe("m4a");
  });
});

describe("previewResolution", () => {
  it("names the size a ceiling lands at", () => {
    expect(previewResolution({ ...DEFAULT_QUALITY, max_height: 1080 })).toBe("1920x1080");
    expect(previewResolution({ ...DEFAULT_QUALITY, max_height: 480 })).toBe("854x480");
    expect(previewResolution({ ...DEFAULT_QUALITY, max_height: 2160 })).toBe("3840x2160");
  });

  it("does not guess at Best or at a raw format", () => {
    expect(previewResolution(DEFAULT_QUALITY)).toBeNull();
    expect(previewResolution({ ...DEFAULT_QUALITY, max_height: 720, format: "18" })).toBeNull();
  });

  it("says audio only in audio mode, as yt-dlp does", () => {
    expect(previewResolution({ ...DEFAULT_QUALITY, mode: "audio" })).toBe("audio only");
  });
});

describe("paths", () => {
  it("splits a rendered path into folders and a file name", () => {
    expect(splitRendered("Some channel/2026/Some video.mkv")).toEqual({
      absolute: false, dirs: ["Some channel", "2026"], file: "Some video.mkv",
    });
    expect(splitRendered("Some video.mkv")).toEqual({ absolute: false, dirs: [], file: "Some video.mkv" });
    expect(splitRendered("./a//b/c.mkv")).toEqual({ absolute: false, dirs: ["a", "b"], file: "c.mkv" });
    expect(splitRendered("/abs/c.mkv").absolute).toBe(true);
  });

  it("draws a folder as segments with its own separator", () => {
    expect(folderSegments("/mnt/media/YouTube/")).toEqual({ segments: ["mnt", "media", "YouTube"], sep: "/" });
    expect(folderSegments("C:\\Users\\me\\Videos")).toEqual({
      segments: ["C:", "Users", "me", "Videos"], sep: "\\",
    });
    expect(folderSegments("/")).toEqual({ segments: [], sep: "/" });
  });

  it("names a folder by its last segment", () => {
    expect(folderName("/home/someone/Videos")).toBe("Videos");
    expect(folderName("/home/someone/Videos/")).toBe("Videos");
    expect(folderName("C:\\Users\\me\\Videos")).toBe("Videos");
    expect(folderName("/")).toBe("/");
    expect(folderName("")).toBe("");
  });
});

describe("sanitizeValue", () => {
  // Both expectations are yt-dlp 2026.08.19's own prepare_filename output for
  // this title, with and without --no-windows-filenames.
  const title = 'Why? A story: part 2/3 "x" * <y> | z \\ end';

  it("changes only the slash where MyTube passes --no-windows-filenames", () => {
    expect(sanitizeValue(title, false)).toBe('Why? A story: part 2⧸3 "x" * <y> | z \\ end');
  });

  it("uses yt-dlp's full-width lookalikes on Windows", () => {
    expect(sanitizeValue(title, true)).toBe("Why？ A story： part 2⧸3 ＂x＂ ＊ ＜y＞ ｜ z ⧹ end");
  });
});
