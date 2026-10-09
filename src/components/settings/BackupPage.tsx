import { useState } from "react";
import { open, save } from "@tauri-apps/plugin-dialog";
import { api } from "../../api";
import { useTransferProgress } from "../../events";
import { formatBytes } from "../../format";
import { usePending } from "../../pending";
import { IconExport, IconImport, IconSectionBackup, IconTerminal } from "../Icons";
import { useToast } from "../Toast";
import TransferDialog from "../TransferDialog";
import type { ArchiveSummary, ImportMode, TransferProgress } from "../../types";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { Block, SettingsPage, Switch } from "./parts";

const count = (n: number, word: string) => `${n.toLocaleString("en-US")} ${word}${n === 1 ? "" : "s"}`;

/** "36 channels · 3,412 videos". */
export function summary({ estimate }: SettingsCtx): RailLine {
  if (!estimate) return { text: "Export or import your library" };
  return { text: `${count(estimate.channelCount, "channel")} · ${count(estimate.videoCount, "video")}` };
}

/**
 * Today, for the export's default filename. Local rather than
 * `toISOString`'s UTC: the stamp should match the calendar on the wall of the
 * machine that wrote the file.
 */
export function todayStamp(d = new Date()): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/* One key for every transfer step: a file chooser is up, or an archive is
   being read or written, and neither transfer may start while the other is
   going -- including from two clicks landing in the same frame. */
const TRANSFER = "transfer";

export default function BackupPage() {
  const { estimate, refreshEstimate, reload } = useSettingsCtx();
  const toast = useToast();
  const { pending, run } = usePending();
  const busy = pending.has(TRANSFER);

  // A choice about one archive, not a setting, so it deliberately never
  // reaches a save: persisting it would write settings.json on every click
  // and carry the choice into every later export.
  const [includeThumbs, setIncludeThumbs] = useState(true);
  // Only once a file is named: no bar under the button while the save dialog
  // is still asking where.
  const [writing, setWriting] = useState(false);
  const [progress, setProgress] = useState<TransferProgress | null>(null);
  const [archive, setArchive] = useState<{ path: string; summary: ArchiveSummary } | null>(null);

  // The import half runs inside TransferDialog, which draws its own bar.
  useTransferProgress((p) => {
    if (p.phase === "export") setProgress(p);
  });

  /** Writes the whole configuration to one zip the user names. */
  function runExport() {
    void run(TRANSFER, async () => {
      const path = await save({
        title: "Save a MyTube export",
        defaultPath: `mytube-export-${todayStamp()}.zip`,
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
      if (!path) return;
      setProgress(null);
      setWriting(true);
      try {
        await api.exportConfig(path, includeThumbs);
        toast.success(`Exported to ${path}.`);
      } finally {
        setWriting(false);
        setProgress(null);
      }
    });
  }

  /** Step one: read the archive's manifest and show the checklist. Nothing is
   *  written to the library yet. */
  function pickArchive() {
    void run(TRANSFER, async () => {
      const path = await open({
        multiple: false,
        directory: false,
        title: "Choose a MyTube export",
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
      if (!path) return;
      const summary = await api.readArchive(path as string);
      setArchive({ path: path as string, summary });
    });
  }

  /** Step two: import exactly the channels that were ticked. */
  function runImport(channelIds: string[], mode: ImportMode, applySettings: boolean) {
    if (!archive) return;
    const { path } = archive;
    void run(TRANSFER, async () => {
      try {
        const r = await api.importConfig(path, channelIds, mode, applySettings);
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
        // settings.json was rewritten under the view, so every page's
        // fields are stale until they are read back.
        if (r.settingsApplied) await reload();
      } finally {
        // Even a failed import may have landed rows before it stopped being
        // sure of itself; the counts are cheap to take again.
        refreshEstimate();
      }
    });
  }

  const pct = progress && progress.total > 0 ? (progress.done / progress.total) * 100 : 0;
  const progressText = progress
    ? `${progress.done} of ${progress.total} · ${progress.current}`
    : "Starting…";

  return (
    <SettingsPage
      icon={<IconSectionBackup />}
      title="Backup & transfer"
      intro="Pack your whole library into one file — to keep, or to pick it up on another computer. Video files stay where they are."
      className="st-bk"
    >
      <div className="st-bk-cards">
        <Block className="st-bk-card">
          <div className="st-bk-card-head">
            <span className="st-bk-glyph" aria-hidden="true"><IconExport /></span>
            <div className="st-bk-card-text">
              <h3 className="st-bk-card-title">Export</h3>
              <p className="st-bk-card-sub">
                {estimate
                  ? `${count(estimate.channelCount, "channel")} · ${count(estimate.videoCount, "video")}`
                  : "Your whole library"}
              </p>
            </div>
          </div>
          <p className="st-bk-card-body">
            Your channels, what you have watched and hidden, and your settings, in one zip.
          </p>

          <Switch
            // No size while the estimate is in flight: a number that later
            // changes is worse than a label that briefly has none.
            label={`Include thumbnails${estimate ? ` (${formatBytes(estimate.thumbBytes)})` : ""}`}
            hint="Leave this off for a small archive — the other machine caches them again as it polls."
            checked={includeThumbs}
            disabled={busy}
            onChange={setIncludeThumbs}
          />

          <div className="st-bk-actions">
            <button type="button" className="btn st-bk-btn" disabled={busy} onClick={runExport}>
              <span className="st-bk-btn-glyph" aria-hidden="true"><IconExport /></span>
              Export…
            </button>
          </div>

          {writing && (
            <div className="st-bk-progress">
              <div
                className="progress-track"
                role="progressbar"
                aria-label="Export"
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={progress ? Math.round(pct) : undefined}
                aria-valuetext={progressText}
              >
                <div className="progress-fill" style={{ width: `${pct}%` }} />
              </div>
              <p className="st-bk-progress-label">{progressText}</p>
            </div>
          )}
        </Block>

        <Block className="st-bk-card">
          <div className="st-bk-card-head">
            <span className="st-bk-glyph" aria-hidden="true"><IconImport /></span>
            <div className="st-bk-card-text">
              <h3 className="st-bk-card-title">Import</h3>
              <p className="st-bk-card-sub">From a MyTube export</p>
            </div>
          </div>
          <p className="st-bk-card-body">
            Pick an export, then choose which channels come in and whether they merge with this
            library or replace it. Nothing changes before you say so.
          </p>

          <div className="st-bk-actions">
            <button type="button" className="btn st-bk-btn" disabled={busy} onClick={pickArchive}>
              <span className="st-bk-btn-glyph" aria-hidden="true"><IconImport /></span>
              Import…
            </button>
          </div>
        </Block>
      </div>

      <p className="st-bk-foot">
        <span className="st-bk-foot-glyph" aria-hidden="true"><IconTerminal /></span>
        <span>
          The same from a terminal, for a computer you reach over SSH: <code>mytube export</code>{" "}
          and <code>mytube import</code> (Linux and macOS).
        </span>
      </p>

      {archive && (
        <TransferDialog
          summary={archive.summary}
          importing={busy}
          onImport={runImport}
          onCancel={() => setArchive(null)}
        />
      )}
    </SettingsPage>
  );
}
