import {
  useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject,
} from "react";
import VideoGrid from "./VideoGrid";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import ConfirmDialog from "./ConfirmDialog";
import CustomDownloadDialog from "./CustomDownloadDialog";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import { useDownloadEvents } from "../events";
import { hasDownloadedFile, seriesRuntime, videoUrl, type CardAction } from "../format";
import { useSiblingMarking } from "../marking";
import { usePending } from "../pending";
import { createProgressStore } from "../progress";
import type { SortOrder, Video, VideoGroup } from "../types";

const PAGE = 100;

/** How many lists' places to keep: the feed, an open series, and whatever the
 *  filter row was set to before this one. Every debounced search query makes a
 *  list of its own, so the oldest are dropped rather than kept for the session. */
const PLACES_KEPT = 16;

/**
 * Where a list was when it was last on screen. The paged-in count belongs with
 * the offset: put one back without the other and the offset lands past the end
 * of a grid that has shrunk to a single page, which is the bottom of page one
 * rather than where you were.
 */
interface Place {
  scroll: number;
  cards: number;
}

/** Files where a list was left, keeping only the most recently seen ones. */
function remember(places: Map<string, Place>, key: string, place: Place) {
  // Deleted first so it is re-inserted last, which makes the oldest key the
  // first one the eviction walk below reaches.
  places.delete(key);
  places.set(key, place);
  for (const oldest of places.keys()) {
    if (places.size <= PLACES_KEPT) break;
    places.delete(oldest);
  }
}

/** Whole pages enough to hold `cards`, so a restored grid still ends on a page
 *  boundary and `hasMore` keeps meaning "the last page came back full". */
function pagesFor(cards: number): number {
  return Math.max(PAGE, Math.ceil(cards / PAGE) * PAGE);
}

/** A row whose download has been cleared, as the backend writes it. */
const NO_FILE = { download_state: "none", download_error: null, file_path: null } as const;

/** The fields `NO_FILE` overwrites, as they were, for putting them back. */
function fileOf(v: Video): Pick<Video, "download_state" | "download_error" | "file_path"> {
  return { download_state: v.download_state, download_error: v.download_error, file_path: v.file_path };
}

/** What Download would take: never fetched, or fetched and failed. */
function downloadable(v: Video): boolean {
  return v.download_state === "none" || v.download_state === "failed";
}

/**
 * Marking and unlinking both end in a refetch that regroups the feed, and the
 * grid that is up stays up -- dimmed once any of them is slow -- until the new
 * one lands. Each runs under a key of its own behind this prefix: the same mark
 * or unlink pressed twice goes out once, but a *different* one made while the
 * first is out still goes out. One shared key used to drop it without a word,
 * after the selection it came from had already been cleared. Racing refetches
 * need no guard here -- `fetchPage`'s request counter lets only the newest land.
 */
const SIBLINGS = "siblings:";

/** What an open series holds, for the breadcrumb that names it. */
export interface SeriesTally {
  parts: number;
  runtime: string;
  unknown: number;
}

interface Props {
  /** The channels to show, sorted; empty for every channel. Held in the
   *  shell's state, so it keeps its identity until the pick changes. */
  channelIds: string[];
  search: string;
  hideWatched: boolean;
  downloadedOnly: boolean;
  sort: SortOrder;
  showHidden: boolean;
  /** Collapse each channel's multi-part uploads behind one card. */
  grouped: boolean;
  /** Grouped only: leave out lone videos, keeping the multi-part series. */
  groupsOnly: boolean;
  /** Grouped only: just the series with some parts watched and some not. */
  inProgress: boolean;
  /** When set, the grid shows this video's series instead of the feed. */
  siblingOf: Video | null;
  /** The stem comes from a series card; a lone card has only its title. */
  onFindSiblings: (video: Video, stem?: string | null) => void;
  /** Hands the nav's breadcrumb what the open series turned out to hold. */
  onSeriesTally: (t: SeriesTally) => void;
  cardSize: number;
  onCardSize: (px: number) => void;
  /** The shell's scroll container. The view needs it to put the feed back where
   *  it was: a series, or a poll's refetch, replaces the whole grid, and the
   *  browser clamps the offset to whatever short list arrives in its place. */
  scrollRef: RefObject<HTMLElement | null>;
  /** Bumped by the shell after a poll or an add, to force a refetch. */
  reloadToken: number;
  channelCount: number;
  onAdd: () => void;
}

