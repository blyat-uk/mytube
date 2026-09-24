import { useEffect, useState } from "react";
import { useToast } from "./Toast";
import { useToolsProgress, useToolsStatus } from "../events";
import { api, errText } from "../api";
import { formatBytes, formatRelative } from "../format";
import type { ToolKind, ToolProgress, ToolSource, ToolState, ToolStatus } from "../types";

/** The names a person reads — the same as `ToolKind::label()` on the Rust side. */
export const TOOL_LABEL: Record<ToolKind, string> = {
  ytdlp: "yt-dlp",
  ffmpeg: "ffmpeg",
  deno: "deno",
};

const SOURCE_WORDS: Record<ToolSource, string> = {
  managed: "Managed by MyTube",
  system: "System",
  override: "Custom path (settings.json)",
  missing: "Not installed",
};

const inFlight = (s: ToolState) => s === "installing" || s === "updating";

/**
 * The shell's one toast for provisioning: which tools just went from
 * installing to ready, as a sentence, or null when none did. An update is left
 * out on purpose — it changes nothing the user can do, whereas a first install
 * is the moment downloads start working.
 */
export function toolsReadyMessage(
  prev: ReadonlyMap<ToolKind, ToolState>,
  next: ToolStatus[],
): string | null {
  const ready = next
    .filter((t) => t.state === "ready" && prev.get(t.kind) === "installing")
    .map((t) => TOOL_LABEL[t.kind]);
  if (ready.length === 0) return null;
  if (ready.length === 1) return `${ready[0]} is ready.`;
  return `${ready.slice(0, -1).join(", ")} and ${ready[ready.length - 1]} are ready.`;
}

interface Props {
  channel: string;
  autoUpdate: boolean;
  onChannel: (channel: string) => void;
  onAutoUpdate: (on: boolean) => void;
}

/**
 * Where yt-dlp, ffmpeg and deno come from, and the one place a failed install
 * is visible. Provisioning itself is silent — it runs at startup with no dialog
 * — so this section is read, not driven; its two buttons exist for the cases
 * the background loop would otherwise only get round to on the next tick.
 */
