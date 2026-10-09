import { useEffect, useRef, useState, type ReactNode } from "react";
import { api, errText } from "../api";
import { useSyncStatus } from "../events";
import { formatRelative } from "../format";
import { usePending } from "../pending";
import ConfirmDialog from "./ConfirmDialog";
import { useToast } from "./Toast";
import type { SyncPeer, SyncStatus } from "../types";

/* usePending keys. Turn on, Reset and "Turn on sync again" are one command,
   so they share one key; any of the three state changes disables the others. */
const TURN_ON = "turn-on";
const JOIN = "join";
const LEAVE = "leave";
const NOW = "now";

export const EXPIRED_TEXT =
  "This sync code is no longer in use: nothing synced with it for 90 days.";
const WAIT_HINT = "Finish the check in the window that opened.";

/** How long "Syncing…" may stand in for a status that never came. The backend
 *  sends at most one request a minute, so a nudge is answered well inside it. */
const NUDGE_TIMEOUT = 90_000;
/** How often the relative times are redrawn while sync is on. */
const TICK = 30_000;

/**
 * The code with everything after its version prefix blotted out, one dot per
 * character, so Show swaps the text without the field changing width.
 */
export function maskCode(code: string): string {
  const dash = code.indexOf("-");
  const [head, tail] = dash < 0 ? ["", code] : [code.slice(0, dash + 1), code.slice(dash + 1)];
  return head + tail.replace(/[^-]/g, "•");
}

/** "laptop · 3m ago, work · just now", or the sentence for having none. */
export function peersLine(peers: SyncPeer[], now?: number): string {
  if (peers.length === 0) return "No other computer has joined yet.";
  const each = peers.map((p) =>
    p.lastWriteAt ? `${p.name} · ${formatRelative(p.lastWriteAt, now)}` : p.name);
  return `Also syncing: ${each.join(", ")}`;
}

/** The status line under the machines, and whether it is a warning. An error
 *  outranks everything: it is the one thing the user may have to act on. */
export function statusLine(s: SyncStatus, now?: number): { text: string; warn: boolean } {
  if (s.error) return { text: s.error, warn: true };
  if (s.pending) return { text: "Changes will be sent within a minute.", warn: false };
  if (s.lastSyncAt) return { text: `Synced ${formatRelative(s.lastSyncAt, now)}`, warn: false };
  return { text: "Not synced yet", warn: false };
}

/**
 * Settings' Sync block: turn sync on or join a group, then the code, the other
 * computers and how the last attempt went. Everything here is read from the
 * backend's `SyncStatus` -- once at mount, then from `sync://status` -- and
 * every button hands back a fresh one, so nothing is ever guessed on screen.
 */
