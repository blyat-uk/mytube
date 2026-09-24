import type { ReactNode } from "react";

/**
 * One labelled setting. `htmlFor` makes the label a real `<label>` for the
 * control it names — a select has no other way to tell a screen reader what it
 * picks — while a field wrapping several controls keeps the plain caption.
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
      {hint && <div className="field-hint">{hint}</div>}
    </div>
  );
}
