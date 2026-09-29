import { useCallback, useEffect, useRef, useState } from "react";
import QualityForm from "./QualityForm";
import { api, errText } from "../api";
import {
  DEFAULT_QUALITY, initialPick, resolveExact, sanitizeQuality, type ExactPick,
} from "../quality";
import type { Quality, Video, VideoFormats } from "../types";

interface Props {
  video: Video;
  /** Called once the download is queued, so the card can show it at once. */
  onQueued: (video: Video) => void;
  onClose: () => void;
}

type Stage =
  | { kind: "loading" }
  | { kind: "exact"; formats: VideoFormats; pick: ExactPick }
  /** The probe failed: the preferences form, seeded from Settings, with why. */
  | { kind: "generic"; error: string; quality: Quality };

/**
 * Download (custom)…: one video, downloaded some other way than Settings says.
 * It asks yt-dlp what the video really has and offers only that, starting on
 * what the Settings default would have picked. When the probe fails — no
 * network, a members-only video with no cookies — the generic form stands in,
 * so a custom download is still possible, just by preference rather than id.
 */
export default function CustomDownloadDialog({ video, onQueued, onClose }: Props) {
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

  useEffect(() => {
    let alive = true;
    // Asked alongside the probe rather than after it: the exact form takes its
    // mode and post-processing from Settings, and the fallback all of it.
    const base = api.getSettings()
      .then((s) => sanitizeQuality(s.quality))
      .catch(() => ({ ...DEFAULT_QUALITY }));
    api.probeFormats(video.id).then(
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
  }, [video.id]);

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
      await api.enqueueDownload(video.id, quality);
      // Queued is queued whether or not the dialog is still up to see it.
      onQueued(video);
      if (mounted.current) onClose();
    } catch (err) {
      if (!mounted.current) return;
      setSubmitError(errText(err));
      setSubmitting(false);
    }
  }

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
        <p className="quality-dialog-video" title={video.title}>{video.title}</p>

        {stage.kind === "loading" && (
          <div className="quality-loading">
            <span className="spinner" aria-hidden="true" />
            Reading the formats this video has…
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
            <p className="tool-error quality-probe-error" role="alert">
              Could not read this video's formats: {stage.error}
            </p>
            <p className="field-hint quality-note">
              These preferences are used instead; yt-dlp picks the closest match.
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
            {submitting ? "Queueing…" : "Download"}
          </button>
        </div>
      </div>
    </div>
  );
}
