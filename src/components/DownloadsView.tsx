import {
  useCallback, useEffect, useMemo, useRef, useState, type ReactNode, type RefObject,
} from "react";
import ConfirmDialog from "./ConfirmDialog";
import { IconCancel, IconDelete, IconPlay, IconRetry, IconWatched } from "./Icons";
import { useToast } from "./Toast";
import { api, errText, thumbSrc } from "../api";
import { useDownloadEvents } from "../events";
import { formatDuration, formatRelative, hasDownloadedFile, popoutPlacement } from "../format";
import type { DownloadProgress, DownloadState, Video } from "../types";

/** Downloads are rare relative to the library, so one wide sweep is enough. */
const SWEEP = 500;

/**
 * Newest download first. `downloaded_at` is queue time until an attempt ends
 * and finish time afterwards, so this orders by when you fetched a video, not
 * by when it aired. A null -- a file that predates the column and could not be
 * dated from its mtime -- falls back to release order and sorts last, matching
 * the `downloaded` ORDER BY the sweeps below ask for.
 */
function byDownloadRecency(a: Video, b: Video): number {
  if (a.downloaded_at !== b.downloaded_at) {
    if (a.downloaded_at === null) return 1;
    if (b.downloaded_at === null) return -1;
    return b.downloaded_at - a.downloaded_at;
  }
  return (b.sort_at ?? 0) - (a.sort_at ?? 0);
}

/**
 * Mirrors `Db::set_download_state`'s stamping rule so a row lands in the right
 * place the moment its event arrives, rather than only after the next reload.
 */
function stampFor(state: DownloadState, current: number | null): number | null {
  if (state === "none") return null;
  if (state === "downloading") return current;
  return Math.floor(Date.now() / 1000);
}

interface Props {
  reloadToken: number;
  /** The Subscriptions zoom, which is the size a thumbnail pops out to. */
  cardSize: number;
  /** The shell's scroll container, which a popped-out thumbnail must stay inside. */
  scrollRef: RefObject<HTMLElement | null>;
}

