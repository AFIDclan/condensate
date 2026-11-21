import { MouseEvent, useCallback, useEffect, useState } from "react";
import { LibraryItem } from "./api";
import { primaryOf, runPrimary, useActions } from "./data";
import { Icon } from "./icons";

export interface MenuItem {
  label: string;
  icon?: string;
  onClick: () => void;
  danger?: boolean;
  disabled?: boolean;
}

/** Right-click menu. Render `element` once; call `open(event, items)` from onContextMenu. */
export function useContextMenu() {
  const [menu, setMenu] = useState<{ x: number; y: number; items: MenuItem[] } | null>(null);
  const close = useCallback(() => setMenu(null), []);

  useEffect(() => {
    if (!menu) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && close();
    window.addEventListener("pointerdown", close);
    window.addEventListener("keydown", onKey);
    window.addEventListener("resize", close);
    window.addEventListener("wheel", close);
    return () => {
      window.removeEventListener("pointerdown", close);
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("resize", close);
      window.removeEventListener("wheel", close);
    };
  }, [menu, close]);

  const open = useCallback((e: MouseEvent, items: MenuItem[]) => {
    e.preventDefault();
    e.stopPropagation();
    const x = Math.min(e.clientX, window.innerWidth - 230);
    const y = Math.min(e.clientY, window.innerHeight - items.length * 36 - 20);
    setMenu({ x, y, items });
  }, []);

  const element = menu && (
    <div className="ctx-menu" style={{ left: menu.x, top: menu.y }} onPointerDown={(e) => e.stopPropagation()}>
      {menu.items.map((item) => (
        <button
          key={item.label}
          className={item.danger ? "danger" : ""}
          disabled={item.disabled}
          onClick={() => {
            close();
            item.onClick();
          }}
        >
          {item.icon && <Icon name={item.icon} size={15} />}
          {item.label}
        </button>
      ))}
    </div>
  );

  return { open, element };
}

/** Standard right-click actions for a library game. */
export function gameMenu(item: LibraryItem, actions: ReturnType<typeof useActions>): MenuItem[] {
  const kind = primaryOf(item);
  const slug = item.game.slug;
  const items: MenuItem[] = [];
  if (kind === "play") items.push({ label: "Play", icon: "play", onClick: () => runPrimary(item, actions) });
  if (kind === "install") items.push({ label: "Install", icon: "download", onClick: () => runPrimary(item, actions) });
  if (kind === "update") items.push({ label: "Update", icon: "update", onClick: () => runPrimary(item, actions) });
  if (item.installed) {
    items.push({ label: "Open folder", icon: "folder", onClick: () => actions.openFolder(slug) });
    items.push({
      label: "Uninstall",
      icon: "trash",
      danger: true,
      disabled: item.running || !!item.job,
      onClick: () => confirm(`Uninstall ${item.game.title}?`) && actions.uninstall(slug),
    });
  } else if (!item.job) {
    items.push({ label: "Remove from library", icon: "x", danger: true, onClick: () => actions.remove(slug) });
  }
  return items;
}
