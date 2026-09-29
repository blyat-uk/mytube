/**
 * Download quality: the option lists the Settings form offers, and the
 * narrowing the Download (custom)… dialog does over one video's real formats.
 *
 * Pure, so every rule is testable without a DOM. The generic form edits a
 * `Quality` directly; the exact form edits an `ExactPick` — two track ids and
 * the post-processing choices — and `resolveExact` turns that into a `Quality`
 * naming those ids, which is what the backend stores as the video's override.
 */
import { formatBytes } from "./format";
import type {
  AudioTrack, Quality, QualityAudioFormat, QualityCodec, QualityContainer, QualityFps,
  QualityMode, VideoFormats, VideoTrack,
} from "./types";

/** Mirrors `Quality::default()` on the Rust side, which yields exactly the
 *  argv MyTube used before quality was a setting. */
export const DEFAULT_QUALITY: Quality = {
  mode: "video", max_height: 0, vcodec: "any", fps: "any", container: "mkv",
  audio_format: "original", format: "",
};

export interface Option<T> { value: T; label: string }

export const MODE_OPTIONS: Option<QualityMode>[] = [
  { value: "video", label: "Video" },
  { value: "audio", label: "Audio only" },
];

export const HEIGHT_OPTIONS: Option<number>[] = [
  { value: 0, label: "Best available" },
  ...[2160, 1440, 1080, 720, 480, 360].map((h) => ({ value: h, label: `Up to ${h}p` })),
];

export const CODEC_OPTIONS: Option<QualityCodec>[] = [
  { value: "any", label: "No preference" },
  { value: "av1", label: "AV1 (smallest files)" },
  { value: "vp9", label: "VP9" },
  { value: "h264", label: "H.264 (plays everywhere)" },
];

export const FPS_OPTIONS: Option<QualityFps>[] = [
  { value: "any", label: "No preference" },
  { value: "prefer60", label: "Prefer 60 fps" },
  { value: "max30", label: "At most 30 fps" },
];

export const CONTAINER_OPTIONS: Option<QualityContainer>[] = [
  { value: "mkv", label: "MKV" },
  { value: "mp4", label: "MP4" },
];

export const AUDIO_FORMAT_OPTIONS: Option<QualityAudioFormat>[] = [
  { value: "original", label: "Original (no conversion)" },
  { value: "m4a", label: "M4A" },
  { value: "mp3", label: "MP3" },
  { value: "opus", label: "Opus" },
];

const oneOf = <T>(options: Option<T>[], value: unknown, fallback: T): T =>
  options.some((o) => o.value === value) ? (value as T) : fallback;

/**
 * The frontend's half of `Quality::sanitize`. Rust already repairs the block on
 * every read; this is for a Settings built without one — a backend a step
 * behind, or a test fixture — so the form never renders an undefined select.
 */
export function sanitizeQuality(q: Partial<Quality> | null | undefined): Quality {
  const d = DEFAULT_QUALITY;
  if (!q) return { ...d };
  return {
    mode: oneOf(MODE_OPTIONS, q.mode, d.mode),
    max_height: oneOf(HEIGHT_OPTIONS, q.max_height, d.max_height),
    vcodec: oneOf(CODEC_OPTIONS, q.vcodec, d.vcodec),
    fps: oneOf(FPS_OPTIONS, q.fps, d.fps),
    container: oneOf(CONTAINER_OPTIONS, q.container, d.container),
    audio_format: oneOf(AUDIO_FORMAT_OPTIONS, q.audio_format, d.audio_format),
    format: typeof q.format === "string" ? q.format : d.format,
  };
}

/** A raw `-f` picks the streams itself, so the selects that would have picked
 *  them stop meaning anything; the form disables them to say so. */
export function overriddenByFormat(q: { format: string }): boolean {
  return q.format.trim() !== "";
}

/** "~30 MB". Every size here is YouTube's estimate, and one it may not give. */
export function formatSize(bytes: number | null): string {
  if (bytes === null || !(bytes > 0)) return "";
  return `~${formatBytes(bytes)}`;
}

const withSize = (label: string, bytes: number | null) => {
  const size = formatSize(bytes);
  return size ? `${label} · ${size}` : label;
};

