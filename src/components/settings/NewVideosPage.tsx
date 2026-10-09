import {
  useCallback, useEffect, useId, useRef, useState, type CSSProperties, type KeyboardEvent,
} from "react";
import { api } from "../../api";
import { usePollEvents } from "../../events";
import { IconSectionNewVideos } from "../Icons";
import type { Channel } from "../../types";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { LIMITS, clamp } from "./doc";
import { Block, Cap, Segmented, SettingsPage, Stepper, Switch, type SegOption } from "./parts";

/** "30 min", "2 h", "90 min". */
export function intervalText(minutes: number): string {
  return minutes >= 60 && minutes % 60 === 0 ? `${minutes / 60} h` : `${minutes} min`;
}

/** "Every 30 min · 30 back". */
export function summary({ settings: s }: SettingsCtx): RailLine {
  return { text: `Every ${intervalText(s.poll_interval_minutes)} · ${s.backfill_count} back` };
}

/* --- check every ------------------------------------------------------------- */

/** The intervals offered as one click, in minutes. Anything else is Custom. */
export const INTERVAL_PRESETS = [15, 30, 60, 180, 360, 720] as const;

const CUSTOM = "custom";
type IntervalPick = number | typeof CUSTOM;

const presetLabel = (m: number) => (m < 60 ? `${m} min` : m === 60 ? "1 hour" : `${m / 60} hours`);

const INTERVAL_OPTIONS: SegOption<IntervalPick>[] = [
  ...INTERVAL_PRESETS.map((m) => ({ value: m, label: presetLabel(m) })),
  { value: CUSTOM, label: "Custom…" },
];

const isPreset = (m: number) => (INTERVAL_PRESETS as readonly number[]).includes(m);

/** The newest poll across the channels, in unix seconds; null when none of
 *  them has been polled yet. */
export function newestPoll(channels: Channel[]): number | null {
  let newest: number | null = null;
  for (const c of channels) {
    if (c.last_polled_at && (newest === null || c.last_polled_at > newest)) newest = c.last_polled_at;
  }
  return newest;
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

/**
 * "Last checked 18 minutes ago". Spelled out rather than `formatRelative`'s
 * "18m ago": this is a sentence under a control, not a column in a list, and
 * "m" there reads as either minutes or months.
 */
export function lastCheckedText(ts: number, now = Date.now() / 1000): string {
  const d = Math.max(0, now - ts);
  if (d < 60) return "Last checked just now";
  if (d < 3600) return `Last checked ${plural(Math.floor(d / 60), "minute")} ago`;
  if (d < 86400) return `Last checked ${plural(Math.floor(d / 3600), "hour")} ago`;
  return `Last checked ${plural(Math.floor(d / 86400), "day")} ago`;
}

/**
 * When the library was last polled: read once on open and again after every
 * poll. Null while the read is out, when nothing was ever polled, and when the
 * read fails -- the line is a reassurance, and a wrong one is worse than none.
 */
function useLastPolled(): number | null {
  const [last, setLast] = useState<number | null>(null);
  // A poll can finish while the first read is still out; only the newest
  // request may answer, or a slow first read would overwrite a fresh one.
  const seq = useRef(0);

  const read = useCallback(() => {
    const mine = ++seq.current;
    api.listChannels()
      .then((list) => { if (seq.current === mine) setLast(newestPoll(list)); })
      .catch(() => { if (seq.current === mine) setLast(null); });
  }, []);

  useEffect(() => {
    read();
    // Unmounting bumps the counter, so an answer landing afterwards is dropped.
    return () => { seq.current += 1; };
  }, [read]);

  usePollEvents({ onFinished: read });
  return last;
}

/** The clock "Last checked" reads, a minute at a time, so the line keeps
 *  telling the truth while the page sits open. */
function useMinuteClock(): number {
  const [now, setNow] = useState(() => Date.now() / 1000);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now() / 1000), 60_000);
    return () => window.clearInterval(t);
  }, []);
  return now;
}

/* --- how far back -------------------------------------------------------------- */

const DEPTH = LIMITS.backfill_count;

/** Positions on the native range: enough that every count from 1 to 500 has
 *  one of its own, even at the top where the scale is steepest. */
export const SLIDER_STEPS = 1000;

/** Where the scale is labelled. */
export const DEPTH_TICKS = [1, 30, 100, 250, 500] as const;

/**
 * Slider position (0–1) → videos per channel, on a square-root scale:
 * n = 1 + 499·p². Most people want somewhere between 10 and 100, and on a
 * linear 1–500 track that whole range would be a sliver at the left end; here
 * it takes up the first half.
 */
