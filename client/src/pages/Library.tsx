import { useState } from "react";
import { formatPlaytime, LibraryItem, timeAgo } from "../api";
import { GameImage } from "../art";
import { jobFraction, primaryLabel, primaryOf, runPrimary, useActions } from "../data";
import { Icon } from "../icons";
import { gameMenu, useContextMenu } from "../menu";
import { Spinner } from "../components";

type Filter = "all" | "installed" | "updates";
type Sort = "recent" | "name";
type Actions = ReturnType<typeof useActions>;

export default function Library({
  items,
  actions,
  open,
  goStore,
}: {
  items: LibraryItem[] | null;
  actions: Actions;
  open: (slug: string) => void;
  goStore: () => void;
}) {
  const menu = useContextMenu();
  const [filter, setFilter] = useState<Filter>("all");
  const [q, setQ] = useState("");
  const [sort, setSort] = useState<Sort>("recent");

  if (!items) return <div className="center-fill"><Spinner /></div>;

  if (items.length === 0) {
    return (
      <div className="empty">
        <h1>Your library is empty</h1>
        <p className="dim">Find something your friends made.</p>
        <button className="btn k-install" onClick={goStore}>
          <span>Browse the store</span>
        </button>
      </div>
    );
  }

  const updates = items.filter((i) => i.update_available).length;
  const installed = items.filter((i) => i.installed).length;
  const visible = items.filter(
    (i) =>
      (filter === "all" || (filter === "installed" && i.installed) || (filter === "updates" && i.update_available)) &&
      i.game.title.toLowerCase().includes(q.toLowerCase()),
  );
  visible.sort((a, b) =>
    sort === "name"
      ? a.game.title.localeCompare(b.game.title)
      : (b.last_played_at ?? 0) - (a.last_played_at ?? 0) || (b.latest?.created_at ?? 0) - (a.latest?.created_at ?? 0),
  );

  return (
    <div className="scroll page-pad">
      <div className="wall-bar">
        <div className="chips">
          <button className={filter === "all" ? "on" : ""} onClick={() => setFilter("all")}>All {items.length}</button>
          <button className={filter === "installed" ? "on" : ""} onClick={() => setFilter("installed")}>Installed {installed}</button>
          <button className={filter === "updates" ? "on" : ""} onClick={() => setFilter("updates")}>Updates {updates}</button>
        </div>
        <div className="chips sort">
          <button className={sort === "recent" ? "on" : ""} onClick={() => setSort("recent")}>Recent</button>
          <button className={sort === "name" ? "on" : ""} onClick={() => setSort("name")}>A–Z</button>
        </div>
        <label className="search">
          <Icon name="search" size={16} />
          <input placeholder="Filter library" value={q} onChange={(e) => setQ(e.target.value)} />
        </label>
      </div>
      {visible.length === 0 && <p className="dim">Nothing matches.</p>}
      <div className="wall">
        {visible.map((item) => (
          <Poster
            key={item.game.slug}
            item={item}
            onOpen={() => open(item.game.slug)}
            onPrimary={() => runPrimary(item, actions)}
            onMenu={(e) => menu.open(e, [{ label: "View details", icon: "notes", onClick: () => open(item.game.slug) }, ...gameMenu(item, actions)])}
          />
        ))}
      </div>
      {menu.element}
    </div>
  );
}

function Poster({ item, onOpen, onPrimary, onMenu }: { item: LibraryItem; onOpen: () => void; onPrimary: () => void; onMenu: (e: React.MouseEvent) => void }) {
  const kind = primaryOf(item);
  const f = jobFraction(item.job);
  let ribbon: string | null = null;
  if (item.running) ribbon = "Playing";
  else if (item.job) ribbon = f !== null ? `${Math.floor(f * 100)}%` : "…";
  else if (item.update_available) ribbon = "Update";
  else if (!item.installed) ribbon = "Not installed";

  return (
    <div className={`poster ${!item.installed && !item.job ? "dim" : ""}`} onClick={onOpen} onDoubleClick={onPrimary} onContextMenu={onMenu}>
      <GameImage game={item.game} kind="cover" />
      <div className="shade" />
      {ribbon && <span className={`ribbon r-${kind}`}>{ribbon}</span>}
      {item.job && (
        <div className="poster-bar">
          <div style={{ width: `${Math.round((f ?? 0.05) * 100)}%` }} />
        </div>
      )}
      {(kind === "play" || kind === "update" || kind === "install") && (
        <button
          className="quick"
          title={primaryLabel(item)}
          onClick={(e) => {
            e.stopPropagation();
            onPrimary();
          }}
        >
          <Icon name={kind === "play" ? "play" : kind === "update" ? "update" : "download"} size={22} />
        </button>
      )}
      <div className="poster-text">
        <div className="poster-title">{item.game.title}</div>
        <div className="poster-sub">
          {item.last_played_at ? `${formatPlaytime(item.playtime_seconds)} · played ${timeAgo(item.last_played_at)}` : item.game.owner}
        </div>
      </div>
    </div>
  );
}
