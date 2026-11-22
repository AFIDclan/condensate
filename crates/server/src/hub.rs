//! Live connections: who is online, what they're playing, and pushing events to them.
//! Each logged-in client keeps one WebSocket open; a user may have several (multiple PCs).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::Response;
use condensate_core::api::{Presence, PresenceGame, PresenceLobby, WsCommand, WsEvent};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

use crate::auth::{authenticate, AuthUser};
use crate::db::now;
use crate::error::ApiResult;
use crate::state::AppState;

#[derive(Default)]
pub struct Hub {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    conns: HashMap<i64, Vec<(u64, UnboundedSender<String>)>>,
    playing: HashMap<i64, PresenceGame>,
    /// A lobby each user is in that their friends can join (kept by play.rs).
    lobby: HashMap<i64, PresenceLobby>,
    next_id: u64,
}

impl Hub {
    /// Register a connection. Returns its id, its outbox, and whether the user just came online.
    fn connect(&self, user: i64) -> (u64, UnboundedReceiver<String>, bool) {
        let (tx, rx) = unbounded_channel();
        let mut g = self.inner.lock().unwrap();
        g.next_id += 1;
        let id = g.next_id;
        let list = g.conns.entry(user).or_default();
        list.push((id, tx));
        (id, rx, list.len() == 1)
    }

    /// Returns true when the user's last connection closed.
    fn disconnect(&self, user: i64, conn: u64) -> bool {
        let mut g = self.inner.lock().unwrap();
        let gone = match g.conns.get_mut(&user) {
            Some(list) => {
                list.retain(|(id, _)| *id != conn);
                list.is_empty()
            }
            None => true,
        };
        if gone {
            g.conns.remove(&user);
            g.playing.remove(&user);
        }
        gone
    }

    fn set_playing(&self, user: i64, game: Option<PresenceGame>) {
        let mut g = self.inner.lock().unwrap();
        match game {
            Some(game) => g.playing.insert(user, game),
            None => g.playing.remove(&user),
        };
    }

    /// Returns true when it changed.
    pub fn set_lobby(&self, user: i64, lobby: Option<PresenceLobby>) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.lobby.get(&user) == lobby.as_ref() {
            return false;
        }
        match lobby {
            Some(l) => g.lobby.insert(user, l),
            None => g.lobby.remove(&user),
        };
        true
    }

    pub fn presence(&self, user: i64, last_seen_at: i64) -> Presence {
        let g = self.inner.lock().unwrap();
        let online = g.conns.contains_key(&user);
        let lobby = g.lobby.get(&user).cloned();
        // A game in a lobby counts as playing, even when run from an editor rather than the app.
        let game = g.playing.get(&user).cloned().or_else(|| lobby.as_ref().map(|l| l.game.clone()));
        Presence {
            status: if game.is_some() { "in_game" } else if online || lobby.is_some() { "online" } else { "offline" }.into(),
            game,
            last_seen_at: if online { now() } else { last_seen_at },
            lobby,
        }
    }

    pub fn send(&self, user: i64, event: &WsEvent) {
        let Ok(text) = serde_json::to_string(event) else { return };
        let g = self.inner.lock().unwrap();
        for (_, tx) in g.conns.get(&user).into_iter().flatten() {
            let _ = tx.send(text.clone());
        }
    }
}

/// Tell a user's friends about their current presence.
pub async fn broadcast_presence(st: &AppState, user_id: i64) {
    let Ok(Some((username, last_seen))) =
        sqlx::query_as::<_, (String, i64)>("SELECT username, last_seen_at FROM users WHERE id = ?").bind(user_id).fetch_optional(&st.db).await
    else {
        return;
    };
    let presence = st.hub.presence(user_id, last_seen);
    let event = WsEvent::Presence { user: username, presence };
    for friend in crate::routes::social::friend_ids(&st.db, user_id).await.unwrap_or_default() {
        st.hub.send(friend, &event);
    }
}

#[derive(Deserialize)]
pub struct WsQuery {
    token: String,
}

/// `GET /api/v1/ws?token=...`. Browsers can't set headers on WebSockets, hence the query.
pub async fn ws(State(st): State<AppState>, Query(q): Query<WsQuery>, upgrade: WebSocketUpgrade) -> ApiResult<Response> {
    let user = authenticate(&st, &q.token).await?;
    Ok(upgrade.on_upgrade(move |socket| run(st, user, socket)))
}

async fn run(st: AppState, user: AuthUser, socket: WebSocket) {
    let (mut sink, mut stream) = socket.split();
    let (conn, mut outbox, first) = st.hub.connect(user.id);
    if first {
        broadcast_presence(&st, user.id).await;
    }
    let writer = tokio::spawn(async move {
        while let Some(text) = outbox.recv().await {
            if sink.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    });

    // Clients ping every 30s; a connection silent for 90s is considered dead.
    while let Ok(Some(Ok(msg))) = tokio::time::timeout(Duration::from_secs(90), stream.next()).await {
        match msg {
            Message::Text(text) => {
                if let Ok(cmd) = serde_json::from_str::<WsCommand>(text.as_str()) {
                    handle(&st, &user, cmd).await;
                }
            }
            Message::Close(_) => break,
            _ => {}
        }
    }

    writer.abort();
    if st.hub.disconnect(user.id, conn) {
        let _ = sqlx::query("UPDATE users SET last_seen_at = ? WHERE id = ?").bind(now()).bind(user.id).execute(&st.db).await;
        broadcast_presence(&st, user.id).await;
    }
}

async fn handle(st: &AppState, user: &AuthUser, cmd: WsCommand) {
    match cmd {
        WsCommand::Ping => {}
        WsCommand::Playing { game } => {
            let game = match game {
                Some(slug) => sqlx::query_as::<_, (String, String)>("SELECT slug, title FROM games WHERE slug = ?")
                    .bind(&slug)
                    .fetch_optional(&st.db)
                    .await
                    .ok()
                    .flatten()
                    .map(|(slug, title)| PresenceGame { slug, title }),
                None => None,
            };
            st.hub.set_playing(user.id, game);
            broadcast_presence(st, user.id).await;
        }
    }
}
