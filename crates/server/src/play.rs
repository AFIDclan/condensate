//! Multiplayer: lobbies, voice rooms and the packet relay behind `/api/v1/play`. The protocol is
//! described in `condensate_core::play`. Everything lives in memory; a lobby ends when its host
//! leaves (or the server restarts). A voice room is a lobby with no game.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use condensate_core::api::{PresenceGame, PresenceLobby, PresenceRoom, WsEvent};
use condensate_core::play::*;
use futures::{SinkExt, StreamExt};
use rand::Rng;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use crate::auth::{authenticate, AuthUser};
use crate::error::{not_found, ApiResult};
use crate::feedback::{create_session, Recorder};
use crate::hub::broadcast_presence;
use crate::routes::games::visible_to;
use crate::routes::social::friend_ids;
use crate::state::AppState;

/// Messages queued for one connection. A connection this far behind on reliable traffic is
/// dropped; unreliable packets are skipped instead.
const OUTBOX: usize = 1024;

pub struct Play {
    inner: Mutex<Inner>,
}

struct Inner {
    next_conn: u64,
    conns: HashMap<u64, Conn>,
    lobbies: HashMap<String, Lobby>,
    recorder: Recorder,
}

struct Conn {
    user_id: i64,
    user: PlayUser,
    /// `None` for a voice room connection (from the app, with no game).
    game: Option<PresenceGame>,
    /// `None` once the connection was dropped for falling behind.
    tx: Option<mpsc::Sender<Message>>,
    /// (lobby id, member id)
    lobby: Option<(String, u32)>,
    /// Carries the player's voice (set by the Condensate app).
    voice: bool,
    /// Include this player's voice in feedback recordings.
    recorded: bool,
}

struct Lobby {
    id: String,
    /// `None` for a voice room.
    game: Option<PresenceGame>,
    host_user: i64,
    host_name: String,
    max_members: u32,
    visibility: LobbyVisibility,
    data: Map<String, Value>,
    /// member id -> connection
    members: BTreeMap<u32, u64>,
    next_member: u32,
    invited: HashSet<i64>,
    feedback: Option<Recording>,
}

/// A feedback recording in progress.
#[derive(Clone)]
struct Recording {
    session: Arc<str>,
    started: Instant,
    /// The game it's for (a room's host picks one; a game lobby records its own game).
    game: PresenceGame,
}

fn same_game(a: &Option<PresenceGame>, b: &Option<PresenceGame>) -> bool {
    a.as_ref().map(|g| &g.slug) == b.as_ref().map(|g| &g.slug)
}

impl Lobby {
    fn others(&self, member: u32) -> Vec<u64> {
        self.members.iter().filter(|(m, _)| **m != member).map(|(_, c)| *c).collect()
    }

    fn can_join(&self, user: i64, friends: &HashSet<i64>) -> bool {
        // The same account may join its own lobby: handy for testing with two copies of a game.
        self.host_user == user
            || self.invited.contains(&user)
            || (self.visibility == LobbyVisibility::Friends && friends.contains(&self.host_user))
    }

    /// For a game lobby.
    fn presence(&self) -> Option<PresenceLobby> {
        Some(PresenceLobby {
            id: self.id.clone(),
            game: self.game.clone()?,
            members: self.members.len() as u32,
            max_members: self.max_members,
            recording: self.feedback.is_some(),
        })
    }

    /// For a voice room.
    fn room_presence(&self) -> Option<PresenceRoom> {
        self.game.is_none().then(|| PresenceRoom {
            id: self.id.clone(),
            host: self.host_name.clone(),
            members: self.members.len() as u32,
            max_members: self.max_members,
            recording: self.feedback.is_some(),
        })
    }
}

impl Inner {
    fn send(&mut self, conn: u64, msg: Message, reliable: bool) {
        let Some(c) = self.conns.get_mut(&conn) else { return };
        let Some(tx) = &c.tx else { return };
        if let Err(mpsc::error::TrySendError::Full(_)) = tx.try_send(msg) {
            if reliable {
                c.tx = None; // closes its socket
            }
        }
    }

