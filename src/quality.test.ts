import { describe, it, expect } from "vitest";
import type { Quality, VideoFormats } from "./types";
import { MB, a, rickroll, v } from "./quality.fixture";
import {
  DEFAULT_QUALITY, HEIGHT_OPTIONS, audioChoices, canEmbedSubtitles, codecChoices, fpsChoices,
  formatSize, hdrChoices, heightChoices, initialPick, languageChoices, overriddenByFormat,
  pickSize, pickVideo, resolveExact, sanitizeQuality, withAudioLanguage, withVideo,
} from "./quality";

describe("formatSize", () => {
  it("marks every size approximate", () => {
    expect(formatSize(30 * MB)).toBe("~30 MB");
    expect(formatSize(1.5 * MB)).toBe("~1.5 MB");
    expect(formatSize(360 * MB)).toBe("~360 MB");
  });
  it("says nothing for a size YouTube did not give", () => {
    expect(formatSize(null)).toBe("");
    expect(formatSize(0)).toBe("");
  });
});

describe("DEFAULT_QUALITY", () => {
  it("leaves subtitles out and puts chapters in, as Quality::default() does", () => {
    expect(DEFAULT_QUALITY.subtitles).toBe(false);
    expect(DEFAULT_QUALITY.chapters).toBe(true);
  });
});

describe("sanitizeQuality", () => {
  it("fills a missing block with the defaults", () => {
    expect(sanitizeQuality(undefined)).toEqual(DEFAULT_QUALITY);
  });
  it("repairs unknown values field by field", () => {
    const q = sanitizeQuality({ mode: "audio", vcodec: "h265" as never, max_height: 1080 });
    expect(q).toEqual({ ...DEFAULT_QUALITY, mode: "audio", max_height: 1080 });
  });
  it("keeps both switches when they are booleans", () => {
    const q = sanitizeQuality({ ...DEFAULT_QUALITY, subtitles: true, chapters: false });
    expect(q).toEqual({ ...DEFAULT_QUALITY, subtitles: true, chapters: false });
  });
  it("reads a block from before the switches as their defaults", () => {
    const { subtitles: _s, chapters: _c, ...older } = DEFAULT_QUALITY;
    expect(sanitizeQuality(older)).toEqual(DEFAULT_QUALITY);
  });
  it("repairs a switch that is not a boolean to its own default, and nothing else", () => {
    for (const bad of ["yes", 1, 0, null, "true"]) {
      const q = sanitizeQuality({ ...DEFAULT_QUALITY, mode: "audio", subtitles: bad as never, chapters: bad as never });
      expect(q).toEqual({ ...DEFAULT_QUALITY, mode: "audio" });
    }
    // Each on its own: a broken one never resets the other.
    expect(sanitizeQuality({ ...DEFAULT_QUALITY, subtitles: true, chapters: "no" as never }))
      .toEqual({ ...DEFAULT_QUALITY, subtitles: true });
    expect(sanitizeQuality({ ...DEFAULT_QUALITY, subtitles: 1 as never, chapters: false }))
      .toEqual({ ...DEFAULT_QUALITY, chapters: false });
  });
  it("offers the heights the backend accepts, best first", () => {
    expect(HEIGHT_OPTIONS.map((o) => o.value)).toEqual([0, 2160, 1440, 1080, 720, 480, 360]);
  });
});

describe("overriddenByFormat", () => {
  it("is nothing while the raw field is empty", () => {
    expect(overriddenByFormat(DEFAULT_QUALITY)).toBe(false);
    expect(overriddenByFormat({ ...DEFAULT_QUALITY, format: "   " })).toBe(false);
  });
  it("is the stream selects once a raw -f is typed", () => {
    expect(overriddenByFormat({ ...DEFAULT_QUALITY, format: "137+140" })).toBe(true);
  });
});

describe("canEmbedSubtitles", () => {
  const can = (q: Partial<Quality>) => canEmbedSubtitles({ ...DEFAULT_QUALITY, ...q });

  it("is the merged video, in either container", () => {
    expect(can({})).toBe(true);
    expect(can({ container: "mp4" })).toBe(true);
  });
  it("is never an audio file, whatever it is converted to or picked by", () => {
    for (const audio_format of ["original", "m4a", "mp3", "opus"] as const) {
      expect(can({ mode: "audio", audio_format })).toBe(false);
    }
    expect(can({ mode: "audio", format: "251" })).toBe(false);
    expect(can({ mode: "audio", format: "bv+ba" })).toBe(false);
  });
  it("follows a raw -f: a merge can, a single stream cannot", () => {
    expect(can({ format: "399+251" })).toBe(true);
    expect(can({ format: "bv*+ba/b" })).toBe(true);
    expect(can({ format: "18" })).toBe(false);
    expect(can({ format: "bv*/b" })).toBe(false);
  });
  it("reads a raw -f of only spaces as none, as Rust's sanitize does", () => {
    expect(can({ format: "   " })).toBe(true);
    expect(can({ format: " 18 " })).toBe(false);
  });
  it("asks only whether the file could hold them, not whether they are wanted", () => {
    expect(can({ subtitles: false })).toBe(true);
    expect(can({ subtitles: true, mode: "audio" })).toBe(false);
  });
});

