import type { ReactNode } from "react";

/** The id a field's hint carries, so the control `htmlFor` names can point
 *  `aria-describedby` at it: a label says what a control is, the hint what it
 *  does, and a screen reader only reads the second one if it is linked. */
export const hintId = (htmlFor: string) => `${htmlFor}-hint`;

/**
 * One labelled setting. `htmlFor` makes the label a real `<label>` for the
 * control it names — a select has no other way to tell a screen reader what it
 * picks — while a field wrapping several controls keeps the plain caption.
 * With `htmlFor` the hint also gets `hintId(htmlFor)`; the control has to carry
 * the matching `aria-describedby` itself, since only the caller renders it.
 */
export default function Field({ label, hint, htmlFor, children }: {
  label: string;
  hint?: ReactNode;
  htmlFor?: string;
  children: ReactNode;
}) {
  return (
    <div className="field">
      {htmlFor
        ? <label className="field-label" htmlFor={htmlFor}>{label}</label>
        : <div className="field-label">{label}</div>}
      {children}
      {hint && <div className="field-hint" id={htmlFor ? hintId(htmlFor) : undefined}>{hint}</div>}
    </div>
  );
}
