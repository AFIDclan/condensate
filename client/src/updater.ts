// App self-update. The server hosts latest.json + the signed installer under /download/.
import { useEffect, useState } from "react";
import { check, Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

let pending: Update | null = null;
const subscribers = new Set<(version: string | null) => void>();

/** Look for a newer version. Returns its version string, or null. Errors are swallowed
 *  (offline, or a dev build with no update feed) unless `loud` is set. */
export async function checkForUpdate(loud = false): Promise<string | null> {
  try {
    const update = await check();
    pending = update;
    subscribers.forEach((f) => f(update?.version ?? null));
    return update?.version ?? null;
  } catch (e) {
    if (loud) throw e;
    return null;
  }
}

export async function installUpdate() {
  if (!pending) return;
  await pending.downloadAndInstall();
  await relaunch();
}

/** Available update version; checks on start and every 6 hours. */
export function useAvailableUpdate(): string | null {
  const [version, setVersion] = useState<string | null>(pending?.version ?? null);
  useEffect(() => {
    subscribers.add(setVersion);
    checkForUpdate();
    const t = setInterval(() => checkForUpdate(), 6 * 3600 * 1000);
    return () => {
      subscribers.delete(setVersion);
      clearInterval(t);
    };
  }, []);
  return version;
}