const VCODEC_LABEL: Record<VideoTrack["vcodec"], string> = {
  av1: "AV1", vp9: "VP9", h264: "H.264", other: "Other",
};
const ACODEC_LABEL: Record<AudioTrack["acodec"], string> = {
  opus: "Opus", aac: "AAC", other: "Other",
};
/** The order codecs are offered in, whatever order the sizes put them in. */
const VCODEC_ORDER: VideoTrack["vcodec"][] = ["av1", "vp9", "h264", "other"];

/* ------------------------------------------------------------------ *
 * Exact: one video's real formats
 * ------------------------------------------------------------------ */

/** What the exact form holds. The track ids are the selection; every row of
 *  the form is read back off the chosen track rather than stored twice. */
export interface ExactPick {
  mode: QualityMode;
  videoId: string | null;
  audioId: string | null;
  container: QualityContainer;
  audio_format: QualityAudioFormat;
  /** The Advanced raw `-f` field, empty unless typed in. */
  format: string;
}

interface VideoWant {
  height?: number; vcodec?: VideoTrack["vcodec"]; fps?: number | null; hdr?: boolean;
}

/**
 * Narrows by one attribute: the tracks that have the wanted value, or, when
 * none do, the tracks sharing the first one's — which, sorted by size, is the
 * largest. So changing the height keeps the codec where that height has it,
 * and quietly moves off it where it does not, instead of offering nothing.
 */
function narrow<T, K>(list: T[], key: (t: T) => K, want: K | undefined): T[] {
  if (list.length === 0) return list;
  if (want !== undefined) {
    const hit = list.filter((t) => key(t) === want);
    if (hit.length) return hit;
  }
  const first = key(list[0]);
  return list.filter((t) => key(t) === first);
}

/** The track a set of wishes lands on, in priority order height → codec →
 *  fps → HDR. Null only when the video has no track at that height at all. */
export function pickVideo(tracks: VideoTrack[], want: VideoWant): VideoTrack | null {
  let list = want.height === undefined ? tracks : tracks.filter((t) => t.height === want.height);
  list = narrow(list, (t) => t.vcodec, want.vcodec);
  list = narrow(list, (t) => t.fps, want.fps);
  list = narrow(list, (t) => t.hdr, want.hdr);
  return list[0] ?? null;
}

const inList = <T extends { id: string }>(list: T[], id: string | null) =>
  (id !== null && list.some((t) => t.id === id) ? id : list[0]?.id ?? null);

/**
 * Starts the dialog on what the Settings default would have downloaded, so
 * Download without touching anything is that default made explicit. An id the
 * lists do not hold — a pre-muxed pick the backend filtered out — falls back
 * to the top track. A raw `-f` in settings is not carried over: the
 * preselection is already what it chose.
 */
export function initialPick(f: VideoFormats, base: Quality): ExactPick {
  const videoId = inList(f.video, f.defaultVideo);
  const audioId = inList(f.audio, f.defaultAudio);
  let mode = base.mode;
  if (videoId === null) mode = "audio";
  else if (audioId === null) mode = "video";
  return {
    mode, videoId, audioId, container: base.container, audio_format: base.audio_format,
    format: "",
  };
}

export const selectedVideo = (f: VideoFormats, pick: ExactPick) =>
  f.video.find((t) => t.id === pick.videoId) ?? null;
export const selectedAudio = (f: VideoFormats, pick: ExactPick) =>
  f.audio.find((t) => t.id === pick.audioId) ?? null;

export const withVideo = (pick: ExactPick, videoId: string): ExactPick => ({ ...pick, videoId });
export const withAudio = (pick: ExactPick, audioId: string): ExactPick => ({ ...pick, audioId });

/** One option of an exact select: the value the row shows, and the track that
 *  choosing it lands on — whose size is what the label quotes. */
export interface Choice<T> { value: T; label: string; trackId: string }

function distinct<T, K>(list: T[], key: (t: T) => K): K[] {
  const seen: K[] = [];
  for (const t of list) if (!seen.includes(key(t))) seen.push(key(t));
  return seen;
}

function choices<K>(
  f: VideoFormats, values: K[], want: (k: K) => VideoWant, label: (k: K) => string,
): Choice<K>[] {
  const out: Choice<K>[] = [];
  for (const k of values) {
    const t = pickVideo(f.video, want(k));
    if (t) out.push({ value: k, label: withSize(label(k), t.size), trackId: t.id });
  }
  return out;
}

