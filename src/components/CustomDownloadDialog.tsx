import { useCallback, useEffect, useRef, useState } from "react";
import QualityForm from "./QualityForm";
import { api, errText } from "../api";
import {
  DEFAULT_QUALITY, initialPick, resolveExact, sanitizeQuality, type ExactPick,
} from "../quality";
import type { Quality, Video, VideoFormats } from "../types";

/** Several videos downloaded one way: a playlist's ticked parts. */
export interface Batch {
  ids: string[];
  /** What they are, for the line under the heading: the playlist's title. */
  title: string;
}

type Props = { onClose: () => void } & (
  | {
    video: Video;
    /** Called once the download is queued, so the card can show it at once. */
    onQueued: (video: Video) => void;
    batch?: never;
  }
  | {
    batch: Batch;
    /** Called with how many of the batch were queued. */
    onBatchQueued: (queued: number) => void;
    video?: never;
  }
);

type Stage =
  | { kind: "loading" }
  | { kind: "exact"; formats: VideoFormats; pick: ExactPick }
  /** The probe failed, or there is no one video to probe: the preferences
   *  form, seeded from Settings. `error` says why, when there was a failure. */
  | { kind: "generic"; error: string | null; quality: Quality };

/** Settings' quality, or the built-in default when it cannot be read. */
function settingsQuality(): Promise<Quality> {
  return api.getSettings()
    .then((s) => sanitizeQuality(s.quality))
    .catch(() => ({ ...DEFAULT_QUALITY }));
}

/**
 * Download (custom)…: downloaded some other way than Settings says.
 *
 * For one video it asks yt-dlp what the video really has and offers only that,
 * starting on what the Settings default would have picked. When the probe
 * fails — no network, a members-only video with no cookies — the generic form
 * stands in, so a custom download is still possible, just by preference
 * rather than id.
 *
 * For a batch (a playlist's parts) there is no probe at all: exact format ids
 * differ from video to video, so one set of them could only fit one. The
 * generic form is the whole dialog, and the one quality it makes is stored
 * on every part as its override.
 */
export default function CustomDownloadDialog(props: Props) {
  const { onClose } = props;
  const batch = props.batch ?? null;
  const video = props.video ?? null;
  const [stage, setStage] = useState<Stage>({ kind: "loading" });
  const [submitting, setSubmitting] = useState(false);
  const [submitError, setSubmitError] = useState<string | null>(null);
  // The probe takes seconds, and closing the dialog does not stop it; nothing
  // it or a queued download resolves with may land on a dialog that is gone.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);

  const videoId = video?.id ?? null;
  useEffect(() => {
    let alive = true;
    // Asked alongside the probe rather than after it: the exact form takes its
    // mode and post-processing from Settings, and the fallback all of it.
    const base = settingsQuality();
    if (videoId === null) {
      base.then((q) => { if (alive) setStage({ kind: "generic", error: null, quality: q }); });
      return () => { alive = false; };
    }
    api.probeFormats(videoId).then(
      async (formats) => {
        const q = await base;
        if (alive) setStage({ kind: "exact", formats, pick: initialPick(formats, q) });
      },
      async (err) => {
        const q = await base;
        if (alive) setStage({ kind: "generic", error: errText(err), quality: q });
      },
    );
    return () => { alive = false; };
  }, [videoId]);

  const cancel = useCallback(() => {
    if (!submitting) onClose();
  }, [submitting, onClose]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") cancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [cancel]);

  const quality =
    stage.kind === "exact" ? resolveExact(stage.formats, stage.pick)
      : stage.kind === "generic" ? stage.quality
        : null;

  async function download() {
    if (!quality) return;
    setSubmitting(true);
    setSubmitError(null);
    try {
      // Queued is queued whether or not the dialog is still up to see it.
      if (props.batch) {
        props.onBatchQueued(await api.enqueueDownloads(props.batch.ids, quality));
      } else {
        await api.enqueueDownload(props.video.id, quality);
        props.onQueued(props.video);
      }
      if (mounted.current) onClose();
    } catch (err) {
      if (!mounted.current) return;
      setSubmitError(errText(err));
      setSubmitting(false);
    }
  }

  const count = batch?.ids.length ?? 1;
  const subject = batch
    ? `${batch.title} · ${count} video${count === 1 ? "" : "s"}`
    : video!.title;

  return (
    <div className="modal-backdrop" onMouseDown={cancel}>
      <div
        className="modal quality-dialog"
        role="dialog"
        aria-modal="true"
        aria-label="Custom download"
        onMouseDown={(e) => e.stopPropagation()}
        onClick={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">Custom download</h2>
        <p className="quality-dialog-video" title={subject}>{subject}</p>

        {stage.kind === "loading" && (
          <div className="quality-loading">
            <span className="spinner" aria-hidden="true" />
            {batch ? "Reading your Settings…" : "Reading the formats this video has…"}
          </div>
        )}

        {stage.kind === "exact" && (
          <QualityForm
            source="exact"
            formats={stage.formats}
            value={stage.pick}
            onChange={(pick) => setStage({ ...stage, pick })}
          />
        )}

        {stage.kind === "generic" && (
          <>
            {stage.error !== null && (
              <p className="tool-error quality-probe-error" role="alert">
                Could not read this video's formats: {stage.error}
              </p>
            )}
            <p className="field-hint quality-note">
              {batch
                ? `One quality for all ${count} videos: their formats differ, so yt-dlp picks the closest match for each.`
                : "These preferences are used instead; yt-dlp picks the closest match."}
            </p>
            <QualityForm
              source="generic"
              value={stage.quality}
              onEdit={(q) => setStage({ ...stage, quality: q })}
              onPick={(q) => setStage({ ...stage, quality: q })}
            />
          </>
        )}

        {submitError && <p className="tool-error" role="alert">{submitError}</p>}

        <div className="confirm-actions">
          <button type="button" className="btn btn-quiet" onClick={cancel} disabled={submitting}>
            Cancel
          </button>
          <button
            type="button"
            className="btn btn-primary"
            disabled={quality === null || submitting}
            onClick={() => void download()}
          >
            {submitting ? "Queueing…" : batch ? `Download ${count}` : "Download"}
          </button>
        </div>
      </div>
    </div>
  );
}
