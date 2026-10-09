/**
 * The Downloads page's file-name preview: the output template rendered the way
 * yt-dlp renders it, against one real video, so "what will my files be called"
 * is answered by a file name rather than by reading `%(uploader)s`.
 *
 * Pure, and a faithful *subset* of yt-dlp's `prepare_outtmpl` (YoutubeDL.py):
 * `%(field)s` with printf flags, width and precision; `>strftime` dates;
 * `,alternate` fields, `&replacement` and `|default`; `%%`. What it does not do
 * -- maths (`%(duration-60)s`), object traversal (`%(formats.0.id)s`), a negated
 * field -- renders `NA`, the same placeholder yt-dlp prints for a field it has
 * no value for, so the preview never invents text yt-dlp would not write.
 */
import type { Quality } from "../../types";

export type FieldValue = string | number | null | undefined;
/** yt-dlp's info dict, the fields of it the preview can know. A key that is
 *  absent or null renders `NA`, as yt-dlp's does. */
export type TemplateFields = Record<string, FieldValue>;

/** The parts of a video a file name can be made of. */
export interface PreviewVideo {
  id: string;
  title: string;
  /** The channel's name. On YouTube `uploader` and `channel` are the same. */
  uploader: string;
  /** Unix seconds, or null when the date is not known yet. */
  published_at: number | null;
  duration_secs: number | null;
  view_count: number | null;
}

/**
 * Stands in for the newest video while the library is empty or cannot be read.
 * Dated, and at a time of day, so the dated presets and a `%H-%M` template show
 * something rather than a row of `NA`s; 18 August is the date the presets'
 * own hints in format.ts use.
 */
export const SAMPLE_VIDEO: PreviewVideo = {
  id: "abc123def45",
  title: "Some video",
  uploader: "Some channel",
  published_at: Date.UTC(2026, 7, 18, 14, 30, 0) / 1000,
  duration_secs: 754,
  view_count: 48210,
};

/** yt-dlp's `outtmpl_na_placeholder` default. */
const NA = "NA";

/** The width YouTube serves at each height on a 16:9 upload. */
const WIDTH_AT: Record<number, number> = {
  360: 640, 480: 854, 720: 1280, 1080: 1920, 1440: 2560, 2160: 3840,
};

/**
 * The extension the finished file will have.
 *
 * Video is merged into the container, whatever the streams were. Converted
 * audio is the format it was converted to. `original` keeps the stream's own
 * extension, which only yt-dlp can know -- `final_ext` in quality.rs declines to
 * guess, since it may be webm (opus). The preview has to show something, so it
 * shows `m4a`: that is what the audio-only probe printed when quality.rs's
 * `final_ext` was verified (2026-09-29, `-f ba/b`).
 *
 * A raw `-f` naming one stream is neither merged nor converted and keeps that
 * stream's extension, which the preview cannot know either; the container is
 * still the answer for anything that merges, which is what nearly every raw
 * format does.
 */
export function previewExt(q: Quality): string {
  if (q.mode === "audio") return q.audio_format === "original" ? "m4a" : q.audio_format;
  return q.container;
}

/**
 * `%(resolution)s` as yt-dlp would print it, or null (`NA`) when only the
 * download could say. A ceiling names the size a 16:9 upload lands at; Best is
 * whatever each video's largest is, and a raw `-f` picks its own, so neither
 * pretends to a number. Audio is yt-dlp's own "audio only".
 */
export function previewResolution(q: Quality): string | null {
  if (q.mode === "audio") return "audio only";
  if (q.format.trim() !== "" || !q.max_height) return null;
  const width = WIDTH_AT[q.max_height] ?? Math.round((q.max_height * 16) / 9);
  return `${width}x${q.max_height}`;
}

const pad2 = (n: number) => String(n).padStart(2, "0");

/** YYYYMMDD in UTC: yt-dlp derives YouTube's `upload_date` from the timestamp
 *  in UTC, so an upload late in the evening west of Greenwich is tomorrow's. */
export function uploadDate(ts: number): string {
  const d = new Date(ts * 1000);
  return `${d.getUTCFullYear()}${pad2(d.getUTCMonth() + 1)}${pad2(d.getUTCDate())}`;
}

