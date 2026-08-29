import { useCallback, useEffect, useRef, useState } from "react";
import VideoGrid from "./VideoGrid";
import ContextMenu, { type MenuItem } from "./ContextMenu";
import ConfirmDialog from "./ConfirmDialog";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import { useDownloadEvents } from "../events";
import type { CardAction } from "../format";
import type { DownloadProgress, SortOrder, Video } from "../types";

const PAGE = 100;

interface Props {
  channelId: string | null;
  search: string;
  hideWatched: boolean;
  downloadedOnly: boolean;
  sort: SortOrder;
  showHidden: boolean;
  cardSize: number;
  onCardSize: (px: number) => void;
  /** Bumped by the shell after a poll or an add, to force a refetch. */
  reloadToken: number;
  channelCount: number;
  onAdd: () => void;
}

export default function SubscriptionsView(p: Props) {
  const toast = useToast();
  const [videos, setVideos] = useState<Video[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [loading, setLoading] = useState(true);
  const [hasMore, setHasMore] = useState(false);
  const offset = useRef(0);
  // Guards against a slow first request overwriting a newer filter's results.
  const request = useRef(0);

  const { channelId, search, hideWatched, downloadedOnly, showHidden, sort, reloadToken } = p;
  const [menu, setMenu] = useState<{ video: Video; x: number; y: number } | null>(null);
  const [confirm, setConfirm] = useState<Video | null>(null);

  const fetchPage = useCallback(async (from: number) => {
    const id = ++request.current;
    setLoading(true);
    try {
      const page = await api.listVideos({
        channelId,
        hideWatched,
        downloadedOnly,
        showHidden,
        search: search.trim() ? search.trim() : null,
        sort,
        limit: PAGE,
        offset: from,
      });
      if (id !== request.current) return;
      setVideos((prev) => (from === 0 ? page : [...prev, ...page]));
      offset.current = from + page.length;
      setHasMore(page.length === PAGE);
    } catch (err) {
      if (id !== request.current) return;
      setHasMore(false);
      toast.error(errText(err));
    } finally {
      if (id === request.current) setLoading(false);
    }
  }, [channelId, search, hideWatched, downloadedOnly, showHidden, sort, toast]);

  useEffect(() => {
    offset.current = 0;
    void fetchPage(0);
  }, [fetchPage, reloadToken]);

  const patch = useCallback((videoId: string, fields: Partial<Video>) => {
    setVideos((prev) => prev.map((v) => (v.id === videoId ? { ...v, ...fields } : v)));
  }, []);

  // Live updates land straight on the cards; no refetch, no scroll jump.
  useDownloadEvents({
    onProgress: (pr) => setProgress((prev) => ({ ...prev, [pr.videoId]: pr })),
    onState: (s) => {
      setVideos((prev) => prev.map((v) => (
        v.id === s.videoId
          ? {
              ...v,
              download_state: s.state,
              file_path: s.filePath ?? (s.state === "done" ? v.file_path : null),
              download_error: s.error,
            }
          : v
      )));
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
      }
    } catch (err) {
      patch(video.id, before);
      toast.error(errText(err));
    }
  }, [patch, toast]);

  const onToggleWatched = useCallback(async (video: Video) => {
    const next = !video.watched;
    patch(video.id, {
      watched: next,
      watched_at: next ? Math.floor(Date.now() / 1000) : null,
    });
    try {
      await api.setWatched(video.id, next);
      // With "hide watched" on, a freshly watched card no longer belongs here.
      if (next && hideWatched) {
        setVideos((prev) => prev.filter((v) => v.id !== video.id));
      }
    } catch (err) {
      patch(video.id, { watched: video.watched, watched_at: video.watched_at });
      toast.error(errText(err));
    }
  }, [patch, hideWatched, toast]);

  /** Removes the card from view without waiting for a refetch. */
  const drop = useCallback((id: string) => {
    setVideos((prev) => prev.filter((v) => v.id !== id));
  }, []);

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
    const hasFile = video.download_state === "done" && !!video.file_path;
    if (hasFile) setConfirm(video);
    else void removeVideo(video, false);
  }, [removeVideo]);

  const menuItems = useCallback((video: Video): MenuItem[] => {
    const items: MenuItem[] = [
      {
        label: video.watched ? "Mark as unwatched" : "Mark as watched",
        onSelect: () => void onToggleWatched(video),
      },
    ];
    if (video.download_state === "done" && video.file_path) {
      items.push({ label: "Play", onSelect: () => void onAction("play", video) });
    } else if (video.download_state === "none" || video.download_state === "failed") {
      items.push({ label: "Download", onSelect: () => void onAction("download", video) });
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
  }, [onToggleWatched, onAction, unhideVideo, requestRemove]);

  const openMenu = useCallback((video: Video, x: number, y: number) => {
    setMenu({ video, x, y });
  }, []);

  const filtered = channelId !== null || search.trim() !== "" || hideWatched || downloadedOnly;

  return (
    <>
    <VideoGrid
      videos={videos}
      progress={progress}
      loading={loading}
      hasMore={hasMore}
      cardSize={p.cardSize}
      onCardSize={p.onCardSize}
      onLoadMore={() => fetchPage(offset.current)}
      onAction={onAction}
      onToggleWatched={onToggleWatched}
      onContextMenu={openMenu}
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

    {confirm && (
      <ConfirmDialog
        title={confirm.added_manually ? "Delete this video?" : "Hide this video?"}
        body={
          confirm.added_manually
            ? "It will be removed from MyTube. The downloaded file is kept unless you choose otherwise."
            : "It will stay out of your feed even after future refreshes. The downloaded file is kept unless you choose otherwise."
        }
        choices={[
          {
            label: confirm.added_manually ? "Delete, keep file" : "Hide, keep file",
            value: "keep",
            primary: true,
          },
          {
            label: confirm.added_manually ? "Delete and remove file" : "Hide and remove file",
            value: "delete",
            danger: true,
          },
        ]}
        onCancel={() => setConfirm(null)}
        onChoose={(choice) => {
          const target = confirm;
          setConfirm(null);
          void removeVideo(target, choice === "delete");
        }}
      />
    )}
    </>
  );
}
