import { formatTotalRuntime } from "../../format";
import type { Video } from "../../types";

/**
 * What the series bands draw, worked out from the loaded parts alone: no
 * backend, no DOM. Everything the component shows comes from `seriesModel`, so
 * the order, the widths, the next part and every word under the bands can be
 * tested without a layout engine -- jsdom measures nothing.
 */

/** Where a part stands, for the figures: watched (W), left to watch (L), or
 *  hidden without being watched (H). A watched part counts as watched whether
 *  or not it was hidden afterwards: you did watch it. */
export type PartKind = "watched" | "left" | "hidden";

export function partKind(v: Pick<Video, "watched" | "hidden">): PartKind {
  if (v.watched) return "watched";
  return v.hidden ? "hidden" : "left";
}

function nullsLast(a: number | null, b: number | null): number {
  if (a === b) return 0;
  if (a === null) return 1;
  if (b === null) return -1;
  return a - b;
}

/**
 * The parts in the order they were uploaded, whatever order the grid shows
 * them in: `published_at` ascending with unknown dates last, then `sort_at`,
 * then the id. It is `Db::download_order`'s ORDER BY to the letter, so "Part 8"
 * here is the eighth part Download N parts would queue -- and never the number
 * in a title, which a channel can skip, repeat or leave out.
 */
export function uploadOrder(parts: readonly Video[]): Video[] {
  return [...parts].sort(
    (a, b) =>
      nullsLast(a.published_at, b.published_at) ||
      nullsLast(a.sort_at, b.sort_at) ||
      (a.id < b.id ? -1 : a.id > b.id ? 1 : 0),
  );
}

/** A duration that says something: null and 0 both mean "not resolved yet". */
function known(secs: number | null): secs is number {
  return secs !== null && secs > 0;
}

/**
 * Each band's share of the width: its part's runtime. A part whose runtime is
 * unknown takes the average of the known ones -- a typical width, rather than
 * a sliver that would read as a short part or a gap that would read as none;
 * with nothing known, every band gets the same.
 */
export function bandWeights(durations: readonly (number | null)[]): number[] {
  const knownOnes = durations.filter(known);
  if (knownOnes.length === 0) return durations.map(() => 1);
  const average = knownOnes.reduce((a, b) => a + b, 0) / knownOnes.length;
  return durations.map((d) => (known(d) ? d : average));
}

/**
 * The gap between two bands, in px. Three reads as separate parts; a long
 * series would spend its width on gaps instead, so it narrows as the count
 * grows and a hundred-odd parts still read as slivers rather than as a row of
 * gaps with colour between them.
 */
export function bandGap(count: number): number {
  if (count <= 24) return 3;
  if (count <= 60) return 2;
  return 1;
}

/** The first part in upload order neither watched nor hidden; -1 when none is
 *  left (finished, or everything left was hidden). */
export function nextIndex(ordered: readonly Pick<Video, "watched" | "hidden">[]): number {
  return ordered.findIndex((v) => partKind(v) === "left");
}

/**
 * The runtime of some parts as the figures say it: "24h 15m", with a "+" when
 * any of them has no runtime yet, so a total that is a floor never reads as
 * exact. With none known there is no number to give, and a dash says so.
 */
export function runtimeOf(parts: readonly Pick<Video, "duration_secs">[]): string {
  let total = 0;
  let unknown = false;
  for (const p of parts) {
    if (known(p.duration_secs)) total += p.duration_secs;
    else unknown = true;
  }
  const text = formatTotalRuntime(total);
  if (!text) return "—";
  return unknown ? `${text}+` : text;
}

function plural(n: number, one: string, many: string): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** One figure under the bands: a runtime over a caption. `downloaded` is the
 *  left figure's green clause ("2 downloaded"), 0 when it has none. */
export interface Figure {
  runtime: string;
  caption: string;
  downloaded: number;
}

/**
 * The two figures, and whether they can stand in brackets.
 *
 * Brackets claim that the watched parts are the front of the series and the
 * rest is what remains: true only with a clean prefix -- no part left to watch
 * coming before a watched one. Then the split falls after the last watched
 * part, and a hidden part sits in whichever bracket its position puts it. Any
 * other pattern (a part skipped and watched past, say) gets the same figures
 * plain, since a bracket around it would draw a claim the bands contradict.
 */
export type Figures =
  | {
      kind: "brackets";
      /** The left bracket spans parts 1..split (1-based), the right one the rest. */
      split: number;
      watched: Figure;
      left: Figure;
    }
  | { kind: "plain"; watched: Figure | null; left: Figure | null }
  | { kind: "none" };