export default function DownloadsView({ reloadToken, cardSize, scrollRef }: Props) {
  const toast = useToast();
  const [items, setItems] = useState<Video[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [loading, setLoading] = useState(true);
  /** A video just marked watched, whose file may now be done with. */
  const [confirmFile, setConfirmFile] = useState<Video | null>(null);
  const reloadTimer = useRef<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      // Both sweeps sort by download recency, so the window they cover is the
      // most recent downloads rather than the newest uploads -- a library with
      // more than SWEEP downloads would otherwise miss the ones this tab is
      // most likely to be opened for. The first catches queued/downloading/
      // failed rows; the second guarantees completed downloads show even when
      // a burst of queued rows fills the first.
      const [recent, done] = await Promise.all([
        api.listVideos({
          channelId: null, hideWatched: false, downloadedOnly: false, showHidden: true,
          search: null, siblingOf: null, sort: "downloaded", limit: SWEEP, offset: 0,
        }),
        api.listVideos({
          channelId: null, hideWatched: false, downloadedOnly: true, showHidden: true,
          search: null, siblingOf: null, sort: "downloaded", limit: SWEEP, offset: 0,
        }),
      ]);
      const byId = new Map<string, Video>();
      for (const v of [...recent, ...done]) byId.set(v.id, v);
      setItems([...byId.values()].filter((v) => v.download_state !== "none"));
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setLoading(false);
    }
  }, [toast]);

  useEffect(() => { void load(); }, [load, reloadToken]);

  useEffect(() => () => {
    if (reloadTimer.current !== null) window.clearTimeout(reloadTimer.current);
  }, []);

  const scheduleReload = useCallback(() => {
    if (reloadTimer.current !== null) window.clearTimeout(reloadTimer.current);
    reloadTimer.current = window.setTimeout(() => {
      reloadTimer.current = null;
      void load();
    }, 400);
  }, [load]);

  // A cheap membership check that does not depend on a state updater running
  // synchronously, which React does not guarantee.
  const knownIds = useRef<Set<string>>(new Set());
  useEffect(() => { knownIds.current = new Set(items.map((v) => v.id)); }, [items]);

  useDownloadEvents({
    onProgress: (p) => setProgress((prev) => ({ ...prev, [p.videoId]: p })),
    onState: (s) => {
      const known = knownIds.current.has(s.videoId);
      if (known) {
        setItems((prev) => prev
          .map((v) => (v.id === s.videoId
            ? {
                ...v,
                download_state: s.state,
                file_path: s.filePath ?? (s.state === "done" ? v.file_path : null),
                download_error: s.error,
                downloaded_at: stampFor(s.state, v.downloaded_at),
              }
            : v))
          .filter((v) => v.download_state !== "none"));
      } else {
        // A video queued from the Subscriptions tab is not in this list yet.
        scheduleReload();
      }
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

  // Completed and Failed are histories, so they read newest first. Active is a
  // queue, which runs oldest first -- reversing it there makes the list read
  // top-to-bottom in the order the downloads will actually happen.
  const { active, completed, failed } = useMemo(() => ({
    active: items
      .filter((v) => v.download_state === "downloading" || v.download_state === "queued")
      .sort((a, b) => byDownloadRecency(b, a)),
    completed: items.filter((v) => v.download_state === "done").sort(byDownloadRecency),
    failed: items.filter((v) => v.download_state === "failed").sort(byDownloadRecency),
  }), [items]);

  /**
   * Settles where a thumbnail will land before `:hover` starts growing it. The
   * result goes straight onto the slot as custom properties rather than into
   * state, so pointing at a thumbnail costs no render. The client box, not the
   * bounding one, is the edge that matters: the scrollbar would clip it too.
   */
  const placePopout = useCallback((slot: HTMLElement) => {
    const area = scrollRef.current;
    if (!area) return;
    const box = area.getBoundingClientRect();
    const left = box.left + area.clientLeft;
    const top = box.top + area.clientTop;
    const { width, offsetY } = popoutPlacement(
      slot.getBoundingClientRect(),
      { left, top, right: left + area.clientWidth, bottom: top + area.clientHeight },
      cardSize,
    );
    slot.style.setProperty("--pop-w", `${width}px`);
    slot.style.setProperty("--pop-y", `${offsetY}px`);
  }, [scrollRef, cardSize]);

  const guard = useCallback(async (fn: () => Promise<void>) => {
    try { await fn(); } catch (err) { toast.error(errText(err)); }
  }, [toast]);

  const cancel = (v: Video) => guard(async () => { await api.cancelDownload(v.id); });
  const play = (v: Video) => guard(async () => { await api.openInPlayer(v.id); });
  const retry = (v: Video) => guard(async () => {
    setItems((prev) => prev.map((x) => (
      x.id === v.id ? { ...x, download_state: "queued", download_error: null } : x
    )));
    await api.enqueueDownload(v.id);
  });
  const remove = (v: Video) => guard(async () => {
    await api.deleteDownload(v.id);
    setItems((prev) => prev.filter((x) => x.id !== v.id));
    toast.success(`Deleted the file for “${v.title}”.`);
  });
  /** The Subscriptions menu's rule: watching a video asks about its file, and
   *  un-watching one asks nothing. */
  const toggleWatched = async (v: Video) => {
    const next = !v.watched;
    const watched = { watched: next, watched_at: next ? Math.floor(Date.now() / 1000) : null };
    const put = (fields: Pick<Video, "watched" | "watched_at">) =>
      setItems((prev) => prev.map((x) => (x.id === v.id ? { ...x, ...fields } : x)));
    put(watched);
    try {
      await api.setWatched(v.id, next);
      if (next && hasDownloadedFile(v)) setConfirmFile({ ...v, ...watched });
    } catch (err) {
      put({ watched: v.watched, watched_at: v.watched_at });
      toast.error(errText(err));
    }
  };

  if (loading && items.length === 0) {
    return (
      <div className="page">
        <div className="grid-loading"><span className="spinner" aria-hidden="true" />Loading…</div>
      </div>
    );
  }

  if (items.length === 0) {
    return (
      <div className="page">
        <div className="empty">
          <h2>No downloads</h2>
          <p>Click a video card in Subscriptions to download it. Progress shows up here.</p>
        </div>
      </div>
    );
  }

  return (
    <div className="page" style={{ ["--card-w" as string]: `${cardSize}px` }}>
      <Section title="Active" count={active.length}>
        {active.map((v) => (
          <Row key={v.id} video={v} onPopout={placePopout}>
            <div className="row-progress">
              <div className="bar">
                <div
                  className={`bar-fill${v.download_state === "queued" ? " is-idle" : ""}`}
                  style={{ width: `${v.download_state === "queued" ? 100 : progress[v.id]?.percent ?? 0}%` }}
                />
              </div>
              <span className="row-progress-text">
                {v.download_state === "queued"
                  ? "Queued"
                  : `${(progress[v.id]?.percent ?? 0).toFixed(1)}%${
                      progress[v.id]?.speed ? ` · ${progress[v.id]!.speed}` : ""
                    }${progress[v.id]?.eta ? ` · ETA ${progress[v.id]!.eta}` : ""}`}
              </span>
            </div>
            <RowAction label="Cancel download" onClick={() => cancel(v)}><IconCancel /></RowAction>
          </Row>
        ))}
      </Section>

      <Section title="Completed" count={completed.length}>
        {completed.map((v) => (
          <Row key={v.id} video={v} onPopout={placePopout}>
            <RowAction label="Play" tone="is-primary" onClick={() => play(v)}><IconPlay /></RowAction>
            <RowAction
              label={v.watched ? "Mark as unwatched" : "Mark as watched"}
              tone={v.watched ? "is-on" : undefined}
              onClick={() => void toggleWatched(v)}
            >
              <IconWatched done={v.watched} />
            </RowAction>
            <RowAction label="Delete file" tone="is-danger" onClick={() => remove(v)}><IconDelete /></RowAction>
          </Row>
        ))}
      </Section>

      <Section title="Failed" count={failed.length}>
        {failed.map((v) => (
          <Row key={v.id} video={v} onPopout={placePopout}>
            <span className="row-error" title={v.download_error ?? ""}>
              {v.download_error ?? "Download failed"}
            </span>
            <RowAction label="Retry download" onClick={() => retry(v)}><IconRetry /></RowAction>
          </Row>
        ))}
      </Section>

      {confirmFile && (
        <ConfirmDialog
          title="Delete the downloaded file?"
          body={`“${confirmFile.title}” is marked as watched. Its file is still on disk.`}
          choices={[{ label: "Delete file", value: "delete", danger: true }]}
          onCancel={() => setConfirmFile(null)}
          onChoose={() => {
            const target = confirmFile;
            setConfirmFile(null);
            void remove(target);
          }}
        />
      )}
    </div>
  );
}

/** A row's actions are glyphs, so the tooltip and the screen-reader name carry
 *  the words the buttons used to. */
function RowAction({ label, tone, onClick, children }: {
  label: string;
  tone?: "is-primary" | "is-on" | "is-danger";
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      className={`icon-btn${tone ? ` ${tone}` : ""}`}
      title={label}
      onClick={onClick}
    >
      {children}
      <span className="sr-only">{label}</span>
    </button>
  );
}

function Section({ title, count, children }: {
  title: string; count: number; children: ReactNode;
}) {
  if (count === 0) return null;
  return (
    <section className="dl-section">
      <h2 className="section-title">
        {title} <span className="section-count">{count}</span>
      </h2>
      <div className="dl-rows">{children}</div>
    </section>
  );
}

function Row({ video, onPopout, children }: {
  video: Video;
  onPopout: (slot: HTMLElement) => void;
  children: ReactNode;
}) {
  const src = thumbSrc(video);
  const duration = formatDuration(video.duration_secs);
  return (
    <div className="dl-row">
      {/* The slot holds the row's layout still while the picture inside it lifts
          out. A striped placeholder has nothing worth enlarging. */}
      <div
        className={`dl-thumb${src ? " can-pop" : ""}`}
        onMouseEnter={src ? (e) => onPopout(e.currentTarget) : undefined}
      >
        <div className="dl-pop">
          {src
            ? <img src={src} alt="" loading="lazy" decoding="async" />
            : <div className="card-img-blank" aria-hidden="true" />}
          {duration && <span className="pill pill-duration">{duration}</span>}
        </div>
      </div>
      <div className="dl-meta">
        <div className="dl-title" title={video.title}>{video.title}</div>
        <div className="dl-sub">
          {video.channel_title}
          <span className="dot" aria-hidden="true">·</span>
          {formatRelative(video.published_at)}
        </div>
      </div>
      <div className="dl-actions">{children}</div>
    </div>
  );
}
