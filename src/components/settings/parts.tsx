import { useId, type ReactNode } from "react";
import { IconMinus, IconPlus } from "../Icons";

/*
 * The Settings view's vocabulary. Every page is built from these, so the pages
 * read as one design: a header, cards, and single-choice groups made of real
 * radios under `.sr-only` -- native arrow keys and semantics, drawn as pills.
 */

/** A section: its header (glyph, title, one line of why) and its blocks. */
export function SettingsPage({ icon, title, intro, className, children }: {
  icon: ReactNode;
  title: string;
  intro?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section className={`st-page${className ? ` ${className}` : ""}`} aria-labelledby={id}>
      <header className="st-head">
        <span className="st-head-glyph" aria-hidden="true">{icon}</span>
        <div className="st-head-text">
          <h2 className="st-head-title" id={id}>{title}</h2>
          {intro && <p className="st-head-intro">{intro}</p>}
        </div>
      </header>
      {children}
    </section>
  );
}

/** A card. `title` is a real heading; `aside` sits at the head's right end. */
export function Block({ title, aside, className, children }: {
  title?: ReactNode;
  aside?: ReactNode;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div className={`st-block${className ? ` ${className}` : ""}`}>
      {(title || aside) && (
        <div className="st-block-head">
          {title && <h3 className="st-block-title">{title}</h3>}
          {aside && <div className="st-block-aside">{aside}</div>}
        </div>
      )}
      {children}
    </div>
  );
}

/** A small uppercase caption over the control it names, with an optional
 *  quiet note at its right end. Pass `id` to let a control be labelled by it. */
export function Cap({ id, right, children }: { id?: string; right?: ReactNode; children: ReactNode }) {
  return (
    <div className="st-cap">
      <span id={id}>{children}</span>
      {right && <span className="st-cap-right">{right}</span>}
    </div>
  );
}

export interface SegOption<T> {
  value: T;
  label: ReactNode;
  /** A quiet word after the label: "AV1 *smallest*". */
  note?: ReactNode;
  icon?: ReactNode;
  disabled?: boolean;
  /** A tooltip; the label is still what a screen reader reads. */
  title?: string;
}

/**
 * One choice out of a few, as a pill track (`variant="seg"`) or separate chips
 * (`variant="chips"`). Real radios: `getByRole("radio", { name })` finds each
 * option, and the group is named by `label` (or `labelledBy`).
 */
export function Segmented<T>({
  label, labelledBy, describedBy, options, value, onChange, variant = "seg", size, wide, disabled,
  className,
}: {
  label?: string;
  labelledBy?: string;
  /** The id of a hint about the whole group, e.g. an `OptRow`'s (`hintId`). */
  describedBy?: string;
  options: SegOption<T>[];
  value: T;
  onChange: (value: T) => void;
  variant?: "seg" | "chips";
  size?: "big";
  /** Options share the track's whole width. */
  wide?: boolean;
  disabled?: boolean;
  className?: string;
}) {
  const name = useId();
  const cls = [
    variant === "chips" ? "st-chips" : "st-seg",
    size === "big" ? "is-big" : "",
    wide ? "is-wide" : "",
    disabled ? "is-disabled" : "",
    className ?? "",
  ].filter(Boolean).join(" ");
  return (
    <div
      className={cls}
      role="radiogroup"
      aria-label={label}
      aria-labelledby={labelledBy}
      aria-describedby={describedBy}
    >
      {options.map((o, i) => {
        const on = o.value === value;
        const off = disabled || o.disabled;
        return (
          <label
            key={i}
            className={`st-opt-pill${on ? " is-on" : ""}${off ? " is-off" : ""}`}
            title={o.title}
          >
            <input
              type="radio"
              className="sr-only"
              name={name}
              checked={on}
              disabled={off}
              onChange={() => onChange(o.value)}
            />
            {o.icon && <span className="st-opt-icon" aria-hidden="true">{o.icon}</span>}
            <span>{o.label}</span>
            {/* The space keeps the note its own word for a screen reader:
                "AV1 smallest", not "AV1smallest". */}
            {o.note && <>{" "}<em className="st-opt-note">{o.note}</em></>}
          </label>
        );
      })}
    </div>
  );
}

/** A row: what it is on the left, its control on the right. Pass `hintId`
 *  to give the hint an id the control can name in `aria-describedby`. */
export function OptRow({ title, hint, hintId, children, className }: {
  title: ReactNode;
  hint?: ReactNode;
  hintId?: string;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={`st-row${className ? ` ${className}` : ""}`}>
      <div className="st-row-text">
        <div className="st-row-title">{title}</div>
        {hint && <div className="st-row-hint" id={hintId}>{hint}</div>}
      </div>
      <div className="st-row-control">{children}</div>
    </div>
  );
}

/**
 * An on/off setting as a row with a switch at its end. A real checkbox with
 * `role="switch"`, named by `label` and described by `hint`; the title and the
 * switch both toggle it. `extra` sits between the text and the switch (e.g. a button) --
 * outside the label, so clicking it never flips the switch.
 */