export function seriesFigures(ordered: readonly Video[]): Figures {
  const kinds = ordered.map(partKind);
  const watched = ordered.filter((_, i) => kinds[i] === "watched");
  const left = ordered.filter((_, i) => kinds[i] === "left");
  if (watched.length === 0 && left.length === 0) return { kind: "none" };

  const lastWatched = kinds.lastIndexOf("watched");
  const firstLeft = kinds.indexOf("left");
  const downloaded = left.filter((v) => v.download_state === "done").length;
  const leftFigure: Figure | null = left.length
    ? {
        runtime: runtimeOf(left),
        caption: `left in ${plural(left.length, "part", "parts")}`,
        downloaded,
      }
    : null;

  if (watched.length && left.length && lastWatched < firstLeft) {
    const split = lastWatched + 1;
    return {
      kind: "brackets",
      split,
      watched: {
        runtime: runtimeOf(watched),
        caption:
          split === 1 ? "watched, part 1"
          : split === 2 ? "watched, parts 1 and 2"
          : `watched, parts 1 to ${split}`,
        downloaded: 0,
      },
      left: leftFigure!,
    };
  }

  const watchedFigure: Figure | null = watched.length
    ? {
        runtime: runtimeOf(watched),
        caption:
          watched.length === ordered.length
            ? `watched, all ${ordered.length} parts`
            : `watched, ${plural(watched.length, "part", "parts")}`,
        downloaded: 0,
      }
    : null;
  return { kind: "plain", watched: watchedFigure, left: leftFigure };
}

/** One band: a part in upload order, with everything its band and its tooltip
 *  show. */
export interface Band {
  video: Video;
  /** 1-based, in upload order. */
  position: number;
  /** Its share of the row, as an `fr`. */
  weight: number;
  kind: PartKind;
  downloaded: boolean;
  next: boolean;
  /** Its own runtime, "" when unknown. */
  runtime: string;
}

export interface SeriesModel {
  bands: Band[];
  /** The next part's band, or null when nothing is left to watch. */
  next: Band | null;
  figures: Figures;
  gap: number;
  /** The grid's `grid-template-columns`: one track per part, bands and
   *  brackets alike, so a bracket over parts i..j meets their bands exactly.
   *  `minmax(0, …)`, because a bare `fr` track grows to fit what spans it and a
   *  bracket's caption would then widen its parts' bands. */
  columns: string;
  /** What the row says to a screen reader, in place of the picture. */
  label: string;
}

/** A part's state in words, for its tooltip: what it is, never what it is not. */
export function bandStates(b: Pick<Band, "kind" | "downloaded" | "next" | "video">): string[] {
  const out: string[] = [];
  if (b.video.watched) out.push("watched");
  if (b.downloaded) out.push("downloaded");
  if (b.video.hidden) out.push("hidden");
  if (b.next) out.unshift("next");
  else if (out.length === 0) out.push("to watch");
  return out;
}

function summary(bands: readonly Band[], next: Band | null, figures: Figures): string {
  const count = (f: (b: Band) => boolean) => bands.filter(f).length;
  const watched = count((b) => b.video.watched);
  const downloaded = count((b) => b.downloaded);
  const hidden = count((b) => b.video.hidden);
  const parts = [`${bands.length} parts`];
  if (watched === bands.length) parts.push("all watched");
  else if (watched) parts.push(`${watched} watched`);
  if (downloaded) parts.push(`${downloaded} downloaded`);
  if (hidden) parts.push(`${hidden} hidden`);
  if (next) parts.push(`part ${next.position} is next`);
  let text = parts.join(", ");
  const said: string[] = [];
  const w = figures.kind === "none" ? null : figures.watched;
  const l = figures.kind === "none" ? null : figures.left;
  if (w && w.runtime !== "—") said.push(`${w.runtime} watched`);
  if (l && l.runtime !== "—") said.push(`${l.runtime} left`);
  if (said.length) text += `. ${said.join(", ")}.`;
  return text;
}

export function seriesModel(parts: readonly Video[]): SeriesModel {
  const ordered = uploadOrder(parts);
  const weights = bandWeights(ordered.map((v) => v.duration_secs));
  const at = nextIndex(ordered);
  const bands: Band[] = ordered.map((video, i) => ({
    video,
    position: i + 1,
    weight: weights[i],
    kind: partKind(video),
    downloaded: video.download_state === "done",
    next: i === at,
    runtime: known(video.duration_secs) ? formatTotalRuntime(video.duration_secs) : "",
  }));
  const next = at >= 0 ? bands[at] : null;
  const figures = seriesFigures(ordered);
  return {
    bands,
    next,
    figures,
    gap: bandGap(bands.length),
    // Rounded: the ratio is what matters, and an average carries a tail of
    // digits that would only lengthen the style attribute.
    columns: weights.map((w) => `minmax(0, ${Math.round(w * 100) / 100}fr)`).join(" "),
    label: summary(bands, next, figures),
  };
}

/**
 * Where a box `width` wide goes so its middle sits on `centre`, without
 * leaving `lo`..`hi`: centred while there is room, held against the edge it
 * would cross when there is not. When it is wider than the whole span it keeps
 * to `lo`, so its start -- where reading begins -- stays in view.
 */
export function placeAround(centre: number, width: number, lo: number, hi: number): number {
  const ideal = centre - width / 2;
  return Math.max(lo, Math.min(ideal, hi - width));
}

/** A CSS `url()` for a custom property, quoted so a path with spaces or
 *  parentheses survives. */
export function cssUrl(src: string): string {
  return `url("${src.replace(/["\\\n]/g, (c) => (c === "\n" ? "" : `\\${c}`))}")`;
}
