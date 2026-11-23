import { invoke } from "@tauri-apps/api/core";

export interface User {
  id: number;
  username: string;
  display_name: string;
  bio: string;
  is_admin: boolean;
  created_at: number;
  avatar: string | null;
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
  cover: string | null;
  banner: string | null;
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
  playtime_seconds: number;
  last_played_at: number | null;
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

export interface PresenceLobby {
  id: string;
  game: { slug: string; title: string };
  members: number;
  max_members: number;
  /** The lobby is recording feedback for the game's developer. */
  recording?: boolean;
}

export interface Presence {
  status: "online" | "in_game" | "offline";
  game: { slug: string; title: string } | null;
  last_seen_at: number;
  /** A multiplayer lobby of theirs you can join. */
  lobby: PresenceLobby | null;
}

export interface Friend {
  user: User;
  presence: Presence;
  since: number;
}

export interface FriendsResponse {
  friends: Friend[];
  incoming: User[];
  outgoing: User[];
}

export interface ChatMessage {
  id: number;
  from: string;
  to: string;
  body: string;
  created_at: number;
  read_at: number | null;
}

export interface Conversation {
  user: User;
  last: ChatMessage | null;
  unread: number;
}

export type WsEvent =
  | { type: "presence"; user: string; presence: Presence }
  | { type: "message"; message: ChatMessage }
  | { type: "friends_changed" }
  | { type: "lobby_invite"; from: string; from_name: string; lobby: PresenceLobby };

export interface TokenInfo {
  id: number;
  kind: string;
  name: string;
  game: string | null;
  created_at: number;
  last_used_at: number;
}

export interface GameChanges {
  title?: string;
  summary?: string;
  description?: string;
  visibility?: string;
  cover_image?: string;
  banner_image?: string;
}

/** The lobby a running game is in, and voice chat (from the `voice-state` event). */
export interface VoiceState {
  game: { slug: string; title: string } | null;
  lobby: string | null;
  host: boolean;
  members: VoiceMember[];
  feedback: string | null;
  connected: boolean;
  muted: boolean;
  deafened: boolean;
  problems: string[];
  level: number | null;
  talking: boolean;
}

export interface VoiceMember {
  id: number;
  username: string;
  display_name: string;
  you: boolean;
  voice: boolean;
  recorded: boolean;
  speaking: boolean;
}

export interface VoiceSettings {
  input: string | null;
  output: string | null;
  auto_sensitivity: boolean;
  threshold_db: number;
  noise_suppression: boolean;
  volume: number;
  muted: boolean;
  deafened: boolean;
}

export interface FeedbackSummary {
  id: string;
  game: string;
  game_title: string;
  host: string;
  started_at: number;
  ended_at: number | null;
  status: "recording" | "transcribing" | "ready" | "untranscribed" | "failed";
  note: string | null;
  version: string | null;
  players: string[];
  utterances: number;
  shots: number;
}

export interface FeedbackUtterance {
  id: number;
  username: string;
  display_name: string;
  at_ms: number;
  duration_ms: number;
  text: string | null;
  file: string;
}

export interface FeedbackShot {
  id: number;
  username: string;
  display_name: string;
  at_ms: number;
  file: string;
  meta: { scene?: string; version?: string; note?: string; context?: unknown };
}

/** `feedback_detail` returns the summary fields plus the timeline under the same names. */
export type FeedbackTimeline = Omit<FeedbackSummary, "utterances" | "shots"> & {
  utterances: FeedbackUtterance[];
  shots: FeedbackShot[];
};

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
  joinLobby: (slug: string, lobby: string) => invoke<void>("join_lobby", { slug, lobby }),
  openGameFolder: (slug: string) => invoke<void>("open_game_folder", { slug }),
  createInvite: () => invoke<Invite>("create_invite"),
  listInvites: () => invoke<Invite[]>("list_invites"),
  updateProfile: (displayName: string | null, bio: string | null, avatarImage: string | null = null) =>
    invoke<User>("update_profile", { displayName, bio, avatarImage }),
  uploadImage: (bytes: ArrayBuffer) => invoke<string>("upload_image", bytes),
  friends: () => invoke<FriendsResponse>("friends"),
  searchUsers: (query: string) => invoke<User[]>("search_users", { query }),
  addFriend: (username: string) => invoke<void>("add_friend", { username }),
  removeFriend: (username: string) => invoke<void>("remove_friend", { username }),
  conversations: () => invoke<Conversation[]>("conversations"),
  messages: (username: string, before?: number) => invoke<ChatMessage[]>("messages", { username, before: before ?? null }),
  sendMessage: (username: string, body: string) => invoke<ChatMessage>("send_message", { username, body }),
  markRead: (username: string) => invoke<void>("mark_read", { username }),
  myGames: () => invoke<GameSummary[]>("my_games"),
  createGame: (slug: string, title: string) => invoke<GameDetail>("create_game", { slug, title }),
  updateGame: (slug: string, changes: GameChanges) => invoke<GameDetail>("update_game", { slug, changes }),
  deleteVersion: (slug: string, version: string) => invoke<void>("delete_version", { slug, version }),
  listTokens: () => invoke<TokenInfo[]>("list_tokens"),
  createToken: (name: string, game: string | null) => invoke<{ token: string; info: TokenInfo }>("create_token", { name, game }),
  revokeToken: (id: number) => invoke<void>("revoke_token", { id }),
  setLibraryDir: (path: string) => invoke<string>("set_library_dir", { path }),
  voiceState: () => invoke<VoiceState>("voice_state"),
  voiceDevices: () => invoke<{ inputs: string[]; outputs: string[] }>("voice_devices"),
  voiceSettings: () => invoke<VoiceSettings>("voice_settings"),
  setVoiceSettings: (settings: VoiceSettings) => invoke<void>("set_voice_settings", { settings }),
  micTest: (on: boolean) => invoke<void>("mic_test", { on }),
  setFeedback: (on: boolean) => invoke<void>("set_feedback", { on }),
  setRecordMe: (on: boolean) => invoke<void>("set_record_me", { on }),
  feedbackList: (slug: string) => invoke<FeedbackSummary[]>("feedback_list", { slug }),
  feedbackDetail: (id: string) => invoke<FeedbackTimeline>("feedback_detail", { id }),
  feedbackFile: (id: string, file: string) => invoke<ArrayBuffer>("feedback_file", { id, file }),
  deleteFeedback: (id: string) => invoke<void>("delete_feedback", { id }),
  retranscribeFeedback: (id: string) => invoke<void>("retranscribe_feedback", { id }),
  exportFeedback: (id: string, dir: string) => invoke<string>("export_feedback", { id, dir }),
};

// Image URLs need the server address; set once the session is known.
let serverBase = "";
export function setServer(server: string) {
  serverBase = server.replace(/\/+$/, "");
}
export function imageUrl(id: string | null | undefined): string | null {
  return id && serverBase ? `${serverBase}/images/${id}` : null;
}

export function formatPlaytime(seconds: number): string {
  if (seconds < 60) return seconds > 0 ? "under a minute" : "never played";
  if (seconds < 3600) return `${Math.round(seconds / 60)} min`;
  const h = seconds / 3600;
  return `${h < 10 ? h.toFixed(1) : Math.round(h)} h`;
}

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
