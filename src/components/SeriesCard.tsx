import { useMemo, useState } from "react";
import { thumbSrc } from "../api";
import { formatDate, formatRelative, seriesRuntime } from "../format";
import { IconSeries } from "./Icons";
import type { CardMark } from "../marking";
import type { Video, VideoGroup } from "../types";

interface Props {
  group: VideoGroup;
  /** Opens the series: the sibling view, anchored on the leader and carrying
   *  the shared stem, which is the name the breadcrumb then wears. */
  onOpen: (leader: Video, stem: string | null) => void;
  /** Right-click. The view answers with a menu holding nothing but the marking
   *  item, and with no menu at all when there is nothing to mark — a full one
   *  would act on the leader alone, which is what this card exists not to do. */
  onContextMenu: (leader: Video, e: React.MouseEvent) => void;
  /** Selection and drag, when the view offers sibling marking. */
  mark?: CardMark;
}

/**
 * A multi-part upload as one card: the leading part's thumbnail with the part
 * count stacked over it, and sheet edges peeking out behind the top.
 *
 * Deliberately no download or play button. The card is one click target that
 * opens the series, and the parts are acted on inside it — a button here would
 * silently act on one part out of seven.
 */
export default function SeriesCard({ group, onOpen, onContextMenu, mark }: Props) {
  const leader = group.videos[0];
  const parts = group.videos.length;
  const watched = group.videos.filter((v) => v.watched).length;
  const unwatched = parts - watched;
  // A series you have finished dims exactly as a finished video does. It takes
  // every part, not the leading one: the card stands for the whole series.
  const done = unwatched === 0;

  // The series' runtime, not the leading part's. A part whose duration has not
  // resolved yet contributes nothing, which would silently understate the
  // total -- so the count of those is carried out and marks the number with a
  // "+" rather than letting it read as exact.
  const { runtime, unknown } = useMemo(() => seriesRuntime(group.videos), [group.videos]);
  const runtimeHint = runtime
    ? `${runtime} across ${parts} parts` +
      (unknown > 0 ? ` · ${unknown} ${unknown === 1 ? "has" : "have"} no runtime yet` : "")
    : "";

  // Same fallback chain as VideoCard: cached thumb, remote URL, then a blank.
  const sources = useMemo(() => {
    const list: string[] = [];
    const primary = thumbSrc(leader);
    if (primary) list.push(primary);
    if (leader.thumb_url && leader.thumb_url !== primary) list.push(leader.thumb_url);
    return list;
  }, [leader.thumb_path, leader.thumb_url]);
  const [srcIndex, setSrcIndex] = useState(0);
  const src = sources[srcIndex];

  const title = group.stem ?? leader.title;
  const open = () => onOpen(leader, group.stem);

  return (
    <article
      className={`card series-card${done ? " is-watched" : ""}${mark?.selected ? " is-selected" : ""}${mark?.dropTarget ? " is-drop-target" : ""}`}
      role="button"
      tabIndex={0}
      aria-label={
        `${title} — ${parts} parts, ` +
        (runtime ? `${runtime} total, ` : "") +
        `${watched} watched, ${unwatched} unwatched`
      }
      onClick={(e) => {
        if (mark?.onClick(e)) return;
        open();
      }}
      onContextMenu={(e) => onContextMenu(leader, e)}
      onKeyDown={(e) => {
        if (e.key !== "Enter" && e.key !== " ") return;
        e.preventDefault();
        open();
      }}
      {...mark?.drag}
      data-video-id={leader.id}
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

        {/* Read out by the card's own aria-label above, so the panel itself is
            decoration as far as a screen reader is concerned. */}
        <div className="series-count" aria-hidden="true">
          <IconSeries />
          <span className="series-count-n">{parts}</span>
          <span className="series-count-label">parts</span>
          {/* White is what you have watched, blue what is left — the same two
              colours the tally under it uses, so the bar needs no legend. */}
          <span className="series-progress">
            <span
              className="series-progress-fill"
              style={{ width: `${(watched / parts) * 100}%` }}
            />
          </span>
          <span className="series-tally">
            <span className="series-tally-seen">{watched} watched</span>
            {/* Nothing left to watch is not news, so a zero drops back to the
                muted colour rather than sitting there in accent blue. */}
            <span className={`series-tally-left${unwatched > 0 ? " is-due" : ""}`}>
              {unwatched} unwatched
            </span>
            {/* A third line of the tally rather than a block of its own: the
                panel is only 42% of a card wide and is clipped by the thumb, so
                at the smallest zoom another gap is the line that overflows. */}
            {runtime && (
              <span className="series-runtime" data-testid="series-runtime" title={runtimeHint}>
                {runtime}{unknown > 0 ? "+" : ""}
              </span>
            )}
          </span>
        </div>
      </div>

      <div className="card-body">
        <h3 className="card-title" title={title}>{title}</h3>
        <div className="card-meta">
          <span className="card-channel" title={leader.channel_title}>
            {leader.channel_title}
          </span>
          <span className="dot" aria-hidden="true">·</span>
          {/* The leader's date, not the series': it is the part that put this
              card where it is in the feed. */}
          <span className="card-date" title={formatDate(leader.published_at)}>
            latest {formatRelative(leader.published_at)}
          </span>
        </div>
      </div>
    </article>
  );
}