    fn event(&mut self, conn: u64, event: &PlayEvent) {
        if let Ok(text) = serde_json::to_string(event) {
            self.send(conn, Message::Text(text.into()), true);
        }
    }

    fn user_of(&self, conn: u64) -> Option<i64> {
        self.conns.get(&conn).map(|c| c.user_id)
    }

    fn member(&self, id: u32, conn: u64) -> Option<Member> {
        let c = self.conns.get(&conn)?;
        Some(Member { id, username: c.user.username.clone(), display_name: c.user.display_name.clone(), voice: c.voice, recorded: c.recorded })
    }

    fn info(&self, lobby: &Lobby) -> LobbyInfo {
        LobbyInfo {
            id: lobby.id.clone(),
            game: lobby.game.as_ref().map(|g| g.slug.clone()).unwrap_or_default(),
            max_members: lobby.max_members,
            visibility: lobby.visibility,
            members: lobby.members.iter().filter_map(|(id, c)| self.member(*id, *c)).collect(),
            data: lobby.data.clone(),
            feedback: lobby.feedback.as_ref().map(|r| r.session.to_string()),
            feedback_game: lobby.feedback.as_ref().map(|r| r.game.clone()),
        }
    }

    /// Tell everyone in this connection's lobby (it included) about its voice/recording flags.
    fn member_updated(&mut self, conn: u64) {
        let Some((lobby_id, id)) = self.conns.get(&conn).and_then(|c| c.lobby.clone()) else { return };
        let Some(member) = self.member(id, conn) else { return };
        let everyone: Vec<u64> = self.lobbies.get(&lobby_id).map(|l| l.members.values().copied().collect()).unwrap_or_default();
        for c in everyone {
            self.event(c, &PlayEvent::MemberUpdated { member: member.clone() });
        }
    }

    /// Take a connection out of its lobby, telling the others. When the host leaves the lobby
    /// closes. Returns the users whose presence may have changed.
    fn leave(&mut self, conn: u64) -> Vec<i64> {
        let Some((lobby_id, member)) = self.conns.get_mut(&conn).and_then(|c| c.lobby.take()) else { return vec![] };
        let mut affected: Vec<i64> = self.user_of(conn).into_iter().collect();
        if member == 1 {
            let Some(lobby) = self.lobbies.remove(&lobby_id) else { return affected };
            if let Some(rec) = lobby.feedback.clone() {
                self.recorder.stop_session(rec.session);
            }
            for other in lobby.others(1) {
                if let Some(c) = self.conns.get_mut(&other) {
                    c.lobby = None;
                    affected.push(c.user_id);
                }
                self.event(other, &PlayEvent::LobbyClosed { reason: "host_left".into() });
            }
        } else if let Some(lobby) = self.lobbies.get_mut(&lobby_id) {
            lobby.members.remove(&member);
            let others = lobby.others(member);
            for other in others {
                affected.extend(self.user_of(other));
                self.event(other, &PlayEvent::MemberLeft { member });
            }
        }
        affected
    }

    /// Lobbies the user is in that friends can see.
    fn visible_lobbies(&self, user: i64) -> impl Iterator<Item = &Lobby> {
        self.conns
            .values()
            .filter(move |c| c.user_id == user)
            .filter_map(|c| self.lobbies.get(&c.lobby.as_ref()?.0))
            .filter(|l| l.visibility == LobbyVisibility::Friends)
    }

    /// A game lobby the user is in that friends can see, for their presence.
    fn presence_lobby(&self, user: i64) -> Option<PresenceLobby> {
        self.visible_lobbies(user).find_map(Lobby::presence)
    }

    /// A voice room the user is in that friends can see, for their presence.
    fn presence_room(&self, user: i64) -> Option<PresenceRoom> {
        self.visible_lobbies(user).find_map(Lobby::room_presence)
    }
}

impl Play {
    pub fn new(recorder: Recorder) -> Self {
        Self { inner: Mutex::new(Inner { next_conn: 0, conns: HashMap::new(), lobbies: HashMap::new(), recorder }) }
    }

