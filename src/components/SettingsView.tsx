import { useCallback, useEffect, useRef, useState } from "react";
import { TEMPLATE_PRESETS, TOKEN_CHIPS, formatBytes, insertToken } from "../format";
import { open, save } from "@tauri-apps/plugin-dialog";
import TransferDialog from "./TransferDialog";
import Field from "./Field";
import PlayerField from "./PlayerField";
import CookiesField from "./CookiesField";
import ToolsSection from "./ToolsSection";
import { useTransferProgress } from "../events";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import {
  type ArchiveSummary, type ImportMode, type Settings, type TransferEstimate,
  type TransferProgress,
} from "../types";

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

  // Transfer state. `includeThumbs` is a choice about one archive, not a
  // setting, so it deliberately never reaches `commit()`.
  const [estimate, setEstimate] = useState<TransferEstimate | null>(null);
  const [includeThumbs, setIncludeThumbs] = useState(true);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [archive, setArchive] = useState<{ path: string; summary: ArchiveSummary } | null>(null);
  const [progress, setProgress] = useState<TransferProgress | null>(null);

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

  useEffect(() => {
    let alive = true;
    api.transferEstimate()
      .then((e) => { if (alive) setEstimate(e); })
      // A missing estimate costs the tick its size and nothing else; an export
      // that cannot be weighed can still be written.
      .catch(() => {});
    return () => { alive = false; };
  }, []);

  // The import half runs inside TransferDialog, which draws its own bar.
  useTransferProgress((p) => {
    if (p.phase === "export") setProgress(p);
  });

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

  const templateRef = useRef<HTMLInputElement | null>(null);

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

  /** Several keys that change together, in one save: a cookie choice sets the
   *  browser and clears the file, and two writes would race each other. */
  const patchAndSave = useCallback((patch: Partial<Settings>) => {
    const base = current.current;
    if (!base) return;
    const next = { ...base, ...patch };
    current.current = next;
    setSettings(next);
    void commit(next);
  }, [commit]);

  const blurCommit = useCallback(() => {
    if (current.current) void commit(current.current);
  }, [commit]);

  /** Replaces the whole template with a preset and saves immediately. */
  const applyTemplate = useCallback((value: string) => {
    setAndSave("filename_template", value);
  }, [setAndSave]);

  /** Inserts a field at the caret, keeping the caret after what was inserted. */
  const applyToken = useCallback((token: string) => {
    const base = current.current;
    const el = templateRef.current;
    if (!base) return;
    const caret = el && document.activeElement === el ? el.selectionStart : null;
    const { value, caret: next } = insertToken(base.filename_template, caret, token);
    setAndSave("filename_template", value);
    requestAnimationFrame(() => {
      if (!el) return;
      el.focus();
      el.setSelectionRange(next, next);
    });
  }, [setAndSave]);

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

  /** Writes the whole configuration to one zip the user names. */
  async function runExport() {
    setExporting(true);
    setProgress(null);
    try {
      const path = await save({
        title: "Save a MyTube export",
        defaultPath: `mytube-export-${todayStamp()}.zip`,
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
      if (!path) return;
      await api.exportConfig(path, includeThumbs);
      toast.success(`Exported to ${path}.`);
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setExporting(false);
      setProgress(null);
    }
  }

  /** Step one: read the archive's manifest and show the checklist. Nothing is
   *  written to the library yet. */
  async function pickArchive() {
    setImporting(true);
    try {
      const path = await open({
        multiple: false,
        directory: false,
        title: "Choose a MyTube export",
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
      if (!path) return;
      const summary = await api.readArchive(path as string);
      setArchive({ path: path as string, summary });
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setImporting(false);
    }
  }

  /** Step two: import exactly the channels that were ticked. */
  async function runImport(channelIds: string[], mode: ImportMode, applySettings: boolean) {
    if (!archive) return;
    setImporting(true);
    try {
      const r = await api.importConfig(archive.path, channelIds, mode, applySettings);
      setArchive(null);
      toast.success(
        `Imported ${count(r.channelsAdded, "channel")} and ${count(r.videosAdded, "video")}` +
        (r.downloadsRelinked ? `, and found ${r.downloadsRelinked} already downloaded` : "") +
        ".",
      );
      // Replace's removals are the one thing worth saying twice, because the
      // number is the whole point of the mode and the toast above never shows it.
      if (r.channelsRemoved || r.videosRemoved) {
        toast.info(
          `Removed ${count(r.videosRemoved, "video row")} and ` +
          `${count(r.channelsRemoved, "channel")} from the library. No files were deleted.`,
        );
      }
      if (r.downloadDirKept) {
        toast.info("Kept this machine's download folder; the archive's is not here.");
      }
      if (r.settingsApplied) {
        // settings.json was rewritten under us, so the fields on screen are
        // stale until they are read back.
        const s = await api.getSettings();
        current.current = s;
        persisted.current = JSON.stringify(s);
        setSettings(s);
      }
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setImporting(false);
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
  // One flag for both buttons: a file chooser is up, or an archive is being
  // read or written, and neither transfer may start while the other is going.
  const busy = exporting || importing;
  const exportPct = progress && progress.total > 0 ? (progress.done / progress.total) * 100 : 0;

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
          ref={templateRef}
          className="text-input mono"
          value={s.filename_template}
          onChange={(e) => set("filename_template", e.currentTarget.value)}
          onBlur={blurCommit}
          spellCheck={false}
        />

        <div className="preset-block">
          <span className="preset-label">Presets</span>
          <div className="chip-row">
            {TEMPLATE_PRESETS.map((preset) => (
              <button
                key={preset.value}
                type="button"
                className={`preset-chip${s.filename_template === preset.value ? " is-on" : ""}`}
                title={preset.hint}
                onClick={() => applyTemplate(preset.value)}
              >
                {preset.label}
              </button>
            ))}
          </div>
        </div>

        <div className="preset-block">
          <span className="preset-label">Insert a field</span>
          <div className="chip-row">
            {TOKEN_CHIPS.map((token) => (
              <button
                key={token}
                type="button"
                className="token-chip mono"
                title={`Insert ${token} at the cursor`}
                onClick={() => applyToken(token)}
              >
                {token}
              </button>
            ))}
          </div>
        </div>
      </Field>

      <PlayerField
        value={s.player_command}
        onEdit={(v) => set("player_command", v)}
        onBlur={blurCommit}
        onPick={(v) => setAndSave("player_command", v)}
      />

      {/* Rust fills both keys on every read; the fallbacks are its defaults,
          for a Settings built by hand without them. */}
      <CookiesField
        browser={s.cookies_browser}
        file={s.cookies_file}
        onPick={patchAndSave}
      />

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

      <p className="settings-note">Changes save when a field loses focus.</p>

      <ToolsSection
        channel={s.ytdlp_channel}
        autoUpdate={s.ytdlp_auto_update}
        onChannel={(v) => setAndSave("ytdlp_channel", v)}
        onAutoUpdate={(on) => setAndSave("ytdlp_auto_update", on)}
      />

      <div className="transfer-block">
        <div className="field-label">Backup &amp; transfer</div>
        <p className="field-hint">
          Everything lives in <code>~/.config/mytube</code> — channels, videos, settings and
          cached thumbnails. This packs it into one zip, enough to pick the library up on
          another machine. Video files stay where they are.
        </p>

        <label className="switch-row transfer-thumbs">
          <input
            type="checkbox"
            checked={includeThumbs}
            disabled={busy}
            onChange={(e) => setIncludeThumbs(e.currentTarget.checked)}
          />
          <span>
            <span className="switch-label">
              {/* No placeholder while the estimate is in flight: a number that
                  later changes is worse than a label that briefly has none. */}
              Include thumbnails{estimate ? ` (${formatBytes(estimate.thumbBytes)})` : ""}
            </span>
            <span className="field-hint">
              Leave this off for a small archive — the other machine caches them again as
              it polls.
            </span>
          </span>
        </label>

        <div className="row-inline transfer-actions">
          <button type="button" className="btn" disabled={busy} onClick={() => void runExport()}>
            Export…
          </button>
          <button type="button" className="btn" disabled={busy} onClick={() => void pickArchive()}>
            Import…
          </button>
          {estimate && (
            <span className="transfer-estimate">
              {count(estimate.channelCount, "channel")} · {count(estimate.videoCount, "video")}
            </span>
          )}
        </div>

        {exporting && (
          <div className="transfer-progress">
            <div className="progress-track">
              <div className="progress-fill" style={{ width: `${exportPct}%` }} />
            </div>
            <p className="progress-label">
              {progress
                ? `${progress.done} of ${progress.total} · ${progress.current}`
                : "Starting…"}
            </p>
          </div>
        )}
      </div>

      {archive && (
        <TransferDialog
          summary={archive.summary}
          importing={importing}
          onImport={(ids, mode, apply) => void runImport(ids, mode, apply)}
          onCancel={() => setArchive(null)}
        />
      )}
    </div>
  );
}

const count = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * Today, for the export's default filename. Local rather than
 * `toISOString`'s UTC: the stamp should match the calendar on the wall of the
 * machine that wrote the file.
 */
function todayStamp(d = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}
