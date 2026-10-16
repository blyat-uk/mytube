import type { DownloadState, PlaylistEntry } from "../../types";

/**
 * Whether Download would queue a part: nothing on disk and nothing on its way.
 * A failed part counts, since downloading it again is the retry; a queued or
 * downloading one does not, since `enqueue_downloads` would skip it and the
 * button would promise one more than it starts.
 */
export function wantsDownload(state: DownloadState): boolean {
  return state === "none" || state === "failed";
}

/** What the summary's tags say about the whole playlist, ticked or not. */
export interface PlaylistTally {
  /** Not in the library yet. */
  fresh: number;
  /** Already in the library, downloaded or not. */
  inLibrary: number;
  downloaded: number;
  membersOnly: number;
}

export function tallyPlaylist(entries: readonly PlaylistEntry[]): PlaylistTally {
  const t: PlaylistTally = { fresh: 0, inLibrary: 0, downloaded: 0, membersOnly: 0 };
  for (const e of entries) {
    if (e.inLibrary) t.inLibrary += 1;
    else t.fresh += 1;
    if (e.downloadState === "done") t.downloaded += 1;
    if (e.membersOnly) t.membersOnly += 1;
  }
  return t;
}

/** What the buttons would act on: the ticked parts, and of those the ones each
 *  action would actually touch. */
export interface TickedCounts {
  ticked: number;
  /** Ticked and not in the library: what Add adds. */
  toAdd: number;
  /** Ticked and wanting a download: what Download queues. */
  toDownload: number;
  /** Ticked and already downloaded, so a 0 on Download can say why. */
  downloaded: number;
  /** The ticked parts' durations added up; one not known counts as zero... */
  runtimeSecs: number;
  /** ...and is counted here, so the total can be marked as a lower bound. */
  unknownRuntime: number;
}

export function countTicked(
  entries: readonly PlaylistEntry[], ticked: ReadonlySet<string>,
): TickedCounts {
  const c: TickedCounts = {
    ticked: 0, toAdd: 0, toDownload: 0, downloaded: 0, runtimeSecs: 0, unknownRuntime: 0,
  };
  for (const e of entries) {
    if (!ticked.has(e.id)) continue;
    c.ticked += 1;
    if (!e.inLibrary) c.toAdd += 1;
    if (wantsDownload(e.downloadState)) c.toDownload += 1;
    if (e.downloadState === "done") c.downloaded += 1;
    if (e.durationSecs) c.runtimeSecs += e.durationSecs;
    else c.unknownRuntime += 1;
  }
  return c;
}

/** The ticked parts, in the playlist's own order -- which is the order they
 *  are handed to `add_playlist`, and so the order they are inserted in. */
export function tickedEntries(
  entries: readonly PlaylistEntry[], ticked: ReadonlySet<string>,
): PlaylistEntry[] {
  return entries.filter((e) => ticked.has(e.id));
}

/** The one-word state a part's row wears, most decisive first. */
export type PartState = "downloaded" | "downloading" | "in library" | "members" | "new";

export function partState(e: PlaylistEntry): PartState {
  if (e.downloadState === "done") return "downloaded";
  if (e.downloadState === "queued" || e.downloadState === "downloading") return "downloading";
  if (e.inLibrary) return "in library";
  if (e.membersOnly) return "members";
  return "new";
}
