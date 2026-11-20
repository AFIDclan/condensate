import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import {
  api,
  errorMessage,
  formatBytes,
  GameDetail,
  Job,
  JobFinished,
  LibraryItem,
  platformLabel,
  timeAgo,
} from "../api";
import { Cover, EmptyState, ProgressBar, Spinner, useToast } from "../components";
import type { Route } from "../App";

export default function Library({ selected, navigate }: { selected?: string; navigate: (r: Route) => void }) {
  const toast = useToast();
  const [items, setItems] = useState<LibraryItem[] | null>(null);
  const [current, setCurrent] = useState<string | undefined>(selected);
  const [filter, setFilter] = useState("");

  const refresh = useCallback(async () => {
    try {
      setItems(await api.library());
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }, [toast]);

  useEffect(() => {
    refresh();
    const unlisten = [
      listen<Job>("job", (e) =>
        setItems((list) => list?.map((i) => (i.game.slug === e.payload.slug ? { ...i, job: e.payload } : i)) ?? list),
      ),
      listen<JobFinished>("job-finished", (e) => {
        const { ok, error, report, slug } = e.payload;
        if (!ok) toast("error", `${slug}: ${error}`);
        else if (report && !report.up_to_date) {
          const verb = report.from_version ? `Updated to ${report.to_version}` : `Installed ${report.to_version}`;
          toast("info", `${verb} — downloaded ${formatBytes(report.downloaded_bytes)}`);
        }
        refresh();
      }),
      listen<string>("game-started", (e) =>
        setItems((list) => list?.map((i) => (i.game.slug === e.payload ? { ...i, running: true } : i)) ?? list),
      ),
      listen<{ slug: string; code: number | null }>("game-exited", (e) => {
        setItems((list) => list?.map((i) => (i.game.slug === e.payload.slug ? { ...i, running: false } : i)) ?? list);
        if (e.payload.code !== null && e.payload.code !== 0) toast("error", `${e.payload.slug} exited with code ${e.payload.code}`);
      }),
    ];
    // Pick up new versions published while the app is open.
    const timer = setInterval(refresh, 60_000);
    return () => {
      clearInterval(timer);
      unlisten.forEach((p) => p.then((f) => f()));
    };
  }, [refresh, toast]);

  const visible = useMemo(
    () => items?.filter((i) => i.game.title.toLowerCase().includes(filter.toLowerCase())) ?? [],
    [items, filter],
  );
  const selectedItem = items?.find((i) => i.game.slug === current) ?? visible[0];

  if (!items) return <div className="center-fill"><Spinner /></div>;
  if (items.length === 0) {
    return (
      <EmptyState title="Your library is empty">
        <p className="muted">Find something your friends made in the store.</p>
        <button className="primary" onClick={() => navigate({ page: "store" })}>Browse the store</button>
      </EmptyState>
    );
  }

  return (
    <div className="library">
      <aside className="game-list">
        <input className="search" placeholder="Search library" value={filter} onChange={(e) => setFilter(e.target.value)} />
        {visible.map((i) => (
          <button
            key={i.game.slug}
            className={`game-row ${selectedItem?.game.slug === i.game.slug ? "active" : ""}`}
            onClick={() => setCurrent(i.game.slug)}
          >
            <Cover slug={i.game.slug} title={i.game.title} size="sm" />
            <span className="game-row-title">{i.game.title}</span>
            <StatusDot item={i} />
          </button>
        ))}
      </aside>
      {selectedItem && <GamePanel key={selectedItem.game.slug} item={selectedItem} refresh={refresh} />}
    </div>
  );
}

function StatusDot({ item }: { item: LibraryItem }) {
  if (item.running) return <span className="dot dot-running" title="Running" />;
  if (item.job) return <span className="dot dot-busy" title="Downloading" />;
  if (item.update_available) return <span className="dot dot-update" title="Update available" />;
  if (item.installed) return <span className="dot dot-ready" title="Installed" />;
  return null;
}

function GamePanel({ item, refresh }: { item: LibraryItem; refresh: () => void }) {
  const toast = useToast();
  const { game, installed, latest, job } = item;
  const [detail, setDetail] = useState<GameDetail | null>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.game(game.slug).then(setDetail).catch((e) => toast("error", errorMessage(e)));
  }, [game.slug, game.latest_version, toast]);

  async function run(action: () => Promise<void>, after?: () => void) {
    setBusy(true);
    try {
      await action();
      after?.();
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(false);
    }
  }

  let primary;
  if (item.running) primary = <button className="play big" disabled>Running</button>;
  else if (job) primary = <button className="primary big" disabled>{jobLabel(job)}</button>;
  else if (!latest) primary = <button className="big" disabled>Not available for your platform</button>;
  else if (!installed)
    primary = <button className="primary big" disabled={busy} onClick={() => run(() => api.install(game.slug))}>Install</button>;
  else if (item.update_available)
    primary = <button className="primary big" disabled={busy} onClick={() => run(() => api.install(game.slug))}>Update</button>;
  else primary = <button className="play big" disabled={busy} onClick={() => run(() => api.play(game.slug))}>Play</button>;

  return (
    <section className="game-panel">
      <div className="hero">
        <Cover slug={game.slug} title={game.title} size="lg" />
        <div className="hero-text">
          <h1>{game.title}</h1>
          <div className="muted">by {game.owner}</div>
          {game.summary && <p>{game.summary}</p>}
        </div>
      </div>

      <div className="action-bar">
        {primary}
        <div className="facts">
          <Fact label="Installed" value={installed ? installed.version : "—"} />
          <Fact label="Latest" value={latest ? latest.version : "—"} />
          <Fact label="Size" value={latest ? formatBytes(latest.total_size) : "—"} />
          <Fact label="Updated" value={latest ? timeAgo(latest.created_at) : "—"} />
        </div>
        <div className="spacer" />
        {installed && !job && (
          <>
            <button disabled={busy} onClick={() => run(() => api.openGameFolder(game.slug))}>Open folder</button>
            <button
              className="danger-outline"
              disabled={busy || item.running}
              onClick={() => {
                if (confirm(`Uninstall ${game.title}? Saves stored in the game folder will be deleted too.`))
                  run(() => api.uninstall(game.slug), refresh);
              }}
            >
              Uninstall
            </button>
          </>
        )}
        {!installed && !job && (
          <button className="danger-outline" disabled={busy} onClick={() => run(() => api.removeFromLibrary(game.slug), refresh)}>
            Remove
          </button>
        )}
      </div>

      {job && (
        <div className="job">
          <ProgressBar value={job.phase === "downloading" && job.total_bytes > 0 ? job.done_bytes / job.total_bytes : null} />
          <div className="muted small">
            {job.phase === "downloading"
              ? `${formatBytes(job.done_bytes)} of ${formatBytes(job.total_bytes)}` +
                (job.reused_bytes > 0 ? ` · reusing ${formatBytes(job.reused_bytes)} already on disk` : "")
              : jobLabel(job)}
          </div>
        </div>
      )}

      <div className="panel-body">
        <div className="panel-main">
          {detail?.description && (
            <div className="block">
              <h3>About</h3>
              <p className="pre">{detail.description}</p>
            </div>
          )}
          <div className="block">
            <h3>Patch notes</h3>
            {!detail && <Spinner />}
            {detail?.versions.length === 0 && <p className="muted">No versions yet.</p>}
            <ul className="changelog">
              {detail?.versions.map((v) => (
                <li key={v.version} className={installed?.version === v.version ? "current" : ""}>
                  <div className="changelog-head">
                    <strong>{v.version}</strong>
                    <span className="muted small">
                      {timeAgo(v.created_at)} · {v.platforms.map(platformLabel).join(", ")}
                    </span>
                    {installed?.version === v.version && <span className="badge">installed</span>}
                  </div>
                  <p>{v.changelog}</p>
                </li>
              ))}
            </ul>
          </div>
        </div>
      </div>
    </section>
  );
}

function Fact({ label, value }: { label: string; value: string }) {
  return (
    <div className="fact">
      <div className="fact-label">{label}</div>
      <div className="fact-value">{value}</div>
    </div>
  );
}

function jobLabel(job: Job): string {
  if (job.phase === "checking") return "Preparing…";
  if (job.phase === "installing") return "Installing…";
  return job.total_bytes > 0 ? `Downloading ${Math.floor((job.done_bytes / job.total_bytes) * 100)}%` : "Downloading…";
}
