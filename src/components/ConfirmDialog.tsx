import { useEffect } from "react";

export interface ConfirmChoice {
  label: string;
  value: string;
  danger?: boolean;
  primary?: boolean;
}

interface Props {
  title: string;
  body?: string;
  choices: ConfirmChoice[];
  onChoose: (value: string) => void;
  onCancel: () => void;
}

/**
 * A modal offering several named outcomes rather than a bare yes/no, so a
 * destructive option can sit beside a safe one and be labelled for what it does.
 */
export default function ConfirmDialog({ title, body, choices, onChoose, onCancel }: Props) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className="modal-backdrop" onMouseDown={onCancel}>
      <div
        className="modal confirm"
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <h2 className="modal-title">{title}</h2>
        {body && <p className="confirm-body">{body}</p>}
        <div className="confirm-actions">
          <button type="button" className="btn btn-quiet" onClick={onCancel}>
            Cancel
          </button>
          {choices.map((c) => (
            <button
              key={c.value}
              type="button"
              className={`btn ${c.danger ? "btn-danger" : c.primary ? "btn-primary" : "btn-quiet"}`}
              onClick={() => onChoose(c.value)}
            >
              {c.label}
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
