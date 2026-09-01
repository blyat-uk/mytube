import { useCallback, useEffect, useRef, useState } from "react";
import VideoGrid from "./VideoGrid";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import ConfirmDialog from "./ConfirmDialog";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import { useDownloadEvents } from "../events";
import { hasDownloadedFile, seriesRuntime, videoUrl, type CardAction } from "../format";
import type { DownloadProgress, SortOrder, Video, VideoGroup } from "../types";

const PAGE = 100;

/** What an open series holds, for the breadcrumb that names it. */
export interface SeriesTally {
  parts: number;
  runtime: string;
  unknown: number;
}

interface Props {
  channelId: string | null;
  search: string;
  hideWatched: boolean;
  downloadedOnly: boolean;
  sort: SortOrder;
  showHidden: boolean;
  /** Collapse each channel's multi-part uploads behind one card. */
  grouped: boolean;
  /** When set, the grid shows this video's series instead of the feed. */
  siblingOf: Video | null;
  /** The stem comes from a series card; a lone card has only its title. */
  onFindSiblings: (video: Video, stem?: string | null) => void;
  /** Hands the nav's breadcrumb what the open series turned out to hold. */
  onSeriesTally: (t: SeriesTally) => void;
  cardSize: number;
  onCardSize: (px: number) => void;
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
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const offset = useRef(0);
  // Guards against a slow first request overwriting a newer filter's results.
  const request = useRef(0);

  const {
    channelId, search, hideWatched, downloadedOnly, showHidden, sort, reloadToken, siblingOf,
    onSeriesTally,
  } = p;
  // A series view is a flat list of parts on purpose, so grouping stands down
  // while one is open rather than collapsing the very series being shown.
  const grouped = p.grouped && siblingOf === null;
  const [menu, setMenu] = useState<{ video: Video; x: number; y: number } | null>(null);
  /**
   * Two dialogs share this slot: "remove" asks how to hide or delete a video,
   * "file" asks only whether to delete a watched video's download.
   */
  const [confirm, setConfirm] = useState<{ kind: "remove" | "file"; video: Video } | null>(null);