    /// Whether a lobby is recording this feedback session right now.
    pub fn recording(&self, session: &str) -> bool {
        self.with(|g| g.lobbies.values().any(|l| l.feedback.as_ref().is_some_and(|r| &*r.session == session)))
    }

    /// If this user is in the lobby recording `session`: milliseconds since recording started.
    pub fn recording_member(&self, session: &str, user: i64) -> Option<i64> {
        self.with(|g| {
            let lobby = g.lobbies.values().find(|l| l.feedback.as_ref().is_some_and(|r| &*r.session == session))?;
            let started = lobby.feedback.as_ref()?.started;
            lobby.members.values().any(|c| g.user_of(*c) == Some(user)).then(|| started.elapsed().as_millis() as i64)
        })
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner) -> R) -> R {
        f(&mut self.inner.lock().unwrap())
    }

    fn reply(&self, conn: u64, event: PlayEvent) {
        self.with(|g| g.event(conn, &event));
    }

    fn error(&self, conn: u64, rid: Option<u64>, message: impl Into<String>) {
        self.reply(conn, PlayEvent::Error { rid, message: message.into() });
    }

    /// Forward a game packet to its lobby member(s), stamped with the sender's member id.
    fn relay(&self, conn: u64, frame: Bytes) {
        let Some((target, _, mode, _)) = decode_packet(&frame) else { return };
        let mut g = self.inner.lock().unwrap();
        let Some((lobby_id, me)) = g.conns.get(&conn).and_then(|c| c.lobby.clone()) else { return };
        let Some(lobby) = g.lobbies.get(&lobby_id) else { return };
        match frame[5] {
            MODE_VOICE => return relay_voice(&mut g, conn, &lobby_id, me, frame),
            MODE_SCREENSHOT => return, // the app uploads these over HTTP
            _ => {}
        }
        let targets = match target {
            0 => lobby.others(me),
            t if t != me => lobby.members.get(&t).copied().into_iter().collect(),
            _ => vec![],
        };
        let mut out = BytesMut::from(&frame[..]);
        out[0..4].copy_from_slice(&me.to_le_bytes());
        let out = out.freeze();
        for t in targets {
            g.send(t, Message::Binary(out.clone()), mode == TransferMode::Reliable);
        }
    }
}

/// Voice: to every other member whose connection carries voice (dropped, never queued, when they
/// fall behind), and into the feedback recording.
fn relay_voice(g: &mut Inner, conn: u64, lobby_id: &str, me: u32, frame: Bytes) {
    let Some(c) = g.conns.get(&conn) else { return };
    if !c.voice || frame.len() <= PACKET_HEADER {
        return;
    }
    let (user, recorded) = (c.user_id, c.recorded);
    let Some(lobby) = g.lobbies.get(lobby_id) else { return };
    if let (Some(rec), true) = (&lobby.feedback, recorded) {
        g.recorder.frame(rec.session.clone(), user, frame.slice(PACKET_HEADER..));
    }
    let targets: Vec<u64> = lobby.others(me).into_iter().filter(|t| g.conns.get(t).is_some_and(|c| c.voice)).collect();
    let mut out = BytesMut::from(&frame[..]);
    out[0..4].copy_from_slice(&me.to_le_bytes());
    let out = out.freeze();
    for t in targets {
        g.send(t, Message::Binary(out.clone()), false);
    }
}

/// Update the "joinable lobby" and "voice room" parts of these users' presence, telling their
/// friends.
async fn refresh_presence(st: &AppState, mut users: Vec<i64>) {
    users.sort_unstable();
    users.dedup();
    for user in users {
        let (lobby, room) = st.play.with(|g| (g.presence_lobby(user), g.presence_room(user)));
        let lobby_changed = st.hub.set_lobby(user, lobby);
        let room_changed = st.hub.set_room(user, room);
        if lobby_changed || room_changed {
            broadcast_presence(st, user).await;
        }
    }
}

/// A random id that's easy to read out loud (no 0/o or 1/l/i).
pub fn random_id(len: usize) -> String {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..len).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

