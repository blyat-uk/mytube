import { useMemo, useState } from "react";
import ConfirmDialog from "./ConfirmDialog";
import { IconClose } from "./Icons";
import { useTransferProgress } from "../events";
import { formatDate } from "../format";
import type { ArchiveSummary, ImportMode, TransferProgress } from "../types";

interface Props {
  summary: ArchiveSummary;
  importing: boolean;
  onImport: (channelIds: string[], mode: ImportMode, applySettings: boolean) => void;
  onCancel: () => void;
}

const MODES: { value: ImportMode; label: string }[] = [
  { value: "merge", label: "Merge" },
  { value: "replace", label: "Replace" },
];

const MERGE_NOTE =
  "Adds what the archive holds to what is already here. Nothing local is lost.";

/** The one promise worth repeating wherever a removal is mentioned. */
const DISK_PROMISE =
  "No file is ever deleted from disk — not a download, not a cached thumbnail.";

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/** "2 channels and 312 videos", dropping whichever half is zero; empty when a
 *  Replace would take nothing away, which is worth saying in words instead. */
function removalPhrase(channels: number, videos: number): string {
  const parts: string[] = [];
  if (channels) parts.push(plural(channels, "channel"));
  if (videos) parts.push(plural(videos, "video"));
  return parts.join(" and ");
}

/**
 * Pre-import checklist for an export archive, and the one place the difference
 * between merging and replacing is spelled out. Nothing has been written when
 * this opens — `read_archive` only read the manifest — so every choice here is
 * still free.
 */