export function depthAt(p: number): number {
  const q = Math.min(1, Math.max(0, p));
  return Math.round(DEPTH.min + (DEPTH.max - DEPTH.min) * q * q);
}

/** Videos per channel → slider position (0–1); the inverse of `depthAt`. */
export function depthPosition(n: number): number {
  const c = Math.min(DEPTH.max, Math.max(DEPTH.min, n));
  return Math.sqrt((c - DEPTH.min) / (DEPTH.max - DEPTH.min));
}

/**
 * What a key does to the count. The native range would step its *position*,
 * and near the low end one position is a fraction of a video -- an arrow key
 * could be pressed forty times before 1 became 2. Null for a key it ignores.
 */
export function depthAfterKey(n: number, key: string): number | null {
  const to = (v: number) => clamp(v, DEPTH);
  switch (key) {
    case "ArrowUp": case "ArrowRight": return to(n + 1);
    case "ArrowDown": case "ArrowLeft": return to(n - 1);
    case "PageUp": return to(n + 10);
    case "PageDown": return to(n - 10);
    case "Home": return DEPTH.min;
    case "End": return DEPTH.max;
    default: return null;
  }
}

/**
 * A native range, invisible, over a drawn track: the browser does the dragging,
 * clicking and touch, and the picture is the mockup's. `onInput` follows the
 * thumb; `onRelease` fires once it is let go -- a drag saves once, not on every
 * pixel of the way.
 */
function DepthSlider({ value, labelledBy, onInput, onRelease }: {
  value: number;
  labelledBy: string;
  onInput: (n: number) => void;
  onRelease: (n: number) => void;
}) {
  const ref = useRef<HTMLInputElement | null>(null);
  const release = useRef(onRelease);
  release.current = onRelease;
  // Whether the thumb moved since the last release. A browser reports letting
  // go several ways at once (pointerup, then change, then perhaps blur), and a
  // save's write lands only after all of them have fired -- so each would
  // write again. One drag, one save.
  const moved = useRef(false);

  // Read from the element, not from `value`: a release can arrive in the same
  // event as the last move, before React has rendered it.
  const releaseNow = useCallback(() => {
    const el = ref.current;
    if (!el || !moved.current) return;
    moved.current = false;
    release.current(depthAt(Number(el.value) / SLIDER_STEPS));
  }, []);

  const move = (n: number) => {
    moved.current = true;
    onInput(n);
  };

  // React's onChange on a range is the `input` event, every step of a drag.
  // The native `change` is the one a browser fires once, on letting go --
  // wherever the pointer is by then, which pointerup on the input is not.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.addEventListener("change", releaseNow);
    return () => el.removeEventListener("change", releaseNow);
  }, [releaseNow]);

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    const next = depthAfterKey(value, e.key);
    if (next === null) return;
    e.preventDefault();
    if (next !== value) move(next);
  }

  const style = { "--p": depthPosition(value) } as CSSProperties;
  return (
    <div className="st-nv-slider" style={style}>
      <input
        ref={ref}
        type="range"
        className="st-nv-range"
        min={0}
        max={SLIDER_STEPS}
        step={1}
        value={Math.round(depthPosition(value) * SLIDER_STEPS)}
        aria-labelledby={labelledBy}
        aria-valuetext={plural(value, "video")}
        onChange={(e) => move(depthAt(Number(e.currentTarget.value) / SLIDER_STEPS))}
        onKeyDown={onKeyDown}
        onKeyUp={releaseNow}
        onPointerUp={releaseNow}
        onBlur={releaseNow}
      />
      <span className="st-nv-track" aria-hidden="true" />
      <span className="st-nv-fill" aria-hidden="true" />
      <span className="st-nv-thumb" aria-hidden="true" />
    </div>
  );
}

/* --- the page ------------------------------------------------------------------- */