fn new_lobby_id() -> String {
    random_id(10)
}

fn data_too_big(data: &Map<String, Value>) -> bool {
    serde_json::to_string(data).map_or(true, |s| s.len() > MAX_LOBBY_DATA)
}

#[derive(Deserialize)]
pub struct PlayQuery {
    token: String,
    #[serde(default)]
    game: Option<String>,
}

/// A game the user can see, by id.
async fn visible_game(st: &AppState, slug: &str, user: &AuthUser) -> ApiResult<Option<PresenceGame>> {
    let row: Option<(String, String, i64, String)> =
        sqlx::query_as("SELECT slug, title, owner_id, visibility FROM games WHERE slug = ?").bind(slug).fetch_optional(&st.db).await?;
    Ok(row.filter(|(_, _, owner, vis)| visible_to(*owner, vis, user)).map(|(slug, title, _, _)| PresenceGame { slug, title }))
}

/// `GET /api/v1/play?token=...&game=<id>`: one connection per running game. Without `game`, a
/// voice room connection (from the Condensate app).
pub async fn ws(State(st): State<AppState>, Query(q): Query<PlayQuery>, upgrade: WebSocketUpgrade) -> ApiResult<Response> {
    let user = authenticate(&st, &q.token).await?;
    user.require_session()?;
    let game = match q.game.as_deref().filter(|g| !g.is_empty()) {
        Some(slug) => Some(visible_game(&st, slug, &user).await?.ok_or_else(|| not_found(format!("no game called {slug}")))?),
        None => None,
    };
    let display_name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = ?").bind(user.id).fetch_one(&st.db).await?;
    let me = PlayUser { username: user.username.clone(), display_name };
    Ok(upgrade.max_message_size(MAX_PACKET + PACKET_HEADER).on_upgrade(move |socket| run(st, user, me, game, socket)))
}

async fn run(st: AppState, user: AuthUser, me: PlayUser, game: Option<PresenceGame>, socket: WebSocket) {
    let user_id = user.id;
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(OUTBOX);
    let welcome = PlayEvent::Welcome { you: me.clone(), game: game.clone() };
    let conn = st.play.with(|g| {
        g.next_conn += 1;
        let id = g.next_conn;
        g.conns.insert(id, Conn { user_id, user: me, game, tx: Some(tx), lobby: None, voice: false, recorded: true });
        g.event(id, &welcome);
        id
    });
    let mut writer = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if sink.send(msg).await.is_err() {
                return;
            }
        }
        let _ = sink.close().await;
    });

    // Clients ping every 30s; a connection silent for 90s is considered dead.
    loop {
        tokio::select! {
            _ = &mut writer => break,
            msg = tokio::time::timeout(Duration::from_secs(90), stream.next()) => match msg {
                Ok(Some(Ok(Message::Binary(frame)))) => st.play.relay(conn, frame),
                Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<PlayCommand>(text.as_str()) {
                    Ok(cmd) => handle(&st, conn, &user, cmd).await,
                    Err(e) => st.play.error(conn, None, format!("bad command: {e}")),
                },
                Ok(Some(Ok(_))) => {}
                _ => break,
            },
        }
    }

    writer.abort();
    let affected = st.play.with(|g| {
        let affected = g.leave(conn);
        g.conns.remove(&conn);
        affected
    });
    refresh_presence(&st, affected).await;
}

