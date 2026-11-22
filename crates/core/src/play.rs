//! The multiplayer protocol: lobbies plus a packet relay, so games launched from Condensate can
//! play together without running their own servers.
//!
//! ```text
//! game ──(local WebSocket)── Condensate app ──(wss /api/v1/play)── server ── other players
//! ```
//!
//! A game connects to the Condensate app on its own PC (see "Game connection" below). The app
//! logs it in and forwards everything to the server's `/api/v1/play` socket, so a game never
//! sees the player's login token. Because the app sits in the middle, it can later route packets
//! over faster paths (direct between players, or a UDP relay) without games changing.
//!
//! On either socket, text frames are JSON control messages ([`PlayCommand`] from the game,
//! [`PlayEvent`] to it). Binary frames are game packets with a 6-byte header:
//!
//! ```text
//! [peer: u32 little-endian][channel: u8][mode: u8][payload...]
//! ```
//!
//! Sending, `peer` is the lobby member to send to (0 = everyone else in the lobby). Receiving,
//! it is the member who sent the packet. `mode` is a [`TransferMode`] value. Packets are
//! delivered in order; over the TCP relay even "unreliable" ones arrive, but they may be dropped
//! when a player's connection falls behind.
//!
//! Lobby members are numbered per lobby: the host (whoever created it) is always 1 and everyone
//! else gets 2, 3, ... in join order. When the host leaves, the lobby closes. This matches
//! Godot's client/server model, where the server is peer 1.
//!
//! ## Game connection
//!
//! When Condensate starts a game it sets `CONDENSATE_API` (a `ws://127.0.0.1:<port>/` URL),
//! `CONDENSATE_TOKEN` and `CONDENSATE_GAME` (the game's id). The game connects to
//! `<CONDENSATE_API>?token=<CONDENSATE_TOKEN>`. A game run from its editor has none of these; it
//! reads the port and token from `game-api.json` in the app's config folder (see
//! [`GAME_API_FILE`]) and passes its id as `&game=<id>`.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::api::PresenceGame;

/// Size of the binary packet header.
pub const PACKET_HEADER: usize = 6;
/// Largest packet payload the relay accepts.
pub const MAX_PACKET: usize = 1 << 20;
/// Most members a lobby can have.
pub const MAX_MEMBERS: u32 = 64;
/// Largest lobby `data`, as JSON.
pub const MAX_LOBBY_DATA: usize = 8 * 1024;
/// Written by the app to its config folder: `{"api": "ws://127.0.0.1:<port>/", "token": "..."}`.
/// Lets games run from an editor connect without being launched by Condensate.
pub const GAME_API_FILE: &str = "game-api.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TransferMode {
    Unreliable = 0,
    UnreliableOrdered = 1,
    Reliable = 2,
}

impl TransferMode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Unreliable,
            1 => Self::UnreliableOrdered,
            _ => Self::Reliable,
        }
    }
}

/// Who can find and join a lobby without an invite.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LobbyVisibility {
    /// The host's friends see it on their Friends page and can join (and anyone invited).
    #[default]
    Friends,
    /// Only people invited can join.
    InviteOnly,
}

/// Game -> server. `rid` is an optional request id echoed in the reply, so a game can match
/// replies to requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlayCommand {
    /// Create a lobby and join it as its host (member 1). Leaves any current lobby.
    CreateLobby {
        #[serde(default)]
        rid: Option<u64>,
        #[serde(default = "default_max_members")]
        max_members: u32,
        #[serde(default)]
        visibility: LobbyVisibility,
        #[serde(default)]
        data: Map<String, Value>,
    },
    /// Join a lobby by id. Leaves any current lobby.
    JoinLobby {
        #[serde(default)]
        rid: Option<u64>,
        lobby: String,
    },
    LeaveLobby {
        #[serde(default)]
        rid: Option<u64>,
    },
    /// Invite a friend to the current lobby. They get a notification in Condensate.
    Invite {
        #[serde(default)]
        rid: Option<u64>,
        user: String,
    },
    /// Replace the lobby's data (host only). Everyone in the lobby gets [`PlayEvent::LobbyData`].
    SetLobbyData {
        #[serde(default)]
        rid: Option<u64>,
        data: Map<String, Value>,
    },
    /// Remove a member from the lobby (host only).
    Kick {
        #[serde(default)]
        rid: Option<u64>,
        member: u32,
    },
    /// Lobbies for this game that you can join.
    ListLobbies {
        #[serde(default)]
        rid: Option<u64>,
    },
    /// Your friends and what they're doing.
    ListFriends {
        #[serde(default)]
        rid: Option<u64>,
    },
    /// Keepalive; the connection closes after 90 seconds of silence.
    Ping,
}

fn default_max_members() -> u32 {
    8
}

/// Server (or app) -> game.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PlayEvent {
    /// First message on every connection.
    Welcome { you: PlayUser, game: PresenceGame },
    /// A request with nothing else to report succeeded.
    Ok {
        #[serde(default)]
        rid: Option<u64>,
    },
    Error {
        #[serde(default)]
        rid: Option<u64>,
        message: String,
    },
    /// You created or joined a lobby. `you` is your member id in it.
    Lobby {
        #[serde(default)]
        rid: Option<u64>,
        lobby: LobbyInfo,
        you: u32,
    },
    Lobbies {
        #[serde(default)]
        rid: Option<u64>,
        lobbies: Vec<LobbySummary>,
    },
    Friends {
        #[serde(default)]
        rid: Option<u64>,
        friends: Vec<PlayFriend>,
    },
    MemberJoined { member: Member },
    MemberLeft { member: u32 },
    LobbyData { data: Map<String, Value> },
    /// You're no longer in the lobby: `host_left` or `kicked`.
    LobbyClosed { reason: String },
    /// Sent by the app: the player accepted an invite or clicked Join on a friend, for this game.
    /// The game should join that lobby (when Condensate launched the game for it, this arrives
    /// right after `welcome`).
    JoinRequested { lobby: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayUser {
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Member {
    pub id: u32,
    pub username: String,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbyInfo {
    pub id: String,
    pub game: String,
    pub max_members: u32,
    pub visibility: LobbyVisibility,
    pub members: Vec<Member>,
    pub data: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LobbySummary {
    pub id: String,
    pub host: PlayUser,
    pub members: u32,
    pub max_members: u32,
    pub data: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayFriend {
    pub username: String,
    pub display_name: String,
    /// online | in_game | offline
    pub status: String,
    /// Game id they're playing, if any.
    pub game: Option<String>,
    /// A lobby of theirs you can join, if any.
    pub lobby: Option<String>,
}

/// Build a packet frame.
pub fn encode_packet(peer: u32, channel: u8, mode: TransferMode, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(PACKET_HEADER + payload.len());
    out.extend_from_slice(&peer.to_le_bytes());
    out.push(channel);
    out.push(mode as u8);
    out.extend_from_slice(payload);
    out
}

/// Split a packet frame into (peer, channel, mode, payload).
pub fn decode_packet(frame: &[u8]) -> Option<(u32, u8, TransferMode, &[u8])> {
    if frame.len() < PACKET_HEADER {
        return None;
    }
    let peer = u32::from_le_bytes(frame[0..4].try_into().ok()?);
    Some((peer, frame[4], TransferMode::from_u8(frame[5]), &frame[PACKET_HEADER..]))
}