export default function NewVideosPage() {
  const { settings: s, set, save, flush } = useSettingsCtx();
  const intervalCap = useId();
  const depthCap = useId();
  const minutesId = useId();
  const lastPolled = useLastPolled();
  const now = useMinuteClock();

  // Custom stays open once picked, even while the minutes typed happen to
  // match a preset: the field must not vanish under the cursor mid-edit.
  const [customOpen, setCustomOpen] = useState(false);
  const minutes = s.poll_interval_minutes;
  const pick: IntervalPick = customOpen || !isPreset(minutes) ? CUSTOM : minutes;

  // The minutes typed, held until blur or Enter. A ref mirrors it so the
  // commit never reads a value one render old.
  const [draft, setDraftState] = useState<string | null>(null);
  const draftRef = useRef<string | null>(null);
  const setDraft = (v: string | null) => {
    draftRef.current = v;
    setDraftState(v);
  };

  function pickInterval(v: IntervalPick) {
    if (v === CUSTOM) {
      setCustomOpen(true);
      return;
    }
    setCustomOpen(false);
    setDraft(null);
    save("poll_interval_minutes", v);
  }

  /** Clamped the way Rust clamps it, so what is shown is what is stored; a
   *  field left unparsable takes the minimum, as the old number box did. */
  function commitMinutes() {
    const raw = draftRef.current;
    if (raw === null) {
      flush();
      return;
    }
    setDraft(null);
    const parsed = Number.parseInt(raw, 10);
    const lim = LIMITS.poll_interval_minutes;
    save("poll_interval_minutes", Number.isFinite(parsed) ? clamp(parsed, lim) : lim.min);
  }

  const depth = s.backfill_count;
  // Two minutes of slack: a poll is stamped when it ends, not when it starts.
  const fresh = lastPolled !== null && now - lastPolled <= minutes * 60 * 2 + 120;

  return (
    <SettingsPage
      icon={<IconSectionNewVideos />}
      title="New videos"
      intro="How often MyTube looks for uploads on the channels you follow, and how far back it reads each one."
      className="st-nv"
    >
      <Block>
        <Cap id={intervalCap}>Check every</Cap>
        <Segmented
          labelledBy={intervalCap}
          options={INTERVAL_OPTIONS}
          value={pick}
          onChange={pickInterval}
          size="big"
          wide
        />

        {pick === CUSTOM && (
          <div className="st-nv-custom">
            <label className="st-nv-custom-label" htmlFor={minutesId}>Minutes between checks</label>
            <input
              id={minutesId}
              className="text-input st-nv-minutes"
              type="number"
              inputMode="numeric"
              min={LIMITS.poll_interval_minutes.min}
              max={LIMITS.poll_interval_minutes.max}
              value={draft ?? String(minutes)}
              onChange={(e) => setDraft(e.currentTarget.value)}
              onBlur={commitMinutes}
              onKeyDown={(e) => { if (e.key === "Enter") commitMinutes(); }}
            />
            <span className="st-nv-custom-hint">1 – 1440, up to a day</span>
          </div>
        )}

        {lastPolled !== null && (
          <p className="st-nv-last">
            {/* Grey once the last check is well past the interval: MyTube was
                closed, or polls are failing. Green would claim all is well. */}
            <span className={`st-nv-dot${fresh ? " is-fresh" : ""}`} aria-hidden="true" />
            {lastCheckedText(lastPolled, now)}
          </p>
        )}

        <div className="st-nv-startup">
          <Switch
            label="Also check when MyTube starts"
            hint="Otherwise the first check waits for the interval."
            checked={s.poll_on_startup}
            onChange={(on) => save("poll_on_startup", on)}
          />
        </div>
      </Block>

      <Block>
        <Cap id={depthCap}>How far back each channel is read</Cap>
        <div className="st-nv-depth">
          <div className="st-nv-bignum">
            <b>{depth}</b>
            <span>{depth === 1 ? "most recent video" : "most recent videos"}</span>
          </div>
          <Stepper
            label="videos per channel"
            value={depth}
            min={DEPTH.min}
            max={DEPTH.max}
            onChange={(n) => save("backfill_count", n)}
          />
        </div>

        <DepthSlider
          value={depth}
          labelledBy={depthCap}
          onInput={(n) => set("backfill_count", n)}
          onRelease={(n) => save("backfill_count", n)}
        />
        <div className="st-nv-ticks" aria-hidden="true">
          {DEPTH_TICKS.map((t) => (
            <span key={t} style={{ "--p": depthPosition(t) } as CSSProperties}>{t}</span>
          ))}
        </div>

        {/* The setting's one subtle rule -- raising it reaches channels you
            already have, lowering it reaches nothing -- in plain words. */}
        <div className="st-nv-tiles">
          <p className="st-nv-tile">
            <strong>A new channel</strong>
            {depth === 1
              ? "starts with its most recent upload."
              : `starts with its ${depth} most recent uploads.`}
          </p>
          <p className="st-nv-tile">
            <strong>Raising it</strong>
            reads every channel deeper once, on the next check.
          </p>
          <p className="st-nv-tile">
            <strong>Lowering it</strong>
            removes nothing already in your library.
          </p>
        </div>
      </Block>
    </SettingsPage>
  );
}
