import { useCallback, useEffect, useLayoutEffect, useRef, type ReactNode } from "react";
import VideoCard from "./VideoCard";
import SeriesCard from "./SeriesCard";
import { nextCardSize, type CardAction } from "../format";
import type { Marking } from "../marking";
import type { DownloadProgress, Video, VideoGroup } from "../types";

interface Props {
  /** One entry per card. Ungrouped views wrap each video in a group of one, so
   *  the grid has a single shape to lay out either way. */
  groups: VideoGroup[];
  progress: Record<string, DownloadProgress>;
  loading: boolean;
  hasMore: boolean;
  cardSize: number;
  onCardSize: (px: number) => void;
  onLoadMore: () => void;
  onAction: (action: CardAction, video: Video) => void;
  onContextMenu: (video: Video, x: number, y: number) => void;
  /** Opens a series, anchored on its leader and named by its shared stem. */
  onOpenSeries: (leader: Video, stem: string | null) => void;
  /** Selecting and dragging cards to mark them siblings by hand. Optional: a
   *  grid without it behaves exactly as it did before marking existed. */
  marking?: Marking;
  empty?: ReactNode;
}

/** Above this many new rows at once, skip the animation and just render. */
const MAX_ANIMATED = 60;
/** Only cards within this margin of the viewport are worth animating. */
const NEAR_VIEWPORT = 400;

export default function VideoGrid({
  groups, progress, loading, hasMore, cardSize, onCardSize,
  onLoadMore, onAction, onContextMenu, onOpenSeries, marking, empty,
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

  // Positions from the last render, measured relative to the grid itself rather
  // than to the viewport: the grid scrolls with its cards, so a scroll offset
  // that moved between two commits -- the user's own scrolling, or the feed
  // being put back where it was after a series or a poll -- cancels out instead
  // of reading as every card having jumped that far.
  const prevRects = useRef<Map<string, { top: number; left: number }>>(new Map());
  const prevIds = useRef<string[]>([]);

  useLayoutEffect(() => {
    const el = gridRef.current;
    if (!el) return;
    const base = el.getBoundingClientRect();

    // Keyed on the leader, which is what the card carries in `data-video-id`.
    const ids = groups.map((g) => g.videos[0].id);
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
          const dx = then.left - (now.left - base.left);
          const dy = then.top - (now.top - base.top);
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

    const next = new Map<string, { top: number; left: number }>();
    for (const child of Array.from(el.children) as HTMLElement[]) {
      const id = child.dataset.videoId;
      if (!id) continue;
      const r = child.getBoundingClientRect();
      next.set(id, { top: r.top - base.top, left: r.left - base.left });
    }
    prevRects.current = next;
    prevIds.current = ids;
  }, [groups]);

  const onCtx = useCallback(
    (video: Video, e: React.MouseEvent) => {
      e.preventDefault();
      onContextMenu(video, e.clientX, e.clientY);
    },
    [onContextMenu],
  );

  const total = groups.reduce((n, g) => n + g.videos.length, 0);

  if (!loading && groups.length === 0) {
    return <div className="grid-empty">{empty ?? <p>Nothing here yet.</p>}</div>;
  }

  return (
    <>
      <div
        ref={gridRef}
        className="video-grid"
        style={{ ["--card-w" as string]: `${cardSize}px` }}
      >
        {groups.map((g) =>
          g.videos.length > 1 ? (
            <SeriesCard
              key={g.videos[0].id}
              group={g}
              onOpen={onOpenSeries}
              onContextMenu={onCtx}
              mark={marking?.forGroup(g)}
            />
          ) : (
            <VideoCard
              key={g.videos[0].id}
              video={g.videos[0]}
              progress={progress[g.videos[0].id]}
              onAction={onAction}
              onContextMenu={onCtx}
              mark={marking?.forGroup(g)}
            />
          ),
        )}
      </div>
      <div ref={sentinel} className="grid-sentinel" aria-hidden="true" />
      {loading && (
        <div className="grid-loading">
          <span className="spinner" aria-hidden="true" />
          Loading…
        </div>
      )}
      {/* Counts videos rather than cards, so the answer to "how much is here"
          does not change when a series is collapsed behind one of them. */}
      {!loading && !hasMore && groups.length > 0 && (
        <div className="grid-end">
          {total} video{total === 1 ? "" : "s"}
        </div>
      )}
    </>
  );
}
