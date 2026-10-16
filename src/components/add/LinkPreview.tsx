import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import {
  IconAdd, IconBusy, IconCaret, IconCheck, IconCustomDownload, IconDownload, IconMember, IconSeries,
} from "../Icons";
import { fileSrc } from "../../api";
import { singleClick } from "../../pending";
import { formatDuration, formatRelative, formatTotalRuntime } from "../../format";
import {
  countTicked, partState, tallyPlaylist, type PartState,
} from "./playlist";
import type { Channel, PlaylistEntry, PlaylistPreview, VideoPreview } from "../../types";

/** Which of a preview's three actions is out, if any, and whether it is slow.
 *  One at a time: an Add and a Download of the same paste must not race. */
export type Acting = "add" | "download" | "custom" | null;
export interface PreviewBusy { acting: Acting; slow: boolean }

/**
 * A button of the preview card: a glyph and its words. `busy` is the card's
 * (any of its actions out disables all three), while only the button pressed
 * spins once the call is slow.
 */
function ActButton({ glyph, label, primary, disabled, spinning, busy, onClick }: {
  glyph: ReactNode;
  label: string;
  primary?: boolean;
  disabled?: boolean;
  spinning?: boolean;
  busy?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      className={[
        "btn btn-sm", primary && "btn-primary", spinning && "is-spinning",
      ].filter(Boolean).join(" ")}
      disabled={disabled || busy}
      aria-busy={spinning || undefined}
      onClick={singleClick(onClick)}
    >
      <span className="btn-glyph" aria-hidden="true">{spinning ? <IconBusy /> : glyph}</span>
      {label}
    </button>
  );
}

/**
 * A thumbnail: the cached copy when the library has one, the remote
 * hqdefault otherwise, and a plain grey box if neither will load -- never the
 * browser's broken-image glyph.
 */
function Thumb({ path, url, className }: { path: string | null; url: string; className?: string }) {
  const [localFailed, setLocalFailed] = useState(false);
  const [remoteFailed, setRemoteFailed] = useState(false);
  const src = path && !localFailed ? fileSrc(path) : url;
  if (!src || (src === url && remoteFailed)) return null;
  return (
    <img
      className={className}
      src={src}
      alt=""
      loading="lazy"
      draggable={false}
      onError={() => (src === url ? setRemoteFailed(true) : setLocalFailed(true))}
    />
  );
}

/* ------------------------------------------------------------------ *
 * A channel
 * ------------------------------------------------------------------ */

/**
 * A channel needs no card: there is nothing to look at before subscribing
 * that the list below would not show better afterwards. One line says what
 * will happen, or -- when it is one you follow -- says so and offers to take
 * you to it.
 */
