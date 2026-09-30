import { memo, useMemo, useState, type ReactNode } from "react";
import { thumbSrc } from "../api";
import {
  cardAction, cardActionLabel, formatDate, formatDuration, formatRelative,
  hasDownloadedFile,
} from "../format";
import type { CardAction } from "../format";
import { IconBusy, IconCancel, IconDownload, IconExternal, IconPlay, IconRetry } from "./Icons";
import type { CardMark } from "../marking";
import { useProgress, type ProgressStore } from "../progress";
import type { Video } from "../types";

interface Props {
  video: Video;
  /** Where this card reads its download's progress, by id. A tick re-renders
   *  this card alone; see `progress.ts`. */
  progress?: ProgressStore;
  /** A call for this video is in flight: its buttons are disabled and a click
   *  on the card does nothing, so a second press cannot land on whatever the
   *  first is about to turn the card into. */
  pending?: boolean;
  /** The call has been out long enough to notice, so the primary button spins. */
  slow?: boolean;
  onAction: (action: CardAction, video: Video) => void;
  onContextMenu: (video: Video, e: React.MouseEvent) => void;
  /** Selection and drag, when the view offers sibling marking. */
  mark?: CardMark;
}

const RING_R = 26;
const RING_C = 2 * Math.PI * RING_R;

const ACTION_ICON: Record<CardAction, ReactNode> = {
  download: <IconDownload />,
  play: <IconPlay />,
  cancel: <IconCancel />,
  retry: <IconRetry />,
  open: <IconExternal />,
};

/**
 * Memoised, and every prop is built to hold its identity -- the view replaces
 * only the video object that changed, its handlers are stable, `mark` is cached
 * per card, progress is read from a store by id -- so a change to one card, or
 * a progress tick, renders that card and leaves the rest of the grid alone.
 */
export default memo(function VideoCard({
  video, progress: store, pending = false, slow = false, onAction, onContextMenu, mark,
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
  const state = video.download_state;
  const busy = state === "downloading" || state === "queued";
  const downloaded = hasDownloadedFile(video);
  const duration = formatDuration(video.duration_secs);
  const percent = state === "downloading" ? Math.max(0, Math.min(100, progress?.percent ?? 0)) : 0;

  const run = () => onAction(action, video);
  /**
   * Card-level click already runs the primary action; buttons must not double
   * up. The second click of a double-click is dropped as well: the action flips
   * with the state -- Download is Cancel once the row is queued -- so a
   * double-click read as two clicks downloads and then cancels, however fast
   * the first call answers.
   */
  const press = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    if (e.detail > 1 || pending) return;
    fn();
  };
  /** The spinner once the call is slow enough to notice, the glyph otherwise.
   *  The span is what `.is-spinning` turns. */
  const glyph = (icon: ReactNode) => (
    <span className="card-glyph" aria-hidden="true">{slow ? <IconBusy /> : icon}</span>
  );

  return (
    <article
      className={`card${video.watched ? " is-watched" : ""}${busy ? " is-busy" : ""}${downloaded ? " is-downloaded" : ""}${video.hidden ? " is-hidden-video" : ""}${mark?.selected ? " is-selected" : ""}${mark?.dropTarget ? " is-drop-target" : ""}${pending ? " is-pending" : ""}`}
      // A ctrl+click is a selection, not a download, so marking gets first
      // refusal on the click and the primary action only runs if it declines.
      onClick={(e) => {
        if (mark?.onClick(e)) return;
        if (e.detail > 1 || pending) return;
        run();
      }}
      onContextMenu={(e) => onContextMenu(video, e)}
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

        {/* The scrim is a plain layer: clicks that miss a button fall through to
            the card, which runs the same primary action. A slow call keeps it
            up, so the spinner is on screen without a hover. */}
        <div className={`card-overlay${busy || slow ? " is-busy" : ""}`}>
          <div className="card-actions">
            {state === "downloading" ? (
              <button
                type="button"
                // No spinner: a cancel takes the ring off the card at once
                // (the view marks the row `none` before the call goes out).
                className="card-btn is-ring"
                title={`Cancel — ${video.title}`}
                disabled={pending}
                aria-busy={pending || undefined}
                onClick={press(run)}
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
                <span className="ring-pct">{Math.round(percent)}%</span>
                <span className="ring-hover" aria-hidden="true"><IconCancel /></span>
                <span className="sr-only">Cancel download</span>
              </button>
            ) : state === "queued" ? (
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
            ) : (
              <button
                type="button"
                className={`card-btn is-primary${slow ? " is-spinning" : ""}`}
                title={`${cardActionLabel(action)} — ${video.title}`}
                disabled={pending}
                aria-busy={pending || undefined}
                onClick={press(run)}
              >
                {glyph(ACTION_ICON[action])}
                <span className="sr-only">{cardActionLabel(action)}</span>
              </button>
            )}

            <button
              type="button"
              className="card-btn"
              title={`${cardActionLabel("open")} — ${video.title}`}
              // Not held by `pending`: leaving for YouTube changes nothing on
              // this card, and the view guards it under a key of its own.
              onClick={(e) => {
                e.stopPropagation();
                if (e.detail > 1) return;
                onAction("open", video);
              }}
            >
              <IconExternal />
              <span className="sr-only">{cardActionLabel("open")}</span>
            </button>
          </div>
        </div>

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
      </div>
    </article>
  );
});
