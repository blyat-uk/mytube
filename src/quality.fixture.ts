/**
 * Test fixtures for the quality code: shared by quality.ts's tests and the
 * components that draw it, so both argue about one video.
 */
import type { AudioTrack, VideoFormats, VideoTrack } from "./types";

export const MB = 1024 * 1024;

export const v = (id: string, height: number, vcodec: VideoTrack["vcodec"], size: number | null,
  over: Partial<VideoTrack> = {}): VideoTrack => ({
  id, height, vcodec, fps: 25, hdr: false, size,
  codec: { av1: "av01.0.08M.08", vp9: "vp9", h264: "avc1.640028", other: "x" }[vcodec],
  ext: vcodec === "h264" || vcodec === "av1" ? "mp4" : "webm",
  ...over,
});
export const a = (id: string, acodec: AudioTrack["acodec"], abr: number, size: number | null,
  over: Partial<AudioTrack> = {}): AudioTrack => ({
  id, acodec, abr, size, language: "en",
  codec: acodec === "opus" ? "opus" : "mp4a.40.2", ext: acodec === "opus" ? "webm" : "m4a",
  ...over,
});

/**
 * The shape of dQw4w9WgXcQ: every height in all three codecs up to 1080p, no
 * H.264 above it, all 25 fps SDR, one English audio language. Sorted the way
 * the backend sends it — height, then size, descending.
 */
export function rickroll(): VideoFormats {
  return {
    title: "Never Gonna Give You Up",
    video: [
      v("401", 2160, "av1", 360 * MB), v("313", 2160, "vp9", 300 * MB),
      v("400", 1440, "av1", 150 * MB), v("271", 1440, "vp9", 120 * MB),
      v("137", 1080, "h264", 80 * MB), v("399", 1080, "av1", 30 * MB),
      v("248", 1080, "vp9", 28 * MB),
      v("136", 720, "h264", 40 * MB), v("398", 720, "av1", 18 * MB), v("247", 720, "vp9", 16 * MB),
      v("160", 144, "h264", 1.5 * MB), v("394", 144, "av1", 2 * MB), v("278", 144, "vp9", 1.8 * MB),
    ],
    audio: [
      a("140-drc", "aac", 258, 7 * MB), a("251", "opus", 257, 6.9 * MB),
      a("140", "aac", 130, 3.4 * MB), a("250", "opus", 129, 3.3 * MB),
      a("249", "opus", 61, 1.6 * MB), a("599", "opus", 46, 1.2 * MB),
    ],
    defaultVideo: "401",
    defaultAudio: "251",
  };
}
