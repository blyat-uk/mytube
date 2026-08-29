import { useMemo, useState } from "react";
import { thumbSrc } from "../api";
import { cardAction, cardActionLabel, formatDate, formatDuration, formatRelative, formatViews } from "../format";
import type { CardAction } from "../format";
import type { DownloadProgress, Video } from "../types";

interface Props {
  video: Video;
  progress?: DownloadProgress;
  onAction: (action: CardAction, video: Video) => void;
  onToggleWatched: (video: Video) => void;
  onContextMenu: (video: Video, e: React.MouseEvent) => void;
}

const RING_R = 26;
const RING_C = 2 * Math.PI * RING_R;

const ACTION_GLYPH: Record<CardAction, string> = {
  download: "↓",
  play: "▶",
  cancel: "✕",
  retry: "↻",
};

export default function VideoCard({ video, progress, onAction, onToggleWatched, onContextMenu }: Props) {
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
  const duration = formatDuration(video.duration_secs);
  const views = formatViews(video.view_count);
  const percent = state === "downloading" ? Math.max(0, Math.min(100, progress?.percent ?? 0)) : 0;

  const run = () => onAction(action, video);

  return (
    <article
      className={`card${video.watched ? " is-watched" : ""}${busy ? " is-busy" : ""}${video.hidden ? " is-hidden-video" : ""}`}
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

        {state === "done" && video.file_path && (
          <span className="pill pill-downloaded" title="Downloaded">
            ↓
          </span>
        )}

        <button
          type="button"
          className={`watch-toggle${video.watched ? " is-on" : ""}`}
          aria-pressed={video.watched}
          title={video.watched ? "Mark as unwatched" : "Mark as watched"}
          onClick={(e) => {
            // Never let the watched toggle trigger the card's own action.
            e.stopPropagation();
            onToggleWatched(video);
          }}
        >
          <span aria-hidden="true">✓</span>
          <span className="sr-only">
            {video.watched ? "Mark as unwatched" : "Mark as watched"}
          </span>
        </button>

        <button
          type="button"
          className={`card-action${busy ? " is-busy" : ""}`}
          title={`${cardActionLabel(action)} — ${video.title}`}
          onClick={(e) => {
            e.stopPropagation();
            run();
          }}
        >
          {state === "downloading" ? (
            <span className="ring-wrap">
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
              <span className="ring-hover" aria-hidden="true">✕</span>
            </span>
          ) : state === "queued" ? (
            <span className="queued-badge">
              <span className="queued-dot" aria-hidden="true" />
              Queued
            </span>
          ) : (
            <span className="action-bubble">
              <span className="action-glyph" aria-hidden="true">{ACTION_GLYPH[action]}</span>
              <span className="action-label">{cardActionLabel(action)}</span>
            </span>
          )}
        </button>

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
        <div className="card-channel" title={video.channel_title}>{video.channel_title}</div>
        <div className="card-meta">
          <span title={formatDate(video.published_at)}>{formatRelative(video.published_at)}</span>
          {views && (
            <>
              <span className="dot" aria-hidden="true">·</span>
              <span>{views} views</span>
            </>
          )}
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