describe("pickVideo", () => {
  const f = rickroll();
  it("keeps the codec when the new height has it", () => {
    expect(pickVideo(f.video, { height: 720, vcodec: "vp9" })?.id).toBe("247");
  });
  it("falls back to the largest track at a height missing that codec", () => {
    // No H.264 above 1080p on this video.
    expect(pickVideo(f.video, { height: 2160, vcodec: "h264" })?.id).toBe("401");
  });
  it("is null for a height the video does not have", () => {
    expect(pickVideo(f.video, { height: 480 })).toBeNull();
  });
});

describe("initialPick", () => {
  it("preselects what the Settings default would download", () => {
    const pick = initialPick(rickroll(), DEFAULT_QUALITY);
    expect(pick).toEqual({
      mode: "video", videoId: "401", audioId: "251", container: "mkv",
      audio_format: "original", format: "", subtitles: false, chapters: true,
    });
  });

  it("falls back to the top tracks when the defaults are not in the lists", () => {
    const f = { ...rickroll(), defaultVideo: "18", defaultAudio: null };
    const pick = initialPick(f, DEFAULT_QUALITY);
    expect(pick.videoId).toBe("401");
    expect(pick.audioId).toBe("140-drc");
  });

  it("takes mode, container and audio format from the settings", () => {
    const base: Quality = { ...DEFAULT_QUALITY, mode: "audio", container: "mp4", audio_format: "mp3" };
    const pick = initialPick(rickroll(), base);
    expect(pick.mode).toBe("audio");
    expect(pick.container).toBe("mp4");
    expect(pick.audio_format).toBe("mp3");
    // A raw -f in settings is not carried: the preselection already is its answer.
    expect(initialPick(rickroll(), { ...DEFAULT_QUALITY, format: "bv+ba" }).format).toBe("");
  });

  it("takes what goes into the file from the settings", () => {
    const base: Quality = { ...DEFAULT_QUALITY, subtitles: true, chapters: false };
    const pick = initialPick(rickroll(), base);
    expect(pick.subtitles).toBe(true);
    expect(pick.chapters).toBe(false);
  });

  it("is audio-only for a video with no video tracks", () => {
    const f = { ...rickroll(), video: [], defaultVideo: null };
    expect(initialPick(f, DEFAULT_QUALITY).mode).toBe("audio");
  });
});

describe("narrowing", () => {
  const f = rickroll();
  const current = f.video.find((t) => t.id === "248")!; // 1080p VP9

  it("offers each height with the size of what it would pick", () => {
    const hs = heightChoices(f, current);
    expect(hs.map((c) => c.value)).toEqual([2160, 1440, 1080, 720, 144]);
    // Keeping VP9 where it exists: 720p VP9 is 247.
    expect(hs.find((c) => c.value === 720)).toMatchObject({ trackId: "247", label: "720p · ~16 MB" });
    // 2160p has VP9 as well: 313.
    expect(hs.find((c) => c.value === 2160)?.trackId).toBe("313");
  });

  it("offers only the codecs that height has", () => {
    const top = f.video.find((t) => t.id === "401")!;
    expect(codecChoices(f, top).map((c) => c.value)).toEqual(["av1", "vp9"]);
    expect(codecChoices(f, current).map((c) => c.label)).toEqual([
      "AV1 · ~30 MB", "VP9 · ~28 MB", "H.264 · ~80 MB",
    ]);
  });

  it("has one frame rate and one dynamic range here, so no choice to show", () => {
    expect(fpsChoices(f, current)).toHaveLength(1);
    expect(hdrChoices(f, current)).toHaveLength(1);
  });

  it("offers 60 fps and HDR where the video has them", () => {
    const g: VideoFormats = {
      ...f,
      video: [
        v("315", 2160, "vp9", 900 * MB, { fps: 60 }),
        v("337", 2160, "vp9", 950 * MB, { fps: 60, hdr: true }),
        v("313", 2160, "vp9", 500 * MB, { fps: 30 }),
      ],
    };
    const cur = g.video[0];
    expect(fpsChoices(g, cur).map((c) => c.label)).toEqual(["60 fps · ~900 MB", "30 fps · ~500 MB"]);
    expect(hdrChoices(g, cur).map((c) => c.value)).toEqual([false, true]);
  });

  it("withVideo moves the pick to the chosen track", () => {
    const pick = initialPick(f, DEFAULT_QUALITY);
    expect(withVideo(pick, "136").videoId).toBe("136");
  });
});

