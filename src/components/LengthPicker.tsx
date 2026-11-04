import { useRef, type RefObject } from "react";
import { usePopover } from "./usePopover";
import { LENGTH_STEPS, PARTS_STEPS, lengthWord } from "../filters";

interface Rows {
  minLength: number;
  onMinLength: (secs: number) => void;
  minParts: number;
  onMinParts: (parts: number) => void;
  /** The parts count only means anything while cards are groups. */
  grouped: boolean;
  /** A series is open, which shows every part whatever the filters. */
  disabled?: boolean;
  title?: string;
}

function Steps({ label, head, value, steps, step, disabled, title, onPick }: {
  label: string; head: string; value: number; steps: readonly number[];
  step: (n: number) => string; disabled: boolean; title?: string; onPick: (n: number) => void;
}) {
  return (
    <div className="length-row">
      <div className="length-head">
        <span>{label}</span>
        <b>{head}</b>
      </div>
      <div className="length-seg" role="radiogroup" aria-label={label}>
        {steps.map((n) => (
          <button
            key={n}
            type="button"
            role="radio"
            aria-checked={value === n}
            className={value === n ? "is-on" : undefined}
            disabled={disabled}
            title={title}
            onClick={() => onPick(n)}
          >
            {n === 0 ? "Any" : `${step(n)}+`}
          </button>
        ))}
      </div>
    </div>
  );
}

/**
 * The Length filter's two rows: total length at least, and parts at least.
 * Never "at most" -- what Length is for is finding the well-developed shows,
 * long and in many parts. A series is measured by all its parts added up, the
 * way Longest first ranks it, and a lone video by its own length.
 */
export function LengthRows(p: Rows) {
  const off = !!p.disabled;
  return (
    <div className="length-rows">
      <Steps
        label="Total length, at least"
        head={lengthWord(p.minLength)}
        value={p.minLength}
        steps={LENGTH_STEPS}
        step={lengthWord}
        disabled={off}
        title={p.title}
        onPick={p.onMinLength}
      />
      <Steps
        label="Parts, at least"
        head={!p.grouped ? "Needs Group siblings" : p.minParts > 1 ? String(p.minParts) : "Any"}
        value={p.grouped ? p.minParts : 0}
        steps={PARTS_STEPS}
        step={String}
        disabled={off || !p.grouped}
        title={p.title ?? (p.grouped ? undefined : "Turn on Group siblings to use this")}
        onPick={p.onMinParts}
      />
    </div>
  );
}

/** The starred Length chip's popover: the same two rows, hung from the chip. */
export function LengthPopover(p: Rows & {
  anchor: RefObject<HTMLElement | null>;
  onClose: (refocus: boolean) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const right = usePopover(ref, p.anchor, p.onClose);
  return (
    <div className="topnav-pop length-pop" role="dialog" aria-label="Length" ref={ref} style={{ right }}>
      <LengthRows {...p} />
    </div>
  );
}
