//! The multiplayer API for games on this PC (protocol: `condensate_core::play`).
//!
//! Games connect to a WebSocket on 127.0.0.1. Each connection is logged in as the current user
//! and forwarded to the server's `/api/v1/play` socket, so games never see the login token. This
//! is also where faster routes (direct between players, a UDP relay) will plug in later, without
//! games changing.
//!
//! A game proves it may connect with a token: the one Condensate passed when it launched the
//! game, or the "dev" token in `game-api.json` (for games run from an editor or the CLI).
//!
//! The app also follows each connection's lobby, so it can:
//! - run voice chat for the lobby: voice frames ride the same server connection, and the app
//!   plays and captures audio itself (`voice.rs`), so games need no audio code;
//! - show the lobby in its own UI, where the host can start a feedback recording;
//! - upload feedback screenshots the game sends (over HTTP, so they don't hold up game traffic).

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use condensate_core::api::PresenceGame;
use condensate_core::play::{LobbyInfo, PlayEvent, PlayUser, GAME_API_FILE, MODE_SCREENSHOT, MODE_VOICE, PACKET_HEADER};
use futures::{SinkExt, StreamExt};
use rand::distributions::Alphanumeric;
use rand::Rng;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::state::AppState;
use crate::voice::{self, VoiceSettings};

/// Request ids the app uses for its own commands; replies to these aren't passed to the game.
const APP_RID: u64 = 1 << 40;

pub struct GameApi {
    /// `ws://127.0.0.1:<port>/`
    pub url: String,
    dev_token: String,
    /// Launch token -> the game it was issued for.
    launches: Mutex<HashMap<String, Launch>>,
    conns: Mutex<Vec<GameConn>>,
    next_id: AtomicU64,
    pub voice_settings: Arc<Mutex<VoiceSettings>>,
    voice: Mutex<VoiceLink>,
    app: AppHandle,
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
    to_game: UnboundedSender<Message>,
    /// Messages for the server that come from the app rather than the game.
    to_server: UnboundedSender<Message>,
    game: Option<PresenceGame>,
    you: Option<PlayUser>,
    /// The lobby this connection is in, and its member id there.
    lobby: Option<(LobbyInfo, u32)>,
}

/// Which connection carries voice, and the audio running for it.
#[derive(Default)]
struct VoiceLink {
    conn: Option<u64>,
    session: Option<voice::Session>,
    /// A microphone test from the settings page (no lobby).
    test: Option<voice::Session>,
    /// What the UI was last told, to only emit changes.
    last_state: Option<String>,
}

/// The lobby and voice chat, for the UI.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VoiceState {
    pub game: Option<PresenceGame>,
    pub lobby: Option<String>,
    pub host: bool,
    pub members: Vec<VoiceMember>,
    /// Feedback recording in progress.
    pub feedback: Option<String>,
    /// Voice is running (in a lobby).
    pub connected: bool,
    pub muted: bool,
    pub deafened: bool,
    /// Problems with the microphone or speakers.
    pub problems: Vec<String>,
    /// Microphone level, dBFS (rounded), while voice or a mic test is running.
    pub level: Option<i32>,
    /// The player is talking.
    pub talking: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VoiceMember {
    pub id: u32,
    pub username: String,
    pub display_name: String,
    pub you: bool,
    pub voice: bool,
    pub recorded: bool,
    pub speaking: bool,
}

fn random_token() -> String {
    rand::thread_rng().sample_iter(&Alphanumeric).take(40).map(char::from).collect()
}