async fn handle(st: &AppState, conn: u64, user: &AuthUser, cmd: PlayCommand) {
    let play = &st.play;
    let user_id = user.id;
    match cmd {
        PlayCommand::Ping => {}

        PlayCommand::Voice { rid, on } => play.with(|g| {
            g.conns.get_mut(&conn).unwrap().voice = on;
            g.member_updated(conn);
            g.event(conn, &PlayEvent::Ok { rid });
        }),

        PlayCommand::RecordMe { rid, on } => play.with(|g| {
            g.conns.get_mut(&conn).unwrap().recorded = on;
            g.member_updated(conn);
            g.event(conn, &PlayEvent::Ok { rid });
        }),

        PlayCommand::SetFeedback { rid, on, game } => set_feedback(st, conn, user, rid, on, game).await,

        PlayCommand::CreateLobby { rid, max_members, visibility, data } => {
            if !(2..=MAX_MEMBERS).contains(&max_members) {
                return play.error(conn, rid, format!("max_members must be between 2 and {MAX_MEMBERS}"));
            }
            if data_too_big(&data) {
                return play.error(conn, rid, "lobby data must be under 8 KB");
            }
            let affected = play.with(|g| {
                let mut affected = g.leave(conn);
                affected.push(user_id);
                let id = new_lobby_id();
                let lobby = Lobby {
                    id: id.clone(),
                    game: g.conns[&conn].game.clone(),
                    host_user: user_id,
                    host_name: g.conns[&conn].user.display_name.clone(),
                    max_members,
                    visibility,
                    data,
                    members: BTreeMap::from([(1, conn)]),
                    next_member: 2,
                    invited: HashSet::new(),
                    feedback: None,
                };
                g.conns.get_mut(&conn).unwrap().lobby = Some((id.clone(), 1));
                let info = g.info(&lobby);
                g.lobbies.insert(id, lobby);
                g.event(conn, &PlayEvent::Lobby { rid, lobby: info, you: 1 });
                affected
            });
            refresh_presence(st, affected).await;
        }

        PlayCommand::JoinLobby { rid, lobby: lobby_id } => {
            let friends: HashSet<i64> = friend_ids(&st.db, user_id).await.unwrap_or_default().into_iter().collect();
            let result = play.with(|g| -> Result<Vec<i64>, &str> {
                let me = &g.conns[&conn];
                if me.lobby.as_ref().is_some_and(|(id, _)| *id == lobby_id) {
                    return Err("you're already in that lobby");
                }
                let lobby = g.lobbies.get(&lobby_id).filter(|l| same_game(&l.game, &me.game)).ok_or("that lobby has closed")?;
                if !lobby.can_join(user_id, &friends) {
                    return Err("you need an invite to join that lobby");
                }
                if lobby.members.len() as u32 >= lobby.max_members {
                    return Err("that lobby is full");
                }
                let mut affected = g.leave(conn);
                let lobby = g.lobbies.get_mut(&lobby_id).ok_or("that lobby has closed")?;
                let member = lobby.next_member;
                lobby.next_member += 1;
                lobby.members.insert(member, conn);
                let others = lobby.others(member);
                g.conns.get_mut(&conn).unwrap().lobby = Some((lobby_id.clone(), member));
                let info = g.info(&g.lobbies[&lobby_id]);
                let joined = info.members.iter().find(|m| m.id == member).cloned();
                g.event(conn, &PlayEvent::Lobby { rid, lobby: info, you: member });
                for other in others {
                    affected.extend(g.user_of(other));
                    if let Some(m) = &joined {
                        g.event(other, &PlayEvent::MemberJoined { member: m.clone() });
                    }
                }
                affected.push(user_id);
                Ok(affected)
            });
            match result {
                Ok(affected) => refresh_presence(st, affected).await,
                Err(e) => play.error(conn, rid, e),
            }
        }

        PlayCommand::LeaveLobby { rid } => {
            let affected = play.with(|g| g.leave(conn));
            play.reply(conn, PlayEvent::Ok { rid });
            refresh_presence(st, affected).await;
        }

        PlayCommand::Invite { rid, user } => {
            let target: Option<(i64, String)> =
                sqlx::query_as("SELECT id, username FROM users WHERE username = ?").bind(&user).fetch_optional(&st.db).await.ok().flatten();
            let friends = friend_ids(&st.db, user_id).await.unwrap_or_default();
            let Some((target_id, _)) = target.filter(|(id, _)| friends.contains(id)) else {
                return play.error(conn, rid, format!("{user} isn't one of your friends"));
            };
            let invite = play.with(|g| {
                let me = g.conns[&conn].user.clone();
                let (lobby_id, _) = g.conns[&conn].lobby.clone()?;
                let lobby = g.lobbies.get_mut(&lobby_id)?;
                lobby.invited.insert(target_id);
                match (lobby.presence(), lobby.room_presence()) {
                    (Some(lobby), _) => Some(WsEvent::LobbyInvite { from: me.username, from_name: me.display_name, lobby }),
                    (_, Some(room)) => Some(WsEvent::RoomInvite { from: me.username, from_name: me.display_name, room }),
                    _ => None,
                }
            });
            match invite {
                Some(event) => {
                    st.hub.send(target_id, &event);
                    play.reply(conn, PlayEvent::Ok { rid });
                }
                None => play.error(conn, rid, "you're not in a lobby"),
            }
        }

        PlayCommand::SetLobbyData { rid, data } => {
            if data_too_big(&data) {
                return play.error(conn, rid, "lobby data must be under 8 KB");
            }
            let result = play.with(|g| -> Result<(), &str> {
                let (lobby_id, member) = g.conns[&conn].lobby.clone().ok_or("you're not in a lobby")?;
                if member != 1 {
                    return Err("only the host can change lobby data");
                }
                let lobby = g.lobbies.get_mut(&lobby_id).ok_or("you're not in a lobby")?;
                lobby.data = data.clone();
                let everyone: Vec<u64> = lobby.members.values().copied().collect();
                for c in everyone {
                    g.event(c, &PlayEvent::LobbyData { data: data.clone() });
                }
                Ok(())
            });
            match result {
                Ok(()) => play.reply(conn, PlayEvent::Ok { rid }),
                Err(e) => play.error(conn, rid, e),
            }
        }

        PlayCommand::Kick { rid, member } => {
            let result = play.with(|g| -> Result<Vec<i64>, &str> {
                let (lobby_id, me) = g.conns[&conn].lobby.clone().ok_or("you're not in a lobby")?;
                if me != 1 {
                    return Err("only the host can kick");
                }
                if member == 1 {
                    return Err("the host can't kick themselves; leave the lobby instead");
                }
                let target = *g.lobbies.get(&lobby_id).and_then(|l| l.members.get(&member)).ok_or("no such member")?;
                let affected = g.leave(target);
                g.event(target, &PlayEvent::LobbyClosed { reason: "kicked".into() });
                Ok(affected)
            });
            match result {
                Ok(affected) => {
                    play.reply(conn, PlayEvent::Ok { rid });
                    refresh_presence(st, affected).await;
                }
                Err(e) => play.error(conn, rid, e),
            }
        }

        PlayCommand::ListLobbies { rid } => {
            let friends: HashSet<i64> = friend_ids(&st.db, user_id).await.unwrap_or_default().into_iter().collect();
            play.with(|g| {
                let me = &g.conns[&conn];
                let mine = me.lobby.as_ref().map(|(id, _)| id.clone());
                let lobbies = g
                    .lobbies
                    .values()
                    .filter(|l| same_game(&l.game, &me.game) && Some(&l.id) != mine.as_ref())
                    .filter(|l| l.can_join(user_id, &friends) && (l.members.len() as u32) < l.max_members)
                    .filter_map(|l| {
                        let host = g.conns.get(l.members.get(&1)?)?.user.clone();
                        Some(LobbySummary {
                            id: l.id.clone(),
                            host,
                            members: l.members.len() as u32,
                            max_members: l.max_members,
                            data: l.data.clone(),
                            recording: l.feedback.is_some(),
                        })
                    })
                    .collect();
                g.event(conn, &PlayEvent::Lobbies { rid, lobbies });
            });
        }

        PlayCommand::ListFriends { rid } => {
            let rows: Vec<(i64, String, String, i64)> = match friend_ids(&st.db, user_id).await {
                Ok(ids) if !ids.is_empty() => {
                    let list = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
                    sqlx::query_as(&format!(
                        "SELECT id, username, display_name, last_seen_at FROM users WHERE id IN ({list}) ORDER BY display_name COLLATE NOCASE"
                    ))
                    .fetch_all(&st.db)
                    .await
                    .unwrap_or_default()
                }
                _ => vec![],
            };
            let my_game = play.with(|g| g.conns[&conn].game.as_ref().map(|g| g.slug.clone()));
            let friends = rows
                .into_iter()
                .map(|(id, username, display_name, last_seen)| {
                    let p = st.hub.presence(id, last_seen);
                    // The lobby (or, for a room connection, the room) this connection could join.
                    let lobby = match &my_game {
                        Some(slug) => p.lobby.filter(|l| &l.game.slug == slug).map(|l| l.id),
                        None => p.room.map(|r| r.id),
                    };
                    PlayFriend { username, display_name, status: p.status, game: p.game.map(|g| g.slug), lobby }
                })
                .collect();
            play.reply(conn, PlayEvent::Friends { rid, friends });
        }
    }
}

