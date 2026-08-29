import { useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { ImportProgress, TakeoutRow } from "../types";

interface Props {
  rows: TakeoutRow[];
  importing: boolean;
  onImport: (channelIds: string[]) => void;
  onCancel: () => void;
}

/**
 * Pre-import checklist. Everything not already subscribed starts ticked, so the
 * common case is still one click — but a 300-channel CSV no longer commits you
 * to all 300 sight unseen.
 */
export default function TakeoutDialog({ rows, importing, onImport, onCancel }: Props) {
  const importable = useMemo(() => rows.filter((r) => !r.alreadySubscribed), [rows]);
  const [picked, setPicked] = useState<Set<string>>(
    () => new Set(importable.map((r) => r.channelId)),
  );
  const [query, setQuery] = useState("");
  const [progress, setProgress] = useState<ImportProgress | null>(null);

  useEffect(() => {
    const un = listen<ImportProgress>("import://progress", (e) => setProgress(e.payload));
    return () => {
      un.then((f) => f());
    };
  }, []);

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return rows;
    return rows.filter(
      (r) => r.title.toLowerCase().includes(q) || r.channelId.toLowerCase().includes(q),
    );
  }, [rows, query]);

  const toggle = (id: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  const setAllVisible = (on: boolean) =>
    setPicked((prev) => {
      const next = new Set(prev);
      for (const r of visible) {
        if (r.alreadySubscribed) continue;
        on ? next.add(r.channelId) : next.delete(r.channelId);
      }
      return next;
    });

  const alreadyCount = rows.length - importable.length;
  const pct = progress && progress.total > 0 ? (progress.done / progress.total) * 100 : 0;

  return (
    <div className="modal-backdrop" onMouseDown={importing ? undefined : onCancel}>
      <div
        className="modal takeout"
        role="dialog"
        aria-modal="true"
        aria-label="Choose subscriptions to import"
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2 className="modal-title">Import subscriptions</h2>
          <span className="takeout-count">
            {picked.size} of {importable.length} selected
            {alreadyCount > 0 && ` · ${alreadyCount} already subscribed`}
          </span>
        </div>

        {importing ? (
          <div className="takeout-progress">
            <div className="progress-track">
              <div className="progress-fill" style={{ width: `${pct}%` }} />
            </div>
            <p className="progress-label">
              {progress
                ? `${progress.done} of ${progress.total} · ${progress.current}`
                : "Starting…"}
            </p>
            <p className="field-hint">Fetching each channel's recent videos. You can leave this open.</p>
          </div>
        ) : (
          <>
            <div className="takeout-tools">
              <input
                className="text-input"
                placeholder="Filter channels"
                value={query}
                onChange={(e) => setQuery(e.currentTarget.value)}
              />
              <button type="button" className="btn btn-quiet" onClick={() => setAllVisible(true)}>
                Select all
              </button>
              <button type="button" className="btn btn-quiet" onClick={() => setAllVisible(false)}>
                None
              </button>
            </div>

            <ul className="takeout-list">
              {visible.map((r) => (
                <li key={r.channelId} className={r.alreadySubscribed ? "is-existing" : ""}>
                  <label>
                    <input
                      type="checkbox"
                      checked={r.alreadySubscribed || picked.has(r.channelId)}
                      disabled={r.alreadySubscribed}
                      onChange={() => toggle(r.channelId)}
                    />
                    <span className="takeout-title">{r.title}</span>
                    {r.alreadySubscribed && <span className="takeout-tag">subscribed</span>}
                  </label>
                </li>
              ))}
              {visible.length === 0 && <li className="takeout-none">No channels match.</li>}
            </ul>
          </>
        )}

        <div className="confirm-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={importing}>
            Cancel
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={importing || picked.size === 0}
            onClick={() => onImport([...picked])}
          >
            {importing ? "Importing…" : `Import ${picked.size}`}
          </button>
        </div>
      </div>
    </div>
  );
}
