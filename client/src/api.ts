import { invoke } from "@tauri-apps/api/core";

export interface User {
  id: number;
  username: string;
  display_name: string;
  bio: string;
  is_admin: boolean;
  created_at: number;
}

export interface Session {
  server: string;
  user: User | null;
  library_dir: string;
  platform: string | null;
}

export interface GameSummary {
  slug: string;
  title: string;
  summary: string;
  owner: string;
  visibility: string;
  latest_version: string | null;
  platforms: string[];
  updated_at: number;
  in_library: boolean;
}

export interface VersionInfo {
  version: string;
  changelog: string;
  created_at: number;
  published_by: string | null;
  platforms: string[];
}

export interface GameDetail extends GameSummary {
  description: string;
  versions: VersionInfo[];
}

export interface LatestBuild {
  version: string;
  manifest_hash: string;
  total_size: number;
  created_at: number;
}

export interface Job {
  slug: string;
  phase: "checking" | "downloading" | "installing";
  done_bytes: number;
  total_bytes: number;
  reused_bytes: number;
}

export interface LibraryItem {
  game: GameSummary;
  latest: LatestBuild | null;
  installed: { version: string; size: number; dir: string } | null;
  update_available: boolean;
  job: Job | null;
  running: boolean;
}

export interface PullReport {
  game: string;
  from_version: string | null;
  to_version: string;
  changelog: string;
  up_to_date: boolean;
  downloaded_bytes: number;
  reused_bytes: number;
}

export interface JobFinished {
  slug: string;
  ok: boolean;
  error: string | null;
  report: PullReport | null;
}

export interface Invite {
  code: string;
  created_at: number;
  used_by: string | null;
  used_at: number | null;
}

export const api = {
  session: () => invoke<Session>("session"),
  login: (server: string, username: string, password: string) =>
    invoke<User>("login", { server, username, password }),
  register: (server: string, username: string, password: string, invite: string) =>
    invoke<User>("register", { server, username, password, invite }),
  logout: () => invoke<void>("logout"),
  library: () => invoke<LibraryItem[]>("library"),
  store: (query: string) => invoke<GameSummary[]>("store", { query: query || null }),
  game: (slug: string) => invoke<GameDetail>("game", { slug }),
  addToLibrary: (slug: string) => invoke<void>("add_to_library", { slug }),
  removeFromLibrary: (slug: string) => invoke<void>("remove_from_library", { slug }),
  install: (slug: string) => invoke<void>("install", { slug }),
  uninstall: (slug: string) => invoke<void>("uninstall", { slug }),
  play: (slug: string) => invoke<void>("play", { slug }),
  openGameFolder: (slug: string) => invoke<void>("open_game_folder", { slug }),
  createInvite: () => invoke<Invite>("create_invite"),
  listInvites: () => invoke<Invite[]>("list_invites"),
  updateProfile: (displayName: string | null, bio: string | null) =>
    invoke<User>("update_profile", { displayName, bio }),
};

const NOT_LOGGED_IN = "NOT_LOGGED_IN";
export const LOGGED_OUT_EVENT = "condensate:logged-out";

/** Human-readable message for a failed command. Signals the app if the session expired. */
export function errorMessage(e: unknown): string {
  const msg = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  if (msg.startsWith(NOT_LOGGED_IN)) {
    window.dispatchEvent(new Event(LOGGED_OUT_EVENT));
    return "Your session expired. Please log in again.";
  }
  return msg;
}

export function formatBytes(n: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return i === 0 ? `${n} B` : `${v.toFixed(v < 10 ? 1 : 0)} ${units[i]}`;
}

export function timeAgo(unixSeconds: number): string {
  const s = Math.max(0, Date.now() / 1000 - unixSeconds);
  if (s < 60) return "just now";
  if (s < 3600) return `${Math.floor(s / 60)}m ago`;
  if (s < 86400) return `${Math.floor(s / 3600)}h ago`;
  if (s < 86400 * 30) return `${Math.floor(s / 86400)}d ago`;
  return new Date(unixSeconds * 1000).toLocaleDateString();
}

export function platformLabel(p: string): string {
  return p === "windows-x64" ? "Windows" : p === "linux-x64" ? "Linux" : p;
}

/** Stable hue per game so covers are recognizable before games have art. */
export function gameHue(slug: string): number {
  let h = 0;
  for (const c of slug) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return h % 360;
}