export default function SyncSection() {
  const toast = useToast();
  const { pending, slow, run } = usePending();
  const [status, setStatus] = useState<SyncStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // An event that lands while the first read is out is the newer of the two.
  const heard = useRef(false);

  const [code, setCode] = useState("");
  const [joinError, setJoinError] = useState<string | null>(null);
  const [shown, setShown] = useState(false);
  const [copyNote, setCopyNote] = useState<string | null>(null);
  // Bumped to select the code once the field has re-rendered with it in full.
  const [selectCode, setSelectCode] = useState(0);
  const codeRef = useRef<HTMLInputElement | null>(null);
  const [confirm, setConfirm] = useState<"reset" | "leave" | null>(null);
  // Sync now was pressed and no status has answered it yet.
  const [nudged, setNudged] = useState(false);
  const [, setTick] = useState(0);

  useEffect(() => {
    let alive = true;
    // Through `Promise.resolve()` so a missing command rejects rather than
    // throwing out of the effect.
    Promise.resolve()
      .then(() => api.syncStatus())
      .then((s) => { if (alive && !heard.current) setStatus(s); })
      // Said in place, like the Tools section: a toast would be gone while the
      // block sat empty.
      .catch((err) => { if (alive && !heard.current) setLoadError(errText(err)); });
    return () => { alive = false; };
  }, []);

  useSyncStatus((s) => {
    heard.current = true;
    setStatus(s);
    setLoadError(null);
    setNudged(false);
  });

  const on = !!status?.enabled && !status.expired;

  // "Synced just now" must not still say so twenty minutes later.
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setTick((n) => n + 1), TICK);
    return () => window.clearInterval(t);
  }, [on]);

  useEffect(() => {
    if (!nudged) return;
    const t = window.setTimeout(() => setNudged(false), NUDGE_TIMEOUT);
    return () => window.clearTimeout(t);
  }, [nudged]);

  useEffect(() => {
    if (!selectCode) return;
    codeRef.current?.focus();
    codeRef.current?.select();
  }, [selectCode]);

  /** Whenever the code changes or goes, the next one starts masked, with no
   *  copy note left over from the last. */
  const forgetCode = () => {
    setShown(false);
    setCopyNote(null);
  };

  const turnOn = (reset: boolean) => run(TURN_ON, async () => {
    const next = await api.syncTurnOn();
    setStatus(next);
    forgetCode();
    // The masked code looks the same before and after, so a reset would
    // otherwise succeed in silence.
    if (reset) toast.success("New sync code created. Enter it on your other computers.");
  });

  const join = () => {
    const text = code.trim();
    if (!text) return;
    setJoinError(null);
    void run(JOIN, async () => {
      try {
        setStatus(await api.syncJoin(text));
        setCode("");
        forgetCode();
      } catch (err) {
        // Shown under the field rather than toasted: the fix is to the code in
        // it, and the reason should still be there while that is done.
        setJoinError(errText(err));
      }
    });
  };

  const leave = () => run(LEAVE, async () => {
    setStatus(await api.syncLeave());
    forgetCode();
  });

  // "Syncing…" goes up before the call rather than after it: the status that
  // answers the nudge could otherwise land first and be followed by a
  // "Syncing…" that nothing would ever clear but the timeout.
  const syncNow = () => run(NOW, () => api.syncNow(), {
    optimistic: () => {
      setNudged(true);
      return () => setNudged(false);
    },
  });

  async function copy() {
    const value = status?.code;
    if (!value) return;
    try {
      await navigator.clipboard.writeText(value);
      setCopyNote(null);
      toast.success("Sync code copied.");
    } catch {
      // No clipboard (or no permission for it): put the code where the user
      // can take it by hand instead.
      setShown(true);
      setCopyNote("Copying didn’t work, so the code is shown and selected. Copy it from there.");
      setSelectCode((n) => n + 1);
    }
  }

  const changing = pending.has(TURN_ON) || pending.has(JOIN) || pending.has(LEAVE);
  const waiting = slow.has(TURN_ON);

  let body: ReactNode = null;
  if (!status) {
    body = loadError
      ? <p className="tool-error">Could not read the sync status: {loadError}</p>
      : null;
  } else if (status.expired) {
    body = (
      <>
        <p className="sync-status is-warn">{EXPIRED_TEXT}</p>
        <div className="row-inline sync-actions">
          <button
            type="button"
            className="btn btn-primary"
            disabled={changing}
            aria-busy={pending.has(TURN_ON)}
            onClick={() => void turnOn(false)}
          >
            {waiting ? "Waiting…" : "Turn on sync again"}
          </button>
          <button
            type="button"
            className="btn"
            disabled={changing}
            aria-busy={pending.has(LEAVE)}
            onClick={() => void leave()}
          >
            Turn off sync
          </button>
        </div>
        {waiting && <p className="field-hint sync-wait" role="status">{WAIT_HINT}</p>}
      </>
    );
  } else if (status.enabled) {
    const line = nudged ? { text: "Syncing…", warn: false } : statusLine(status);
    body = (
      <>
        {status.code && (
          <div className="sync-code-block">
            <label className="sync-code-label" htmlFor="sync-code">Sync code</label>
            <div className="row-inline sync-code-row">
              <input
                id="sync-code"
                ref={codeRef}
                className="text-input mono sync-code"
                readOnly
                value={shown ? status.code : maskCode(status.code)}
                aria-describedby="sync-code-hint"
                spellCheck={false}
              />
              <button
                type="button"
                className="btn"
                aria-pressed={shown}
                onClick={() => {
                  setShown((v) => !v);
                  setCopyNote(null);
                }}
              >
                {shown ? "Hide" : "Show"}
              </button>
              <button type="button" className="btn" onClick={() => void copy()}>
                Copy
              </button>
            </div>
            <p className="field-hint" id="sync-code-hint">
              Enter this code on your other computers. Anyone with it can see and change your
              synced library.
            </p>
            {copyNote && <p className="field-hint sync-copy-note" role="status">{copyNote}</p>}
          </div>
        )}

        <div className="sync-machines">
          <p>This computer: <span className="sync-machine">{status.machineName}</span></p>
          <p>{peersLine(status.peers)}</p>
        </div>

        <p
          className={`sync-status${line.warn ? " is-warn" : ""}`}
          role="status"
          aria-live="polite"
        >
          {line.text}
        </p>

        <div className="row-inline sync-actions">
          <button
            type="button"
            className="btn"
            disabled={changing || pending.has(NOW)}
            onClick={() => void syncNow()}
          >
            Sync now
          </button>
          <button
            type="button"
            className="btn"
            disabled={changing}
            aria-busy={pending.has(TURN_ON)}
            onClick={() => setConfirm("reset")}
          >
            {waiting ? "Waiting…" : "Reset code…"}
          </button>
          <button
            type="button"
            className="btn"
            disabled={changing}
            aria-busy={pending.has(LEAVE)}
            onClick={() => setConfirm("leave")}
          >
            Turn off sync…
          </button>
        </div>
        {waiting && <p className="field-hint sync-wait" role="status">{WAIT_HINT}</p>}
      </>
    );
  } else {
    body = (
      <>
        <p className="field-hint">
          Keep subscriptions and watched, hidden and hand-linked videos the same on all your
          computers. Everything is encrypted on this computer first, and no account is needed.
        </p>
        <div className="row-inline sync-actions">
          <button
            type="button"
            className="btn btn-primary"
            disabled={changing}
            aria-busy={pending.has(TURN_ON)}
            onClick={() => void turnOn(false)}
          >
            {waiting ? "Waiting…" : "Turn on sync"}
          </button>
        </div>
        {waiting && <p className="field-hint sync-wait" role="status">{WAIT_HINT}</p>}

        <form
          className="sync-join"
          onSubmit={(e) => { e.preventDefault(); join(); }}
        >
          <label className="sync-join-label" htmlFor="sync-join-code">
            Have a code from another computer?
          </label>
          <div className="row-inline">
            <input
              id="sync-join-code"
              className="text-input mono"
              value={code}
              placeholder="MYT1-…"
              autoComplete="off"
              spellCheck={false}
              aria-invalid={joinError ? true : undefined}
              aria-describedby={joinError ? "sync-join-error" : undefined}
              onChange={(e) => {
                setCode(e.currentTarget.value);
                setJoinError(null);
              }}
            />
            <button
              type="submit"
              className="btn"
              disabled={!code.trim() || changing}
              aria-busy={pending.has(JOIN)}
            >
              {slow.has(JOIN) ? "Joining…" : "Join"}
            </button>
          </div>
          {joinError && (
            <p className="sync-join-error" id="sync-join-error" role="alert">{joinError}</p>
          )}
        </form>
      </>
    );
  }

  return (
    <section className="settings-block sync-block" aria-labelledby="sync-title">
      <h3 className="field-label" id="sync-title">Sync</h3>
      {body}

      {confirm === "reset" && (
        <ConfirmDialog
          title="Create a new sync code?"
          body={
            "Your other computers stop syncing with this one until you enter the new code on " +
            "them. Nothing in your library is deleted."
          }
          choices={[{ label: "Create new code", value: "reset", primary: true }]}
          onCancel={() => setConfirm(null)}
          onChoose={() => {
            setConfirm(null);
            void turnOn(true);
          }}
        />
      )}
      {confirm === "leave" && (
        <ConfirmDialog
          title="Turn off sync on this computer?"
          body={
            "It forgets the sync code and stops syncing. Your library stays exactly as it is, " +
            "and your other computers keep syncing with each other."
          }
          choices={[{ label: "Turn off sync", value: "leave", danger: true }]}
          onCancel={() => setConfirm(null)}
          onChoose={() => {
            setConfirm(null);
            void leave();
          }}
        />
      )}
    </section>
  );
}
