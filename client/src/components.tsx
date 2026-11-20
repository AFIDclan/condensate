import { createContext, ReactNode, useCallback, useContext, useState } from "react";
import { gameHue } from "./api";

/** Generated cover art until games can upload their own. */
export function Cover({ slug, title, size = "md" }: { slug: string; title: string; size?: "sm" | "md" | "lg" }) {
  const hue = gameHue(slug);
  const initials = title
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((w) => w[0]!.toUpperCase())
    .join("");
  return (
    <div
      className={`cover cover-${size}`}
      style={{
        background: `linear-gradient(135deg, hsl(${hue} 55% 38%), hsl(${(hue + 50) % 360} 60% 22%))`,
      }}
    >
      <span>{initials}</span>
    </div>
  );
}

export function Spinner() {
  return <div className="spinner" aria-label="Loading" />;
}

export function ProgressBar({ value }: { value: number | null }) {
  return (
    <div className={`progress ${value === null ? "indeterminate" : ""}`}>
      <div className="progress-fill" style={{ width: value === null ? undefined : `${Math.round(value * 100)}%` }} />
    </div>
  );
}

export function EmptyState({ title, children }: { title: string; children?: ReactNode }) {
  return (
    <div className="empty">
      <h3>{title}</h3>
      {children}
    </div>
  );
}

// ---- toasts ---------------------------------------------------------------------------------

interface Toast {
  id: number;
  kind: "info" | "error";
  text: string;
}

const ToastContext = createContext<(kind: Toast["kind"], text: string) => void>(() => {});

export function useToast() {
  return useContext(ToastContext);
}

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const push = useCallback((kind: Toast["kind"], text: string) => {
    const id = Date.now() + Math.random();
    setToasts((t) => [...t, { id, kind, text }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), kind === "error" ? 8000 : 5000);
  }, []);
  return (
    <ToastContext.Provider value={push}>
      {children}
      <div className="toasts">
        {toasts.map((t) => (
          <div key={t.id} className={`toast toast-${t.kind}`} onClick={() => setToasts((x) => x.filter((y) => y.id !== t.id))}>
            {t.text}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}
