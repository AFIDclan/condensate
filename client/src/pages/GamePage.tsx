import { useEffect, useState } from "react";
import { api, errorMessage, GameDetail, platformLabel, timeAgo } from "../api";
import { Cover, Spinner, useToast } from "../components";
import type { Route } from "../App";

/** A game's store page. */
export default function GamePage({ slug, navigate }: { slug: string; navigate: (r: Route) => void }) {
  const toast = useToast();
  const [game, setGame] = useState<GameDetail | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.game(slug).then(setGame).catch((e) => toast("error", errorMessage(e)));
  }, [slug, toast]);

  if (!game) return <div className="center-fill"><Spinner /></div>;

  async function addAndInstall() {
    setBusy(true);
    try {
      await api.addToLibrary(slug);
      await api.install(slug);
      navigate({ page: "library", slug });
    } catch (e) {
      toast("error", errorMessage(e));
      setBusy(false);
    }
  }

  return (
    <div className="page">
      <button className="link back" onClick={() => navigate({ page: "store" })}>← Store</button>
      <div className="hero">
        <Cover slug={game.slug} title={game.title} size="lg" />
        <div className="hero-text">
          <h1>{game.title}</h1>
          <div className="muted">
            by {game.owner} · {game.latest_version ? `v${game.latest_version}` : "unreleased"} · updated {timeAgo(game.updated_at)}
          </div>
          {game.summary && <p>{game.summary}</p>}
          <div className="tags">
            {game.platforms.map((p) => (
              <span key={p} className="tag">{platformLabel(p)}</span>
            ))}
          </div>
          <div className="row">
            {game.in_library ? (
              <button className="primary big" onClick={() => navigate({ page: "library", slug })}>Go to library</button>
            ) : (
              <>
                <button className="primary big" disabled={busy} onClick={addAndInstall}>Get & install</button>
                <button
                  disabled={busy}
                  onClick={async () => {
                    try {
                      await api.addToLibrary(slug);
                      setGame({ ...game, in_library: true });
                    } catch (e) {
                      toast("error", errorMessage(e));
                    }
                  }}
                >
                  Add to library
                </button>
              </>
            )}
          </div>
        </div>
      </div>
      {game.description && (
        <div className="block">
          <h3>About</h3>
          <p className="pre">{game.description}</p>
        </div>
      )}
      <div className="block">
        <h3>Patch notes</h3>
        <ul className="changelog">
          {game.versions.map((v) => (
            <li key={v.version}>
              <div className="changelog-head">
                <strong>{v.version}</strong>
                <span className="muted small">{timeAgo(v.created_at)}</span>
              </div>
              <p>{v.changelog}</p>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
