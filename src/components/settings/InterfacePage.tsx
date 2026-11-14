import { useEffect, useId, useRef, type CSSProperties, type KeyboardEvent } from "react";
import { IconSectionInterface } from "../Icons";
import { TEXT_SCALE, applyTextScale, normalizeTextScale } from "../../textScale";
import { partsPanelOf } from "../../format";
import type { PartsPanel } from "../../types";
import { useSettingsCtx, type RailLine, type SettingsCtx } from "./context";
import { Block, Cap, OptRow, Segmented, SettingsPage, Switch, type SegOption } from "./parts";

/** "Text size 125%", then "· stats on", "· panel hidden" and "· deletes when
 *  watched" while they are -- only what is off its default, so the line stays
 *  short. */
export function summary({ settings: s }: SettingsCtx): RailLine {
  const parts = [`Text size ${normalizeTextScale(s.text_scale)}%`];
  if (s.show_stats) parts.push("stats on");
  if (partsPanelOf(s) === "hidden") parts.push("panel hidden");
  if (s.delete_when_watched === true) parts.push("deletes when watched");
  return { text: parts.join(" · ") };
}

const PANEL_OPTIONS: SegOption<PartsPanel>[] = [
  { value: "visible", label: "Visible" },
  { value: "hidden", label: "Hidden" },
];

/**
 * What a key does to the size, or null for a key it ignores. Handled here
 * rather than left to the native range, as New videos' `depthAfterKey` is, so
 * what each key does is written down once instead of being up to each
 * webview. A page is five steps: 100 → 150 → 200.
 */
export function scaleAfterKey(pct: number, key: string): number | null {
  const { min, max, step } = TEXT_SCALE;
  switch (key) {
    case "ArrowUp": case "ArrowRight": return normalizeTextScale(pct + step);
    case "ArrowDown": case "ArrowLeft": return normalizeTextScale(pct - step);
    case "PageUp": return normalizeTextScale(pct + 5 * step);
    case "PageDown": return normalizeTextScale(pct - 5 * step);
    case "Home": return min;
    case "End": return max;
    default: return null;
  }
}

/**
 * A native range, invisible, over New videos' drawn track (its `.st-nv-*`
 * classes, so Settings' two sliders cannot drift apart). `onInput` follows
 * every step of a drag or a key; `onRelease` fires on every way a browser
 * reports letting go -- pointerup, the native `change`, a key coming up, blur
 * -- and is the doc's `flush`, which writes only what differs from disk, so a
 * gesture is one write however many of those arrive.
 */
function ScaleSlider({ value, labelledBy, describedBy, onInput, onRelease }: {
  value: number;
  labelledBy: string;
  describedBy: string;
  onInput: (pct: number) => void;
  onRelease: () => void;
}) {
  const ref = useRef<HTMLInputElement | null>(null);
  const release = useRef(onRelease);
  release.current = onRelease;

  // React's onChange on a range is the `input` event, every step of a drag.
  // The native `change` is the one a browser fires once, on letting go --
  // wherever the pointer is by then, which pointerup on the input is not.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const done = () => release.current();
    el.addEventListener("change", done);
    return () => el.removeEventListener("change", done);
  }, []);

  function onKeyDown(e: KeyboardEvent<HTMLInputElement>) {
    const next = scaleAfterKey(value, e.key);
    if (next === null) return;
    e.preventDefault();
    if (next !== value) onInput(next);
  }

  const { min, max, step } = TEXT_SCALE;
  const style = { "--p": (value - min) / (max - min) } as CSSProperties;
  return (
    <div className="st-nv-slider" style={style}>
      <input
        ref={ref}
        type="range"
        className="st-nv-range"
        min={min}
        max={max}
        step={step}
        value={value}
        aria-labelledby={labelledBy}
        aria-describedby={describedBy}
        aria-valuetext={`${value}%`}
        onChange={(e) => onInput(normalizeTextScale(Number(e.currentTarget.value)))}
        onKeyDown={onKeyDown}
        onKeyUp={() => release.current()}
        onPointerUp={() => release.current()}
        onBlur={() => release.current()}
      />
      <span className="st-nv-track" aria-hidden="true" />
      <span className="st-nv-fill" aria-hidden="true" />
      <span className="st-nv-thumb" aria-hidden="true" />
    </div>
  );
}

/**
 * Text size, for this screen. Moving the slider resizes the whole app's text
 * as it goes -- the app is its own preview -- and the slider has a row to
 * itself, so text growing around it can push it down but never sideways under
 * the pointer.
 *
 * Then whether the feed shows its stats, and whether a series card's parts
 * panel covers its picture until pointed at or the other way round, and
 * whether marking a downloaded video watched deletes its file without asking:
 * picks, saved at once, which the shell hears through the doc's `onSaved` --
 * this view is gone by the time Subscriptions is back on screen.
 */
export default function InterfacePage() {
  const { settings: s, set, save, flush } = useSettingsCtx();
  const capId = useId();
  const hintId = useId();
  const panelHintId = useId();
  const value = normalizeTextScale(s.text_scale);

  // Held, not saved, until the gesture ends: settings.json is written once per
  // adjustment, not once per step.
  function follow(pct: number) {
    applyTextScale(pct);
    set("text_scale", pct);
  }

  function reset() {
    applyTextScale(TEXT_SCALE.default);
    save("text_scale", TEXT_SCALE.default);
  }

  return (
    <SettingsPage
      icon={<IconSectionInterface />}
      title="Interface"
      intro="How MyTube looks and behaves."
    >
      <Block>
        <Cap id={capId}>Text size</Cap>
        <div className="st-ap-now">
          <span className="st-ap-value">{value}%</span>
          <button
            type="button"
            className="btn"
            disabled={value === TEXT_SCALE.default}
            title={`Back to ${TEXT_SCALE.default}%`}
            onClick={reset}
          >
            Reset
          </button>
        </div>

        <ScaleSlider
          value={value}
          labelledBy={capId}
          describedBy={hintId}
          onInput={follow}
          onRelease={flush}
        />
        <div className="st-ap-ends" aria-hidden="true">
          <span>{TEXT_SCALE.min}%</span>
          <span>{TEXT_SCALE.max}%</span>
        </div>

        <p className="st-ap-hint" id={hintId}>
          Text only: icons, thumbnails and spacing stay as they are. Card titles also follow
          the card zoom (Ctrl+scroll).
        </p>
      </Block>

      <Block>
        <Switch
          label="Show stats"
          hint="A month-by-month wave of uploads above the Subscriptions feed, and a part-by-part strip inside a series."
          checked={s.show_stats === true}
          onChange={(on) => save("show_stats", on)}
        />
      </Block>

      <Block>
        <OptRow
          title="Parts panel"
          hint="The part count over a series card's picture. Visible: it slides away while you point at the picture. Hidden: the picture shows whole, and the count slides in while you point at it."
          hintId={panelHintId}
        >
          <Segmented
            label="Parts panel"
            describedBy={panelHintId}
            options={PANEL_OPTIONS}
            value={partsPanelOf(s)}
            onChange={(v) => save("parts_panel", v)}
          />
        </OptRow>
      </Block>

      <Block>
        <Switch
          label="Delete the file when marked watched"
          hint="Marking a downloaded video as watched removes its file at once, without asking. The video stays in your feed."
          // `=== true`: a backend older than the switch sends no key at all.
          checked={s.delete_when_watched === true}
          onChange={(on) => save("delete_when_watched", on)}
        />
      </Block>
    </SettingsPage>
  );
}