/// Host only: start or stop the lobby's feedback recording. A voice room records the game the
/// host names; a game lobby records its own game.
async fn set_feedback(st: &AppState, conn: u64, user: &AuthUser, rid: Option<u64>, on: bool, game: Option<String>) {
    let play = &st.play;
    let user_id = user.id;
    let check = play.with(|g| -> Result<(Option<PresenceGame>, bool), &str> {
        let (lobby_id, member) = g.conns[&conn].lobby.clone().ok_or("you're not in a lobby")?;
        if member != 1 {
            return Err("only the host can start or stop feedback recording");
        }
        let lobby = g.lobbies.get(&lobby_id).ok_or("you're not in a lobby")?;
        Ok((lobby.game.clone(), lobby.feedback.is_some()))
    });
    let (lobby_game, recording) = match check {
        Ok(c) => c,
        Err(e) => return play.error(conn, rid, e),
    };
    if on == recording {
        return play.reply(conn, PlayEvent::Ok { rid });
    }
    // The game being recorded. Only needed (and looked up) when starting.
    let game = match (lobby_game, game.filter(|g| !g.is_empty())) {
        _ if !on => None,
        (Some(game), _) => Some(game),
        (None, Some(slug)) => match visible_game(st, &slug, user).await {
            Ok(Some(game)) => Some(game),
            _ => return play.error(conn, rid, format!("no game called {slug}")),
        },
        (None, None) => return play.error(conn, rid, "say which game to record feedback for"),
    };
    let session: Option<Arc<str>> = if let Some(game) = &game {
        match create_session(&st.db, &game.slug, user_id).await {
            Ok(id) => Some(id.into()),
            Err(e) => {
                tracing::warn!("feedback: can't create session: {e:#}");
                return play.error(conn, rid, "couldn't start recording");
            }
        }
    } else {
        None
    };
    let users = play.with(|g| -> Option<Vec<i64>> {
        let (lobby_id, member) = g.conns[&conn].lobby.clone()?;
        let by = g.conns[&conn].user.clone();
        let lobby = g.lobbies.get_mut(&lobby_id).filter(|_| member == 1)?;
        match (&session, &game) {
            (Some(s), Some(game)) if lobby.feedback.is_none() => {
                let started = Instant::now();
                lobby.feedback = Some(Recording { session: s.clone(), started, game: game.clone() });
                g.recorder.start_session(s.clone(), started);
            }
            (Some(_), _) => return None,
            (None, _) => {
                let old = lobby.feedback.take()?;
                g.recorder.stop_session(old.session);
            }
        }
        let everyone: Vec<u64> = g.lobbies[&lobby_id].members.values().copied().collect();
        let event = PlayEvent::Feedback { session: session.as_ref().map(|s| s.to_string()), by, game: game.clone() };
        let mut users = vec![];
        for c in everyone {
            users.extend(g.user_of(c));
            g.event(c, &event);
        }
        Some(users)
    });
    match users {
        Some(users) => {
            play.reply(conn, PlayEvent::Ok { rid });
            refresh_presence(st, users).await;
        }
        None => {
            // The lobby changed while the session was being created.
            if let Some(s) = session {
                let _ = sqlx::query("DELETE FROM feedback_sessions WHERE id = ?").bind(&*s).execute(&st.db).await;
            }
            play.reply(conn, PlayEvent::Ok { rid });
        }
    }
}