fn command(json: serde_json::Value) -> Message {
    Message::Text(json.to_string().into())
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
            sent |= c.to_game.send(Message::Text(text.clone().into())).is_ok();
        }
        sent
    }

    /// Send a command to the server on the voice connection's behalf.
    fn on_voice_conn(&self, cmd: serde_json::Value) -> Result<(), String> {
        let conn = self.voice.lock().unwrap().conn.ok_or("you're not in a lobby")?;
        let conns = self.conns.lock().unwrap();
        let c = conns.iter().find(|c| c.id == conn).ok_or("you're not in a lobby")?;
        c.to_server.send(command(cmd)).map_err(|_| "the game disconnected".to_string())
    }

    /// Host only: start or stop recording feedback in the current lobby.
    pub fn set_feedback(&self, on: bool) -> Result<(), String> {
        self.on_voice_conn(serde_json::json!({ "op": "set_feedback", "on": on, "rid": APP_RID }))
    }

    /// Include (or leave out) this player's voice in the lobby's feedback recordings.
    pub fn set_record_me(&self, on: bool) -> Result<(), String> {
        self.on_voice_conn(serde_json::json!({ "op": "record_me", "on": on, "rid": APP_RID }))
    }

    /// Settings changed. Device changes restart the audio.
    pub fn apply_voice_settings(&self, new: VoiceSettings) {
        let devices_changed = {
            let mut s = self.voice_settings.lock().unwrap();
            let changed = s.input != new.input || s.output != new.output;
            *s = new;
            changed
        };
        if devices_changed {
            let mut link = self.voice.lock().unwrap();
            if link.session.take().is_some() {
                drop(link);
                self.update_voice();
            } else if link.test.take().is_some() {
                link.test = Some(voice::Session::start(self.voice_settings.clone(), None));
            }
        }
        self.emit_state(true);
    }

    /// Start or stop the microphone test (settings page).
    pub fn mic_test(&self, on: bool) {
        let mut link = self.voice.lock().unwrap();
        link.test = (on && link.session.is_none()).then(|| voice::Session::start(self.voice_settings.clone(), None));
    }

    /// Run voice for one connection that's in a lobby (the first, if several are), or stop it.
    fn update_voice(&self) {
        let conns = self.conns.lock().unwrap();
        let mut link = self.voice.lock().unwrap();
        let current = link.conn.filter(|id| conns.iter().any(|c| c.id == *id && c.lobby.is_some()));
        if current.is_some() && link.session.is_some() {
            return;
        }
        // Stop voice on a connection that left its lobby.
        if let Some(old) = link.conn.take() {
            if let Some(c) = conns.iter().find(|c| c.id == old) {
                let _ = c.to_server.send(command(serde_json::json!({ "op": "voice", "on": false, "rid": APP_RID })));
            }
            link.session = None;
        }
        let Some(c) = conns.iter().find(|c| c.lobby.is_some()) else { return };
        link.test = None;
        let _ = c.to_server.send(command(serde_json::json!({ "op": "voice", "on": true, "rid": APP_RID })));
        let to_server = c.to_server.clone();
        let send = Box::new(move |packet: Vec<u8>| {
            let mut frame = vec![0u8; PACKET_HEADER];
            frame[5] = MODE_VOICE;
            frame.extend_from_slice(&packet);
            let _ = to_server.send(Message::Binary(frame.into()));
        });
        link.conn = Some(c.id);
        link.session = Some(voice::Session::start(self.voice_settings.clone(), Some(send)));
    }

    pub fn state(&self) -> VoiceState {
        let conns = self.conns.lock().unwrap();
        let link = self.voice.lock().unwrap();
        let s = self.voice_settings.lock().unwrap().clone();
        // The voice connection's lobby, else any connection's.
        let conn = link.conn.and_then(|id| conns.iter().find(|c| c.id == id)).or_else(|| conns.iter().find(|c| c.lobby.is_some()));
        let speaking = link.session.as_ref().map(|v| v.speaking()).unwrap_or_default();
        let meters = link.session.as_ref().or(link.test.as_ref()).map(|v| v.meters.clone());
        let talking = meters.as_ref().is_some_and(|m| m.speaking.load(Ordering::Relaxed));
        let (lobby, you) = match conn.and_then(|c| c.lobby.as_ref()) {
            Some((l, you)) => (Some(l), *you),
            None => (None, 0),
        };
        VoiceState {
            game: conn.and_then(|c| c.game.clone()),
            lobby: lobby.map(|l| l.id.clone()),
            host: you == 1,
            members: lobby
                .map(|l| {
                    l.members
                        .iter()
                        .map(|m| VoiceMember {
                            id: m.id,
                            username: m.username.clone(),
                            display_name: m.display_name.clone(),
                            you: m.id == you,
                            voice: m.voice,
                            recorded: m.recorded,
                            speaking: if m.id == you { talking && link.session.is_some() } else { speaking.contains(&m.id) },
                        })
                        .collect()
                })
                .unwrap_or_default(),
            feedback: lobby.and_then(|l| l.feedback.clone()),
            connected: link.session.is_some(),
            muted: s.muted,
            deafened: s.deafened,
            problems: link.session.as_ref().or(link.test.as_ref()).map(|v| v.problems.clone()).unwrap_or_default(),
            level: meters.map(|m| m.level_db().round() as i32),
            talking,
        }
    }

    /// Tell the UI about the lobby and voice, if anything changed (or always, with `force`).
    fn emit_state(&self, force: bool) {
        let state = self.state();
        let Ok(json) = serde_json::to_string(&state) else { return };
        {
            let mut link = self.voice.lock().unwrap();
            if !force && link.last_state.as_deref() == Some(json.as_str()) {
                return;
            }
            link.last_state = Some(json);
        }
        let _ = self.app.emit("voice-state", &state);
    }

    fn with_conn<R>(&self, id: u64, f: impl FnOnce(&mut GameConn) -> R) -> Option<R> {
        self.conns.lock().unwrap().iter_mut().find(|c| c.id == id).map(f)
    }

    /// Follow lobby changes in a message from the server. Returns false for replies to the app's
    /// own requests, which the game shouldn't see.
    fn observe(&self, id: u64, text: &str) -> bool {
        let Ok(event) = serde_json::from_str::<PlayEvent>(text) else { return true };
        if let PlayEvent::MemberLeft { member } = &event {
            let link = self.voice.lock().unwrap();
            if let (true, Some(session)) = (link.conn == Some(id), &link.session) {
                session.forget(*member);
            }
        }
        let mut lobby_changed = true;
        let pass = self
            .with_conn(id, |c| {
                match event {
                    PlayEvent::Welcome { you, game } => {
                        c.you = Some(you);
                        c.game = Some(game);
                    }
                    PlayEvent::Lobby { lobby, you, .. } => c.lobby = Some((lobby, you)),
                    PlayEvent::LobbyClosed { .. } => c.lobby = None,
                    PlayEvent::MemberJoined { member } => {
                        if let Some((l, _)) = &mut c.lobby {
                            l.members.push(member);
                        }
                    }
                    PlayEvent::MemberLeft { member } => {
                        if let Some((l, _)) = &mut c.lobby {
                            l.members.retain(|m| m.id != member);
                        }
                    }
                    PlayEvent::MemberUpdated { member } => {
                        if let Some((l, _)) = &mut c.lobby {
                            if let Some(m) = l.members.iter_mut().find(|m| m.id == member.id) {
                                *m = member;
                            }
                        }
                    }
                    PlayEvent::Feedback { session, by } => {
                        if let Some((l, _)) = &mut c.lobby {
                            l.feedback = session.clone();
                        }
                        let _ = self.app.emit("feedback-changed", serde_json::json!({ "session": session, "by": by.display_name }));
                    }
                    PlayEvent::Ok { rid: Some(r) } if r >= APP_RID => {
                        lobby_changed = false;
                        return false;
                    }
                    PlayEvent::Error { rid: Some(r), message } if r >= APP_RID => {
                        let _ = self.app.emit("voice-error", message);
                        lobby_changed = false;
                        return false;
                    }
                    _ => lobby_changed = false,
                }
                true
            })
            .unwrap_or(true);
        if lobby_changed {
            self.update_voice();
            self.emit_state(false);
        }
        pass
    }

    /// A message from the game, before it goes to the server.
    fn observe_game(&self, id: u64, text: &str) {
        if text.contains(r#""leave_lobby""#) && serde_json::from_str::<serde_json::Value>(text).is_ok_and(|v| v["op"] == "leave_lobby") {
            self.with_conn(id, |c| c.lobby = None);
            self.update_voice();
            self.emit_state(false);
        }
    }

    fn voice_in(&self, id: u64, frame: &[u8]) {
        let link = self.voice.lock().unwrap();
        if link.conn == Some(id) {
            if let Some(session) = &link.session {
                let member = u32::from_le_bytes(frame[0..4].try_into().unwrap_or_default());
                session.incoming(member, &frame[PACKET_HEADER..]);
            }
        }
    }

    /// Upload a screenshot the game took to the lobby's feedback recording.
    fn screenshot(&self, id: u64, frame: &[u8]) {
        let found = self.with_conn(id, |c| (c.lobby.as_ref().and_then(|(l, _)| l.feedback.clone()), c.to_game.clone()));
        let Some((session, to_game)) = found else { return };
        let tell = move |message: String| {
            let text = serde_json::to_string(&PlayEvent::Error { rid: None, message }).unwrap_or_default();
            let _ = to_game.send(Message::Text(text.into()));
        };
        let Some(session) = session else {
            return tell("feedback recording isn't on, so the screenshot wasn't saved".into());
        };
        let client = match self.app.state::<AppState>().client() {
            Ok(c) => c,
            Err(_) => return tell("log in to Condensate to save screenshots".into()),
        };
        let body = frame[PACKET_HEADER..].to_vec();
        tauri::async_runtime::spawn(async move {
            if let Err(e) = client.upload_feedback_shot(&session, body).await {
                tell(format!("couldn't save the screenshot: {e:#}"));
            }
        });
    }
}

