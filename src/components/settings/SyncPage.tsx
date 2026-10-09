import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { api, errText } from "../../api";
import { formatRelative } from "../../format";
import { usePending } from "../../pending";
import ConfirmDialog from "../ConfirmDialog";
import {
  IconBusy, IconCheck, IconComputer, IconCopy, IconHidden, IconLaptop, IconMinus, IconPlus,
  IconSectionSync, IconShield, IconShow, IconWarning,
} from "../Icons";
import { useToast } from "../Toast";
import type { SyncPeer, SyncStatus } from "../../types";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { Block, Cap, SettingsPage, StatusCard, type StatusTone } from "./parts";

/* usePending keys. Turn on, Reset and "Turn on sync again" are one command,
   so they share one key; any of the three state changes disables the others. */
const TURN_ON = "turn-on";
const JOIN = "join";
const LEAVE = "leave";
const NOW = "now";

export const EXPIRED_TEXT =
  "This sync code is no longer in use: nothing synced with it for 90 days.";
const WAIT_HINT = "Finish the check in the window that opened.";
export const SYNCED_CHANGES_TEXT = "Synced. Changes from your other computers are in.";
export const SYNCED_TEXT = "Synced. Everything is up to date.";

/** How often the relative times are redrawn while sync is on. */
const TICK = 30_000;

/** The server's cap on one group: a sixth computer is refused, so the
 *  "add a computer" card goes once there is no room left for one. */
export const MAX_COMPUTERS = 5;

/* What a snapshot carries and what each machine keeps to itself -- the
   "Cloud sync" section of CLAUDE.md, in the user's words. Change these only
   with `Db::sync_snapshot`. */
const TRAVELS = [
  "Subscriptions and memberships",
  "Watched and hidden videos",
  "Series you linked by hand",
  "Videos you added yourself",
];
const STAYS = [
  "Settings and downloaded files",
  "Auto-download choices",
  "Thumbnails (each computer fetches its own)",
];

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

/** The headline of how sync stands, and whether it is a warning. An error
 *  outranks everything: it is the one thing the user may have to act on. */
export function statusLine(s: SyncStatus, now?: number): { text: string; warn: boolean } {
  if (s.error) return { text: s.error, warn: true };
  if (s.pending) return { text: "Changes will be sent within a minute.", warn: false };
  if (s.lastSyncAt) return { text: `Synced ${formatRelative(s.lastSyncAt, now)}`, warn: false };
  return { text: "Not synced yet", warn: false };
}

/** The status card while sync is on: `statusLine`'s verdict, toned, with the
 *  line under it saying what that means. An error's sentence goes underneath,
 *  in the card's pink, under the same words the rail uses for it. */
function statusCard(s: SyncStatus, now?: number): { tone: StatusTone; title: string; detail: string | null } {
  const line = statusLine(s, now);
  if (line.warn) return { tone: "error", title: "Last sync failed", detail: line.text };
  if (s.pending) {
    return {
      tone: "busy",
      title: line.text,
      detail: s.lastSyncAt ? `Last synced ${formatRelative(s.lastSyncAt, now)}.` : null,
    };
  }
  if (s.lastSyncAt) {
    return {
      tone: "ok",
      title: line.text,
      detail: "Everything is up to date. Changes you make are sent within a minute.",
    };
  }
  return { tone: "neutral", title: line.text, detail: "Changes you make are sent within a minute." };
}

const ATTENTION = { text: "!", tone: "error", label: "Needs attention" } as const;

/**
 * "Off", "On · laptop, work", "On · no other computer yet", or the trouble.
 * No "3m ago" here, unlike the page: the rail has no clock of its own and is
 * redrawn only when something in the shell changes, so a time in it would
 * still say "just now" half an hour later.
 */
