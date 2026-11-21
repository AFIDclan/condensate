import { useState } from "react";
import { LibraryItem, platformLabel, timeAgo } from "../api";
import { GameImage } from "../art";
import { useStoreSearch } from "../data";
import { Icon } from "../icons";
import { Spinner } from "../components";

export default function Store({ items, open }: { items: LibraryItem[] | null; open: (slug: string) => void }) {
  const [q, setQ] = useState("");
  const games = useStoreSearch(q);
  const owned = new Set(items?.map((i) => i.game.slug));

  return (
    <div className="scroll page-pad">
      <div className="store-head">
        <h1 className="page-title">Store</h1>
        <label className="search">
          <Icon name="search" size={16} />
          <input placeholder="Search games" value={q} onChange={(e) => setQ(e.target.value)} />
        </label>
      </div>
      {!games && <Spinner />}
      {games?.length === 0 && (
        <div className="empty">
          <h1>{q ? "No games found" : "Nothing here yet"}</h1>
          <p className="dim">Games show up once someone publishes a version.</p>
        </div>
      )}
      <div className="store-grid">
        {games?.map((g) => (
          <button key={g.slug} className="store-card" onClick={() => open(g.slug)}>
            <div className="store-art">
              <GameImage game={g} kind="banner" />
              {!g.banner && !g.cover && (
                <>
                  <div className="shade" />
                  <div className="art-title">{g.title}</div>
                </>
              )}
              {owned.has(g.slug) && <span className="tag on-art">In library</span>}
            </div>
            <div className="store-body">
              {(g.banner || g.cover) && <div className="store-title">{g.title}</div>}
              <div className="store-meta">
                by {g.owner} · {g.latest_version ? `v${g.latest_version}` : "unreleased"} · updated {timeAgo(g.updated_at)}
              </div>
              {g.summary && <p>{g.summary}</p>}
              <div className="tags">
                {g.platforms.map((p) => (
                  <span key={p} className="tag">{platformLabel(p)}</span>
                ))}
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