/// Start listening, and write `game-api.json` for games that weren't launched by the app.
pub fn start(app: &AppHandle, config_dir: &Path, voice_settings: VoiceSettings) -> std::io::Result<Arc<GameApi>> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let api = Arc::new(GameApi {
        url: format!("ws://127.0.0.1:{}/", listener.local_addr()?.port()),
        dev_token: random_token(),
        launches: Mutex::new(HashMap::new()),
        conns: Mutex::new(Vec::new()),
        next_id: AtomicU64::new(1),
        voice_settings: Arc::new(Mutex::new(voice_settings)),
        voice: Mutex::new(VoiceLink::default()),
        app: app.clone(),
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
    // Speaking indicators and the mic meter change without any message; poll them for the UI.
    let api3 = api.clone();
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            tick.tick().await;
            api3.emit_state(false);
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
    let Ok(client) = state.client() else {
        let _ = game.send(error_text("log in to Condensate to play online")).await;
        return;
    };
    let play_url = |u: String| u.replacen("/api/v1/ws?", "/api/v1/play?", 1) + "&game=" + &slug;
    let server = match crate::route::connect_ws(&client, play_url).await {
        Ok(s) => s,
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
    let (to_game, mut for_game) = unbounded_channel::<Message>();
    let (to_server, mut for_server) = unbounded_channel::<Message>();
    api.conns.lock().unwrap().push(GameConn { id, slug, to_game, to_server, game: None, you: None, lobby: None });

    let (mut game_tx, mut game_rx) = game.split();
    let (mut server_tx, mut server_rx) = server.split();
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            msg = game_rx.next() => match msg {
                Some(Ok(Message::Binary(frame))) if frame.len() >= PACKET_HEADER && frame[5] == MODE_SCREENSHOT => api.screenshot(id, &frame),
                Some(Ok(Message::Binary(frame))) if frame.len() >= PACKET_HEADER && frame[5] == MODE_VOICE => {} // only the app sends voice
                Some(Ok(msg @ (Message::Text(_) | Message::Binary(_)))) => {
                    if let Message::Text(t) = &msg {
                        api.observe_game(id, t.as_str());
                    }
                    if server_tx.send(msg).await.is_err() { break; }
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
            msg = server_rx.next() => match msg {
                Some(Ok(Message::Binary(frame))) if frame.len() > PACKET_HEADER && frame[5] == MODE_VOICE => api.voice_in(id, &frame),
                Some(Ok(msg @ (Message::Text(_) | Message::Binary(_)))) => {
                    let (welcome, pass) = match &msg {
                        Message::Text(t) => (t.as_str().starts_with(r#"{"op":"welcome""#), api.observe(id, t.as_str())),
                        _ => (false, true),
                    };
                    if pass && game_tx.send(msg).await.is_err() { break; }
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
            Some(msg) = for_game.recv() => {
                if game_tx.send(msg).await.is_err() { break; }
            }
            Some(msg) = for_server.recv() => {
                if server_tx.send(msg).await.is_err() { break; }
            }
            _ = ping.tick() => {
                if server_tx.send(Message::Text(r#"{"op":"ping"}"#.into())).await.is_err() { break; }
            }
        }
    }

    api.conns.lock().unwrap().retain(|c| c.id != id);
    api.update_voice();
    api.emit_state(false);
    let _ = game_tx.close().await;
    let _ = server_tx.close().await;
}
