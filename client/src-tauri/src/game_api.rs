//! The multiplayer API for games on this PC (protocol: `condensate_core::play`).
//!
//! Games connect to a WebSocket on 127.0.0.1. Each connection is logged in as the current user
//! and forwarded to the server's `/api/v1/play` socket, so games never see the login token. This
//! is also where faster routes (direct between players, a UDP relay) will plug in later, without
//! games changing.
//!
//! A game proves it may connect with a token: the one Condensate passed when it launched the
//! game, or the "dev" token in `game-api.json` (for games run from an editor or the CLI).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use condensate_core::play::{PlayEvent, GAME_API_FILE};
use futures::{SinkExt, StreamExt};
use rand::distributions::Alphanumeric;
use rand::Rng;
use tauri::{AppHandle, Manager};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::state::AppState;

pub struct GameApi {
    /// `ws://127.0.0.1:<port>/`
    pub url: String,
    dev_token: String,
    /// Launch token -> the game it was issued for.
    launches: Mutex<HashMap<String, Launch>>,
    conns: Mutex<Vec<GameConn>>,
    next_id: AtomicU64,
}

struct Launch {
    slug: String,
    /// Lobby to join once connected (the game was launched from an invite or a Join button).
    join: Option<String>,
}

struct GameConn {
    id: u64,
    slug: String,
    /// Messages for the game that come from the app rather than the server.
    tx: UnboundedSender<String>,
}

fn random_token() -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).take(40).map(char::from).collect()
}

impl GameApi {
    /// Register a game about to be launched; returns the environment to start it with.
    pub fn launch_env(&self, slug: &str, join: Option<String>) -> (String, Vec<(&'static str, String)>) {
        let token = random_token();
        self.launches.lock().unwrap().insert(token.clone(), Launch { slug: slug.into(), join });
        let env = vec![("CONDENSATE_API", self.url.clone()), ("CONDENSATE_TOKEN", token.clone()), ("CONDENSATE_GAME", slug.into())];
        (token, env)
    }

    /// The game started with this token has exited.
    pub fn forget_launch(&self, token: &str) {
        self.launches.lock().unwrap().remove(token);
    }

    /// Ask running copies of a game to join a lobby. Returns false when none is connected.
    pub fn request_join(&self, slug: &str, lobby: &str) -> bool {
        let Ok(text) = serde_json::to_string(&PlayEvent::JoinRequested { lobby: lobby.into() }) else { return false };
        let conns = self.conns.lock().unwrap();
        let mut sent = false;
        for c in conns.iter().filter(|c| c.slug == slug) {
            sent |= c.tx.send(text.clone()).is_ok();
        }
        sent
    }
}

/// Start listening, and write `game-api.json` for games that weren't launched by the app.
pub fn start(app: &AppHandle, config_dir: &Path) -> std::io::Result<Arc<GameApi>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let api = Arc::new(GameApi {
        url: format!("ws://127.0.0.1:{}/", listener.local_addr()?.port()),
        dev_token: random_token(),
        launches: Mutex::new(HashMap::new()),
        conns: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(1),
    });
    write_api_file(config_dir, &api)?;

    let (app, api2) = (app.clone(), api.clone());
    tauri::async_runtime::spawn(async move {
        let listener = match tokio::net::TcpListener::from_std(listener) {
            Ok(l) => l,
            Err(e) => return eprintln!("game API: {e}"),
        };
        while let Ok((stream, _)) = listener.accept().await {
            tauri::async_runtime::spawn(serve(app.clone(), api2.clone(), stream));
        }
    });
    Ok(api)
}

fn write_api_file(config_dir: &Path, api: &GameApi) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let path = config_dir.join(GAME_API_FILE);
    let json = serde_json::json!({ "api": api.url, "token": api.dev_token });
    std::fs::write(&path, serde_json::to_vec_pretty(&json)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=')).filter(|v| !v.is_empty())
}

