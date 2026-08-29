import { useCallback, useEffect, useLayoutEffect, useRef, type ReactNode } from "react";
import VideoCard from "./VideoCard";
import { nextCardSize, type CardAction } from "../format";
import type { DownloadProgress, Video } from "../types";

interface Props {
  videos: Video[];
  progress: Record<string, DownloadProgress>;
  loading: boolean;
  hasMore: boolean;
  cardSize: number;
  onCardSize: (px: number) => void;
  onLoadMore: () => void;
  onAction: (action: CardAction, video: Video) => void;
  onToggleWatched: (video: Video) => void;
  onContextMenu: (video: Video, x: number, y: number) => void;
  empty?: ReactNode;
}

/** Above this many new rows at once, skip the animation and just render. */
const MAX_ANIMATED = 60;
/** Only cards within this margin of the viewport are worth animating. */
const NEAR_VIEWPORT = 400;

export default function VideoGrid({
  videos, progress, loading, hasMore, cardSize, onCardSize,
  onLoadMore, onAction, onToggleWatched, onContextMenu, empty,
}: Props) {
  const sentinel = useRef<HTMLDivElement | null>(null);
  const gridRef = useRef<HTMLDivElement | null>(null);
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

  /* ---------------- Ctrl+scroll zoom ---------------- */

  const size = useRef(cardSize);
  size.current = cardSize;

  useEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    // Must be non-passive: the browser would otherwise page-zoom on ctrl+wheel
    // and preventDefault() would be ignored.
    const onWheel = (e: WheelEvent) => {
      if (!e.ctrlKey) return;
      e.preventDefault();
      const next = nextCardSize(size.current, e.deltaY);
      if (next !== size.current) onCardSize(next);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [onCardSize]);

  /* ---------------- FLIP insertion animation ----------------
   * Cards already on screen glide to their new positions while a newly
   * inserted card scales in, so videos arriving mid-list read as "pushed in"
   * rather than as the grid silently teleporting.
   *
   * Insertions only. Re-sorting, filtering or searching replaces the whole
   * list, and animating every card then is noise, not information.            */

  const prevRects = useRef<Map<string, DOMRect>>(new Map());
  const prevIds = useRef<string[]>([]);

  useLayoutEffect(() => {
    const el = gridRef.current;
    if (!el) return;

    const ids = videos.map((v) => v.id);
    const before = prevRects.current;
    const beforeIds = new Set(prevIds.current);
    const added = ids.filter((id) => !beforeIds.has(id));
    const kept = ids.filter((id) => beforeIds.has(id));

    // A pure insertion keeps every previous id; anything else is a re-render we
    // deliberately do not animate.
    const isInsertion =
      prevIds.current.length > 0 &&
      added.length > 0 &&
      added.length <= MAX_ANIMATED &&
      kept.length === prevIds.current.length;

    if (isInsertion) {
      const vh = window.innerHeight;
      for (const child of Array.from(el.children) as HTMLElement[]) {
        const id = child.dataset.videoId;
        if (!id) continue;
        const now = child.getBoundingClientRect();
        if (now.bottom < -NEAR_VIEWPORT || now.top > vh + NEAR_VIEWPORT) continue;

        const then = before.get(id);
        if (then) {
          const dx = then.left - now.left;
          const dy = then.top - now.top;
          if (dx || dy) {
            child.animate(
              [
                { transform: `translate(${dx}px, ${dy}px)` },
                { transform: "translate(0, 0)" },
              ],
              { duration: 320, easing: "cubic-bezier(0.22, 1, 0.36, 1)" },
            );
          }
        } else {
          child.animate(
            [
              { transform: "scale(0.82)", opacity: 0 },
              { transform: "scale(1)", opacity: 1 },
            ],
            { duration: 320, delay: 40, easing: "cubic-bezier(0.22, 1, 0.36, 1)", fill: "backwards" },
          );
        }
      }
    }

    const next = new Map<string, DOMRect>();
    for (const child of Array.from(el.children) as HTMLElement[]) {
      const id = child.dataset.videoId;
      if (id) next.set(id, child.getBoundingClientRect());
    }
    prevRects.current = next;
    prevIds.current = ids;
  }, [videos]);

  const onCtx = useCallback(
    (video: Video, e: React.MouseEvent) => {
      e.preventDefault();
      onContextMenu(video, e.clientX, e.clientY);
    },
    [onContextMenu],
  );

  if (!loading && videos.length === 0) {
    return <div className="grid-empty">{empty ?? <p>Nothing here yet.</p>}</div>;
  }

  return (
    <>
      <div
        ref={gridRef}
        className="video-grid"
        style={{ ["--card-min" as string]: `${cardSize}px` }}
      >
        {videos.map((v) => (
          <VideoCard
            key={v.id}
            video={v}
            progress={progress[v.id]}
            onAction={onAction}
            onToggleWatched={onToggleWatched}
            onContextMenu={onCtx}
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
