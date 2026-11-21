import { formatBytes, formatPlaytime, LibraryItem, platformLabel, timeAgo } from "../api";
import { GameImage } from "../art";
import { PrimaryButton, Spinner } from "../components";
import { jobFraction, primaryLabel, useActions, useGame } from "../data";
import { Icon } from "../icons";
import { gameMenu, useContextMenu } from "../menu";

/** Everything about one game: owned or not, installed or not. */
export default function Game({
  slug,
  item,
  actions,
  backLabel,
  onBack,
}: {
  slug: string;
  item?: LibraryItem;
  actions: ReturnType<typeof useActions>;
  backLabel: string;
  onBack: () => void;
}) {
  const game = useGame(slug, item?.latest?.version);
  const menu = useContextMenu();
  if (!game) return <div className="center-fill"><Spinner /></div>;

  const f = jobFraction(item?.job ?? null);
  const latest = game.versions[0];

  return (
    <div className="scroll">
      <section className="gp-hero">
        <div className="gp-backdrop">
          <GameImage game={game} kind="banner" />
        </div>
        <div className="gp-hero-inner">
          <div className="gp-poster">
            <GameImage game={game} kind="cover" />
          </div>
          <div className="gp-info">
            <button className="text-btn gp-back" onClick={onBack}>
              ← {backLabel}
            </button>
            <h1 className="gp-title">{game.title}</h1>
            <div className="gp-byline">
              by <b>{game.owner}</b>
              {latest && <> · v{latest.version} · updated {timeAgo(latest.created_at)}</>}
            </div>
            {game.summary && <p className="gp-summary">{game.summary}</p>}
            <div className="gp-actions">
              {item ? (
                <PrimaryButton item={item} actions={actions} />
              ) : (
                <button className="btn k-install" disabled={!latest} onClick={() => actions.getAndInstall(slug)}>
                  <span>
                    <Icon name="download" size={18} /> Get & install
                  </span>
                </button>
              )}
              {item?.installed && (
                <button className="icon-btn" title="Open folder" onClick={() => actions.openFolder(slug)}>
                  <Icon name="folder" />
                </button>
              )}
              {item && (
                <button className="icon-btn" title="More" onClick={(e) => menu.open(e, gameMenu(item, actions))}>
                  <Icon name="more" />
                </button>
              )}
              {!item && latest && (
                <button className="plain-btn outline" onClick={() => actions.add(slug)}>
                  Add to library
                </button>
              )}
            </div>
            {item?.job && (
              <div className="gp-progress">
                <div className="bar">
                  <div style={{ width: `${Math.round((f ?? 0.05) * 100)}%` }} />
                </div>
                <span>
                  {f !== null ? `${formatBytes(item.job.done_bytes)} of ${formatBytes(item.job.total_bytes)}` : primaryLabel(item)}
                  {item.job.reused_bytes > 0 && ` · ${formatBytes(item.job.reused_bytes)} reused from disk`}
                </span>
              </div>
            )}
          </div>
        </div>
      </section>

      <div className="gp-body">
        <section className="card">
          <h2>Patch notes</h2>
          {game.versions.length === 0 && <p className="dim">No versions published yet.</p>}
          {game.versions.map((v) => {
            const isInstalled = item?.installed?.version === v.version;
            return (
              <div key={v.version} className={`timeline-item ${isInstalled ? "current" : ""}`}>
                <div className="timeline-dot" />
                <div>
                  <div className="timeline-head">
                    <b>v{v.version}</b>
                    <span className="dim small">
                      {timeAgo(v.created_at)}
                      {v.published_by && ` · ${v.published_by}`} · {v.platforms.map(platformLabel).join(", ")}
                    </span>
                    {isInstalled && <span className="tag green">Installed</span>}
                    {v === latest && !isInstalled && item?.installed && <span className="tag">New</span>}
                  </div>
                  <p>{v.changelog}</p>
                </div>
              </div>
            );
          })}
        </section>

        <aside className="side">
          <div className="card">
            <h2>Details</h2>
            <dl className="facts">
              <dt>Developer</dt>
              <dd>{game.owner}</dd>
              <dt>Latest</dt>
              <dd>{latest ? `v${latest.version}` : "—"}</dd>
              <dt>Installed</dt>
              <dd>{item?.installed ? `v${item.installed.version}` : "No"}</dd>
              <dt>Download</dt>
              <dd>{item?.latest ? formatBytes(item.latest.total_size) : "—"}</dd>
              <dt>Platforms</dt>
              <dd>{game.platforms.map(platformLabel).join(", ") || "—"}</dd>
              <dt>Versions</dt>
              <dd>{game.versions.length}</dd>
              {item && (
                <>
                  <dt>Played</dt>
                  <dd>{formatPlaytime(item.playtime_seconds)}</dd>
                  <dt>Last played</dt>
                  <dd>{item.last_played_at ? timeAgo(item.last_played_at) : "—"}</dd>
                </>
              )}
            </dl>
          </div>
          {game.description && (
            <div className="card">
              <h2>About</h2>
              <p className="about">{game.description}</p>
            </div>
          )}
        </aside>
      </div>
      {menu.element}
    </div>
  );
}