export function ChannelAnswer({ alreadySubscribed, checking, busy, spinning, onSubscribe, onShow }: {
  /** The channel the paste names, when you already follow it; `true` when the
   *  backend says so but the list has not caught up with it yet. */
  alreadySubscribed: Channel | true | null;
  /** Still asking whether it is one you follow. */
  checking: boolean;
  busy: boolean;
  spinning: boolean;
  onSubscribe: () => void;
  onShow: () => void;
}) {
  if (alreadySubscribed) {
    return (
      <div className="add-line">
        <p className="add-line-text">
          {alreadySubscribed === true
            ? "You're already subscribed to this channel."
            : <>You're already subscribed to <b>{alreadySubscribed.title}</b>.</>}
        </p>
        {alreadySubscribed !== true && (
          <button type="button" className="btn btn-sm" onClick={onShow}>Show in list</button>
        )}
      </div>
    );
  }
  return (
    <div className="add-line">
      <p className="add-line-text">
        This looks like a channel. It will be subscribed and its recent videos backfilled.
      </p>
      <ActButton
        glyph={<IconAdd />}
        label="Subscribe"
        primary
        disabled={checking}
        busy={busy}
        spinning={spinning}
        onClick={onSubscribe}
      />
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * A video
 * ------------------------------------------------------------------ */

export function VideoCard({ video: v, playlistId, busy, onAdd, onDownload, onCustom, onWholePlaylist }: {
  video: VideoPreview;
  playlistId: string | null;
  busy: PreviewBusy;
  onAdd: () => void;
  onDownload: () => void;
  onCustom: () => void;
  onWholePlaylist: () => void;
}) {
  const onDisk = v.downloadState === "done";
  const onItsWay = v.downloadState === "queued" || v.downloadState === "downloading";
  const out = busy.acting !== null;
  // A failed download keeps Download: pressing it again is the retry.
  const downloadLabel = onDisk ? "Downloaded" : onItsWay ? "Downloading…" : "Download";
  return (
    <div className="add-pv" aria-label={`Video: ${v.title}`} role="group">
      <div className="add-pv-top">
        <span className="add-thumb">
          <Thumb path={v.thumbPath} url={v.thumbUrl} />
          {v.durationSecs ? <span className="add-thumb-dur">{formatDuration(v.durationSecs)}</span> : null}
        </span>
        <div className="add-pv-body">
          <div className="add-pv-text">
            <p className="add-pv-title" title={v.title}>{v.title}</p>
            <p className="add-pv-meta">
              <span className="add-pv-meta-text">
                {v.channelTitle}
                {v.publishedAt ? ` · uploaded ${formatRelative(v.publishedAt)}` : ""}
              </span>
              {v.membersOnly && (
                <span className="add-tag is-members">
                  <IconMember joined /> Members-only
                </span>
              )}
            </p>
          </div>
          <div className="add-pv-actions">
            <ActButton
              glyph={v.inLibrary ? <IconCheck /> : <IconAdd />}
              label={v.inLibrary ? "In your library" : "Add"}
              primary={!v.inLibrary}
              disabled={v.inLibrary}
              busy={out}
              spinning={busy.slow && busy.acting === "add"}
              onClick={onAdd}
            />
            <ActButton
              glyph={onDisk ? <IconCheck /> : <IconDownload />}
              label={downloadLabel}
              disabled={onDisk || onItsWay}
              busy={out}
              spinning={busy.slow && busy.acting === "download"}
              onClick={onDownload}
            />
            <ActButton
              glyph={<IconCustomDownload />}
              label="Download (custom)…"
              disabled={onDisk || onItsWay}
              busy={out}
              spinning={busy.slow && busy.acting === "custom"}
              onClick={onCustom}
            />
          </div>
        </div>
      </div>
      {playlistId && (
        <button type="button" className="add-pv-foot" onClick={onWholePlaylist}>
          <span className="btn-glyph" aria-hidden="true"><IconSeries /></span>
          Part of a playlist — add the whole playlist instead
        </button>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ *
 * A playlist
 * ------------------------------------------------------------------ */

/** "47h 55m", or "47h 55m+" when some part's length is not known yet. */
function runtimeLabel(secs: number, unknown: number): string {
  const r = formatTotalRuntime(secs);
  if (!r) return unknown ? "length unknown" : "";
  return unknown ? `${r}+` : r;
}

/**
 * A playlist, summed up: a stacked thumbnail, what it is and how long, and a
 * line of tags saying what is new, what you already have and what is
 * members-only. The parts themselves wait behind a disclosure, closed by
 * default -- most of the time a playlist is simply taken whole -- and opening
 * it gives a checklist whose ticks the three buttons count.
 */
export function PlaylistCard({ playlist: pl, ticked, onTicked, busy, onAdd, onDownload, onCustom }: {
  playlist: PlaylistPreview;
  ticked: ReadonlySet<string>;
  onTicked: (next: Set<string>) => void;
  busy: PreviewBusy;
  onAdd: () => void;
  onDownload: () => void;
  onCustom: () => void;
}) {
  const uid = useId();
  const [open, setOpen] = useState(false);
  // Five hundred rows of thumbnails are not built for a disclosure nobody
  // opened; once opened they stay, so closing it can animate.
  const [built, setBuilt] = useState(false);
  const tally = tallyPlaylist(pl.entries);
  const all = countTicked(pl.entries, new Set(pl.entries.map((e) => e.id)));
  const c = countTicked(pl.entries, ticked);
  const n = pl.entries.length;
  const out = busy.acting !== null;
  const read = n + pl.unavailable;
  const first = pl.entries[0];

  const addLabel = c.toAdd > 0
    ? `Add ${c.toAdd} new`
    : c.ticked > 0 ? "All in your library" : "Add";
  const downloadLabel = c.toDownload > 0
    ? `Download ${c.toDownload}`
    : c.ticked > 0 && c.downloaded === c.ticked ? "Downloaded" : "Download";

  const allBox = useRef<HTMLInputElement | null>(null);
  useEffect(() => {
    if (allBox.current) allBox.current.indeterminate = c.ticked > 0 && c.ticked < n;
  }, [c.ticked, n, built]);

  return (
    <div className="add-pv is-playlist" role="group" aria-label={`Playlist: ${pl.title}`}>
      <div className="add-pv-top">
        <span className="add-stack">
          <span className="add-stack-img">
            {first && <Thumb path={first.thumbPath} url={first.thumbUrl} />}
            <span className="add-stack-count">
              <b>{pl.total.toLocaleString("en")}</b>
              <small>video{pl.total === 1 ? "" : "s"}</small>
            </span>
          </span>
        </span>
        <div className="add-pv-body">
          <div className="add-pv-text">
            <p className="add-pv-title" title={pl.title}>{pl.title}</p>
            <p className="add-pv-meta">
              <span className="add-pv-meta-text">
                {[
                  pl.channelTitle,
                  `${pl.total.toLocaleString("en")} video${pl.total === 1 ? "" : "s"}`,
                  runtimeLabel(all.runtimeSecs, all.unknownRuntime),
                ].filter(Boolean).join(" · ")}
              </span>
            </p>
          </div>
          <div className="add-tags">
            {tally.fresh > 0 && <span className="add-tag is-new">{tally.fresh} new</span>}
            {tally.inLibrary > 0 && <span className="add-tag">{tally.inLibrary} in your library</span>}
            {tally.downloaded > 0 && (
              <span className="add-tag is-ok"><IconDownload /> {tally.downloaded} downloaded</span>
            )}
            {tally.membersOnly > 0 && (
              <span className="add-tag is-members"><IconMember joined /> {tally.membersOnly} members-only</span>
            )}
            {pl.unavailable > 0 && (
              <span className="add-tag is-faint" title="Private or deleted videos in the playlist, left out">
                {pl.unavailable} unavailable
              </span>
            )}
            {pl.truncated && (
              <span className="add-tag is-faint">
                Showing the first {read.toLocaleString("en")} of {pl.total.toLocaleString("en")}
              </span>
            )}
          </div>
          <div className="add-pv-actions">
            <ActButton
              glyph={c.toAdd === 0 && c.ticked > 0 ? <IconCheck /> : <IconAdd />}
              label={addLabel}
              primary={c.toAdd > 0}
              disabled={c.toAdd === 0}
              busy={out}
              spinning={busy.slow && busy.acting === "add"}
              onClick={onAdd}
            />
            <ActButton
              glyph={<IconDownload />}
              label={downloadLabel}
              disabled={c.toDownload === 0}
              busy={out}
              spinning={busy.slow && busy.acting === "download"}
              onClick={onDownload}
            />
            <ActButton
              glyph={<IconCustomDownload />}
              label="Download (custom)…"
              disabled={c.toDownload === 0}
              busy={out}
              spinning={busy.slow && busy.acting === "custom"}
              onClick={onCustom}
            />
          </div>
        </div>
      </div>

      {n > 0 && (
        <button
          type="button"
          className={`add-pv-foot add-disclose${open ? " is-open" : ""}`}
          aria-expanded={open}
          aria-controls={`${uid}-parts`}
          onClick={() => { setBuilt(true); setOpen(!open); }}
        >
          <span className="btn-glyph add-disclose-caret" aria-hidden="true"><IconCaret /></span>
          {open ? "Hide" : "Show"} the {n.toLocaleString("en")} part{n === 1 ? "" : "s"}
          {c.ticked < n && <span className="add-disclose-note"> · {c.ticked} ticked</span>}
        </button>
      )}

      <div className={`add-parts${open ? " is-open" : ""}`} id={`${uid}-parts`} inert={!open}>
        <div className="add-parts-clip">
          {built && (
            <>
              <div className="add-parts-head">
                <label className="add-parts-all">
                  <input
                    ref={allBox}
                    type="checkbox"
                    checked={c.ticked === n}
                    onChange={() => onTicked(c.ticked === n ? new Set() : new Set(pl.entries.map((e) => e.id)))}
                  />
                  All
                </label>
                <span className="add-parts-sum">
                  {c.ticked} of {n}
                  {c.ticked > 0 && runtimeLabel(c.runtimeSecs, c.unknownRuntime)
                    ? ` · ${runtimeLabel(c.runtimeSecs, c.unknownRuntime)}`
                    : ""}
                </span>
              </div>
              <ul className="add-parts-list">
                {pl.entries.map((e) => (
                  <PartRow
                    key={e.id}
                    entry={e}
                    on={ticked.has(e.id)}
                    onToggle={() => {
                      const next = new Set(ticked);
                      if (next.has(e.id)) next.delete(e.id);
                      else next.add(e.id);
                      onTicked(next);
                    }}
                  />
                ))}
              </ul>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

const PART_TAG: Record<PartState, string> = {
  downloaded: "is-ok",
  downloading: "is-ok",
  "in library": "",
  members: "is-members",
  new: "is-new",
};

function PartRow({ entry: e, on, onToggle }: {
  entry: PlaylistEntry;
  on: boolean;
  onToggle: () => void;
}) {
  const state = partState(e);
  return (
    <li>
      <label className={`add-part${on ? "" : " is-off"}`}>
        <input type="checkbox" checked={on} onChange={onToggle} />
        <span className="add-part-thumb"><Thumb path={e.thumbPath} url={e.thumbUrl} /></span>
        <span className="add-part-title" title={e.title}>{e.title}</span>
        <span className={`add-tag ${PART_TAG[state]}`}>
          {state === "members" && <IconMember joined />}
          {state}
        </span>
        <span className="add-part-dur">{formatDuration(e.durationSecs)}</span>
      </label>
    </li>
  );
}

/* ------------------------------------------------------------------ *
 * Loading
 * ------------------------------------------------------------------ */

/**
 * The card a video or playlist is about to become, drawn from the same parts
 * -- the same title block, the same three buttons with the same words -- so
 * the real card lands in exactly the space it held and nothing below moves.
 */
export function PreviewSkeleton({ kind }: { kind: "video" | "playlist" }) {
  const bone = (label: string, glyph: ReactNode) => (
    <span className="btn btn-sm is-bone">
      <span className="btn-glyph">{glyph}</span>
      {label}
    </span>
  );
  return (
    <div className={`add-pv is-skeleton${kind === "playlist" ? " is-playlist" : ""}`} aria-hidden="true">
      <div className="add-pv-top">
        {kind === "playlist"
          ? <span className="add-stack"><span className="add-stack-img" /></span>
          : <span className="add-thumb" />}
        <div className="add-pv-body">
          <div className="add-pv-text">
            <p className="add-pv-title"><span className="add-bone">Reading this video's title from YouTube, which can take a moment or two</span></p>
            <p className="add-pv-meta"><span className="add-bone">The channel · uploaded</span></p>
          </div>
          {kind === "playlist" && (
            <div className="add-tags">
              <span className="add-tag add-bone">12 new</span>
              <span className="add-tag add-bone">3 in your library</span>
            </div>
          )}
          <div className="add-pv-actions">
            {bone(kind === "playlist" ? "Add 12 new" : "Add", <IconAdd />)}
            {bone(kind === "playlist" ? "Download 12" : "Download", <IconDownload />)}
            {bone("Download (custom)…", <IconCustomDownload />)}
          </div>
        </div>
      </div>
      {kind === "playlist" && (
        <span className="add-pv-foot add-disclose"><span className="add-bone">Show the 12 parts</span></span>
      )}
    </div>
  );
}