export function summary({ sync }: SettingsCtx): RailLine {
  const s = sync.status;
  if (!s) return { text: sync.loadError ? "Status unavailable" : "Checking…" };
  if (s.expired) return { text: "Sync code expired", tone: "error", flag: ATTENTION };
  if (!s.enabled) return { text: "Off" };
  if (s.error) return { text: "Last sync failed", tone: "error", flag: ATTENTION };
  return { text: s.peers.length ? `On · ${s.peers.map((p) => p.name).join(", ")}` : "On · no other computer yet" };
}

/**
 * Settings' Sync page: turn sync on or join a group, then how it stands, the
 * computers in it and the code. Everything here is read from the backend's
 * `SyncStatus`, which the shell owns (first read + `sync://status`) so the
 * rail and the page can never disagree; every button hands back a fresh one
 * through `setStatus`, so nothing is ever guessed on screen.
 */
export default function SyncPage() {
  const { sync } = useSettingsCtx();
  const { status, loadError, setStatus } = sync;
  const toast = useToast();
  const { pending, slow, run } = usePending();

  const [code, setCode] = useState("");
  const [joinError, setJoinError] = useState<string | null>(null);
  const [shown, setShown] = useState(false);
  const [copyNote, setCopyNote] = useState<string | null>(null);
  // Bumped to select the code once the field has re-rendered with it in full.
  const [selectCode, setSelectCode] = useState(0);
  const codeRef = useRef<HTMLInputElement | null>(null);
  const [confirm, setConfirm] = useState<"reset" | "leave" | null>(null);
  const [, setTick] = useState(0);
  const ids = useId();

  // A code that changes or goes by any route -- a button here, or an event
  // from another path -- takes its Show and its copy note with it.
  const liveCode = status?.code ?? null;
  const [seenCode, setSeenCode] = useState(liveCode);
  if (liveCode !== seenCode) {
    setSeenCode(liveCode);
    setShown(false);
    setCopyNote(null);
  }

  const on = !!status?.enabled && !status.expired;

  // "Synced just now" must not still say so twenty minutes later.
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setTick((n) => n + 1), TICK);
    return () => window.clearInterval(t);
  }, [on]);

  useEffect(() => {
    if (!selectCode) return;
    codeRef.current?.focus();
    codeRef.current?.select();
  }, [selectCode]);

  /** Whenever the code changes or goes, the next one starts masked, with no
   *  copy note left over from the last. Said outright after each command as
   *  well, since a reset that handed back the same string would otherwise
   *  leave it shown. */
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

  // The call waits for the sync itself, so the button is busy for exactly as
  // long as it takes. The status card has already moved by the time it
  // answers (`sync://status` goes first); the toast says what came of it, and
  // a failure is toasted by `run` as well as shown in the card.
  const syncNow = () => run(NOW, async () => {
    const changed = await api.syncNow();
    toast.success(changed ? SYNCED_CHANGES_TEXT : SYNCED_TEXT);
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
  const waitHint = waiting && <p className="st-sy-wait" role="status">{WAIT_HINT}</p>;

  let body: ReactNode = null;
  if (!status) {
    // Said in place, like Tools: a toast would be gone while the page sat empty.
    body = loadError && (
      <StatusCard tone="error" icon={<IconWarning />} title="Sync status unavailable">
        Could not read the sync status: {loadError}
      </StatusCard>
    );
  } else if (status.expired) {
    body = (
      <>
        <StatusCard
          tone="error"
          icon={<IconWarning />}
          title="Sync code expired"
          action={
            <>
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
            </>
          }
        >
          {EXPIRED_TEXT}
        </StatusCard>
        {waitHint}
      </>
    );
  } else if (status.enabled) {
    const card = statusCard(status);
    const nowSlow = slow.has(NOW);
    body = (
      <>
        <StatusCard
          tone={card.tone}
          icon={card.tone === "error" ? <IconWarning /> : undefined}
          title={card.title}
          action={
            <button
              type="button"
              className={`btn st-sy-btn${nowSlow ? " is-spinning" : ""}`}
              disabled={changing || pending.has(NOW)}
              aria-busy={pending.has(NOW)}
              onClick={() => void syncNow()}
            >
              <span className="st-sy-glyph" aria-hidden="true">
                {nowSlow ? <IconBusy /> : <IconSectionSync />}
              </span>
              {nowSlow ? "Syncing…" : "Sync now"}
            </button>
          }
        >
          {card.detail}
        </StatusCard>

        <Computers status={status} />

        {status.code && (
          <Block className="st-sy-code-block">
            <Cap
              id={`${ids}-code`}
              right={<span id={`${ids}-enter`}>Enter this code on your other computers.</span>}
            >
              Sync code
            </Cap>
            <div className="st-sy-code-row">
              <input
                ref={codeRef}
                className="text-input mono st-sy-code"
                readOnly
                value={shown ? status.code : maskCode(status.code)}
                aria-labelledby={`${ids}-code`}
                aria-describedby={`${ids}-enter ${ids}-shield`}
                spellCheck={false}
              />
              <button
                type="button"
                className="btn st-sy-btn"
                aria-pressed={shown}
                onClick={() => {
                  setShown((v) => !v);
                  setCopyNote(null);
                }}
              >
                <span className="st-sy-glyph" aria-hidden="true">
                  {shown ? <IconHidden /> : <IconShow />}
                </span>
                {shown ? "Hide" : "Show"}
              </button>
              <button type="button" className="btn st-sy-btn" onClick={() => void copy()}>
                <span className="st-sy-glyph" aria-hidden="true"><IconCopy /></span>
                Copy
              </button>
            </div>
            <p className="st-sy-shield" id={`${ids}-shield`}>
              <span className="st-sy-glyph" aria-hidden="true"><IconShield /></span>
              Anyone with this code can see and change your synced library. Treat it like a password.
            </p>
            {copyNote && <p className="st-sy-note" role="status">{copyNote}</p>}
          </Block>
        )}

        <WhatSyncs />

        <div className="st-sy-foot">
          <button
            type="button"
            className="btn btn-quiet"
            disabled={changing}
            aria-busy={pending.has(TURN_ON)}
            onClick={() => setConfirm("reset")}
          >
            {waiting ? "Waiting…" : "Reset code…"}
          </button>
          <button
            type="button"
            className="btn btn-quiet st-sy-leave"
            disabled={changing}
            aria-busy={pending.has(LEAVE)}
            onClick={() => setConfirm("leave")}
          >
            Turn off sync…
          </button>
        </div>
        {/* Under the button that is waiting on it, at the foot. */}
        {waiting && <p className="st-sy-wait is-end" role="status">{WAIT_HINT}</p>}
      </>
    );
  } else {
    body = (
      <>
        <div className="st-sy-hero">
          <SyncArt />
          <div className="st-sy-hero-text">
            <h3 className="st-sy-hero-title">Keep your computers in step</h3>
            <p className="st-sy-hero-intro">
              Keep subscriptions and watched, hidden and hand-linked videos the same on all your
              computers. Everything is encrypted on this computer first, and no account is needed.
            </p>
            <div className="st-sy-hero-actions">
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
            {waitHint}
          </div>
        </div>

        <WhatSyncs />

        <Block className="st-sy-join">
          <form onSubmit={(e) => { e.preventDefault(); join(); }}>
            <label className="st-sy-join-label" htmlFor={`${ids}-join`}>
              Have a code from another computer?
            </label>
            <div className="st-sy-join-row">
              <input
                id={`${ids}-join`}
                className="text-input mono st-sy-join-input"
                value={code}
                placeholder="MYT1-…"
                autoComplete="off"
                spellCheck={false}
                aria-invalid={joinError ? true : undefined}
                aria-describedby={joinError ? `${ids}-join-error` : undefined}
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
              <p className="st-sy-join-error" id={`${ids}-join-error`} role="alert">{joinError}</p>
            )}
          </form>
        </Block>
      </>
    );
  }

  return (
    <SettingsPage
      icon={<IconSectionSync />}
      title="Sync"
      intro="Your subscriptions and what you have watched, the same on every computer. Encrypted on this one before it leaves; no account."
      className="st-sy"
    >
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
    </SettingsPage>
  );
}

