import { useEffect, useRef, type ReactNode } from "react";
import VideoCard from "./VideoCard";
import type { CardAction } from "../format";
import type { DownloadProgress, Video } from "../types";

interface Props {
  videos: Video[];
  progress: Record<string, DownloadProgress>;
  loading: boolean;
  hasMore: boolean;
  onLoadMore: () => void;
  onAction: (action: CardAction, video: Video) => void;
  onToggleWatched: (video: Video) => void;
  empty?: ReactNode;
}

export default function VideoGrid({
  videos, progress, loading, hasMore, onLoadMore, onAction, onToggleWatched, empty,
}: Props) {
  const sentinel = useRef<HTMLDivElement | null>(null);
  // Held in a ref so the observer survives every parent re-render.
  const more = useRef({ hasMore, loading, onLoadMore });
  more.current = { hasMore, loading, onLoadMore };

  useEffect(() => {
    const node = sentinel.current;
    if (!node) return;
    const io = new IntersectionObserver(
      (entries) => {
        const { hasMore: h, loading: l, onLoadMore: fn } = more.current;
        if (entries.some((e) => e.isIntersecting) && h && !l) fn();
      },
      { rootMargin: "600px 0px" },
    );
    io.observe(node);
    return () => io.disconnect();
  }, []);

  if (!loading && videos.length === 0) {
    return <div className="grid-empty">{empty ?? <p>Nothing here yet.</p>}</div>;
  }

  return (
    <>
      <div className="video-grid">
        {videos.map((v) => (
          <VideoCard
            key={v.id}
            video={v}
            progress={progress[v.id]}
            onAction={onAction}
            onToggleWatched={onToggleWatched}
          />
        ))}
      </div>
      <div ref={sentinel} className="grid-sentinel" aria-hidden="true" />
      {loading && (
        <div className="grid-loading">
          <span className="spinner" aria-hidden="true" />
          Loading…
        </div>
      )}
      {!loading && !hasMore && videos.length > 0 && (
        <div className="grid-end">
          {videos.length} video{videos.length === 1 ? "" : "s"}
        </div>
      )}
    </>
  );
}
