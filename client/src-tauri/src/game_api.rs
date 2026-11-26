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
//!
//! The app can also hold a **voice room** itself: a play connection with no game, so friends can
//! talk before launching anything. While in a room, voice runs there rather than in a game's
//! lobby, and the room's host can record feedback for the game they're playing.

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
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::handshake::server::{Request, Response};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::state::AppState;
use crate::voice::{self, VoiceSettings};

/// Request ids the app uses for its own commands; replies to these aren't passed to the game.
const APP_RID: u64 = 1 << 40;
/// Most people in a voice room.
const ROOM_SIZE: u32 = 16;

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
    /// Game titles by id, from the library, for naming games launched without the addon.
    titles: Mutex<HashMap<String, String>>,
    app: AppHandle,
}

struct Launch {
    slug: String,
    /// Lobby to join once connected (the game was launched from an invite or a Join button).
    join: Option<String>,
}

struct GameConn {
    id: u64,
    /// Empty for the voice room connection.
    slug: String,
    /// The app's own voice room connection (no game on the other end).
    room: bool,
    /// Messages for the game that come from the app rather than the server.
    to_game: UnboundedSender<Message>,
    /// Messages for the server that come from the app rather than the game.
    to_server: UnboundedSender<Message>,
    game: Option<PresenceGame>,
    you: Option<PlayUser>,
    /// The lobby this connection is in, and its member id there.
    lobby: Option<(LobbyInfo, u32)>,
    /// The voice room recording this game, as last told to the game.
    room_feedback: Option<String>,
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
    /// The lobby is a voice room rather than a game's lobby.
    pub room: bool,
    pub host: bool,
    pub members: Vec<VoiceMember>,
    /// Feedback recording in progress.
    pub feedback: Option<String>,
    /// The game being recorded.
    pub feedback_game: Option<PresenceGame>,
    /// In a room: the game you're playing, which "Record feedback" would record.
    pub record_game: Option<PresenceGame>,
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

    /// Remember game titles (from the library) to name running games.
    pub fn remember_titles(&self, games: impl IntoIterator<Item = (String, String)>) {
        self.titles.lock().unwrap().extend(games);
    }

    /// The game this player is playing: one connected to multiplayer, else one Condensate
    /// launched.
    fn playing(&self, conns: &[GameConn]) -> Option<PresenceGame> {
        if let Some(game) = conns.iter().filter(|c| !c.room).find_map(|c| c.game.clone()) {
            return Some(game);
        }
        let state = self.app.state::<AppState>();
        let slug = state.running.lock().unwrap().keys().min().cloned()?;
        let title = self.titles.lock().unwrap().get(&slug).cloned().unwrap_or_else(|| slug.clone());
        Some(PresenceGame { slug, title })
    }

    /// Host only: start or stop recording feedback in the current lobby. In a voice room it
    /// records the game you're playing.
    pub fn set_feedback(&self, on: bool) -> Result<(), String> {
        let in_room = {
            let conns = self.conns.lock().unwrap();
            let voice = self.voice.lock().unwrap().conn;
            conns.iter().any(|c| c.room && Some(c.id) == voice)
        };
        let mut cmd = serde_json::json!({ "op": "set_feedback", "on": on, "rid": APP_RID });
        if in_room && on {
            let game = self.playing(&self.conns.lock().unwrap()).ok_or("Start the game first, then record feedback for it.")?;
            cmd["game"] = game.slug.into();
        }
        self.on_voice_conn(cmd)
    }

