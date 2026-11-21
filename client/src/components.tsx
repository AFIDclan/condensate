import { createContext, CSSProperties, ReactNode, useCallback, useContext, useState } from "react";
import { api, imageUrl, LibraryItem } from "./api";
import { jobFraction, primaryLabel, primaryOf, runPrimary, useActions } from "./data";
import { Icon } from "./icons";

export function Spinner() {
  return <div className="spinner" aria-label="Loading" />;
}

/** The main Play / Install / Update button; fills up while downloading. */
export function PrimaryButton({ item, actions, size }: { item: LibraryItem; actions: ReturnType<typeof useActions>; size?: "sm" }) {
  const kind = primaryOf(item);
  const f = jobFraction(item.job);
  return (
    <button
      className={`btn ${size ?? ""} k-${kind}`}
      disabled={kind === "busy" || kind === "running" || kind === "unavailable"}
      onClick={(e) => {
        e.stopPropagation();
        runPrimary(item, actions);
      }}
      style={{ "--fill": `${Math.round((f ?? 0) * 100)}%` } as CSSProperties}
    >
      <span>
        {kind === "play" && <Icon name="play" size={size ? 15 : 18} />}
        {kind === "install" && <Icon name="download" size={size ? 15 : 18} />}
        {kind === "update" && <Icon name="update" size={size ? 15 : 18} />}
        {primaryLabel(item)}
      </span>
    </button>
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

/** Round avatar: the uploaded image, or the first letter of the name. */
export function Avatar({ user, size = 36, status }: { user: { display_name: string; avatar?: string | null }; size?: number; status?: string }) {
  const url = imageUrl(user.avatar);
  return (
    <span className="avatar-wrap" style={{ width: size, height: size }}>
      {url ? (
        <img className="avatar-img" src={url} alt="" draggable={false} />
      ) : (
        <span className="avatar-letter" style={{ fontSize: size * 0.42 }}>
          {user.display_name[0]?.toUpperCase()}
        </span>
      )}
      {status && <span className={`presence-dot p-${status}`} />}
    </span>
  );
}

/** Ask for an image file and upload it; resolves to the image id (or null if cancelled). */
export function pickAndUploadImage(): Promise<string | null> {
  return new Promise((resolve, reject) => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept = "image/png,image/jpeg,image/gif,image/webp";
    input.onchange = async () => {
      const file = input.files?.[0];
      if (!file) return resolve(null);
      try {
        resolve(await api.uploadImage(await file.arrayBuffer()));
      } catch (e) {
        reject(e);
      }
    };
    input.click();
  });
}
