import { memo, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { thumbSrc } from "../api";
import {
  cardAction, cardTiles, formatDate, formatDuration, formatRelative, hasDownloadedFile,
} from "../format";
import type { Tile, TileAction } from "../format";
import {
  IconBusy, IconCancel, IconDelete, IconDownload, IconExternal, IconHidden, IconPlay, IconRetry,
  IconShow, IconWatched,
} from "./Icons";
import type { CardMark } from "../marking";
import { useProgress, type ProgressStore } from "../progress";
import type { Video } from "../types";

/**
 * Why a card is asking, in place of a dialog, whether to delete its file: the
 * trash tile or the menu's Delete downloaded file ("file"), or a downloaded
 * video just marked watched ("watched").
 */
export type CardAsk = "file" | "watched";

interface Props {
  video: Video;
  /** Where this card reads its download's progress, by id. A tick re-renders
   *  this card alone; see `progress.ts`. */
  progress?: ProgressStore;
  /** A call for this video is in flight: its buttons are disabled and a click
   *  on the card does nothing, so a second press cannot land on whatever the
   *  first is about to turn the card into. */
  pending?: boolean;
  /** The call has been out long enough to notice, so the primary tile spins. */
  slow?: boolean;
  onAction: (action: TileAction, video: Video) => void;
  onContextMenu: (video: Video, e: React.MouseEvent) => void;
  /** Selection and drag, when the view offers sibling marking. */
  mark?: CardMark;
  /** Set on the one card asking about its file; every other card gets nothing,
   *  so the memo holds for the rest of the grid. */
  asking?: CardAsk;
  /** The answer: `true` deletes the file. */
  onAnswer?: (video: Video, deleteFile: boolean) => void;
}

const RING_R = 26;
const RING_C = 2 * Math.PI * RING_R;

/** A question nobody is looking at goes as Keep: never the file. The pointer
 *  on the card holds it open for as long as it stays. */
const ASK_UNATTENDED_MS = 6000;

function tileIcon(tile: Tile, video: Video): ReactNode {
  switch (tile.action) {
    case "download": return <IconDownload />;
    case "play": return <IconPlay />;
    case "cancel": return <IconCancel />;
    case "retry": return <IconRetry />;
    case "open": return <IconExternal />;
    case "watched": return <IconWatched done={video.watched} />;
    case "delete-file":
    case "delete": return <IconDelete />;
    case "hide": return <IconHidden />;
    case "unhide": return <IconShow />;
  }
}

/**
 * Memoised, and every prop is built to hold its identity -- the view replaces
 * only the video object that changed, its handlers are stable, `mark` is cached
 * per card, progress is read from a store by id -- so a change to one card, or
 * a progress tick, renders that card and leaves the rest of the grid alone.
 * Hovering renders nothing at all: the bar is always in the DOM, and showing it
 * is CSS.
 */
export default memo(function VideoCard({
  video, progress: store, pending = false, slow = false, onAction, onContextMenu, mark, asking,
  onAnswer,
}: Props) {
  const progress = useProgress(store, video.id);
  // Prefer the locally cached thumbnail, fall back to the remote URL, then to a
  // plain placeholder — a broken image icon in a grid looks like a bug.
  const sources = useMemo(() => {
    const list: string[] = [];
    const primary = thumbSrc(video);
    if (primary) list.push(primary);
    if (video.thumb_url && video.thumb_url !== primary) list.push(video.thumb_url);
    return list;
  }, [video.thumb_path, video.thumb_url]);
  const [srcIndex, setSrcIndex] = useState(0);
  const src = sources[srcIndex];

  const action = cardAction(video);
  const tiles = cardTiles(video);
  const state = video.download_state;
  const busy = state === "downloading" || state === "queued";
  const downloaded = hasDownloadedFile(video);
  const duration = formatDuration(video.duration_secs);
  const percent = state === "downloading" ? Math.max(0, Math.min(100, progress?.percent ?? 0)) : 0;

  const card = useRef<HTMLElement | null>(null);
  const keep = useRef<HTMLButtonElement | null>(null);
  const unattended = useRef(0);
  useEffect(() => {
    if (!asking || !onAnswer) return;
    // Keep has the focus, so Enter and Escape both answer the safe way.
    keep.current?.focus({ preventScroll: true });
    // Asked from under the pointer (the trash tile), it waits for the pointer
    // to leave; asked from the menu, the pointer may never come, so a clock.
    if (card.current?.matches(":hover")) return;
    unattended.current = window.setTimeout(() => onAnswer(video, false), ASK_UNATTENDED_MS);
    return () => window.clearTimeout(unattended.current);
  }, [asking, onAnswer, video]);

  const run = () => onAction(action, video);
  /**
   * Card-level click already runs the primary action; buttons must not double
   * up. The second click of a double-click is dropped as well: the action flips
   * with the state -- Download is Cancel once the row is queued -- so a
   * double-click read as two clicks downloads and then cancels, however fast
   * the first call answers. `held`: the button waits out a call in flight.
   */
  const press = (fn: () => void, held = true) => (e: React.MouseEvent) => {
    e.stopPropagation();
    if (e.detail > 1 || (held && pending)) return;
    fn();
  };
  /** Which tiles a call in flight holds: the ones on the card's own key
   *  (`frontend.md`). Watched and YouTube have keys of their own. */
  const heldBy = (a: TileAction) => a !== "watched" && a !== "open";

  return (
    <article
      ref={card}
      className={`card video-card${video.watched ? " is-watched" : ""}${busy ? " is-busy" : ""}${downloaded ? " is-downloaded" : ""}${video.hidden ? " is-hidden-video" : ""}${mark?.selected ? " is-selected" : ""}${mark?.dropTarget ? " is-drop-target" : ""}${pending ? " is-pending" : ""}${slow ? " is-slow" : ""}${asking ? " is-asking" : ""}`}
      // A ctrl+click is a selection, not a download, so marking gets first
      // refusal on the click and the primary action only runs if it declines.
      onClick={(e) => {
        if (mark?.onClick(e)) return;
        if (e.detail > 1 || pending) return;
        run();
      }}
      onContextMenu={(e) => onContextMenu(video, e)}
      onMouseEnter={asking ? () => window.clearTimeout(unattended.current) : undefined}
      onMouseLeave={asking && onAnswer ? () => onAnswer(video, false) : undefined}
      {...mark?.drag}
      aria-busy={pending || undefined}
      data-state={state}
      data-video-id={video.id}
    >
      <div className="card-thumb">
        {src ? (
          <img
            className="card-img"
            src={src}
            alt=""
            loading="lazy"
            decoding="async"
            draggable={false}
            onError={() => setSrcIndex((i) => i + 1)}
          />
        ) : (
          <div className="card-img card-img-blank" aria-hidden="true" />
        )}

        {video.added_manually && (
          <span className="pill pill-added" title="Added manually, not from a subscription">
            Added
          </span>
        )}

        {/* Without this a hand-built group is invisible, and "Unlink from
            siblings" is a menu item nobody would think to look for. */}
        {video.sibling_group && (
          <span className="pill pill-linked" title="Marked as part of a series by hand">
            Linked
          </span>
        )}

        {duration && <span className="pill pill-duration">{duration}</span>}

        {/* Only a download in progress darkens the picture: the ring and the
            Queued badge are status, and must show without a hover. Every
            action lives in the bar below. Clicks that miss the ring fall
            through to the card, which cancels just the same. */}
        {busy && (
          <div className="card-overlay">
            {state === "downloading" ? (
              // Not a button of its own any more: the bar's Cancel is that, and
              // a click here falls through to the card, which cancels too. Its
              // hover X still says so. No spinner: a cancel takes the ring off
              // the card at once (the view marks the row `none` before the call
              // goes out).
              <div
                className="card-btn is-ring"
                role="progressbar"
                aria-label="Download progress"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={Math.round(percent)}
                title={`Cancel — ${video.title}`}
              >
                <svg className="ring" viewBox="0 0 64 64" aria-hidden="true">
                  <circle className="ring-track" cx="32" cy="32" r={RING_R} />
                  <circle
                    className="ring-value"
                    cx="32"
                    cy="32"
                    r={RING_R}
                    strokeDasharray={RING_C}
                    strokeDashoffset={RING_C * (1 - percent / 100)}
                  />
                </svg>
                <span className="ring-pct" aria-hidden="true">{Math.round(percent)}%</span>
                <span className="ring-hover" aria-hidden="true"><IconCancel /></span>
              </div>
            ) : (
              <button
                type="button"
                className="queued-badge"
                title={`Cancel — ${video.title}`}
                disabled={pending}
                aria-busy={pending || undefined}
                onClick={press(run)}
              >
                <span className="queued-dot" aria-hidden="true" />
                Queued
              </button>
            )}
          </div>
        )}

        {state === "downloading" && (
          <div className="card-progressbar">
            <div className="card-progressbar-fill" style={{ width: `${percent}%` }} />
          </div>
        )}
      </div>

      {state === "failed" && (
        <div className="card-error" title={video.download_error ?? undefined}>
          {video.download_error ?? "Download failed"}
        </div>
      )}

      <div className="card-body">
        <h3 className="card-title" title={video.title}>{video.title}</h3>
        {/* Channel and date share one line: two short strings never justified a
            row each, and the card is that much shorter for merging them. The
            channel is the only part allowed to shrink, so the date survives. */}
        <div className="card-meta">
          <span className="card-channel" title={video.channel_title}>{video.channel_title}</span>
          <span className="dot" aria-hidden="true">·</span>
          <span className="card-date" title={formatDate(video.published_at)}>
            {formatRelative(video.published_at)}
          </span>
          {/* The green card carries the state visually, which is enough on
              screen; the word stays for anyone who can't rely on the colour. */}
          {downloaded && <span className="sr-only">Downloaded</span>}
          {state === "downloading" && progress?.speed && (
            <>
              <span className="dot" aria-hidden="true">·</span>
              <span className="card-speed">
                {progress.speed}
                {progress.eta ? ` · ETA ${progress.eta}` : ""}
              </span>
            </>
          )}
        </div>

        {/* The hover bar takes the whole body, title and meta both: at the
            moment you act you need neither, and the card's height never
            changes, so the grid never shifts. Showing it, its cascade and each
            tile's word are all CSS (`.card-bar` in App.css). */}
        <div className="card-bar">
          {tiles.map((tile, i) => {
            const held = heldBy(tile.action);
            const spin = i === 0 && slow;
            return (
              <button
                key={tile.action}
                type="button"
                className={`card-tile${i === 0 && tile.action !== "cancel" ? " is-main" : ""}${tile.danger ? " is-danger" : ""}${tile.action === "watched" && video.watched ? " is-on" : ""}${spin ? " is-spinning" : ""}`}
                aria-label={tile.label}
                title={`${tile.label} — ${video.title}`}
                disabled={held && pending}
                aria-busy={(held && pending) || undefined}
                onClick={press(() => onAction(tile.action, video), held)}
              >
                <span className="card-glyph" aria-hidden="true">
                  {spin ? <IconBusy /> : tileIcon(tile, video)}
                </span>
                <span className="card-tile-word" aria-hidden="true">{tile.word}</span>
              </button>
            );
          })}
        </div>

        {/* The bar's title is out of sight, so the question names the video. */}
        {asking && onAnswer && (
          <div
            className="card-ask"
            role="group"
            aria-label="Delete the downloaded file?"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              if (e.key === "Escape") onAnswer(video, false);
            }}
          >
            {/* The question first and short, so the smallest card still shows
                it whole; the note after it may be cut, the question never. */}
            <div className="card-ask-q">
              Delete the file?
              {asking === "watched" ? (
                <span className="card-ask-note"> Marked watched.</span>
              ) : !video.watched && (
                <span className="card-ask-note"> Not watched yet.</span>
              )}
            </div>
            <div className="card-ask-title" title={video.title}>{video.title}</div>
            <div className="card-ask-row">
              <button
                ref={keep}
                type="button"
                className="card-ask-keep"
                onClick={press(() => onAnswer(video, false), false)}
              >
                Keep
              </button>
              <button
                type="button"
                className="card-ask-delete"
                disabled={pending}
                onClick={press(() => onAnswer(video, true))}
              >
                Delete file
              </button>
            </div>
          </div>
        )}
      </div>
    </article>
  );
});
