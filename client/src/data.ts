// Data + actions shared by every UI style. Styles decide presentation only.
import { useCallback, useEffect, useMemo, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, errorMessage, formatBytes, GameDetail, GameSummary, Job, JobFinished, LibraryItem } from "./api";
import { useToast } from "./components";

/** The user's library, kept live from install/launch events. */
export function useLibrary() {
  const toast = useToast();
  const [items, setItems] = useState<LibraryItem[] | null>(null);

  const refresh = useCallback(async () => {
    try {
      setItems(await api.library());
    } catch (e) {
      toast("error", errorMessage(e));
    }
  }, [toast]);

  useEffect(() => {
    refresh();
    const patch = (slug: string, f: (i: LibraryItem) => LibraryItem) =>
      setItems((list) => list?.map((i) => (i.game.slug === slug ? f(i) : i)) ?? list);
    const unlisten = [
      listen<Job>("job", (e) => patch(e.payload.slug, (i) => ({ ...i, job: e.payload }))),
      listen<JobFinished>("job-finished", (e) => {
        const { ok, error, report, slug } = e.payload;
        if (!ok) toast("error", `${slug}: ${error}`);
        else if (report && !report.up_to_date) {
          const verb = report.from_version ? `Updated to ${report.to_version}` : `Installed ${report.to_version}`;
          toast("info", `${verb}: downloaded ${formatBytes(report.downloaded_bytes)}`);
        }
        refresh();
      }),
      listen<string>("game-started", (e) => patch(e.payload, (i) => ({ ...i, running: true }))),
      listen<{ slug: string; code: number | null }>("game-exited", (e) => {
        patch(e.payload.slug, (i) => ({ ...i, running: false }));
        if (e.payload.code !== null && e.payload.code !== 0) toast("error", `${e.payload.slug} exited with code ${e.payload.code}`);
      }),
    ];
    const timer = setInterval(refresh, 60_000);
    return () => {
      clearInterval(timer);
      unlisten.forEach((p) => p.then((f) => f()));
    };
  }, [refresh, toast]);

  return { items, refresh };
}

export function useGame(slug: string | undefined, version?: string | null) {
  const toast = useToast();
  const [game, setGame] = useState<GameDetail | null>(null);
  useEffect(() => {
    if (!slug) return setGame(null);
    let live = true;
    api.game(slug).then((g) => live && setGame(g)).catch((e) => toast("error", errorMessage(e)));
    return () => {
      live = false;
    };
  }, [slug, version, toast]);
  return game;
}

export function useStoreSearch(query: string) {
  const toast = useToast();
  const [games, setGames] = useState<GameSummary[] | null>(null);
  useEffect(() => {
    const t = setTimeout(() => {
      api.store(query).then(setGames).catch((e) => toast("error", errorMessage(e)));
    }, 150);
    return () => clearTimeout(t);
  }, [query, toast]);
  return games;
}

export type PrimaryKind = "play" | "install" | "update" | "running" | "busy" | "unavailable";

/** What the big button should do for a library item. */
export function primaryOf(item: LibraryItem): PrimaryKind {
  if (item.running) return "running";
  if (item.job) return "busy";
  if (!item.latest) return "unavailable";
  if (!item.installed) return "install";
  if (item.update_available) return "update";
  return "play";
}

export function primaryLabel(item: LibraryItem): string {
  const kind = primaryOf(item);
  if (kind === "busy") return jobText(item.job!);
  if (kind === "running") return "Playing";
  if (kind === "unavailable") return "Not on this OS";
  return kind[0].toUpperCase() + kind.slice(1);
}

export function jobFraction(job: Job | null): number | null {
  if (!job || job.phase !== "downloading" || job.total_bytes === 0) return null;
  return job.done_bytes / job.total_bytes;
}

export function jobText(job: Job): string {
  if (job.phase === "checking") return "Preparing";
  if (job.phase === "installing") return "Installing";
  const f = jobFraction(job);
  return f === null ? "Downloading" : `Downloading ${Math.floor(f * 100)}%`;
}

/** Games whose Play was clicked a moment ago. Extra clicks are ignored until the library shows
 * the game as running, so a double-click doesn't launch two copies. */
const launching = new Set<string>();

async function playOnce(slug: string): Promise<void> {
  if (launching.has(slug)) return;
  launching.add(slug);
  try {
    await api.play(slug);
  } finally {
    setTimeout(() => launching.delete(slug), 3000);
  }
}

/** Library actions with error toasts. Each resolves to true on success. */
export function useActions(refresh: () => void) {
  const toast = useToast();
  return useMemo(
    () => {
      const wrap =
        (f: (slug: string) => Promise<void>, after?: boolean) =>
        async (slug: string): Promise<boolean> => {
          try {
            await f(slug);
            if (after) refresh();
            return true;
          } catch (e) {
            toast("error", errorMessage(e));
            return false;
          }
        };
      return {
        install: wrap(api.install),
        play: wrap(playOnce),
        openFolder: wrap(api.openGameFolder),
        uninstall: wrap(api.uninstall, true),
        remove: wrap(api.removeFromLibrary, true),
        add: wrap(api.addToLibrary, true),
        getAndInstall: wrap(async (slug) => {
          await api.addToLibrary(slug);
          await api.install(slug);
        }, true),
      };
    },
    [refresh, toast],
  );
}

/** Run the primary action for an item. */
export async function runPrimary(item: LibraryItem, actions: ReturnType<typeof useActions>) {
  const kind = primaryOf(item);
  if (kind === "play") return actions.play(item.game.slug);
  if (kind === "install" || kind === "update") return actions.install(item.game.slug);
  return false;
}

/** Live clock text that re-renders each minute. */
export function useClock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const t = setInterval(() => setNow(new Date()), 15_000);
    return () => clearInterval(t);
  }, []);
  return now;
}
