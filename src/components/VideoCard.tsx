import { useMemo, useState, type ReactNode } from "react";
import { thumbSrc } from "../api";
import {
  cardAction, cardActionLabel, formatDate, formatDuration, formatRelative,
  hasDownloadedFile,
} from "../format";
import type { CardAction } from "../format";
import { IconCancel, IconDownload, IconExternal, IconPlay, IconRetry } from "./Icons";
import type { DownloadProgress, Video } from "../types";

interface Props {
  video: Video;
  progress?: DownloadProgress;
  onAction: (action: CardAction, video: Video) => void;
  onContextMenu: (video: Video, e: React.MouseEvent) => void;
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

export default function VideoCard({ video, progress, onAction, onContextMenu }: Props) {
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
  /** Card-level click already runs the primary action; buttons must not double up. */
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };

  return (
    <article
      className={`card${video.watched ? " is-watched" : ""}${busy ? " is-busy" : ""}${downloaded ? " is-downloaded" : ""}${video.hidden ? " is-hidden-video" : ""}`}
      onClick={run}
      onContextMenu={(e) => onContextMenu(video, e)}
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

        {duration && <span className="pill pill-duration">{duration}</span>}

        {/* The scrim is a plain layer: clicks that miss a button fall through to
            the card, which runs the same primary action. */}
        <div className={`card-overlay${busy ? " is-busy" : ""}`}>
          <div className="card-actions">
            {state === "downloading" ? (
              <button
                type="button"
                className="card-btn is-ring"
                title={`Cancel — ${video.title}`}
                onClick={stop(run)}
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
                onClick={stop(run)}
              >
                <span className="queued-dot" aria-hidden="true" />
                Queued
              </button>
            ) : (
              <button
                type="button"
                className="card-btn is-primary"
                title={`${cardActionLabel(action)} — ${video.title}`}
                onClick={stop(run)}
              >
                {ACTION_ICON[action]}
                <span className="sr-only">{cardActionLabel(action)}</span>
              </button>
            )}

            <button
              type="button"
              className="card-btn"
              title={`${cardActionLabel("open")} — ${video.title}`}
              onClick={stop(() => onAction("open", video))}
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
}