/**
 * This computer, each other one, and -- while the group has room -- how to add
 * one more. A list, so a screen reader hears how many there are; each card
 * says when that computer last did something, which is the one thing that
 * tells a live machine from one that has gone quiet.
 */
function Computers({ status }: { status: SyncStatus }) {
  const capId = useId();
  const count = status.peers.length + 1;
  return (
    <Block className="st-sy-computers">
      <Cap id={capId} right={`${count} of ${MAX_COMPUTERS}`}>Computers</Cap>
      <ul className="st-sy-devs" aria-labelledby={capId}>
        <li className="st-sy-dev is-here">
          <span className="st-sy-dev-glyph" aria-hidden="true"><IconComputer /></span>
          <span className="st-sy-dev-name">
            <span className="st-sy-dev-label">{status.machineName}</span>
            <span className="st-sy-dev-tag">This one</span>
          </span>
          <span className="st-sy-dev-meta">
            {status.lastSyncAt ? `Synced ${formatRelative(status.lastSyncAt)}` : "Not synced yet"}
          </span>
        </li>
        {status.peers.map((p, i) => (
          // Names are what the user typed on each machine and need not be
          // unique, so the position settles the key.
          <li key={`${i}-${p.name}`} className="st-sy-dev">
            <span className="st-sy-dev-glyph" aria-hidden="true"><IconComputer /></span>
            <span className="st-sy-dev-name">
              <span className="st-sy-dev-label">{p.name}</span>
            </span>
            <span className="st-sy-dev-meta">
              {p.lastWriteAt ? `Last change ${formatRelative(p.lastWriteAt)}` : "No changes yet"}
            </span>
          </li>
        ))}
        {count < MAX_COMPUTERS && (
          <li className="st-sy-dev is-add">
            <span className="st-sy-dev-glyph" aria-hidden="true"><IconPlus /></span>
            {status.peers.length === 0 && (
              <span className="st-sy-dev-none">No other computer has joined yet.</span>
            )}
            <span>Add a computer: enter the code below on it</span>
          </li>
        )}
      </ul>
    </Block>
  );
}

