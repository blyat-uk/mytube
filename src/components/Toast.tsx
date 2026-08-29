import {
  createContext, useCallback, useContext, useMemo, useRef, useState,
  type ReactNode,
} from "react";

export type ToastKind = "error" | "info" | "success";

interface ToastItem {
  id: number;
  kind: ToastKind;
  text: string;
}

interface ToastApi {
  error: (text: string) => void;
  info: (text: string) => void;
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

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const nextId = useRef(1);

  const dismiss = useCallback((id: number) => {
    setItems((prev) => prev.filter((t) => t.id !== id));
  }, []);

  const push = useCallback((kind: ToastKind, text: string) => {
    const id = nextId.current++;
    // Keep the stack short; an error storm should not paper over the app.
    setItems((prev) => [...prev.slice(-4), { id, kind, text }]);
    window.setTimeout(() => dismiss(id), LIFETIME[kind]);
  }, [dismiss]);

  const api = useMemo<ToastApi>(() => ({
    error: (t) => push("error", t),
    info: (t) => push("info", t),
    success: (t) => push("success", t),
  }), [push]);

  return (
    <ToastContext.Provider value={api}>
      {children}
      <div className="toast-stack" role="status" aria-live="polite">
        {items.map((t) => (
          <button
            key={t.id}
            type="button"
            className={`toast toast-${t.kind}`}
            onClick={() => dismiss(t.id)}
            title="Dismiss"
          >
            <span className="toast-glyph" aria-hidden="true">
              {t.kind === "error" ? "!" : t.kind === "success" ? "✓" : "i"}
            </span>
            <span className="toast-text">{t.text}</span>
          </button>
        ))}
      </div>
    </ToastContext.Provider>
  );
}