export default function ToolsSection({ channel, autoUpdate, onChannel, onAutoUpdate }: Props) {
  const toast = useToast();
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [progress, setProgress] = useState<Partial<Record<ToolKind, ToolProgress>>>({});
  const [checking, setChecking] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api.toolsStatus()
      .then((list) => { if (alive) setTools(list); })
      // Said in place rather than toasted: the section would otherwise sit on
      // "Checking…" forever with the reason already gone from the screen.
      .catch((err) => { if (alive) setLoadError(errText(err)); });
    return () => { alive = false; };
  }, []);

  const apply = (list: ToolStatus[]) => {
    setTools(list);
    setLoadError(null);
    // A finished download's last byte count would otherwise sit under the row
    // and reappear, stale, the next time that tool installs.
    setProgress((prev) => {
      const next = { ...prev };
      for (const t of list) if (!inFlight(t.state)) delete next[t.kind];
      return next;
    });
  };

  useToolsStatus(apply);
  useToolsProgress((p) => setProgress((prev) => ({ ...prev, [p.tool]: p })));

  const anyError = tools?.some((t) => t.state === "error") ?? false;
  const anyBusy = tools?.some((t) => inFlight(t.state)) ?? false;
  const ytdlp = tools?.find((t) => t.kind === "ytdlp");

  async function updateNow(announce: boolean) {
    setChecking(true);
    const before = ytdlp?.version ?? null;
    // A yt-dlp that was not already MyTube's own is not updated by this call
    // but installed, and the shell says "yt-dlp is ready." for that — a second
    // "updated to" toast on top would announce one event twice.
    const wasManaged = ytdlp?.source === "managed";
    try {
      const list = await api.toolsUpdateNow();
      apply(list);
      // Only a managed yt-dlp is ever updated, so only then does "up to date"
      // mean something; a Retry speaks through its rows instead.
      const after = list.find((t) => t.kind === "ytdlp");
      if (announce && wasManaged && after?.source === "managed" && after.state === "ready") {
        if (after.version && after.version !== before) {
          toast.success(`yt-dlp updated to ${after.version}.`);
        } else {
          toast.info("yt-dlp is up to date.");
        }
      }
    } catch (err) {
      toast.error(errText(err));
    } finally {
      setChecking(false);
    }
  }

  return (
    <section className="settings-block" aria-labelledby="tools-title">
      <h3 className="field-label" id="tools-title">Tools</h3>
      <p className="field-hint">
        MyTube fetches whatever is missing and keeps its own yt-dlp current. A path set by
        hand in <code>settings.json</code> always wins.
      </p>

      {tools === null ? (
        loadError
          ? <p className="tool-error">Could not read the tools' status: {loadError}</p>
          : <div className="tools-loading"><span className="spinner" aria-hidden="true" />Checking…</div>
      ) : (
        <ul className="tool-list">
          {tools.map((t) => (
            <ToolRow key={t.kind} tool={t} progress={progress[t.kind]} />
          ))}
        </ul>
      )}

      <div className="row-inline tools-actions">
        <button
          type="button"
          className="btn"
          disabled={checking || anyBusy || tools === null}
          onClick={() => void updateNow(true)}
        >
          {checking ? "Checking…" : "Check for updates"}
        </button>
        {anyError && (
          <button
            type="button"
            className="btn btn-primary"
            disabled={checking || anyBusy}
            onClick={() => void updateNow(false)}
          >
            Retry
          </button>
        )}
        {ytdlp?.lastCheck ? (
          <span className="tools-checked">Last checked {formatRelative(ytdlp.lastCheck)}</span>
        ) : null}
      </div>

      <div className="tools-prefs">
        <div className="field">
          <label className="field-label" htmlFor="ytdlp-channel">yt-dlp channel</label>
          <select
            id="ytdlp-channel"
            className="select settings-select"
            value={channel === "stable" ? "stable" : "nightly"}
            aria-describedby="ytdlp-channel-hint"
            onChange={(e) => onChannel(e.currentTarget.value)}
          >
            <option value="nightly">Nightly (recommended)</option>
            <option value="stable">Stable</option>
          </select>
          <div className="field-hint" id="ytdlp-channel-hint">
            YouTube breaks things often, and the fixes reach nightly first.
          </div>
        </div>

        <label className="switch-row">
          <input
            type="checkbox"
            checked={autoUpdate}
            onChange={(e) => onAutoUpdate(e.currentTarget.checked)}
          />
          <span>
            <span className="switch-label">Update yt-dlp automatically</span>
            <span className="field-hint">
              Checks once a day, and never swaps it while a download is running.
            </span>
          </span>
        </label>
      </div>
    </section>
  );
}

function ToolRow({ tool, progress }: { tool: ToolStatus; progress?: ToolProgress }) {
  const busy = inFlight(tool.state);
  const pct = progress?.total ? Math.min(100, (progress.received / progress.total) * 100) : null;

  let status = "Starting…";
  if (progress?.phase === "extract") status = "Unpacking…";
  else if (progress) {
    status = progress.total
      ? `${formatBytes(progress.received)} of ${formatBytes(progress.total)}`
      : formatBytes(progress.received);
  }

  return (
    <li className={`tool-row${tool.state === "error" ? " is-error" : ""}`}>
      <div className="tool-head">
        <span className="tool-name">{TOOL_LABEL[tool.kind]}</span>
        {tool.version && <span className="tool-version">{tool.version}</span>}
        <span className={`tool-source is-${tool.source}`}>{SOURCE_WORDS[tool.source]}</span>
      </div>
      {tool.path && <div className="tool-path" title={tool.path}>{tool.path}</div>}
      {busy && (
        <div className="tool-progress">
          <div
            className={`progress-track${pct === null ? " is-indeterminate" : ""}`}
            role="progressbar"
            aria-label={`${tool.state === "updating" ? "Updating" : "Installing"} ${TOOL_LABEL[tool.kind]}`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={pct === null ? undefined : Math.round(pct)}
          >
            <div className="progress-fill" style={{ width: pct === null ? undefined : `${pct}%` }} />
          </div>
          <span className="tool-status">
            {tool.state === "updating" ? "Updating" : "Installing"} · {status}
          </span>
        </div>
      )}
      {/* Only a broken tool gets the alert. A working one can still carry an
          error — a managed yt-dlp whose update check failed stays usable at the
          version it has — and that is a note, not a reason to reach for Retry. */}
      {tool.error && (tool.state === "error"
        ? <p className="tool-error">{tool.error}</p>
        : <p className="tool-note">{tool.error}</p>)}
    </li>
  );
}