export function Switch({ checked, onChange, label, hint, disabled, extra }: {
  checked: boolean;
  onChange: (on: boolean) => void;
  label: string;
  hint?: ReactNode;
  disabled?: boolean;
  extra?: ReactNode;
}) {
  const id = useId();
  return (
    <div className={`st-row st-switch-row${disabled ? " is-disabled" : ""}`}>
      {/* The hint stays outside the label so the switch's name is the label
          alone -- `getByLabelText("…")` and a screen reader both get the
          short name, and the hint arrives as its description. */}
      <div className="st-row-text">
        <label className="st-row-title" htmlFor={id}>{label}</label>
        {hint && <div className="st-row-hint" id={`${id}-hint`}>{hint}</div>}
      </div>
      {extra && <div className="st-row-extra">{extra}</div>}
      <label className="st-switch" htmlFor={id}>
        <input
          id={id}
          type="checkbox"
          role="switch"
          className="sr-only"
          checked={checked}
          disabled={disabled}
          aria-describedby={hint ? `${id}-hint` : undefined}
          onChange={(e) => onChange(e.currentTarget.checked)}
        />
        <span className="st-switch-track" aria-hidden="true" />
      </label>
    </div>
  );
}

/** − value +, each press handed back clamped to [min, max]. */
export function Stepper({ value, min, max, step = 1, onChange, label, disabled }: {
  value: number;
  min: number;
  max: number;
  step?: number;
  onChange: (value: number) => void;
  /** What is counted, for the buttons' names: "Decrease concurrent downloads". */
  label: string;
  disabled?: boolean;
}) {
  const to = (n: number) => onChange(Math.min(max, Math.max(min, n)));
  return (
    <div className="st-stepper" role="group" aria-label={label}>
      <button
        type="button"
        className="st-stepper-btn"
        aria-label={`Decrease ${label}`}
        disabled={disabled || value <= min}
        onClick={() => to(value - step)}
      >
        <IconMinus />
      </button>
      <output className="st-stepper-value" aria-live="polite">{value}</output>
      <button
        type="button"
        className="st-stepper-btn"
        aria-label={`Increase ${label}`}
        disabled={disabled || value >= max}
        onClick={() => to(value + step)}
      >
        <IconPlus />
      </button>
    </div>
  );
}

export type StatusTone = "ok" | "news" | "warn" | "error" | "busy" | "neutral";

/**
 * The sentence at the top of a page with a state: what is true right now. With
 * no `icon`, `ok` and `busy` draw a glowing dot instead of a ring.
 */
export function StatusCard({ tone, icon, title, children, action, live = true }: {
  tone: StatusTone;
  icon?: ReactNode;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
  /** Announce changes politely; off for a card whose text never changes. */
  live?: boolean;
}) {
  return (
    <div className={`st-status is-${tone}`}>
      {icon
        ? <span className="st-status-ring" aria-hidden="true">{icon}</span>
        : <span className="st-status-dot" aria-hidden="true" />}
      <div className="st-status-text" role={live ? "status" : undefined} aria-live={live ? "polite" : undefined}>
        <div className="st-status-title">{title}</div>
        {children && <div className="st-status-detail">{children}</div>}
      </div>
      {action && <div className="st-status-action">{action}</div>}
    </div>
  );
}

export type PillTone = "ok" | "dim" | "blue" | "warn" | "error";

export function Pill({ tone, children }: { tone: PillTone; children: ReactNode }) {
  return <span className={`st-pill is-${tone}`}>{children}</span>;
}

/** A list of `Choice` rows: one radio group. */
export function ChoiceList({ label, labelledBy, children }: {
  label?: string;
  labelledBy?: string;
  children: ReactNode;
}) {
  return (
    <div className="st-choices" role="radiogroup" aria-label={label} aria-labelledby={labelledBy}>
      {children}
    </div>
  );
}

/** A visual break between groups of choices in one list. */
export const ChoiceGap = () => <div className="st-choice-gap" aria-hidden="true" />;

/**
 * One option in a `ChoiceList`: radio ring, glyph tile, title, a detail line
 * and a state pill. `children` render under the row while it is checked (a
 * custom command's field, a cookies file's path) -- outside the label, so
 * clicking into them never re-selects the row.
 */
export function Choice({
  name, checked, onSelect, icon, title, detail, pill, disabled, children, describedBy,
}: {
  /** Shared by every row of one list. */
  name: string;
  checked: boolean;
  onSelect: () => void;
  icon: ReactNode;
  title: ReactNode;
  detail?: ReactNode;
  pill?: ReactNode;
  disabled?: boolean;
  children?: ReactNode;
  describedBy?: string;
}) {
  const id = useId();
  return (
    <div className={`st-choice${checked ? " is-on" : ""}${disabled ? " is-off" : ""}`}>
      <label className="st-choice-row">
        {/* Named by the title alone, so "Firefox" is the radio's name; the
            detail line and the state pill arrive as its description. */}
        <input
          type="radio"
          className="sr-only"
          name={name}
          checked={checked}
          disabled={disabled}
          aria-labelledby={`${id}-t`}
          aria-describedby={[
            detail ? `${id}-d` : "", pill ? `${id}-p` : "", describedBy ?? "",
          ].filter(Boolean).join(" ") || undefined}
          onChange={onSelect}
        />
        <span className="st-radio" aria-hidden="true" />
        <span className="st-choice-icon" aria-hidden="true">{icon}</span>
        <span className="st-choice-text">
          <span className="st-choice-title" id={`${id}-t`}>{title}</span>
          {detail && <span className="st-choice-detail" id={`${id}-d`}>{detail}</span>}
        </span>
        {pill && <span className="st-choice-pill" id={`${id}-p`}>{pill}</span>}
      </label>
      {checked && children && <div className="st-choice-more">{children}</div>}
    </div>
  );
}
