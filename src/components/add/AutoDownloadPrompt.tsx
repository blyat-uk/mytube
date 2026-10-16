import { useEffect, useRef, useState } from "react";
import { IconAutoDownload, IconCheck } from "../Icons";
import { api, errText } from "../../api";
import type { Channel } from "../../types";

export type Backlog = { includeWatched: boolean; includeHidden: boolean };

/**
 * Asks how far back turning auto-download on should reach. "From now on" is
 * the default and queues nothing today; "Everything so far" takes what is
 * already in the library — never a fresh listing — and quotes a live count so
 * the confirm button says exactly how much it is about to start.
 */
export default function AutoDownloadPrompt({ channel, onConfirm, onCancel }: {
  channel: Channel;
  onConfirm: (backlog: Backlog | null) => void;
  onCancel: () => void;
}) {
  const [scope, setScope] = useState<"new" | "all">("new");
  const [includeWatched, setIncludeWatched] = useState(false);
  const [includeHidden, setIncludeHidden] = useState(false);
  const [count, setCount] = useState<number | null>(null);
  const [countError, setCountError] = useState<string | null>(null);
  // Every tick fires a count; only the newest one may land.
  const countReq = useRef(0);
  const boxRef = useRef<HTMLDivElement | null>(null);
  const titleId = `auto-dl-${channel.id}`;

  // It opens inside the scrolling channel list, often below the fold.
  useEffect(() => {
    boxRef.current?.scrollIntoView?.({ block: "nearest" });
  }, []);

  useEffect(() => {
    const id = ++countReq.current;
    if (scope !== "all") return;
    setCount(null);
    setCountError(null);
    api.autoDownloadBacklogCount(channel.id, includeWatched, includeHidden).then(
      (n) => { if (id === countReq.current) setCount(n); },
      (err) => { if (id === countReq.current) setCountError(errText(err)); },
    );
  }, [scope, includeWatched, includeHidden, channel.id]);

  const all = scope === "all";
  const label = all && count ? `Turn on & download ${count}` : "Turn on";
  const summary = !all
    ? "Nothing is queued today."
    : countError
      ? countError
      : count === null
        ? "Counting…"
        : count === 0
          ? "Nothing in the library left to fetch."
          : null;

  return (
    <div
      ref={boxRef}
      className="auto-dl-prompt"
      role="group"
      aria-labelledby={titleId}
      onKeyDown={(e) => {
        // Escape backs out of the prompt, not the whole dialog.
        if (e.key === "Escape") { e.stopPropagation(); onCancel(); }
      }}
    >
      <div className="auto-dl-head">
        <span className="auto-dl-badge" aria-hidden="true"><IconAutoDownload on /></span>
        <div className="auto-dl-heading">
          <p id={titleId} className="auto-dl-title">Auto-download {channel.title}</p>
          <p className="auto-dl-sub">New uploads join the download queue after every refresh.</p>
        </div>
      </div>

      <div className="auto-dl-scope" role="radiogroup" aria-label="Which videos">
        <ScopeOption
          id={`${titleId}-new`}
          name={`${titleId}-scope`}
          checked={!all}
          onSelect={() => setScope("new")}
          title="From now on"
          detail="Only what is uploaded next."
        />
        <ScopeOption
          id={`${titleId}-all`}
          name={`${titleId}-scope`}
          checked={all}
          onSelect={() => setScope("all")}
          title="Everything so far as well"
          detail="Plus what is already in your library."
        />
      </div>

      {all && (
        <div className="auto-dl-backlog">
          <span className="auto-dl-backlog-label">Also include</span>
          <ToggleChip label="Include watched" checked={includeWatched} onChange={setIncludeWatched}>
            Watched
          </ToggleChip>
          <ToggleChip label="Include hidden" checked={includeHidden} onChange={setIncludeHidden}>
            Hidden
          </ToggleChip>
        </div>
      )}

      <div className="auto-dl-foot">
        <span className={`auto-dl-count${countError ? " is-error" : ""}`} aria-live="polite">
          {summary ?? (
            <>
              <strong>{count}</strong> video{count === 1 ? "" : "s"} will be queued
            </>
          )}
        </span>
        <div className="auto-dl-buttons">
          <button
            type="button"
            className="btn btn-quiet"
            aria-label={`Cancel auto-download for ${channel.title}`}
            onClick={onCancel}
          >
            Cancel
          </button>
          <button
            type="button"
            className="btn btn-primary"
            // Waiting for the count keeps the button from starting a number the
            // user never saw.
            disabled={all && count === null && !countError}
            onClick={() => onConfirm(all ? { includeWatched, includeHidden } : null)}
          >
            {label}
          </button>
        </div>
      </div>
    </div>
  );
}

/** One of the two answers to "how far back?", as a selectable card. The input
 *  is the real control — only its title names it; the detail describes it. */
function ScopeOption({ id, name, checked, onSelect, title, detail }: {
  id: string;
  name: string;
  checked: boolean;
  onSelect: () => void;
  title: string;
  detail: string;
}) {
  return (
    <label className={`auto-dl-opt${checked ? " is-on" : ""}`}>
      <input
        type="radio"
        className="sr-only"
        name={name}
        checked={checked}
        onChange={onSelect}
        aria-labelledby={`${id}-t`}
        aria-describedby={`${id}-d`}
      />
      <span className="auto-dl-radio" aria-hidden="true" />
      <span className="auto-dl-opt-text">
        <span id={`${id}-t`} className="auto-dl-opt-title">{title}</span>
        <span id={`${id}-d`} className="auto-dl-opt-detail">{detail}</span>
      </span>
    </label>
  );
}

/** A real checkbox worn as a pill, ticked with a check glyph. */
function ToggleChip({ label, checked, onChange, children }: {
  label: string;
  checked: boolean;
  onChange: (on: boolean) => void;
  children: string;
}) {
  return (
    <label className={`auto-dl-chip${checked ? " is-on" : ""}`}>
      <input
        type="checkbox"
        className="sr-only"
        aria-label={label}
        checked={checked}
        onChange={(e) => onChange(e.currentTarget.checked)}
      />
      <span className="auto-dl-chip-box" aria-hidden="true">{checked && <IconCheck />}</span>
      {children}
    </label>
  );
}
