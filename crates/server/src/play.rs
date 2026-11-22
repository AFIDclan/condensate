//! Multiplayer: lobbies and the packet relay behind `/api/v1/play`. The protocol is described in
//! `condensate_core::play`. Everything lives in memory; a lobby ends when its host leaves (or the
//! server restarts).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use bytes::{Bytes, BytesMut};
use condensate_core::api::{PresenceGame, PresenceLobby, WsEvent};
use condensate_core::play::*;
use futures::{SinkExt, StreamExt};
use rand::Rng;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::mpsc;

use crate::auth::authenticate;
use crate::error::{not_found, ApiResult};
use crate::hub::broadcast_presence;
use crate::routes::games::visible_to;
use crate::routes::social::friend_ids;
use crate::state::AppState;

/// Messages queued for one connection. A connection this far behind on reliable traffic is
/// dropped; unreliable packets are skipped instead.
const OUTBOX: usize = 1024;

#[derive(Default)]
pub struct Play {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_conn: u64,
    conns: HashMap<u64, Conn>,
    lobbies: HashMap<String, Lobby>,
}

struct Conn {
    user_id: i64,
    user: PlayUser,
    game: PresenceGame,
    /// `None` once the connection was dropped for falling behind.
    tx: Option<mpsc::Sender<Message>>,
    /// (lobby id, member id)
    lobby: Option<(String, u32)>,
}

struct Lobby {
    id: String,
    game: PresenceGame,
    host_user: i64,
    max_members: u32,
    visibility: LobbyVisibility,
    data: Map<String, Value>,
    /// member id -> connection
    members: BTreeMap<u32, u64>,
    next_member: u32,
    invited: HashSet<i64>,
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

    fn presence(&self) -> PresenceLobby {
        PresenceLobby { id: self.id.clone(), game: self.game.clone(), members: self.members.len() as u32, max_members: self.max_members }
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

    fn info(&self, lobby: &Lobby) -> LobbyInfo {
        LobbyInfo {
            id: lobby.id.clone(),
            game: lobby.game.slug.clone(),
            max_members: lobby.max_members,
            visibility: lobby.visibility,
            members: lobby
                .members
                .iter()
                .filter_map(|(id, c)| {
                    let u = &self.conns.get(c)?.user;
                    Some(Member { id: *id, username: u.username.clone(), display_name: u.display_name.clone() })
                })
                .collect(),
            data: lobby.data.clone(),
        }
    }

    /// Take a connection out of its lobby, telling the others. When the host leaves the lobby
    /// closes. Returns the users whose presence may have changed.
    fn leave(&mut self, conn: u64) -> Vec<i64> {
        let Some((lobby_id, member)) = self.conns.get_mut(&conn).and_then(|c| c.lobby.take()) else { return vec![] };
        let mut affected: Vec<i64> = self.user_of(conn).into_iter().collect();
        if member == 1 {
            let Some(lobby) = self.lobbies.remove(&lobby_id) else { return affected };
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

    /// A lobby the user is in that friends can see, for their presence.
    fn presence_lobby(&self, user: i64) -> Option<PresenceLobby> {
        self.conns
            .values()
            .filter(|c| c.user_id == user)
            .filter_map(|c| self.lobbies.get(&c.lobby.as_ref()?.0))
            .find(|l| l.visibility == LobbyVisibility::Friends)
            .map(Lobby::presence)
    }
}

impl Play {
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

/// Update the "joinable lobby" part of these users' presence, telling their friends.
async fn refresh_presence(st: &AppState, mut users: Vec<i64>) {
    users.sort_unstable();
    users.dedup();
    for user in users {
        let lobby = st.play.with(|g| g.presence_lobby(user));
        if st.hub.set_lobby(user, lobby) {
            broadcast_presence(st, user).await;
        }
    }
}

fn new_lobby_id() -> String {
    const ALPHABET: &[u8] = b"23456789abcdefghjkmnpqrstuvwxyz";
    let mut rng = rand::thread_rng();
    (0..10).map(|_| ALPHABET[rng.gen_range(0..ALPHABET.len())] as char).collect()
}

fn data_too_big(data: &Map<String, Value>) -> bool {
    serde_json::to_string(data).map_or(true, |s| s.len() > MAX_LOBBY_DATA)
}

#[derive(Deserialize)]
pub struct PlayQuery {
    token: String,
    game: String,
}

/// `GET /api/v1/play?token=...&game=<id>`: one connection per running game.
pub async fn ws(State(st): State<AppState>, Query(q): Query<PlayQuery>, upgrade: WebSocketUpgrade) -> ApiResult<Response> {
    let user = authenticate(&st, &q.token).await?;
    user.require_session()?;
    let row: Option<(String, String, i64, String)> = sqlx::query_as("SELECT slug, title, owner_id, visibility FROM games WHERE slug = ?")
        .bind(&q.game)
        .fetch_optional(&st.db)
        .await?;
    let Some((slug, title, _, _)) = row.filter(|(_, _, owner, vis)| visible_to(*owner, vis, &user)) else {
        return Err(not_found(format!("no game called {}", q.game)));
    };
    let display_name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id = ?").bind(user.id).fetch_one(&st.db).await?;
    let me = PlayUser { username: user.username.clone(), display_name };
    let game = PresenceGame { slug, title };
    Ok(upgrade.max_message_size(MAX_PACKET + PACKET_HEADER).on_upgrade(move |socket| run(st, user.id, me, game, socket)))
}

async fn run(st: AppState, user_id: i64, me: PlayUser, game: PresenceGame, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<Message>(OUTBOX);
    let welcome = PlayEvent::Welcome { you: me.clone(), game: game.clone() };
    let conn = st.play.with(|g| {
        g.next_conn += 1;
        let id = g.next_conn;
        g.conns.insert(id, Conn { user_id, user: me, game, tx: Some(tx), lobby: None });
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
                    Ok(cmd) => handle(&st, conn, user_id, cmd).await,
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

async fn handle(st: &AppState, conn: u64, user_id: i64, cmd: PlayCommand) {
    let play = &st.play;
    match cmd {
        PlayCommand::Ping => {}

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
                    max_members,
                    visibility,
                    data,
                    members: BTreeMap::from([(1, conn)]),
                    next_member: 2,
                    invited: HashSet::new(),
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
                let lobby = g.lobbies.get(&lobby_id).filter(|l| l.game.slug == me.game.slug).ok_or("that lobby has closed")?;
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
                Some(WsEvent::LobbyInvite { from: me.username, from_name: me.display_name, lobby: lobby.presence() })
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
                    .filter(|l| l.game.slug == me.game.slug && Some(&l.id) != mine.as_ref())
                    .filter(|l| l.can_join(user_id, &friends) && (l.members.len() as u32) < l.max_members)
                    .filter_map(|l| {
                        let host = g.conns.get(l.members.get(&1)?)?.user.clone();
                        Some(LobbySummary { id: l.id.clone(), host, members: l.members.len() as u32, max_members: l.max_members, data: l.data.clone() })
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
            let my_game = play.with(|g| g.conns[&conn].game.slug.clone());
            let friends = rows
                .into_iter()
                .map(|(id, username, display_name, last_seen)| {
                    let p = st.hub.presence(id, last_seen);
                    PlayFriend {
                        username,
                        display_name,
                        status: p.status,
                        game: p.game.map(|g| g.slug),
                        lobby: p.lobby.filter(|l| l.game.slug == my_game).map(|l| l.id),
                    }
                })
                .collect();
            play.reply(conn, PlayEvent::Friends { rid, friends });
        }
    }
}