    /// Start a voice room (`join: None`) or join a friend's. Leaves the room you're in, if any.
    pub async fn open_room(self: &Arc<Self>, join: Option<String>) -> Result<(), String> {
        // Already there (rejoining would drop this connection, ending the room if you host it).
        if let Some(id) = &join {
            let conns = self.conns.lock().unwrap();
            if conns.iter().any(|c| c.room && c.lobby.as_ref().is_some_and(|(l, _)| &l.id == id)) {
                return Ok(());
            }
        }
        let state = self.app.state::<AppState>();
        state.reload(); // the CLI may have logged in since
        let client = state.client()?;
        let server = crate::route::connect_ws(&client, |u| u.replacen("/api/v1/ws?", "/api/v1/play?", 1))
            .await
            .map_err(|e| format!("can't reach the Condensate server: {e}"))?;
        let first = match &join {
            None => serde_json::json!({ "op": "create_lobby", "rid": APP_RID, "max_members": ROOM_SIZE, "visibility": "friends" }),
            Some(lobby) => serde_json::json!({ "op": "join_lobby", "rid": APP_RID, "lobby": lobby }),
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (to_game, _) = unbounded_channel::<Message>(); // nothing on the other end
        let (to_server, for_server) = unbounded_channel::<Message>();
        let _ = to_server.send(command(first));
        self.leave_room();
        self.conns.lock().unwrap().push(GameConn {
            id,
            slug: String::new(),
            room: true,
            to_game,
            to_server,
            game: None,
            you: None,
            lobby: None,
            room_feedback: None,
        });
        let (ready_tx, ready_rx) = oneshot::channel();
        tauri::async_runtime::spawn(run_room(self.clone(), id, server, for_server, ready_tx));
        match tokio::time::timeout(Duration::from_secs(10), ready_rx).await {
            Ok(Ok(result)) => result,
            _ => {
                self.leave_room();
                Err("the server didn't answer".into())
            }
        }
    }

    /// Leave the voice room (it closes for everyone if you're its host).
    pub fn leave_room(&self) {
        let mut left = false;
        for c in self.conns.lock().unwrap().iter_mut().filter(|c| c.room) {
            let _ = c.to_server.send(Message::Close(None));
            left |= c.lobby.take().is_some();
        }
        if left {
            self.update_voice();
            self.sync_room_feedback("");
            self.emit_state(false);
        }
    }

    /// Invite a friend to your voice room.
    pub fn invite_to_room(&self, user: &str) -> Result<(), String> {
        let conns = self.conns.lock().unwrap();
        let c = conns.iter().find(|c| c.room && c.lobby.is_some()).ok_or("Start a voice room first.")?;
        c.to_server
            .send(command(serde_json::json!({ "op": "invite", "user": user, "rid": APP_RID })))
            .map_err(|_| "the voice room disconnected".to_string())
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

    /// Run voice for one connection that's in a lobby, or stop it. The voice room comes first;
    /// otherwise stay on the current connection, else take the first game in a lobby.
    fn update_voice(&self) {
        let conns = self.conns.lock().unwrap();
        let mut link = self.voice.lock().unwrap();
        let in_lobby = |id: u64| conns.iter().any(|c| c.id == id && c.lobby.is_some());
        let want = conns
            .iter()
            .find(|c| c.room && c.lobby.is_some())
            .map(|c| c.id)
            .or(link.conn.filter(|id| in_lobby(*id)))
            .or_else(|| conns.iter().find(|c| c.lobby.is_some()).map(|c| c.id));
        if want.is_some() && want == link.conn && link.session.is_some() {
            return;
        }
        // Stop voice on a connection that left its lobby.
        if let Some(old) = link.conn.take() {
            if let Some(c) = conns.iter().find(|c| c.id == old) {
                let _ = c.to_server.send(command(serde_json::json!({ "op": "voice", "on": false, "rid": APP_RID })));
            }
            link.session = None;
        }
        let Some(c) = want.and_then(|id| conns.iter().find(|c| c.id == id)) else { return };
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
        // The voice connection's lobby, else the room's, else any connection's.
        let conn = link
            .conn
            .and_then(|id| conns.iter().find(|c| c.id == id))
            .or_else(|| conns.iter().find(|c| c.room && c.lobby.is_some()))
            .or_else(|| conns.iter().find(|c| c.lobby.is_some()));
        let room = conn.is_some_and(|c| c.room);
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
            room,
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
            feedback_game: lobby.and_then(|l| l.feedback_game.clone()),
            record_game: if room { self.playing(&conns) } else { None },
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
        let mut by_name = String::new();
        let pass = self
            .with_conn(id, |c| {
                match event {
                    PlayEvent::Welcome { you, game } => {
                        c.you = Some(you);
                        c.game = game;
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
                    PlayEvent::Feedback { session, by, game } => {
                        if let Some((l, _)) = &mut c.lobby {
                            l.feedback = session.clone();
                            l.feedback_game = game.clone();
                        }
                        by_name = by.display_name.clone();
                        let title = game.map(|g| g.title);
                        let _ = self.app.emit("feedback-changed", serde_json::json!({ "session": session, "by": by.display_name, "game": title }));
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
            self.sync_room_feedback(&by_name);
            self.emit_state(false);
        }
        pass
    }

    /// Tell running games whether the voice room is recording them, so they show it and take
    /// F12 screenshots (games only hear about their own lobby from the server).
    fn sync_room_feedback(&self, by: &str) {
        let mut conns = self.conns.lock().unwrap();
        let recording = conns.iter().filter(|c| c.room).find_map(|c| {
            let (l, _) = c.lobby.as_ref()?;
            Some((l.feedback.clone()?, l.feedback_game.as_ref()?.slug.clone()))
        });
        // Only games past `welcome` (so this arrives after it).
        for c in conns.iter_mut().filter(|c| !c.room && c.game.is_some()) {
            let want = recording.as_ref().filter(|(_, slug)| *slug == c.slug).map(|(s, _)| s.clone());
            if want != c.room_feedback {
                c.room_feedback = want.clone();
                if let Ok(text) = serde_json::to_string(&PlayEvent::RoomFeedback { session: want, by: by.into() }) {
                    let _ = c.to_game.send(Message::Text(text.into()));
                }
            }
        }
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

    /// Upload a screenshot the game took to its lobby's feedback recording, or to the voice
    /// room's when that is recording this game.
    fn screenshot(&self, id: u64, frame: &[u8]) {
        let found = {
            let conns = self.conns.lock().unwrap();
            conns.iter().find(|c| c.id == id).map(|c| {
                let lobby = c.lobby.as_ref().and_then(|(l, _)| l.feedback.clone());
                let room = || {
                    conns.iter().filter(|r| r.room).find_map(|r| {
                        let (l, _) = r.lobby.as_ref()?;
                        l.feedback.clone().filter(|_| l.feedback_game.as_ref().is_some_and(|g| g.slug == c.slug))
                    })
                };
                (lobby.or_else(room), c.to_game.clone())
            })
        };
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
        titles: Mutex::new(HashMap::new()),
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
    api.conns.lock().unwrap().push(GameConn { id, slug, room: false, to_game, to_server, game: None, you: None, lobby: None, room_feedback: None });

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

/// The voice room connection: like a game's, with the app on the game's end. `ready` gets the
/// result of creating or joining the room.
async fn run_room(
    api: Arc<GameApi>,
    id: u64,
    server: crate::route::ServerSocket,
    mut for_server: UnboundedReceiver<Message>,
    ready: oneshot::Sender<Result<(), String>>,
) {
    let mut ready = Some(ready);
    let (mut server_tx, mut server_rx) = server.split();
    let mut ping = tokio::time::interval(Duration::from_secs(30));
    loop {
        tokio::select! {
            msg = server_rx.next() => match msg {
                Some(Ok(Message::Binary(frame))) if frame.len() > PACKET_HEADER && frame[5] == MODE_VOICE => api.voice_in(id, &frame),
                Some(Ok(Message::Text(t))) => {
                    match serde_json::from_str::<PlayEvent>(t.as_str()) {
                        Ok(PlayEvent::Error { rid: Some(r), message }) if r >= APP_RID && ready.is_some() => {
                            let _ = ready.take().unwrap().send(Err(message));
                            break;
                        }
                        Ok(PlayEvent::Lobby { .. }) => {
                            if let Some(r) = ready.take() {
                                let _ = r.send(Ok(()));
                            }
                        }
                        Ok(PlayEvent::LobbyClosed { reason }) => {
                            api.observe(id, t.as_str());
                            let _ = api.app.emit("room-closed", reason);
                            break;
                        }
                        _ => {}
                    }
                    api.observe(id, t.as_str());
                }
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => {
                    if ready.is_none() && api.with_conn(id, |c| c.lobby.is_some()).unwrap_or(false) {
                        let _ = api.app.emit("room-closed", "disconnected");
                    }
                    break;
                }
                Some(Ok(_)) => {}
            },
            Some(msg) = for_server.recv() => {
                let close = matches!(msg, Message::Close(_));
                if server_tx.send(msg).await.is_err() || close { break; }
            }
            _ = ping.tick() => {
                if server_tx.send(Message::Text(r#"{"op":"ping"}"#.into())).await.is_err() { break; }
            }
        }
    }
    api.conns.lock().unwrap().retain(|c| c.id != id);
    api.update_voice();
    api.sync_room_feedback("");
    api.emit_state(false);
    let _ = server_tx.close().await;
}
