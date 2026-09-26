import {
  createContext, useCallback, useContext, useMemo, useRef, useState,
  type ReactNode,
} from "react";

export type ToastKind = "error" | "info" | "success";

/** One button beside the text, for a toast that offers something to do rather
 *  than only something to read. Pressing it acts and dismisses. */
export interface ToastAction {
  label: string;
  onClick: () => void;
}

interface ToastItem {
  id: number;
  kind: ToastKind;
  text: string;
  action?: ToastAction;
}

interface ToastApi {
  error: (text: string) => void;
  info: (text: string, action?: ToastAction) => void;
  success: (text: string) => void;
}

const noop = () => {};
const ToastContext = createContext<ToastApi>({ error: noop, info: noop, success: noop });

export function useToast(): ToastApi {
  return useContext(ToastContext);
}

const LIFETIME: Record<ToastKind, number> = {
  error: 9000,
  info: 5000,
  success: 4000,
};

/** A toast with an action is asking for a decision, and five seconds is barely
 *  time to read it, let alone reach for the button. Clicking the body still
 *  dismisses it at once for anyone not interested. */
const ACTION_LIFETIME = 20000;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setItems((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const push = useCallback((kind: ToastKind, text: string, action?: ToastAction) => {
    const id = nextId.current++;
    // Keep the stack short; an error storm should not paper over the app.
    setItems((prev) => [...prev.slice(-4), { id, kind, text, action }]);
    window.setTimeout(() => dismiss(id), action ? ACTION_LIFETIME : LIFETIME[kind]);
  }, [dismiss]);

  const api = useMemo<ToastApi>(() => ({
    error: (t) => push("error", t),
    info: (t, action) => push("info", t, action),
    success: (t) => push("success", t),
  }), [push]);

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div className="toast-stack" role="status" aria-live="polite">
        {/* The toast is a box holding the dismiss target rather than being it:
            a button cannot contain the action's button, so the body is its own
            button and the action sits beside it. */}
        {items.map((t) => (
          <div key={t.id} className={`toast toast-${t.kind}`}>
            <button
              type="button"
              className="toast-body"
              onClick={() => dismiss(t.id)}
              title="Dismiss"
            >
              <span className="toast-glyph" aria-hidden="true">
                {t.kind === "error" ? "!" : t.kind === "success" ? "✓" : "i"}
              </span>
              <span className="toast-text">{t.text}</span>
            </button>
            {t.action && (
              <button
                type="button"
                className="btn btn-primary toast-action"
                onClick={() => {
                  t.action!.onClick();
                  dismiss(t.id);
                }}
              >
                {t.action.label}
              </button>
            )}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}
