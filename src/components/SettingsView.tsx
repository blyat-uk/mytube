import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import type { Settings } from "../types";

type NumField = "max_concurrent_downloads" | "poll_interval_minutes" | "backfill_count";

const LIMITS: Record<NumField, { min: number; max: number }> = {
  max_concurrent_downloads: { min: 1, max: 16 },
  poll_interval_minutes: { min: 1, max: 1440 },
  backfill_count: { min: 1, max: 500 },
};

export default function SettingsView() {
  const toast = useToast();
  const [settings, setSettings] = useState<Settings | null>(null);
  const [drafts, setDrafts] = useState<Partial<Record<NumField, string>>>({});
  const [savedAt, setSavedAt] = useState(0);
  const [failed, setFailed] = useState(false);

  // Mirrors of the state, kept in sync synchronously so blur handlers never
  // read a stale value and so no side effect runs inside a state updater.
  const current = useRef<Settings | null>(null);
  const draftRef = useRef<Partial<Record<NumField, string>>>({});
  // The last value known to be on disk, so an unchanged blur is not a write.
  const persisted = useRef<string>("");

  useEffect(() => {
    let alive = true;
    api.getSettings()
      .then((s) => {
        if (!alive) return;
        current.current = s;
        persisted.current = JSON.stringify(s);
        setSettings(s);
      })
      .catch((err) => {
        if (!alive) return;
        setFailed(true);
        toast.error(errText(err));
      });
    return () => { alive = false; };
  }, [toast]);

  const commit = useCallback(async (next: Settings) => {
    const serialised = JSON.stringify(next);
    if (serialised === persisted.current) return;
    try {
      // `next` is a spread of what the backend handed us, so any unknown keys
      // that Rust flattened in survive the round trip.
      await api.saveSettings(next);
      persisted.current = serialised;
      setSavedAt(Date.now());
    } catch (err) {
      toast.error(errText(err));
    }
  }, [toast]);

  const set = useCallback(<K extends keyof Settings>(key: K, value: Settings[K]) => {
    const base = current.current;
    if (!base) return;
    const next = { ...base, [key]: value };
    current.current = next;
    setSettings(next);
  }, []);

  /** Controls with no "done editing" moment save the instant they change. */
  const setAndSave = useCallback(<K extends keyof Settings>(key: K, value: Settings[K]) => {
    const base = current.current;
    if (!base) return;
    const next = { ...base, [key]: value };
    current.current = next;
    setSettings(next);
    void commit(next);
  }, [commit]);

  const blurCommit = useCallback(() => {
    if (current.current) void commit(current.current);
  }, [commit]);

  const setDraft = useCallback((field: NumField, raw: string) => {
    const next = { ...draftRef.current, [field]: raw };
    draftRef.current = next;
    setDrafts(next);
  }, []);

  const commitNumber = useCallback((field: NumField) => {
    const base = current.current;
    if (!base) return;
    const raw = draftRef.current[field];
    if (raw === undefined) {
      void commit(base);
      return;
    }
    const { min, max } = LIMITS[field];
    const parsed = Number.parseInt(raw, 10);
    const value = Number.isFinite(parsed) ? Math.min(max, Math.max(min, parsed)) : min;

    const rest = { ...draftRef.current };
    delete rest[field];
    draftRef.current = rest;
    setDrafts(rest);

    const next = { ...base, [field]: value };
    current.current = next;
    setSettings(next);
    void commit(next);
  }, [commit]);

  async function pickDirectory() {
    try {
      const path = await open({
        multiple: false,
        directory: true,
        title: "Choose a download folder",
      });
      if (!path) return;
      setAndSave("download_dir", path);
    } catch (err) {
      toast.error(errText(err));
    }
  }

  if (failed) {
    return (
      <div className="page">
        <div className="empty">
          <h2>Settings unavailable</h2>
          <p>Could not read <code>~/.config/mytube/settings.json</code>.</p>
        </div>
      </div>
    );
  }

  if (!settings) {
    return (
      <div className="page">
        <div className="grid-loading"><span className="spinner" aria-hidden="true" />Loading…</div>
      </div>
    );
  }

  const s = settings;
  const numValue = (f: NumField) => drafts[f] ?? String(s[f]);

  return (
    <div className="page page-narrow">
      <div className="settings-head">
        <h2 className="section-title">Settings</h2>
        {savedAt > 0 && <span key={savedAt} className="saved-flash">Saved</span>}
      </div>

      <Field label="Download folder" hint="Where yt-dlp writes finished files. Created on demand.">
        <div className="row-inline">
          <input
            className="text-input"
            value={s.download_dir}
            onChange={(e) => set("download_dir", e.currentTarget.value)}
            onBlur={blurCommit}
            spellCheck={false}
          />
          <button type="button" className="btn" onClick={() => void pickDirectory()}>
            Browse…
          </button>
        </div>
      </Field>

      <Field label="Filename template" hint="yt-dlp output template, relative to the download folder.">
        <input
          className="text-input mono"
          value={s.filename_template}
          onChange={(e) => set("filename_template", e.currentTarget.value)}
          onBlur={blurCommit}
          spellCheck={false}
        />
      </Field>

      <Field
        label="Player command"
        hint="Runs to open a downloaded file. Arguments are allowed, e.g. mpv --fullscreen."
      >
        <input
          className="text-input mono"
          value={s.player_command}
          onChange={(e) => set("player_command", e.currentTarget.value)}
          onBlur={blurCommit}
          spellCheck={false}
        />
      </Field>

      <div className="settings-grid">
        <Field label="Concurrent downloads" hint="1 – 16">
          <input
            className="text-input"
            type="number"
            min={LIMITS.max_concurrent_downloads.min}
            max={LIMITS.max_concurrent_downloads.max}
            value={numValue("max_concurrent_downloads")}
            onChange={(e) => setDraft("max_concurrent_downloads", e.currentTarget.value)}
            onBlur={() => commitNumber("max_concurrent_downloads")}
          />
        </Field>

        <Field label="Poll interval (minutes)" hint="1 – 1440">
          <input
            className="text-input"
            type="number"
            min={LIMITS.poll_interval_minutes.min}
            max={LIMITS.poll_interval_minutes.max}
            value={numValue("poll_interval_minutes")}
            onChange={(e) => setDraft("poll_interval_minutes", e.currentTarget.value)}
            onBlur={() => commitNumber("poll_interval_minutes")}
          />
        </Field>

        <Field label="Backfill count" hint="Videos fetched when subscribing. 1 – 500">
          <input
            className="text-input"
            type="number"
            min={LIMITS.backfill_count.min}
            max={LIMITS.backfill_count.max}
            value={numValue("backfill_count")}
            onChange={(e) => setDraft("backfill_count", e.currentTarget.value)}
            onBlur={() => commitNumber("backfill_count")}
          />
        </Field>
      </div>

      <label className="switch-row">
        <input
          type="checkbox"
          checked={s.poll_on_startup}
          onChange={(e) => setAndSave("poll_on_startup", e.currentTarget.checked)}
        />
        <span>
          <span className="switch-label">Check for new videos on startup</span>
          <span className="field-hint">Otherwise the first poll waits for the interval.</span>
        </span>
      </label>

      <p className="settings-foot">
        Stored in <code>~/.config/mytube/settings.json</code>. Changes save when a field
        loses focus.
      </p>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="field">
      <div className="field-label">{label}</div>
      {children}
      {hint && <div className="field-hint">{hint}</div>}
    </div>
  );
}