export default function TransferDialog({ summary, importing, onImport, onCancel }: Props) {
  const [picked, setPicked] = useState<Set<string>>(
    () => new Set(summary.channels.map((c) => c.channelId)),
  );
  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<ImportMode>("merge");
  const [applySettings, setApplySettings] = useState(true);
  const [confirming, setConfirming] = useState(false);
  const [progress, setProgress] = useState<TransferProgress | null>(null);

  // One channel carries the export half too, and an export can only be running
  // if this dialog is not, but filtering keeps that an observation rather than
  // something this bar depends on.
  useTransferProgress((p) => {
    if (p.phase === "import") setProgress(p);
  });

  const visible = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return summary.channels;
    return summary.channels.filter(
      (c) => c.title.toLowerCase().includes(q) || c.channelId.toLowerCase().includes(q),
    );
  }, [summary.channels, query]);

  const toggle = (id: string) =>
    setPicked((prev) => {
      const next = new Set(prev);
      next.has(id) ? next.delete(id) : next.add(id);
      return next;
    });

  const setAllVisible = (on: boolean) =>
    setPicked((prev) => {
      const next = new Set(prev);
      for (const c of visible) on ? next.add(c.channelId) : next.delete(c.channelId);
      return next;
    });

  const pickedVideos = summary.channels.reduce(
    (n, c) => (picked.has(c.channelId) ? n + c.videoCount : n),
    0,
  );

  // What a Replace would take away, and it is the same number whatever is
  // ticked: a channel left unticked is skipped, never deleted, so the figure
  // the user read before working down the list is still true at the bottom.
  const removal = removalPhrase(summary.localOnlyChannels, summary.localOnlyVideos);

  const NOTES: Record<ImportMode, string> = {
    merge: MERGE_NOTE,
    replace: removal
      ? `Removes ${removal} this archive doesn't contain, and makes the ticked channels ` +
        `match it exactly. ${DISK_PROMISE}`
      : "Nothing here would be removed — the archive contains every channel and video " +
        `you have. Ticked channels are still made to match it exactly. ${DISK_PROMISE}`,
  };

  const fire = () => onImport([...picked], mode, applySettings);
  const pct = progress && progress.total > 0 ? (progress.done / progress.total) * 100 : 0;

  return (
    <div className="modal-backdrop" onMouseDown={importing ? undefined : onCancel}>
      <div
        className="modal transfer"
        role="dialog"
        aria-modal="true"
        aria-label="Import from a MyTube archive"
        // Both handlers matter: a surrounding modal may close on click rather
        // than mousedown, and stopping only one lets the other bubble out and
        // tear this dialog down mid-interaction.
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2 className="modal-title">Import from archive</h2>
          <button
            type="button"
            className="icon-btn"
            title="Close"
            disabled={importing}
            onClick={onCancel}
          >
            <span aria-hidden="true"><IconClose /></span>
            <span className="sr-only">Close without importing</span>
          </button>
        </div>

        <p className="transfer-origin">
          Exported {formatDate(summary.exportedAt)} from{" "}
          <span className="transfer-host">{summary.exportedFrom}</span> · MyTube{" "}
          {summary.appVersion}
        </p>
        <p className="transfer-counts">
          {plural(summary.channels.length, "channel")} · {plural(summary.videoCount, "video")} ·{" "}
          {summary.includesThumbs
            ? `${plural(summary.thumbCount, "thumbnail")} included`
            : "no thumbnails"}
        </p>

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
            <p className="field-hint">Writing rows and thumbnails. You can leave this open.</p>
          </div>
        ) : (
          <>
            {/* Both notes are on screen at once, not just the selected one: a
                pair of chips whose consequence only appears after you have
                clicked is a trap, and Replace is the one choice here nobody
                should make uninformed. */}
            <div className="transfer-modes">
              {MODES.map((m) => (
                <div key={m.value} className="transfer-mode">
                  <div className="chip-row">
                    <button
                      type="button"
                      className={`preset-chip${mode === m.value ? " is-on" : ""}`}
                      aria-pressed={mode === m.value}
                      onClick={() => setMode(m.value)}
                    >
                      {m.label}
                    </button>
                  </div>
                  <p className="field-hint">{NOTES[m.value]}</p>
                </div>
              ))}
            </div>

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
              {visible.map((c) => (
                <li key={c.channelId}>
                  <label>
                    <input
                      type="checkbox"
                      checked={picked.has(c.channelId)}
                      onChange={() => toggle(c.channelId)}
                    />
                    <span className="takeout-title">{c.title}</span>
                    {/* Worded, not a bare number: the row is read out as one
                        string, and "Alpha 12 already here" says nothing. */}
                    <span className="transfer-videos">{plural(c.videoCount, "video")}</span>
                    {c.alreadyHere && <span className="takeout-tag">already here</span>}
                    {/* An uploader that was never subscribed to travels only so
                        that a manually added video of theirs has a parent row. */}
                    {!c.subscribed && (
                      <span
                        className="takeout-tag is-quiet"
                        title="Not a subscription — it carries a manually added video's channel row."
                      >
                        not subscribed
                      </span>
                    )}
                  </label>
                </li>
              ))}
              {visible.length === 0 && <li className="takeout-none">No channels match.</li>}
            </ul>
            {/* Said out loud because the checklist sits directly under Replace
                and would otherwise read as one: unticking is not a delete. */}
            <p className="field-hint">
              Unticked channels are skipped entirely — not imported, not changed, not
              removed.
            </p>

            <label className="transfer-opt">
              <input
                type="checkbox"
                checked={applySettings}
                onChange={(e) => setApplySettings(e.currentTarget.checked)}
              />
              <span>Also apply settings (download folder, template, player, intervals)</span>
            </label>
            {applySettings && !summary.downloadDirExists && (
              <p className="field-hint transfer-warn">
                The archive's download folder <code>{summary.downloadDir}</code> does not exist
                here, so this machine's own folder is kept.
              </p>
            )}
          </>
        )}

        <div className="confirm-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel} disabled={importing}>
            Cancel
          </button>
          <button
            type="button"
            className={`btn ${mode === "replace" ? "btn-danger" : "btn-primary"}`}
            disabled={importing || picked.size === 0}
            // Replace is the one path that takes rows away, so it asks again
            // rather than firing off the same click that chose the channels.
            onClick={() => (mode === "replace" ? setConfirming(true) : fire())}
          >
            {importing ? "Importing…" : `Import ${plural(picked.size, "channel")}`}
          </button>
        </div>

        {/* Inside the modal, not beside it: a mousedown on the confirmation's
            own backdrop would otherwise bubble out to this dialog's backdrop
            and close the whole checklist along with the question. */}
        {confirming && (
          <ConfirmDialog
            title="Replace with the archive's copy?"
            body={
              removal
                ? `Removes ${removal} this archive doesn't contain, and makes the ` +
                  `${plural(picked.size, "ticked channel")} match it exactly — its ` +
                  `${plural(pickedVideos, "video")} become their library. Unticked ` +
                  "channels are left exactly as they are. Database rows only: no " +
                  "downloaded file and no cached thumbnail is deleted from disk."
                : "Nothing would be removed — the archive contains every channel and " +
                  `video you have. Replacing still overwrites the ` +
                  `${plural(picked.size, "ticked channel")} with the archive's copy of ` +
                  "every row, which is the one thing Merge will not do. Unticked channels " +
                  "are left exactly as they are, and nothing is deleted from disk."
            }
            choices={[{ label: "Replace", value: "replace", danger: true }]}
            onChoose={() => {
              setConfirming(false);
              fire();
            }}
            onCancel={() => setConfirming(false)}
          />
        )}
      </div>
    </div>
  );
}