fn error_text(message: impl Into<String>) -> Message {
    Message::Text(serde_json::to_string(&PlayEvent::Error { rid: None, message: message.into() }).unwrap_or_default().into())
}

/// One game connection: authenticate it, connect to the server, and pass messages both ways.
async fn serve(app: AppHandle, api: Arc<GameApi>, stream: TcpStream) {
    let mut query = String::new();
    let callback = |req: &Request, resp: Response| {
        query = req.uri().query().unwrap_or_default().to_string();
        Ok(resp)
    };
    let Ok(mut game) = tokio_tungstenite::accept_hdr_async(stream, callback).await else { return };

    // Which game is this?
    let token = query_param(&query, "token").unwrap_or_default();
    let launched = if token == api.dev_token {
        None
    } else {
        let found = api.launches.lock().unwrap().get_mut(token).map(|l| (l.slug.clone(), l.join.take()));
        if found.is_none() {
            let _ = game.send(error_text("not a valid Condensate token; start the game from Condensate")).await;
            return;
        }
        found
    };
    let (slug, mut join) = match launched {
        Some(l) => l,
        None => match query_param(&query, "game") {
            Some(g) => (g.to_string(), None),
            None => {
                let _ = game.send(error_text("say which game this is with ?game=<game id>")).await;
                return;
            }
        },
    };

    // Log in to the server's play socket as the current user.
    let state = app.state::<AppState>();
    state.reload(); // the CLI may have logged in since
    let url = state.client().ok().and_then(|c| c.ws_url()).map(|u| u.replacen("/api/v1/ws?", "/api/v1/play?", 1) + "&game=" + &slug);
    let Some(url) = url else {
        let _ = game.send(error_text("log in to Condensate to play online")).await;
        return;
    };
    let server = match tokio_tungstenite::connect_async(url.as_str()).await {
        Ok((s, _)) => s,
        Err(e) => {
            let message = match e {
                WsError::Http(r) if r.status() == 404 => format!("Condensate has no game called \"{slug}\" (check the game id)"),
                WsError::Http(r) if r.status() == 401 => "log in to Condensate to play online".to_string(),
                e => format!("can't reach the Condensate server: {e}"),
            };
            let _ = game.send(error_text(message)).await;
            return;
        }
    };

    let id = api.next_id.fetch_add(1, Ordering::Relaxed);
    let (inject, mut injected) = unbounded_channel::<String>();
    api.conns.lock().unwrap().push(GameConn { id, slug, tx: inject });

    let (mut game_tx, mut game_rx) = game.split();
    let (mut server_tx, mut server_rx) = server.split();
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            msg = game_rx.next() => match msg {
                Some(Ok(msg @ (Message::Text(_) | Message::Binary(_)))) => {
                    if server_tx.send(msg).await.is_err() { break; }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            msg = server_rx.next() => match msg {
                Some(Ok(msg @ (Message::Text(_) | Message::Binary(_)))) => {
                    let welcome = matches!(&msg, Message::Text(t) if t.as_str().starts_with(r#"{"op":"welcome""#));
                    if game_tx.send(msg).await.is_err() { break; }
                    // Launched from an invite: join once the game knows who it is.
                    if let (true, Some(lobby)) = (welcome, join.take()) {
                        let text = serde_json::to_string(&PlayEvent::JoinRequested { lobby }).unwrap_or_default();
                        if game_tx.send(Message::Text(text.into())).await.is_err() { break; }
                    }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                    let _ = game_tx.send(error_text("lost the connection to the Condensate server")).await;
                    break;
                }
                Some(Ok(_)) => {}
            },
            Some(text) = injected.recv() => {
                if game_tx.send(Message::Text(text.into())).await.is_err() { break; }
            }
            _ = ping.tick() => {
                if server_tx.send(Message::Text(r#"{"op":"ping"}"#.into())).await.is_err() { break; }
            }
        }
    }

    api.conns.lock().unwrap().retain(|c| c.id != id);
    let _ = game_tx.close().await;
    let _ = server_tx.close().await;
}