/** The fields the preview renders a template against. */
export function templateFields(v: PreviewVideo, q: Quality): TemplateFields {
  return {
    id: v.id,
    title: v.title,
    uploader: v.uploader,
    channel: v.uploader,
    upload_date: v.published_at === null ? null : uploadDate(v.published_at),
    timestamp: v.published_at,
    duration: v.duration_secs,
    view_count: v.view_count,
    resolution: previewResolution(q),
    // A single video is not in a playlist; yt-dlp prints NA for it too.
    playlist_index: null,
    ext: previewExt(q),
  };
}

/* ------------------------------------------------------------------ *
 * The renderer
 * ------------------------------------------------------------------ */

/**
 * `%%`, or one `%(key)<format>`: yt-dlp's EXTERNAL_FORMAT_RE, with Python's
 * conversion types plus yt-dlp's own (`l j h q B U D S`). Scanning left to
 * right with `%%` as an alternative gives Python's `(?<!%)(%%)*` prefix rule for
 * free: `%%(title)s` is the literal text `%(title)s`. Any other `%` -- one that
 * starts no valid spec -- is left as it is, as yt-dlp's `escape_outtmpl` does.
 */
const SPEC = /%%|%\(([^)]*)\)([#0\-+ ]*)(\d+)?(?:\.(\d+))?[hlL]?([diouxXeEfFgGcrsaljhqBUDS])/g;

/**
 * Inside the parentheses: yt-dlp's INTERNAL_FORMAT_RE without the maths and the
 * negation. `fields` may be dotted (object traversal) so the rest still parses;
 * no dotted key is ever in the preview's fields, so it renders NA.
 */
const INNER =
  /^(?<field>\w*(?:\.\w+)*)(?:>(?<strf>.+?))?(?<remaining>(?<alternate>(?<!\\),[^|&)]+)?(?:&(?<replacement>.*?))?(?:\|(?<def>.*?))?)$/s;

type Inner = {
  field: string; strf?: string; remaining: string; alternate?: string;
  replacement?: string; def?: string;
};

/** Python's `float_or_none`: a number, or a string that reads as one. */
function toNumber(v: FieldValue): number | null {
  if (typeof v === "number") return Number.isNaN(v) ? null : v;
  if (typeof v === "string" && v.trim() !== "") {
    const n = Number(v.trim());
    return Number.isNaN(n) ? null : n;
  }
  return null;
}

/** `str(value)` for what the fields hold. */
const pyStr = (v: string | number) => String(v);

/**
 * yt-dlp's `strftime_or_none`: a number is a Unix timestamp (UTC), a string is
 * read as YYYYMMDD; anything else is no date at all. The directives are the
 * ones a file name has any use for; an unknown one stays as typed, as glibc
 * leaves it.
 */
export function strftime(value: FieldValue, format: string): string | null {
  let d: Date;
  if (typeof value === "number" && Number.isFinite(value)) {
    d = new Date(value * 1000);
  } else if (typeof value === "string" && /^\d{8}$/.test(value)) {
    d = new Date(Date.UTC(+value.slice(0, 4), +value.slice(4, 6) - 1, +value.slice(6, 8)));
  } else {
    return null;
  }
  if (Number.isNaN(d.getTime())) return null;
  return format.replace(/%(.)/gs, (all, c: string) => {
    switch (c) {
      case "Y": return String(d.getUTCFullYear());
      case "y": return pad2(d.getUTCFullYear() % 100);
      case "m": return pad2(d.getUTCMonth() + 1);
      case "d": return pad2(d.getUTCDate());
      case "H": return pad2(d.getUTCHours());
      case "M": return pad2(d.getUTCMinutes());
      case "S": return pad2(d.getUTCSeconds());
      case "s": return String(Math.floor(d.getTime() / 1000));
      case "%": return "%";
      default: return all;
    }
  });
}

/** WebView2 says "Windows NT" in its user agent; WebKitGTK and WKWebView never do. */
const ON_WINDOWS = typeof navigator !== "undefined" && /Windows/i.test(navigator.userAgent);

/**
 * What yt-dlp does to a value before it goes into a path. Everywhere but
 * Windows MyTube passes `--no-windows-filenames` (ytdlp.rs), and then only a
 * `/` inside a title changes: it becomes the lookalike `⧸` rather than a
 * directory. On Windows yt-dlp's default holds, and `" * : < > ? |` become
 * their full-width forms and `\` becomes `⧹` as well. Both checked against
 * yt-dlp 2026.08.19's own `prepare_filename`.
 */
export function sanitizeValue(s: string, windows = ON_WINDOWS): string {
  let out = s.replace(/\0/g, "");
  if (windows) {
    out = out.replace(/["*:<>?|\\]/g, (c) =>
      c === "\\" ? "⧹" : String.fromCharCode(c.charCodeAt(0) + 0xfee0));
  }
  return out.replace(/\//g, "⧸");
}


/** Python's `'%<flags><width>s' % s`: `-` left-justifies; `0` means nothing. */
function padText(s: string, flags: string, width: number) {
  const len = Array.from(s).length;
  if (len >= width) return s;
  const fill = " ".repeat(width - len);
  return flags.includes("-") ? s + fill : fill + s;
}

/** The sign, and the zero padding that goes between it and the digits. */
function padNumber(body: string, negative: boolean, flags: string, width: number) {
  const sign = negative ? "-" : flags.includes("+") ? "+" : flags.includes(" ") ? " " : "";
  const len = sign.length + body.length;
  if (len >= width) return sign + body;
  if (flags.includes("-")) return sign + body + " ".repeat(width - len);
  if (flags.includes("0")) return sign + "0".repeat(width - len) + body;
  return " ".repeat(width - len) + sign + body;
}

/** Python's exponent form: at least two exponent digits, `1.5e+03`. */
function expForm(x: number, digits: number, upper: boolean): string {
  const [mant, exp] = x.toExponential(digits).split("e");
  const n = Number(exp);
  const out = `${mant}e${n < 0 ? "-" : "+"}${pad2(Math.abs(n))}`;
  return upper ? out.toUpperCase() : out;
}

/** Python's `%g`: shortest of fixed and exponent at `prec` significant digits,
 *  trailing zeros dropped unless `#`. */
function generalForm(x: number, prec: number, upper: boolean, alt: boolean): string {
  const p = prec === 0 ? 1 : prec;
  const exp = x === 0 ? 0 : Number(x.toExponential(p - 1).split("e")[1]);
  const strip = (s: string) => (alt || !s.includes(".") ? s : s.replace(/\.?0+$/, ""));
  if (exp < -4 || exp >= p) {
    const [mant, e] = expForm(x, p - 1, upper).split(upper ? "E" : "e");
    return `${strip(mant)}${upper ? "E" : "e"}${e}`;
  }
  return strip(x.toFixed(Math.max(0, p - 1 - exp)));
}

/** One numeric conversion, as Python's `%` operator formats it. */
function formatNumber(n: number, type: string, flags: string, width: number, prec?: number) {
  const x = Math.abs(n);
  // An integer conversion truncates first (`'%d' % -0.5` is `0`); a float
  // keeps its sign even at zero (`'%.1f' % -0.04` is `-0.0`).
  const negative = "diuxXo".includes(type) ? Math.trunc(n) < 0 : n < 0 || Object.is(n, -0);
  let body: string;
  switch (type) {
    case "d": case "i": case "u":
      body = String(Math.trunc(x));
      if (prec !== undefined) body = body.padStart(prec, "0");
      break;
    case "x": case "X": case "o": {
      const radix = type === "o" ? 8 : 16;
      body = Math.trunc(x).toString(radix);
      if (type === "X") body = body.toUpperCase();
      if (prec !== undefined) body = body.padStart(prec, "0");
      if (flags.includes("#")) body = (type === "o" ? "0o" : type === "x" ? "0x" : "0X") + body;
      break;
    }
    case "f": case "F":
      body = x.toFixed(prec ?? 6);
      break;
    case "e": case "E":
      body = expForm(x, prec ?? 6, type === "E");
      break;
    default: // g, G
      body = generalForm(x, prec ?? 6, type === "G", flags.includes("#"));
  }
  return padNumber(body, negative, flags, width);
}

/** The value one `%(…)` names, after `,alternate`, `>date` and `&replacement`;
 *  null for NA. Also hands back the default to print instead. */
function resolveKey(key: string, fields: TemplateFields): { value: string | number | null; fallback: string } {
  let fallback = NA;
  let value: string | number | null = null;
  let replacement: string | undefined;
  let m = INNER.exec(key)?.groups as Inner | undefined;
  while (m) {
    if (m.def !== undefined) fallback = m.def;
    const raw = Object.prototype.hasOwnProperty.call(fields, m.field) ? fields[m.field] : null;
    value = raw ?? null;
    if (m.strf !== undefined && value !== null) value = strftime(value, m.strf.replace(/\\,/g, ","));
    // yt-dlp's workaround for #4485: an empty value is no value, in a path.
    if (value === "") value = null;
    replacement = m.replacement;
    if (value === null && m.alternate) {
      m = INNER.exec(m.remaining.slice(1))?.groups as Inner | undefined;
    } else {
      break;
    }
  }
  if (value !== null && replacement !== undefined) {
    // str.format with the value as its only argument: `{}` and `{0}` name it.
    const v = pyStr(value);
    value = replacement.replace(/\{\{|\}\}|\{0?\}/g, (t) => (t === "{{" ? "{" : t === "}}" ? "}" : v));
  }
  return { value, fallback };
}

/** One `%(key)<flags><width>.<prec><type>`, rendered. */
function renderSpec(
  key: string, flags: string, width: number, prec: number | undefined, type: string,
  fields: TemplateFields,
): string {
  const { value, fallback } = resolveKey(key, fields);
  // NA (or the |default) is printed bare: yt-dlp swaps the whole format for a
  // plain `s`, so `%(playlist_index)03d` with no playlist is `NA`, not ` NA`.
  if (value === null) return sanitizeValue(fallback);

  if ("diouxXeEfFgG".includes(type)) {
    const n = toNumber(value);
    if (n === null) return sanitizeValue(fallback);
    return formatNumber(n, type, flags, width, prec);
  }

  let text: string;
  if (type === "c") {
    text = value ? pyStr(value).charAt(0) : pyStr(value);
  } else if (type === "j") {
    text = JSON.stringify(value);
  } else if (type === "r" || type === "a") {
    text = typeof value === "string" ? `'${value}'` : pyStr(value);
  } else {
    // s, and yt-dlp's own l h q B U D S: on one plain value each of those is
    // the value itself as text, near enough for a preview.
    text = pyStr(value);
  }
  if (prec !== undefined) text = Array.from(text).slice(0, prec).join("");
  return padText(sanitizeValue(text), flags, width);
}

/**
 * The path yt-dlp would write for `template`, relative to the download folder,
 * with `fields` as the video's info. `/` in the result is always the
 * template's own: a `/` inside a field value is sanitised away, as yt-dlp does.
 */
export function renderTemplate(template: string, fields: TemplateFields): string {
  return template.replace(
    SPEC,
    (all, key: string | undefined, flags: string, width: string | undefined, prec: string | undefined, type: string) =>
      all === "%%"
        ? "%"
        : renderSpec(key ?? "", flags, width ? Number(width) : 0, prec === undefined ? undefined : Number(prec), type, fields),
  );
}

/* ------------------------------------------------------------------ *
 * Paths, for display
 * ------------------------------------------------------------------ */

/** A rendered path split into the folders it creates and the file name. */
export function splitRendered(path: string): { absolute: boolean; dirs: string[]; file: string } {
  const parts = path.split("/");
  const file = parts.pop() ?? "";
  return {
    absolute: path.startsWith("/"),
    dirs: parts.filter((p) => p !== "" && p !== "."),
    file,
  };
}

/**
 * The download folder as the segments it is drawn in, and the separator to
 * draw between them: a Windows path (backslashes, no slash) keeps its own.
 */
export function folderSegments(dir: string): { segments: string[]; sep: "/" | "\\" } {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return { segments: dir.split(/[\\/]+/).filter(Boolean), sep };
}

/** The folder's own name, which the preview starts from: "Videos" for
 *  `/home/someone/Videos/`. The root is itself. */
export function folderName(dir: string): string {
  const { segments } = folderSegments(dir);
  if (segments.length) return segments[segments.length - 1];
  return dir.trim() ? dir.trim() : "";
}