export function heightChoices(f: VideoFormats, cur: VideoTrack): Choice<number>[] {
  return choices(
    f, distinct(f.video, (t) => t.height),
    (height) => ({ height, vcodec: cur.vcodec, fps: cur.fps, hdr: cur.hdr }),
    (h) => `${h}p`,
  );
}

export function codecChoices(f: VideoFormats, cur: VideoTrack): Choice<VideoTrack["vcodec"]>[] {
  const here = distinct(f.video.filter((t) => t.height === cur.height), (t) => t.vcodec);
  return choices(
    f, VCODEC_ORDER.filter((c) => here.includes(c)),
    (vcodec) => ({ height: cur.height, vcodec, fps: cur.fps, hdr: cur.hdr }),
    (c) => VCODEC_LABEL[c],
  );
}

export function fpsChoices(f: VideoFormats, cur: VideoTrack): Choice<number | null>[] {
  const here = f.video.filter((t) => t.height === cur.height && t.vcodec === cur.vcodec);
  const rates = distinct(here, (t) => t.fps).sort((x, y) => (y ?? 0) - (x ?? 0));
  return choices(
    f, rates,
    (fps) => ({ height: cur.height, vcodec: cur.vcodec, fps, hdr: cur.hdr }),
    (fps) => (fps === null ? "Unknown fps" : `${fps} fps`),
  );
}

export function hdrChoices(f: VideoFormats, cur: VideoTrack): Choice<boolean>[] {
  const here = f.video.filter(
    (t) => t.height === cur.height && t.vcodec === cur.vcodec && t.fps === cur.fps,
  );
  const ranges = distinct(here, (t) => t.hdr).sort((x, y) => Number(x) - Number(y));
  return choices(
    f, ranges,
    (hdr) => ({ height: cur.height, vcodec: cur.vcodec, fps: cur.fps, hdr }),
    (hdr) => (hdr ? "HDR" : "SDR"),
  );
}

/** A select's value for a track with no language tag; no real code is empty. */
const langKey = (t: AudioTrack) => t.language ?? "";

export function languageChoices(f: VideoFormats): Option<string>[] {
  return distinct(f.audio, langKey).map((k) => ({ value: k, label: k || "Unknown" }));
}

export function audioChoices(f: VideoFormats, language: string): Choice<string>[] {
  return f.audio
    .filter((t) => langKey(t) === language)
    .map((t) => {
      const rate = t.abr === null ? "" : ` ${Math.round(t.abr)}k`;
      return {
        value: t.id,
        label: withSize(`${ACODEC_LABEL[t.acodec]}${rate} · ${t.ext}`, t.size),
        trackId: t.id,
      };
    });
}

/** Moving to another language keeps the codec where it can: someone who
 *  picked AAC for a player that wants it should not be handed Opus. */
export function withAudioLanguage(f: VideoFormats, pick: ExactPick, language: string): ExactPick {
  const cur = selectedAudio(f, pick);
  const list = f.audio.filter((t) => langKey(t) === language);
  const next = list.find((t) => t.acodec === cur?.acodec) ?? list[0];
  return next ? { ...pick, audioId: next.id } : pick;
}

export const audioLanguage = (t: AudioTrack) => langKey(t);

/**
 * The override the dialog sends. It names exact format ids and nothing else —
 * no `/b` fallback — because a custom download is a choice of *these* streams,
 * and quietly getting different ones would be worse than yt-dlp's error.
 * Null when the video offers nothing to download.
 */
export function resolveExact(f: VideoFormats, pick: ExactPick): Quality | null {
  const base: Quality = {
    ...DEFAULT_QUALITY, mode: pick.mode, container: pick.container,
    audio_format: pick.audio_format,
  };
  if (overriddenByFormat(pick)) return { ...base, format: pick.format.trim() };
  const video = selectedVideo(f, pick);
  const audio = selectedAudio(f, pick);
  if (pick.mode === "audio") return audio ? { ...base, format: audio.id } : null;
  if (!video) return null;
  return { ...base, format: audio ? `${video.id}+${audio.id}` : video.id };
}

/** What the pick should weigh, or null when any part of it is unknown. */
export function pickSize(f: VideoFormats, pick: ExactPick): number | null {
  const audio = selectedAudio(f, pick);
  if (pick.mode === "audio") return audio?.size ?? null;
  const video = selectedVideo(f, pick);
  if (!video || video.size === null) return null;
  if (!audio) return video.size;
  return audio.size === null ? null : video.size + audio.size;
}