export default function SubscriptionsView(p: Props) {
  const toast = useToast();
  // One entry per card. Ungrouped, every group holds a single video — carrying
  // one shape through the view keeps the two modes off every code path below.
  const [groups, setGroups] = useState<VideoGroup[]>([]);
  // Read by each card for itself: a tick never renders this view.
  const [progress] = useState(createProgressStore);
  const { pending, slow, run } = usePending();
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const offset = useRef(0);
  // Guards against a slow first request overwriting a newer filter's results.
  const request = useRef(0);

  const {
    channelIds, search, hideWatched, downloadedOnly, showHidden, sort, reloadToken, siblingOf,
    onSeriesTally, scrollRef,
  } = p;
  // A series view is a flat list of parts on purpose, so grouping stands down
  // while one is open rather than collapsing the very series being shown.
  const grouped = p.grouped && siblingOf === null;
  const groupsOnly = grouped && p.groupsOnly;
  const inProgress = grouped && p.inProgress;

  // What makes this list this list. A series, a filter, a sort and a search
  // each make a different one; `grouped` is the derived value above, so opening
  // a series out of the grouped feed counts as one change of list, not two.
  const listKey = JSON.stringify([
    channelIds, search.trim(), hideWatched, downloadedOnly, showHidden, sort, grouped, groupsOnly,
    inProgress, siblingOf?.id ?? null,
  ]);
  // Where each list this view has shown was left. Coming back to one -- leaving
  // a series, clearing a search -- comes back to your place in it; a list never
  // shown starts at the top. A refetch of the list already up (a poll, or a
  // sibling mark) is the same list, so it keeps its place too.
  const places = useRef(new Map<string, Place>());
  /** The list the grid holds, "" until its first page has landed. */
  const shown = useRef("");
  /** Where the next landed page zero puts the grid. */
  const restore = useRef(0);
  /** Bumped by each landed page zero, so the restore below runs in the commit
   *  that put those rows on screen and not in some later one. */
  const [pageEpoch, setPageEpoch] = useState(0);
  const [menu, setMenu] = useState<{ video: Video; x: number; y: number } | null>(null);
  /**
   * Three dialogs share this slot: "remove" asks how to hide or delete a video,
   * "file" asks only whether to delete a watched video's download, and
   * "series" asks how to hide a series with a part downloaded.
   */
  const [confirm, setConfirm] = useState<
    { kind: "remove" | "file"; video: Video } | { kind: "series"; group: VideoGroup } | null
  >(null);
  /** The video Download (custom)… is choosing formats for. */
  const [custom, setCustom] = useState<Video | null>(null);

  const fetchPage = useCallback(async (from: number) => {
    const id = ++request.current;
    setLoading(true);
    let limit = PAGE;
    if (from === 0) {
      // Filed now rather than when the rows land, because the grid on screen is
      // still the outgoing list's: by the time a shorter one has replaced it,
      // the browser has already clamped the offset to the bottom of what is
      // left and there is nothing left to read.
      if (shown.current) {
        remember(places.current, shown.current, {
          scroll: scrollRef.current?.scrollTop ?? 0,
          cards: offset.current,
        });
      }
      const back = places.current.get(listKey);
      restore.current = back?.scroll ?? 0;
      // Every page the list had, in one request. Asking for one page and letting
      // the sentinel fetch the rest would put the offset past the end of the
      // grid, which is the only place a scroll can be restored to.
      if (back) limit = pagesFor(back.cards);
      // Page zero is what resets the paging, so a load-more firing while this
      // request is in flight cannot page on from the outgoing list's offset.
      offset.current = 0;
    }
    // The limit counts cards either way, which for the grouped feed means groups.
    const filter = {
      channelIds,
      hideWatched,
      downloadedOnly,
      showHidden,
      groupsOnly,
      inProgress,
      search: search.trim() ? search.trim() : null,
      siblingOf: siblingOf?.id ?? null,
      sort,
      limit,
      offset: from,
    };
    try {
      const page = grouped
        ? await api.listVideoGroups(filter)
        : (await api.listVideos(filter)).map((v) => ({ videos: [v], stem: null }));
      if (id !== request.current) return;
      setGroups((prev) => (from === 0 ? page : [...prev, ...page]));
      offset.current = from + page.length;
      setHasMore(page.length === limit);
      if (from === 0) {
        shown.current = listKey;
        setPageEpoch((n) => n + 1);
      }
    } catch (err) {
      if (id !== request.current) return;
      setHasMore(false);
      toast.error(errText(err));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, [channelIds, search, hideWatched, downloadedOnly, showHidden, siblingOf, sort, grouped, groupsOnly,
      inProgress, listKey, scrollRef, toast]);

  useEffect(() => { void fetchPage(0); }, [fetchPage, reloadToken]);

  // Put the grid back where its list was left, in the same commit the rows
  // arrive in: a frame painted at the top on the way there is the jump this
  // exists to prevent.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = restore.current;
  }, [pageEpoch, scrollRef]);

  // The breadcrumb names the open series and says what it holds -- which this
  // view knows and the nav does not -- so every settled page hands the count
  // and the runtime up. Taken from the loaded list rather than from the page
  // that just arrived, so a second page adds to the tally instead of replacing
  // it.
  useEffect(() => {
    if (!siblingOf) return;
    // Mid-first-load the grid holds nothing yet, and "no other parts found" is
    // an answer, not a placeholder -- so nothing is said until one page is in.
    if (loading && groups.length === 0) return;
    const videos = groups.flatMap((g) => g.videos);
    onSeriesTally({ parts: videos.length, ...seriesRuntime(videos) });
  }, [siblingOf, groups, loading, onSeriesTally]);

  /**
   * Rewrites every video in place, wherever in the grouping it sits. A group
   * none of whose parts changed is handed back as the same object, and so is
   * the whole list when nothing did: the cards are memoised, and a fresh group
   * per event would re-render every series in the grid for one video's state.
   */
  const mapVideos = useCallback((fn: (v: Video) => Video) => {
    setGroups((prev) => {
      let changed = false;
      const next = prev.map((g) => {
        const videos = g.videos.map(fn);
        if (videos.every((v, i) => v === g.videos[i])) return g;
        changed = true;
        return { ...g, videos };
      });
      return changed ? next : prev;
    });
  }, []);

  const patch = useCallback((videoId: string, fields: Partial<Video>) => {
    mapVideos((v) => (v.id === videoId ? { ...v, ...fields } : v));
  }, [mapVideos]);

  // Live updates land straight on the cards; no refetch, no scroll jump.
  useDownloadEvents({
    onProgress: progress.set,
    onState: (s) => {
      mapVideos((v) => (
        v.id === s.videoId
          ? {
              ...v,
              download_state: s.state,
              file_path: s.filePath ?? (s.state === "done" ? v.file_path : null),
              download_error: s.error,
            }
          : v
      ));
      if (s.state !== "downloading") progress.clear(s.videoId);
    },
  });

  /** The grid as last rendered, for the undo `drop` hands back. */
  const groupsRef = useRef(groups);
  groupsRef.current = groups;

  /**
   * Every card action goes through `run`, keyed on the video, so a second press
   * while the first is out does nothing: the card is disabled until it lands.
   * Each change is shown before the call and put back if it fails.
   */
  const onAction = useCallback((action: CardAction, video: Video) => {
    const before = { download_state: video.download_state, download_error: video.download_error };
    const restore = () => patch(video.id, before);
    switch (action) {
      case "download":
      case "retry":
        void run(video.id, () => api.enqueueDownload(video.id), {
          optimistic: () => {
            patch(video.id, { download_state: "queued", download_error: null });
            return restore;
          },
        });
        break;
      case "cancel":
        // The card goes back to Download at once: the backend clears the row
        // before it has finished killing yt-dlp and sweeping its files, and the
        // `none` it sends then only confirms this.
        void run(video.id, () => api.cancelDownload(video.id), {
          optimistic: () => {
            patch(video.id, { download_state: "none", download_error: null });
            progress.clear(video.id);
            return restore;
          },
        });
        break;
      case "play":
        // Nothing to show optimistically, but a double press would otherwise
        // open the file in two players.
        void run(video.id, () => api.openInPlayer(video.id));
        break;
      case "open":
        // A key of its own, so a browser slow to come up never locks the
        // card's download button.
        void run(`open:${video.id}`, () => api.openExternal(videoUrl(video)));
        break;
    }
  }, [patch, progress, run]);

  /** Mirrors what the backend writes when it clears a download. */
  const deleteFile = useCallback((video: Video) => {
    void run(video.id, () => api.deleteDownload(video.id), {
      optimistic: () => {
        patch(video.id, NO_FILE);
        return () => patch(video.id, fileOf(video));
      },
    });
  }, [patch, run]);

  /**
   * Removes a video from view without waiting for a refetch, taking its card
   * with it once nothing is left in the group. Returns how to put it back where
   * it was, for a call that fails after the card has already gone.
   */
  const drop = useCallback((id: string): (() => void) => {
    const at = groupsRef.current.findIndex((g) => g.videos.some((v) => v.id === id));
    if (at < 0) return () => {};
    const group = groupsRef.current[at];
    const index = group.videos.findIndex((v) => v.id === id);
    const video = group.videos[index];
    setGroups((prev) => prev
      .map((g) => (g.videos.some((v) => v.id === id)
        ? { ...g, videos: g.videos.filter((v) => v.id !== id) }
        : g))
      .filter((g) => g.videos.length > 0));
    return () => setGroups((prev) => {
      if (prev.some((g) => g.videos.some((v) => v.id === id))) return prev;
      // Back into its own group if any of that group is still here, at the
      // place it held; otherwise as a card of its own where the group stood.
      const siblings = new Set(group.videos.map((v) => v.id));
      const home = prev.findIndex((g) => g.videos.some((v) => siblings.has(v.id)));
      const next = [...prev];
      if (home >= 0) {
        const videos = [...next[home].videos];
        videos.splice(Math.min(index, videos.length), 0, video);
        next[home] = { ...next[home], videos };
      } else {
        next.splice(Math.min(at, next.length), 0, { ...group, videos: [video] });
      }
      return next;
    });
  }, []);

  /**
   * `drop` for a whole card at once, which is how a series hidden as one
   * leaves. The undo puts the card back where it stood, unless any of its parts
   * is on screen again by then: a refetch has landed in between, and it knows
   * better than the copy kept here.
   */
  const dropCard = useCallback((leaderId: string): (() => void) => {
    const at = groupsRef.current.findIndex((g) => g.videos[0].id === leaderId);
    if (at < 0) return () => {};
    const card = groupsRef.current[at];
    const ids = new Set(card.videos.map((v) => v.id));
    setGroups((prev) => prev.filter((g) => g.videos[0].id !== leaderId));
    return () => setGroups((prev) => {
      if (prev.some((g) => g.videos.some((v) => ids.has(v.id)))) return prev;
      const next = [...prev];
      next.splice(Math.min(at, next.length), 0, card);
      return next;
    });
  }, []);

  const onToggleWatched = useCallback((video: Video) => {
    const next = !video.watched;
    const watched = { watched: next, watched_at: next ? Math.floor(Date.now() / 1000) : null };
    // Its own key: marking watched does not lock the card's download button,
    // but two toggles out at once could land in either order and leave the
    // card saying the opposite of the database.
    void run(`watched:${video.id}`, async () => {
      await api.setWatched(video.id, next);
      // With "hide watched" on, a freshly watched card no longer belongs here --
      // except in a series, which shows every part whatever the filters say.
      if (next && hideWatched && !siblingOf) drop(video.id);
      // Watching something is usually the end of its life on disk — but that is
      // the user's call, and it is a separate step from marking it watched.
      // The dialog gets the video as it now is: its copy reads `watched`, and
      // the argument is the row from before the click.
      if (next && hasDownloadedFile(video)) {
        setConfirm({ kind: "file", video: { ...video, ...watched } });
      }
    }, {
      optimistic: () => {
        patch(video.id, watched);
        return () => patch(video.id, { watched: video.watched, watched_at: video.watched_at });
      },
    });
  }, [patch, drop, hideWatched, siblingOf, run]);

  const unhideVideo = useCallback((video: Video) => {
    void run(video.id, () => api.setVideoHidden(video.id, false), {
      optimistic: () => {
        patch(video.id, { hidden: false });
        return () => patch(video.id, { hidden: true });
      },
    });
  }, [patch, run]);

  /** Records what the titles could not say: these are parts of one series. */
  const markSiblings = useCallback((ids: string[]) => {
    void run(`${SIBLINGS}mark:${[...ids].sort().join(",")}`, async () => {
      // The answer can exceed what was sent -- marking across two hand-built
      // groups merges both -- so the toast reports what actually happened.
      const n = await api.markSiblings(ids);
      toast.success(`Marked ${n} videos as siblings.`);
      await fetchPage(0);
    });
  }, [fetchPage, run, toast]);

  const unlinkSiblings = useCallback((video: Video) => {
    void run(`${SIBLINGS}unlink:${video.id}`, async () => {
      await api.unlinkSiblings(video.id);
      toast.success(`Unlinked “${video.title}”.`);
      await fetchPage(0);
    });
  }, [fetchPage, run, toast]);

  // A dropped card waits here for its confirmation; see `confirmDrop`.
  const [dropped, setDropped] = useState<Video[] | null>(null);

  const marking = useSiblingMarking(
    groups,
    useMemo(
      () => ({
        mark: (ids: string[]) => void markSiblings(ids),
        confirmDrop: setDropped,
        refuse: (message: string) => toast.error(message),
      }),
      [markSiblings, toast],
    ),
  );

  // A new filter, sort or refresh is a new grid, and nothing stays picked
  // across one.
  useEffect(() => { marking.clear(); }, [fetchPage, reloadToken, marking.clear]);

  /**
   * Hides a subscription video, or deletes one added by hand -- and its file
   * first, when asked. The card leaves at once (or turns hidden, with hidden
   * videos on show) and comes back as it was if anything fails.
   */
  const removeVideo = useCallback((video: Video, alsoDeleteFile: boolean) => {
    const deleting = video.added_manually;
    // Set once the file is gone, which no undo can bring back: a hide that
    // fails after it restores the card, but not as downloaded.
    let fileGone = false;
    void run(video.id, async () => {
      if (alsoDeleteFile) {
        await api.deleteDownload(video.id);
        fileGone = true;
      }
      if (deleting) await api.deleteVideo(video.id);
      else await api.setVideoHidden(video.id, true);
      toast.success(deleting ? `Deleted "${video.title}".` : `Hidden "${video.title}".`);
    }, {
      optimistic: () => {
        // A series keeps a part it would otherwise lose to "Include hidden"
        // being off: it shows every part, hidden ones included.
        if (deleting || (!showHidden && !siblingOf)) {
          const undo = drop(video.id);
          return () => {
            undo();
            if (fileGone) patch(video.id, NO_FILE);
          };
        }
        patch(video.id, { hidden: true, ...(alsoDeleteFile ? NO_FILE : {}) });
        return () => patch(video.id, { hidden: false, ...(fileGone ? {} : fileOf(video)) });
      },
    });
  }, [drop, patch, run, showHidden, siblingOf, toast]);

  const requestRemove = useCallback((video: Video) => {
    if (hasDownloadedFile(video)) setConfirm({ kind: "remove", video });
    else removeVideo(video, false);
  }, [removeVideo]);

  /**
   * Queues every part of a series that is not down or on its way. The whole
   * card goes to the backend, which picks those parts out again and queues
   * them oldest first -- so episode 1 starts first, whatever order the card
   * holds them in -- and each one shows queued at once.
   */
  const downloadSeries = useCallback((group: VideoGroup) => {
    const leader = group.videos[0];
    const name = group.stem ?? leader.title;
    const due = new Map(group.videos.filter(downloadable).map((v) => [v.id, v]));
    // A key apart from the hide's: neither bears on the other, so one being out
    // is no reason to refuse the other.
    void run(`series-download:${leader.id}`, async () => {
      const n = await api.enqueueDownloads(group.videos.map((v) => v.id));
      if (n > 0) toast.success(`Queued ${n} ${n === 1 ? "part" : "parts"} of “${name}”.`);
      else toast.info(`Every part of “${name}” is already downloaded or on its way.`);
    }, {
      optimistic: () => {
        mapVideos((v) => (due.has(v.id) ? { ...v, download_state: "queued", download_error: null } : v));
        // Only a part still showing that mark goes back: one a state event has
        // moved on since says what really happened to it.
        return () => mapVideos((v) => {
          const was = due.get(v.id);
          return was && v.download_state === "queued"
            ? { ...v, download_state: was.download_state, download_error: was.download_error }
            : v;
        });
      },
    });
  }, [mapVideos, run, toast]);

  /**
   * Hides a series as a whole -- every part, and any part that joins it later
   * -- removing its downloaded files first when asked. As with `removeVideo`,
   * the card leaves at once (or turns hidden, with hidden videos on show) and
   * comes back as it was if anything fails, less any file already gone.
   */
  const hideSeries = useCallback((group: VideoGroup, alsoDeleteFiles: boolean) => {
    const leader = group.videos[0];
    const name = group.stem ?? leader.title;
    const files = new Set(alsoDeleteFiles ? group.videos.filter(hasDownloadedFile).map((v) => v.id) : []);
    // The parts whose file is gone, which no undo can bring back.
    const gone = new Set<string>();
    // Shared with Un-hide series: two of them out at once could land in either
    // order and leave the card saying the opposite of the database.
    void run(`series-hidden:${leader.id}`, async () => {
      // One at a time, so a failure part way through knows which files went.
      for (const id of files) {
        await api.deleteDownload(id);
        gone.add(id);
      }
      await api.setSeriesHidden(leader.id, true);
      toast.success(`Hidden “${name}”. Parts that join it later stay hidden too.`);
    }, {
      optimistic: () => {
        // Never in a series view in practice -- grouping stands down there, so
        // no series card is up -- but it is the rule `removeVideo` keeps.
        if (!showHidden && !siblingOf) {
          const undo = dropCard(leader.id);
          return () => {
            undo();
            if (gone.size > 0) mapVideos((v) => (gone.has(v.id) ? { ...v, ...NO_FILE } : v));
          };
        }
        const was = new Map(group.videos.map((v) => [v.id, v]));
        mapVideos((v) => (was.has(v.id)
          ? { ...v, hidden: true, series_hidden: true, ...(files.has(v.id) ? NO_FILE : {}) }
          : v));
        return () => mapVideos((v) => {
          const before = was.get(v.id);
          if (!before) return v;
          return {
            ...v,
            hidden: before.hidden,
            series_hidden: before.series_hidden,
            ...(files.has(v.id) && !gone.has(v.id) ? fileOf(before) : {}),
          };
        });
      },
    });
  }, [dropCard, mapVideos, run, showHidden, siblingOf, toast]);

  const requestHideSeries = useCallback((group: VideoGroup) => {
    if (group.videos.some(hasDownloadedFile)) setConfirm({ kind: "series", group });
    else hideSeries(group, false);
  }, [hideSeries]);

  /** Brings every part of a series back and clears the mark, so a part that
   *  joins it later stays in the feed too. */
  const unhideSeries = useCallback((group: VideoGroup) => {
    const leader = group.videos[0];
    const was = new Map(group.videos.map((v) => [v.id, v]));
    void run(`series-hidden:${leader.id}`, () => api.setSeriesHidden(leader.id, false), {
      optimistic: () => {
        mapVideos((v) => (was.has(v.id) ? { ...v, hidden: false, series_hidden: false } : v));
        return () => mapVideos((v) => {
          const before = was.get(v.id);
          return before ? { ...v, hidden: before.hidden, series_hidden: before.series_hidden } : v;
        });
      },
    });
  }, [mapVideos, run]);

  const menuItems = useCallback((video: Video): MenuItem[] => {
    const picked = marking.selected.size;
    const markItem: MenuItem[] =
      picked >= 2 && marking.selected.has(video.id)
        ? [{ label: `Mark ${picked} videos as siblings`, onSelect: marking.markSelected }]
        : [];

    // A series card stands for every one of its parts, so it gets a menu of its
    // own: the items further down all act on a single video, and from here
    // would silently act on one part out of seven. Everything in this one acts
    // on the whole series -- marking too, since it is the card being marked.
    const card = groups.find((g) => g.videos[0].id === video.id);
    if (card && card.videos.length > 1) {
      const series = [...markItem];
      // Watched and hidden parts count: downloading a series is asking for all
      // of it. The backend repeats the pick, so this is only the label's count.
      const due = card.videos.filter(downloadable).length;
      if (due > 0) {
        series.push({
          label: `Download ${due} ${due === 1 ? "part" : "parts"}`,
          onSelect: () => void downloadSeries(card),
        });
      }
      // Offered wherever the mark is, even on a part brought back by hand: the
      // mark is what hides the parts still to come, and this is how it goes.
      if (card.videos.some((v) => v.series_hidden)) {
        series.push({ label: "Un-hide series", onSelect: () => void unhideSeries(card) });
      } else {
        series.push({
          label: "Hide from feed",
          danger: true,
          onSelect: () => requestHideSeries(card),
        });
      }
      return series;
    }

    const items: MenuItem[] = [
      ...markItem,
      {
        label: video.watched ? "Mark as unwatched" : "Mark as watched",
        onSelect: () => void onToggleWatched(video),
      },
    ];
    if (hasDownloadedFile(video)) {
      items.push({ label: "Play", onSelect: () => void onAction("play", video) });
    } else if (downloadable(video)) {
      items.push({ label: "Download", onSelect: () => void onAction("download", video) });
      items.push({ label: "Download (custom)…", onSelect: () => setCustom(video) });
    }
    // Offered for any downloaded file, watched or not: the prompt at
    // marking-time is easy to dismiss, and a video you decide *not* to watch
    // leaves a file behind just the same.
    if (hasDownloadedFile(video)) {
      items.push({
        label: "Delete downloaded file",
        danger: true,
        onSelect: () => setConfirm({ kind: "file", video }),
      });
    }
    items.push({
      label: "Find siblings",
      onSelect: () => p.onFindSiblings(video),
    });
    if (video.sibling_group) {
      items.push({
        label: "Unlink from siblings",
        onSelect: () => void unlinkSiblings(video),
      });
    }
    if (video.hidden) {
      items.push({ label: "Un-hide", onSelect: () => void unhideVideo(video) });
    } else {
      items.push({
        label: video.added_manually ? "Delete" : "Hide from feed",
        danger: true,
        onSelect: () => requestRemove(video),
      });
    }
    return items;
  }, [onToggleWatched, onAction, unhideVideo, requestRemove, unlinkSiblings, groups,
      downloadSeries, unhideSeries, requestHideSeries,
      marking.selected, marking.markSelected, p.onFindSiblings]);

  const openMenu = useCallback((video: Video, x: number, y: number) => {
    // Every card has something to offer -- a series at least its Hide or
    // Un-hide -- but an empty menu is worse than none, so one never opens.
    if (menuItems(video).length === 0) return;
    setMenu({ video, x, y });
  }, [menuItems]);

  // `openMenu` changes with every change to the grid, since the menu reads the
  // groups; handed to the cards as is, it would re-render all of them each time
  // and undo their memo. They get this instead, which never changes.
  const openMenuRef = useRef(openMenu);
  openMenuRef.current = openMenu;
  const onContextMenu = useCallback((video: Video, x: number, y: number) => {
    openMenuRef.current(video, x, y);
  }, []);

  const filtered =
    channelIds.length > 0 || search.trim() !== "" || hideWatched || downloadedOnly || groupsOnly ||
    inProgress;
  /** Any mark or unlink slow enough to show; see `SIBLINGS`. */
  const refreshing = useMemo(() => [...slow].some((k) => k.startsWith(SIBLINGS)), [slow]);
  /** How many parts of the series being asked about have a file, which the
   *  dialog's words follow: one file reads as a lone video's dialog does. */
  const seriesFiles =
    confirm?.kind === "series" ? confirm.group.videos.filter(hasDownloadedFile).length : 0;

  return (
    <>
    <VideoGrid
      groups={groups}
      progress={progress}
      pending={pending}
      slow={slow}
      refreshing={refreshing}
      loading={loading}
      hasMore={hasMore}
      cardSize={p.cardSize}
      onCardSize={p.onCardSize}
      onLoadMore={() => fetchPage(offset.current)}
      onAction={onAction}
      onContextMenu={onContextMenu}
      onOpenSeries={p.onFindSiblings}
      marking={marking}
      empty={
        p.channelCount === 0 && !filtered ? (
          <div className="empty">
            <h2>No subscriptions yet</h2>
            <p>
              Paste a channel URL, an <code>@handle</code>, or a <code>UC…</code> id to start
              tracking a channel. You can also paste a single video URL to download just that one.
            </p>
            <button type="button" className="btn btn-primary" onClick={p.onAdd}>
              Add a channel or video
            </button>
          </div>
        ) : (
          <div className="empty">
            <h2>Nothing matches</h2>
            <p>
              {filtered
                ? "No videos match the current filters. Try clearing the search or the channel filter."
                : "Your subscriptions have no videos yet. Hit refresh to check their feeds."}
            </p>
          </div>
        )
      }
    />

    {menu && (
      <ContextMenu
        x={menu.x}
        y={menu.y}
        items={menuItems(menu.video)}
        onClose={() => setMenu(null)}
      />
    )}

    {custom && (
      <CustomDownloadDialog
        video={custom}
        // The same optimistic mark a plain Download makes; the queue's own
        // state events take over from here.
        onQueued={(v) => patch(v.id, { download_state: "queued", download_error: null })}
        onClose={() => setCustom(null)}
      />
    )}

    {confirm?.kind === "file" && (
      <ConfirmDialog
        title="Delete the downloaded file?"
        body={
          confirm.video.watched
            ? `“${confirm.video.title}” is marked as watched. Its file is still on disk.`
            : `“${confirm.video.title}” has not been watched. Deleting only removes the` +
              ` file — the video stays in your feed.`
        }
        choices={[{ label: "Delete file", value: "delete", danger: true }]}
        onCancel={() => setConfirm(null)}
        onChoose={() => {
          const target = confirm.video;
          setConfirm(null);
          void deleteFile(target);
        }}
      />
    )}

    {dropped && (
      <ConfirmDialog
        title={
          dropped.length === 2
            ? "Mark these two as siblings?"
            : `Mark ${dropped.length} videos as siblings?`
        }
        body={
          (dropped.length === 2
            ? `“${dropped[0].title}” and “${dropped[1].title}”`
            : `${dropped.length - 1} videos and “${dropped[dropped.length - 1].title}”`) +
          " will be shown as parts of one series. “Unlink from siblings” on any of them undoes it."
        }
        choices={[{ label: "Mark as siblings", value: "mark", primary: true }]}
        onCancel={() => setDropped(null)}
        onChoose={() => {
          const ids = dropped.map((v) => v.id);
          setDropped(null);
          marking.clear();
          void markSiblings(ids);
        }}
      />
    )}

    {confirm?.kind === "remove" && (
      <ConfirmDialog
        title={confirm.video.added_manually ? "Delete this video?" : "Hide this video?"}
        body={
          confirm.video.added_manually
            ? "It will be removed from MyTube. The downloaded file is kept unless you choose otherwise."
            : "It will stay out of your feed even after future refreshes. The downloaded file is kept unless you choose otherwise."
        }
        choices={[
          {
            label: confirm.video.added_manually ? "Delete, keep file" : "Hide, keep file",
            value: "keep",
            primary: true,
          },
          {
            label: confirm.video.added_manually ? "Delete and remove file" : "Hide and remove file",
            value: "delete",
            danger: true,
          },
        ]}
        onCancel={() => setConfirm(null)}
        onChoose={(choice) => {
          const target = confirm.video;
          setConfirm(null);
          void removeVideo(target, choice === "delete");
        }}
      />
    )}

    {confirm?.kind === "series" && (
      <ConfirmDialog
        title="Hide this series?"
        body={
          (confirm.group.videos.length === 2 ? "Both parts" : `All ${confirm.group.videos.length} parts`) +
          ` of “${confirm.group.stem ?? confirm.group.videos[0].title}”, and any part that joins` +
          " it later, will stay out of your feed. " +
          (seriesFiles === 1 ? "The downloaded file is" : `The ${seriesFiles} downloaded files are`) +
          " kept unless you choose otherwise."
        }
        choices={[
          { label: seriesFiles === 1 ? "Hide, keep file" : "Hide, keep files", value: "keep", primary: true },
          {
            label: seriesFiles === 1 ? "Hide and remove file" : "Hide and remove files",
            value: "delete",
            danger: true,
          },
        ]}
        onCancel={() => setConfirm(null)}
        onChoose={(choice) => {
          // The card as it is now rather than when the dialog opened: a part
          // that finished downloading meanwhile has a file to remove as well.
          const leader = confirm.group.videos[0].id;
          const target = groupsRef.current.find((g) => g.videos[0].id === leader) ?? confirm.group;
          setConfirm(null);
          void hideSeries(target, choice === "delete");
        }}
      />
    )}
    </>
  );
}