/** What travels and what stays: the question every first sync raises, and
 *  the one the old section never answered. */
function WhatSyncs() {
  const id = useId();
  return (
    <Block className="st-sy-what">
      <div className="st-sy-what-col">
        <Cap id={`${id}-travels`}>Travels between computers</Cap>
        <ul className="st-sy-what-list" aria-labelledby={`${id}-travels`}>
          {TRAVELS.map((t) => (
            <li key={t}><span className="st-sy-glyph" aria-hidden="true"><IconCheck /></span>{t}</li>
          ))}
        </ul>
      </div>
      <div className="st-sy-what-col is-stays">
        <Cap id={`${id}-stays`}>Stays on this computer</Cap>
        <ul className="st-sy-what-list" aria-labelledby={`${id}-stays`}>
          {STAYS.map((t) => (
            <li key={t}><span className="st-sy-glyph" aria-hidden="true"><IconMinus /></span>{t}</li>
          ))}
        </ul>
      </div>
    </Block>
  );
}

/** Two computers and the encrypted line between them. Decoration only. */
function SyncArt() {
  return (
    <div className="st-sy-art" aria-hidden="true">
      <span className="st-sy-art-dev is-here"><IconComputer /></span>
      <span className="st-sy-art-link">
        <span className="st-sy-art-lock"><IconShield /></span>
      </span>
      <span className="st-sy-art-dev"><IconLaptop /></span>
    </div>
  );
}
