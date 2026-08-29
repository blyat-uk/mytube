import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import TakeoutDialog from "./TakeoutDialog";
import { useToast } from "./Toast";
import { api, errText } from "../api";
import type { AddKind, Channel, TakeoutRow } from "../types";

interface Props {
  open: boolean;
  onClose: () => void;
  channels: Channel[];
  /** Something changed server-side; the shell should reload channels and videos. */
  onChanged: () => void;
}

const ACTION_LABEL: Record<AddKind, string> = {
  channel: "Subscribe to channel",
  video: "Download this video",
  short: "MyTube does not handle Shorts",
};

const HINT: Record<AddKind, string> = {
  channel: "This looks like a channel. It will be subscribed and its recent videos backfilled.",
  video: "This looks like a single video. It downloads right away and sorts to the top.",
  short: "Shorts are excluded from MyTube by design.",
};

export default function AddChannelDialog({ open: isOpen, onClose, channels, onChanged }: Props) {
  const toast = useToast();
  const [input, setInput] = useState("");
  const [kind, setKind] = useState<AddKind | null>(null);
  const [classifying, setClassifying] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [importing, setImporting] = useState(false);
  const [takeout, setTakeout] = useState<{ path: string; rows: TakeoutRow[] } | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);
  const classifyId = useRef(0);

  useEffect(() => {
    if (!isOpen) return;
    setConfirmRemove(null);
    inputRef.current?.focus();
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => { if (e.key === "Escape") onClose(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [isOpen, onClose]);

  // Debounced classification: the button has to say what it will do before it
  // is pressed, but every keystroke must not become a command.
  useEffect(() => {
    const text = input.trim();
    if (!text) {
      classifyId.current++;
      setKind(null);
      setProblem(null);
      setClassifying(false);
      return;
    }
    setClassifying(true);
    const id = ++classifyId.current;
    const t = window.setTimeout(async () => {
      try {
        const k = await api.classifyAddInput(text);
        if (id !== classifyId.current) return;
        setKind(k);
        setProblem(null);
      } catch (err) {
        if (id !== classifyId.current) return;
        setKind(null);
        setProblem(errText(err));
      } finally {
        if (id === classifyId.current) setClassifying(false);
      }
    }, 300);
    return () => window.clearTimeout(t);
  }, [input]);

  if (!isOpen) return null;

  const canSubmit = !busy && !classifying && (kind === "channel" || kind === "video");

  async function submit() {
    const text = input.trim();
    if (!canSubmit || !text) return;
    setBusy(true);
    try {
      if (kind === "channel") {
        const c = await api.addChannel(text);
        toast.success(`Subscribed to ${c.title}.`);
      } else {
        const v = await api.addVideo(text);
        toast.success(`Added “${v.title}” — downloading now.`);
      }
      setInput("");
      setKind(null);
      onChanged();
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setBusy(false);
    }
  }

  /** Step one: parse the CSV and show the checklist. Nothing is imported yet. */
  async function pickCsv() {
    setImporting(true);
    try {
      const path = await open({
        multiple: false,
        directory: false,
        title: "Choose a Google Takeout subscriptions CSV",
        filters: [{ name: "CSV", extensions: ["csv"] }],
      });
      if (!path) return;
      const rows = await api.previewTakeoutCsv(path as string);
      if (rows.length === 0) {
        toast.error("That CSV has no channels in it.");
        return;
      }
      setTakeout({ path: path as string, rows });
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setImporting(false);
    }
  }

  /** Step two: import exactly what was ticked. */
  async function runImport(channelIds: string[]) {
    if (!takeout) return;
    setImporting(true);
    try {
      const result = await api.importTakeoutCsv(takeout.path, channelIds);
      const failed = result.failed.length;
      toast.success(
        `Imported ${result.added} channel${result.added === 1 ? "" : "s"}` +
        `, skipped ${result.skipped}` +
        (failed ? `, ${failed} failed` : "") + ".",
      );
      if (failed) toast.error(`Could not import: ${result.failed.slice(0, 5).join(", ")}`);
      setTakeout(null);
      onChanged();
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setImporting(false);
    }
  }

  async function removeChannel(c: Channel) {
    try {
      await api.removeChannel(c.id);
      toast.success(`Removed ${c.title} and its videos.`);
      setConfirmRemove(null);
      onChanged();
    } catch (err) {
      toast.error(errText(err));
    }
  }

  return (
    <>
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-label="Add a channel or video"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="modal-head">
          <h2>Add</h2>
          <button type="button" className="icon-btn" onClick={onClose} title="Close">
            <span aria-hidden="true">✕</span>
            <span className="sr-only">Close</span>
          </button>
        </div>

        <form
          className="add-form"
          onSubmit={(e) => { e.preventDefault(); void submit(); }}
        >
          <input
            ref={inputRef}
            className="text-input"
            value={input}
            placeholder="Channel URL, @handle, UC… id, or a video URL"
            aria-label="Channel or video to add"
            onChange={(e) => setInput(e.currentTarget.value)}
          />
          <button
            type="submit"
            className={`btn ${kind === "short" ? "btn-disabled" : "btn-primary"}`}
            disabled={!canSubmit}
          >
            {busy
              ? "Working…"
              : classifying
                ? "Checking…"
                : kind
                  ? ACTION_LABEL[kind]
                  : "Add"}
          </button>
        </form>

        <p className={`add-hint${kind === "short" || problem ? " is-warn" : ""}`}>
          {problem
            ? problem
            : kind
              ? HINT[kind]
              : "Accepts either a channel to subscribe to, or one video URL to download."}
        </p>

        <div className="modal-divider" />

        <div className="modal-head">
          <h3>
            Subscriptions <span className="section-count">{channels.length}</span>
          </h3>
          <button
            type="button"
            className="btn"
            onClick={() => void pickCsv()}
            disabled={importing}
          >
            {importing ? "Importing…" : "Import Takeout CSV"}
          </button>
        </div>

        {channels.length === 0 ? (
          <p className="add-hint">No channels yet.</p>
        ) : (
          <ul className="channel-list">
            {channels.map((c) => (
              <li key={c.id} className="channel-row">
                <span className="channel-name" title={c.handle ?? c.url}>{c.title}</span>
                {confirmRemove === c.id ? (
                  <span className="confirm">
                    <span className="confirm-text">Remove and delete its videos?</span>
                    <button
                      type="button"
                      className="btn btn-danger"
                      onClick={() => void removeChannel(c)}
                    >
                      Remove
                    </button>
                    <button
                      type="button"
                      className="btn"
                      onClick={() => setConfirmRemove(null)}
                    >
                      Keep
                    </button>
                  </span>
                ) : (
                  <button
                    type="button"
                    className="btn btn-quiet"
                    onClick={() => setConfirmRemove(c.id)}
                  >
                    Remove
                  </button>
                )}
              </li>
            ))}
          </ul>
        )}
      </div>

    </div>

    {/* A sibling, not a child: nesting it inside the backdrop above meant every
        click inside it bubbled to that backdrop's onClose and shut everything. */}
    {takeout && (
      <TakeoutDialog
        rows={takeout.rows}
        importing={importing}
        onImport={(ids) => void runImport(ids)}
        onCancel={() => setTakeout(null)}
      />
    )}
    </>
  );
}