describe("audio", () => {
  const f: VideoFormats = {
    ...rickroll(),
    audio: [
      a("251", "opus", 129, 3.3 * MB), a("140", "aac", 130, 3.4 * MB),
      a("251-1", "opus", 128, 3.2 * MB, { language: "es" }),
      a("140-1", "aac", 128, 3.3 * MB, { language: "es" }),
    ],
  };

  it("lists a language row only when there is more than one", () => {
    expect(languageChoices(rickroll())).toHaveLength(1);
    expect(languageChoices(f).map((c) => c.value)).toEqual(["en", "es"]);
  });

  it("narrows the tracks to the chosen language", () => {
    expect(audioChoices(f, "es").map((c) => c.value)).toEqual(["251-1", "140-1"]);
    expect(audioChoices(f, "en")[1].label).toBe("AAC 130k · m4a · ~3.4 MB");
  });

  it("keeps the codec across a change of language", () => {
    const pick = { ...initialPick(f, DEFAULT_QUALITY), audioId: "140" };
    expect(withAudioLanguage(f, pick, "es").audioId).toBe("140-1");
  });
});

describe("resolveExact", () => {
  const f = rickroll();

  it("names both exact ids in video mode, with no fallback", () => {
    const pick = { ...initialPick(f, DEFAULT_QUALITY), videoId: "399" };
    expect(resolveExact(f, pick)).toEqual({ ...DEFAULT_QUALITY, format: "399+251" });
  });

  it("names only the audio id in audio mode, keeping the audio format", () => {
    const pick = { ...initialPick(f, DEFAULT_QUALITY), mode: "audio" as const, audio_format: "mp3" as const };
    expect(resolveExact(f, pick)).toEqual({
      ...DEFAULT_QUALITY, mode: "audio", audio_format: "mp3", format: "251",
    });
  });

  it("carries the container", () => {
    const pick = { ...initialPick(f, DEFAULT_QUALITY), container: "mp4" as const };
    expect(resolveExact(f, pick)?.container).toBe("mp4");
  });

  it("carries the Settings' subtitles and chapters into the override", () => {
    const base: Quality = { ...DEFAULT_QUALITY, subtitles: true, chapters: false };
    const pick = { ...initialPick(f, base), videoId: "399" };
    expect(resolveExact(f, pick)).toEqual({
      ...DEFAULT_QUALITY, format: "399+251", subtitles: true, chapters: false,
    });
    // Audio mode and a raw -f build their override from the same base.
    expect(resolveExact(f, { ...pick, mode: "audio" })).toMatchObject({ subtitles: true, chapters: false });
    expect(resolveExact(f, { ...pick, format: "18" })).toMatchObject({ subtitles: true, chapters: false });
  });

  it("lets a raw -f win over the tracks", () => {
    const pick = { ...initialPick(f, DEFAULT_QUALITY), format: "  bv*[height<=720]+ba  " };
    expect(resolveExact(f, pick)?.format).toBe("bv*[height<=720]+ba");
  });

  it("sends a video-only track alone when there is no audio", () => {
    const g = { ...f, audio: [], defaultAudio: null };
    expect(resolveExact(g, initialPick(g, DEFAULT_QUALITY))?.format).toBe("401");
  });

  it("is null when the video offers nothing", () => {
    const g = { ...f, video: [], audio: [], defaultVideo: null, defaultAudio: null };
    expect(resolveExact(g, initialPick(g, DEFAULT_QUALITY))).toBeNull();
  });
});

describe("pickSize", () => {
  const f = rickroll();
  it("adds video and audio in video mode", () => {
    expect(pickSize(f, { ...initialPick(f, DEFAULT_QUALITY), videoId: "399" })).toBe(
      30 * MB + 6.9 * MB,
    );
  });
  it("is the audio alone in audio mode", () => {
    expect(pickSize(f, { ...initialPick(f, DEFAULT_QUALITY), mode: "audio" })).toBe(6.9 * MB);
  });
  it("is unknown when either half is", () => {
    const g = { ...f, video: [v("401", 2160, "av1", null)] };
    expect(pickSize(g, initialPick(g, DEFAULT_QUALITY))).toBeNull();
  });
});
