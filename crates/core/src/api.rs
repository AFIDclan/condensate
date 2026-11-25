//! JSON types for the HTTP API (`/api/v1`), shared by the server and every client.

use serde::{Deserialize, Serialize};

use crate::manifest::{LaunchConfig, Manifest};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OkResponse {
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerInfo {
    pub name: String,
    pub server_version: String,
    /// A faster address for the same API (a port forward), e.g. `https://203.0.113.5:7755`.
    pub direct_url: Option<String>,
    /// Hex BLAKE3 of the direct address's self-signed certificate (DER). Clients accept only
    /// this certificate there; they learn it from this response, fetched over the main URL.
    #[serde(default)]
    pub direct_cert: Option<String>,
    /// Registration always needs an invite code.
    pub needs_invite: bool,
}

// ---- accounts -------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub bio: String,
    pub is_admin: bool,
    pub created_at: i64,
    /// Image id; fetch from `<server>/images/<id>`.
    #[serde(default)]
    pub avatar: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    pub display_name: Option<String>,
    pub invite_code: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthResponse {
    pub token: String,
    pub user: User,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateProfileRequest {
    pub display_name: Option<String>,
    pub bio: Option<String>,
    /// An uploaded image id, or "" to remove the avatar.
    #[serde(default)]
    pub avatar_image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangePasswordRequest {
    pub current_password: String,
    pub new_password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user: User,
    pub games: Vec<GameSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invite {
    pub code: String,
    pub created_at: i64,
    pub used_by: Option<String>,
    pub used_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
    /// Restrict the token to publishing one game (slug). `None` = any game you own.
    pub game: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenInfo {
    pub id: i64,
    pub kind: String,
    pub name: String,
    pub game: Option<String>,
    pub created_at: i64,
    pub last_used_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedToken {
    pub token: String,
    pub info: TokenInfo,
}

// ---- games ----------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameSummary {
    pub slug: String,
    pub title: String,
    pub summary: String,
    pub owner: String,
    pub visibility: String,
    pub latest_version: Option<String>,
    pub platforms: Vec<String>,
    pub updated_at: i64,
    pub in_library: bool,
    /// 2:3 poster image id.
    #[serde(default)]
    pub cover: Option<String>,
    /// 16:9 banner image id.
    #[serde(default)]
    pub banner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameDetail {
    #[serde(flatten)]
    pub summary: GameSummary,
    pub description: String,
    pub versions: Vec<VersionInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateGameRequest {
    pub slug: String,
    pub title: String,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateGameRequest {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub description: Option<String>,
    pub visibility: Option<String>,
    /// Uploaded image ids, or "" to remove.
    #[serde(default)]
    pub cover_image: Option<String>,
    #[serde(default)]
    pub banner_image: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub version: String,
    pub changelog: String,
    pub created_at: i64,
    pub published_by: Option<String>,
    pub platforms: Vec<String>,
}

// ---- publishing -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishBuild {
    pub platform: String,
    pub manifest: Manifest,
    pub launch: LaunchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishRequest {
    /// Omit to bump the last number of the previous version (0.1.0 for the first).
    pub version: Option<String>,
    pub changelog: String,
    pub builds: Vec<PublishBuild>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PublishResponse {
    pub version: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingChunksRequest {
    pub hashes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MissingChunksResponse {
    pub missing: Vec<String>,
}

// ---- installing -----------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    pub game: String,
    pub version: String,
    pub platform: String,
    pub changelog: String,
    pub created_at: i64,
    pub manifest_hash: String,
    pub total_size: u64,
    pub launch: LaunchConfig,
    pub manifest: Manifest,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestBuild {
    pub version: String,
    pub manifest_hash: String,
    pub total_size: u64,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub game: GameSummary,
    /// Newest build for the platform asked for, if there is one.
    pub latest: Option<LatestBuild>,
    #[serde(default)]
    pub playtime_seconds: i64,
    #[serde(default)]
    pub last_played_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlaytimeReport {
    pub seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUploaded {
    pub id: String,
}

// ---- social -----------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceGame {
    pub slug: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    /// online | in_game | offline
    pub status: String,
    pub game: Option<PresenceGame>,
    pub last_seen_at: i64,
    /// A multiplayer lobby they're in that their friends can join.
    #[serde(default)]
    pub lobby: Option<PresenceLobby>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PresenceLobby {
    pub id: String,
    pub game: PresenceGame,
    pub members: u32,
    pub max_members: u32,
    /// The lobby is recording feedback (voice and screenshots) for the game's developer.
    #[serde(default)]
    pub recording: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Friend {
    pub user: User,
    pub presence: Presence,
    /// When you became friends.
    pub since: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FriendsResponse {
    pub friends: Vec<Friend>,
    /// Requests waiting for you to accept.
    pub incoming: Vec<User>,
    /// Requests you sent.
    pub outgoing: Vec<User>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub id: i64,
    pub from: String,
    pub to: String,
    pub body: String,
    pub created_at: i64,
    pub read_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendMessageRequest {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conversation {
    pub user: User,
    pub last: Option<ChatMessage>,
    pub unread: i64,
}

/// Server -> client events on the `/api/v1/ws` socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsEvent {
    Presence { user: String, presence: Presence },
    Message { message: ChatMessage },
    FriendsChanged,
    /// A friend invited you to their multiplayer lobby.
    LobbyInvite { from: String, from_name: String, lobby: PresenceLobby },
}

/// Client -> server commands on the socket.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WsCommand {
    Ping,
    /// What the user is playing right now (`None` when they stop).
    Playing { game: Option<String> },
}

/// A feedback recording: a playtest's voice chat (transcribed per player) and the screenshots
/// players took, for the game's owner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackSummary {
    pub id: String,
    pub game: String,
    pub game_title: String,
    /// Display name of whoever hosted the lobby.
    pub host: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    /// recording | transcribing | ready | untranscribed | failed
    pub status: String,
    /// Why transcription failed, or why it didn't run.
    pub note: Option<String>,
    /// Latest published version of the game when recording started.
    pub version: Option<String>,
    /// Display names of everyone who spoke or took a screenshot.
    pub players: Vec<String>,
    pub utterances: i64,
    pub shots: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackDetail {
    #[serde(flatten)]
    pub summary: FeedbackSummary,
    pub utterances: Vec<FeedbackUtterance>,
    pub shots: Vec<FeedbackShot>,
}

/// One stretch of someone talking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackUtterance {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    /// Milliseconds from the start of the recording.
    pub at_ms: i64,
    pub duration_ms: i64,
    /// `None` until transcribed.
    pub text: Option<String>,
    /// The audio (Ogg Opus), fetched like screenshots.
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FeedbackShot {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub at_ms: i64,
    /// File name, fetched from `/api/v1/feedback/{session}/files/{file}`.
    pub file: String,
    /// What the game reported with it: `scene`, `version`, and the game's own `context`.
    pub meta: serde_json::Value,
}
