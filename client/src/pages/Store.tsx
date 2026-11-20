import { useEffect, useState } from "react";
import { api, errorMessage, GameSummary, platformLabel, timeAgo } from "../api";
import { Cover, EmptyState, Spinner, useToast } from "../components";
import type { Route } from "../App";

export default function Store({ navigate }: { navigate: (r: Route) => void }) {
  const toast = useToast();
  const [query, setQuery] = useState("");
  const [games, setGames] = useState<GameSummary[] | null>(null);

  useEffect(() => {
    const t = setTimeout(() => {
      api.store(query).then(setGames).catch((e) => toast("error", errorMessage(e)));
    }, 200);
    return () => clearTimeout(t);
  }, [query, toast]);

  return (
    <div className="page">
      <div className="page-head">
        <h1>Store</h1>
        <input className="search wide" placeholder="Search games" value={query} onChange={(e) => setQuery(e.target.value)} />
      </div>
      {!games && <Spinner />}
      {games?.length === 0 && (
        <EmptyState title={query ? "No games match that search" : "No games yet"}>
          <p className="muted">Games appear here once someone publishes a version.</p>
        </EmptyState>
      )}
      <div className="grid">
        {games?.map((g) => (
          <button key={g.slug} className="store-card" onClick={() => navigate({ page: "game", slug: g.slug })}>
            <Cover slug={g.slug} title={g.title} />
            <div className="store-card-body">
              <div className="store-card-title">{g.title}</div>
              <div className="muted small">by {g.owner} · updated {timeAgo(g.updated_at)}</div>
              {g.summary && <p className="store-card-summary">{g.summary}</p>}
              <div className="tags">
                {g.platforms.map((p) => (
                  <span key={p} className="tag">{platformLabel(p)}</span>
                ))}
                {g.in_library && <span className="tag tag-accent">In library</span>}
              </div>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}
