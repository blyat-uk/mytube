import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useToast } from "./Toast";
import { api, errText, thumbSrc } from "../api";
import { useDownloadEvents } from "../events";
import { formatDuration, formatRelative } from "../format";
import type { DownloadProgress, Video } from "../types";

/** Downloads are rare relative to the library, so one wide sweep is enough. */
const SWEEP = 500;

interface Props {
  reloadToken: number;
}

export default function DownloadsView({ reloadToken }: Props) {
  const toast = useToast();
  const [items, setItems] = useState<Video[]>([]);
  const [progress, setProgress] = useState<Record<string, DownloadProgress>>({});
  const [loading, setLoading] = useState(true);
  const reloadTimer = useRef<number | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      // The recent sweep catches queued/downloading/failed rows; the second pass
      // guarantees completed downloads show even when they are old.
      const [recent, done] = await Promise.all([
        api.listVideos({
          channelId: null, hideWatched: false, downloadedOnly: false, showHidden: true,
          search: null, sort: "newest", limit: SWEEP, offset: 0,
        }),
        api.listVideos({
          channelId: null, hideWatched: false, downloadedOnly: true, showHidden: true,
          search: null, sort: "newest", limit: SWEEP, offset: 0,
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

  const { active, completed, failed } = useMemo(() => ({
    active: items.filter((v) => v.download_state === "downloading" || v.download_state === "queued"),
    completed: items.filter((v) => v.download_state === "done"),
    failed: items.filter((v) => v.download_state === "failed"),
  }), [items]);

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
    <div className="page">
      <Section title="Active" count={active.length}>
        {active.map((v) => (
          <Row key={v.id} video={v}>
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
            <button type="button" className="btn" onClick={() => cancel(v)}>Cancel</button>
          </Row>
        ))}
      </Section>

      <Section title="Completed" count={completed.length}>
        {completed.map((v) => (
          <Row key={v.id} video={v}>
            <span className="row-path" title={v.file_path ?? ""}>{v.file_path}</span>
            <button type="button" className="btn btn-primary" onClick={() => play(v)}>Play</button>
            <button type="button" className="btn btn-danger" onClick={() => remove(v)}>Delete file</button>
          </Row>
        ))}
      </Section>

      <Section title="Failed" count={failed.length}>
        {failed.map((v) => (
          <Row key={v.id} video={v}>
            <span className="row-error" title={v.download_error ?? ""}>
              {v.download_error ?? "Download failed"}
            </span>
            <button type="button" className="btn" onClick={() => retry(v)}>Retry</button>
          </Row>
        ))}
      </Section>
    </div>
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

function Row({ video, children }: { video: Video; children: ReactNode }) {
  const src = thumbSrc(video);
  const duration = formatDuration(video.duration_secs);
  return (
    <div className="dl-row">
      <div className="dl-thumb">
        {src
          ? <img src={src} alt="" loading="lazy" decoding="async" />
          : <div className="card-img-blank" aria-hidden="true" />}
        {duration && <span className="pill pill-duration">{duration}</span>}
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