  const fetchPage = useCallback(async (from: number) => {
    const id = ++request.current;
    setLoading(true);
    // PAGE counts cards either way, which for the grouped feed means groups.
    const filter = {
      channelId,
      hideWatched,
      downloadedOnly,
      showHidden,
      search: search.trim() ? search.trim() : null,
      siblingOf: siblingOf?.id ?? null,
      sort,
      limit: PAGE,
      offset: from,
    };
    try {
      const page = grouped
        ? await api.listVideoGroups(filter)
        : (await api.listVideos(filter)).map((v) => ({ videos: [v], stem: null }));
      if (id !== request.current) return;
      setGroups((prev) => (from === 0 ? page : [...prev, ...page]));
      offset.current = from + page.length;
      setHasMore(page.length === PAGE);
    } catch (err) {
      if (id !== request.current) return;
      setHasMore(false);
      toast.error(errText(err));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, [channelId, search, hideWatched, downloadedOnly, showHidden, siblingOf, sort, grouped, toast]);

  useEffect(() => {
    offset.current = 0;
    void fetchPage(0);
  }, [fetchPage, reloadToken]);

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

  /** Rewrites every video in place, wherever in the grouping it sits. */
  const mapVideos = useCallback((fn: (v: Video) => Video) => {
    setGroups((prev) => prev.map((g) => ({ ...g, videos: g.videos.map(fn) })));
  }, []);

  const patch = useCallback((videoId: string, fields: Partial<Video>) => {
    mapVideos((v) => (v.id === videoId ? { ...v, ...fields } : v));
  }, [mapVideos]);

  // Live updates land straight on the cards; no refetch, no scroll jump.
  useDownloadEvents({
    onProgress: (pr) => setProgress((prev) => ({ ...prev, [pr.videoId]: pr })),
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
      if (s.state !== "downloading") {
        setProgress((prev) => {
          if (!(s.videoId in prev)) return prev;
          const next = { ...prev };
          delete next[s.videoId];
          return next;
        });
      }
    },
  });

  const onAction = useCallback(async (action: CardAction, video: Video) => {
    const before = { download_state: video.download_state, download_error: video.download_error };
    try {
      switch (action) {
        case "download":
        case "retry":
          patch(video.id, { download_state: "queued", download_error: null });
          await api.enqueueDownload(video.id);
          break;
        case "cancel":
          await api.cancelDownload(video.id);
          break;
        case "play":
          await api.openInPlayer(video.id);
          break;
        case "open":
          await api.openExternal(videoUrl(video));
          break;
      }
    } catch (err) {
      patch(video.id, before);
      toast.error(errText(err));
    }
  }, [patch, toast]);

  /** Mirrors what the backend writes when it clears a download. */
  const deleteFile = useCallback(async (video: Video) => {
    try {
      await api.deleteDownload(video.id);
      patch(video.id, { download_state: "none", download_error: null, file_path: null });
    } catch (err) {
      toast.error(errText(err));
    }
  }, [patch, toast]);

  /** Removes a video from view without waiting for a refetch, taking its card
   *  with it once nothing is left in the group. */
  const drop = useCallback((id: string) => {
    setGroups((prev) => prev
      .map((g) => (g.videos.some((v) => v.id === id)
        ? { ...g, videos: g.videos.filter((v) => v.id !== id) }
        : g))
      .filter((g) => g.videos.length > 0));
  }, []);

  const onToggleWatched = useCallback(async (video: Video) => {
    const next = !video.watched;
    patch(video.id, {
      watched: next,
      watched_at: next ? Math.floor(Date.now() / 1000) : null,
    });
    try {
      await api.setWatched(video.id, next);
      // With "hide watched" on, a freshly watched card no longer belongs here.
      if (next && hideWatched) drop(video.id);
      // Watching something is usually the end of its life on disk — but that is
      // the user's call, and it is a separate step from marking it watched.
      if (next && hasDownloadedFile(video)) setConfirm({ kind: "file", video });
    } catch (err) {
      patch(video.id, { watched: video.watched, watched_at: video.watched_at });
      toast.error(errText(err));
    }
  }, [patch, drop, hideWatched, toast]);

  const hideVideo = useCallback(async (video: Video) => {
    try {
      await api.setVideoHidden(video.id, true);
      if (!showHidden) drop(video.id);
      else patch(video.id, { hidden: true });
      toast.success(`Hidden "${video.title}".`);
    } catch (err) {
      toast.error(errText(err));
    }
  }, [drop, patch, showHidden, toast]);

  const unhideVideo = useCallback(async (video: Video) => {
    try {
      await api.setVideoHidden(video.id, false);
      patch(video.id, { hidden: false });
    } catch (err) {
      toast.error(errText(err));
    }
  }, [patch, toast]);

  const deleteVideo = useCallback(async (video: Video) => {
    try {
      await api.deleteVideo(video.id);
      drop(video.id);
      toast.success(`Deleted "${video.title}".`);
    } catch (err) {
      toast.error(errText(err));
    }
  }, [drop, toast]);

  /** Hiding or deleting a downloaded video asks before touching the file. */
  const removeVideo = useCallback(async (video: Video, alsoDeleteFile: boolean) => {
    if (alsoDeleteFile) {
      try {
        await api.deleteDownload(video.id);
      } catch (err) {
        toast.error(errText(err));
        return;
      }
    }
    if (video.added_manually) await deleteVideo(video);
    else await hideVideo(video);
  }, [deleteVideo, hideVideo, toast]);

  const requestRemove = useCallback((video: Video) => {
    if (hasDownloadedFile(video)) setConfirm({ kind: "remove", video });
    else void removeVideo(video, false);
  }, [removeVideo]);

  const menuItems = useCallback((video: Video): MenuItem[] => {
    const items: MenuItem[] = [
      {
        label: video.watched ? "Mark as unwatched" : "Mark as watched",
        onSelect: () => void onToggleWatched(video),
      },
    ];
    if (hasDownloadedFile(video)) {
      items.push({ label: "Play", onSelect: () => void onAction("play", video) });
    } else if (video.download_state === "none" || video.download_state === "failed") {
      items.push({ label: "Download", onSelect: () => void onAction("download", video) });
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
  }, [onToggleWatched, onAction, unhideVideo, requestRemove, p.onFindSiblings]);

  const openMenu = useCallback((video: Video, x: number, y: number) => {
    setMenu({ video, x, y });
  }, []);

  const filtered = channelId !== null || search.trim() !== "" || hideWatched || downloadedOnly;

  return (
    <>
    <VideoGrid
      groups={groups}
      progress={progress}
      loading={loading}
      hasMore={hasMore}
      cardSize={p.cardSize}
      onCardSize={p.onCardSize}
      onLoadMore={() => fetchPage(offset.current)}
      onAction={onAction}
      onContextMenu={openMenu}
      onOpenSeries={p.onFindSiblings}
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
    </>
  );
}
