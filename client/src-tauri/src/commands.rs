//! Commands invoked from the React UI. Errors are plain strings; an error starting with
//! [`NOT_LOGGED_IN`] tells the UI to show the login screen.

use condensate_core::api::*;
use condensate_core::install::read_installed;
use condensate_core::platform;
use condensate_sdk::pull::{PullEvent, PullOptions, PullReport};
use condensate_sdk::{ApiError, Client};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_opener::OpenerExt;

use std::sync::Arc;

use crate::game_api::GameApi;
use crate::state::{AppState, Job};

pub type CmdResult<T> = Result<T, String>;

pub const NOT_LOGGED_IN: &str = "NOT_LOGGED_IN";

/// Turn an SDK error into a UI message, flagging expired sessions.
fn err(e: anyhow::Error) -> String {
    if let Some(api) = e.downcast_ref::<ApiError>() {
        if api.status == 401 {
            return format!("{NOT_LOGGED_IN}: {}", api.message);
        }
        return api.message.clone();
    }
    format!("{e:#}")
}

fn current_platform() -> CmdResult<&'static str> {
    platform::current().ok_or_else(|| "this operating system is not supported yet".to_string())
}

// ---- session ------------------------------------------------------------------------------------

#[derive(Serialize)]
pub struct Session {
    server: String,
    user: Option<User>,
    library_dir: String,
    platform: Option<&'static str>,
}

/// Whether the app is talking to the server's direct address (a port forward) right now,
/// rather than the main URL. Changes are announced with the `connection` event.
#[tauri::command]
pub fn connection_direct(app: AppHandle) -> bool {
    crate::route::is_direct(&app)
}

#[tauri::command]
pub async fn session(app: AppHandle, state: State<'_, AppState>) -> CmdResult<Session> {
    state.reload();
    let server = state.server();
    let user = match state.client() {
        Ok(client) => match client.me().await {
            Ok(u) => Some(u),
            Err(e) if e.downcast_ref::<ApiError>().is_some_and(|a| a.status == 401) => {
                state.update_settings(|s| s.token = None)?;
                None
            }
            Err(e) => return Err(format!("Can't reach {server}: {}", err(e))),
        },
        Err(_) => None,
    };
    if user.is_some() {
        crate::realtime::start(&app);
    }
    Ok(Session {
        server,
        user,
        library_dir: state.library_dir().display().to_string(),
        platform: platform::current(),
    })
}

fn save_login(state: &AppState, server: &str, resp: &AuthResponse) -> CmdResult<()> {
    state.update_settings(|s| {
        s.server = server.to_string();
        s.token = Some(resp.token.clone());
        s.username = Some(resp.user.username.clone());
    })
}

fn normalize_server(server: &str) -> CmdResult<String> {
    let s = server.trim().trim_end_matches('/');
    if !(s.starts_with("https://") || s.starts_with("http://")) {
        return Err("server address must start with https://".into());
    }
    Ok(s.to_string())
}

#[tauri::command]
pub async fn login(state: State<'_, AppState>, server: String, username: String, password: String) -> CmdResult<User> {
    let server = normalize_server(&server)?;
    let client = Client::new(&server, None).map_err(err)?;
    let resp = client.login(&LoginRequest { username, password }).await.map_err(err)?;
    save_login(&state, &server, &resp)?;
    Ok(resp.user)
}

#[tauri::command]
pub async fn register(
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
    invite: String,
) -> CmdResult<User> {
    let server = normalize_server(&server)?;
    let client = Client::new(&server, None).map_err(err)?;
    let req = RegisterRequest { username, password, display_name: None, invite_code: Some(invite) };
    let resp = client.register(&req).await.map_err(err)?;
    save_login(&state, &server, &resp)?;
    Ok(resp.user)
}

#[tauri::command]
pub async fn logout(app: AppHandle, state: State<'_, AppState>) -> CmdResult<()> {
    crate::realtime::stop(&app);
    if let Ok(client) = state.client() {
        let _ = client.logout().await;
    }
    state.update_settings(|s| s.token = None)
}

// ---- library & store -------------------------------------------------------------------------

#[derive(Serialize)]
pub struct InstalledInfo {
    version: String,
    size: u64,
    dir: String,
}

#[derive(Serialize)]
pub struct LibraryItem {
    game: GameSummary,
    latest: Option<LatestBuild>,
    installed: Option<InstalledInfo>,
    update_available: bool,
    job: Option<Job>,
    running: bool,
    playtime_seconds: i64,
    last_played_at: Option<i64>,
}

#[tauri::command]
pub async fn library(state: State<'_, AppState>) -> CmdResult<Vec<LibraryItem>> {
    let client = state.client()?;
    let entries = client.library(platform::current()).await.map_err(err)?;
    let jobs = state.jobs.lock().unwrap().clone();
    let running = state.running.lock().unwrap().clone();
    Ok(entries
        .into_iter()
        .map(|e| {
            let dir = state.game_dir(&e.game.slug);
            let installed = read_installed(&dir).ok().flatten().filter(|s| s.game == e.game.slug);
            let update_available = match (&installed, &e.latest) {
                (Some(i), Some(l)) => i.manifest.hash() != l.manifest_hash || i.version != l.version,
                _ => false,
            };
            LibraryItem {
                installed: installed.map(|s| InstalledInfo {
                    version: s.version,
                    size: s.manifest.total_size(),
                    dir: dir.display().to_string(),
                }),
                update_available,
                job: jobs.get(&e.game.slug).cloned(),
                running: running.contains_key(&e.game.slug),
                playtime_seconds: e.playtime_seconds,
                last_played_at: e.last_played_at,
                latest: e.latest,
                game: e.game,
            }
        })
        .collect())
}

#[tauri::command]
pub async fn store(state: State<'_, AppState>, query: Option<String>) -> CmdResult<Vec<GameSummary>> {
    state.client()?.search_games(query.as_deref()).await.map_err(err)
}

#[tauri::command]
pub async fn game(state: State<'_, AppState>, slug: String) -> CmdResult<GameDetail> {
    state.client()?.game(&slug).await.map_err(err)
}

#[tauri::command]
pub async fn add_to_library(state: State<'_, AppState>, slug: String) -> CmdResult<()> {
    state.client()?.library_add(&slug).await.map_err(err)
}

#[tauri::command]
pub async fn remove_from_library(state: State<'_, AppState>, slug: String) -> CmdResult<()> {
    state.client()?.library_remove(&slug).await.map_err(err)
}

// ---- install / update / uninstall -----------------------------------------------------------

#[derive(Clone, Serialize)]
struct JobFinished {
    slug: String,
    ok: bool,
    error: Option<String>,
    report: Option<PullReport>,
}

/// Start installing or updating a game. Progress arrives as `job` events and the end as a
/// `job-finished` event.
#[tauri::command]
pub async fn install(app: AppHandle, state: State<'_, AppState>, slug: String) -> CmdResult<()> {
    let client = state.client()?;
    let platform = current_platform()?;
    if state.running.lock().unwrap().contains_key(&slug) {
        return Err("close the game before updating it".into());
    }
    {
        let mut jobs = state.jobs.lock().unwrap();
        if jobs.contains_key(&slug) {
            return Err("already installing".into());
        }
        let job = Job { slug: slug.clone(), phase: "checking".into(), done_bytes: 0, total_bytes: 0, reused_bytes: 0 };
        jobs.insert(slug.clone(), job.clone());
        let _ = app.emit("job", job);
    }
    let dir = state.game_dir(&slug);

    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let on_event = |e: PullEvent| {
            let mut jobs = state.jobs.lock().unwrap();
            let Some(job) = jobs.get_mut(&slug) else { return };
            match e {
                PullEvent::Checking => job.phase = "checking".into(),
                PullEvent::Planned { download_bytes, reused_bytes, .. } => {
                    job.phase = "downloading".into();
                    job.total_bytes = download_bytes;
                    job.reused_bytes = reused_bytes;
                }
                PullEvent::Downloading { done_bytes, total_bytes, .. } => {
                    job.phase = "downloading".into();
                    job.done_bytes = done_bytes;
                    job.total_bytes = total_bytes;
                }
                PullEvent::Applying => job.phase = "installing".into(),
            }
            let _ = app.emit("job", job.clone());
        };
        let opts = PullOptions { game: slug.clone(), platform: platform.into(), dir, repair: false };
        let result = condensate_sdk::pull::pull(&client, opts, &on_event).await;
        state.jobs.lock().unwrap().remove(&slug);
        let finished = match result {
            Ok(report) => JobFinished { slug, ok: true, error: None, report: Some(report) },
            Err(e) => JobFinished { slug, ok: false, error: Some(err(e)), report: None },
        };
        let _ = app.emit("job-finished", finished);
    });
    Ok(())
}

#[tauri::command]
pub async fn uninstall(state: State<'_, AppState>, slug: String) -> CmdResult<()> {
    if state.running.lock().unwrap().contains_key(&slug) {
        return Err("close the game first".into());
    }
    if state.jobs.lock().unwrap().contains_key(&slug) {
        return Err("wait for the download to finish".into());
    }
    let dir = state.game_dir(&slug);
    if dir.exists() {
        tauri::async_runtime::spawn_blocking(move || std::fs::remove_dir_all(dir))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("couldn't delete the game files: {e}"))?;
    }
    Ok(())
}

// ---- play -----------------------------------------------------------------------------------

#[derive(Clone, Serialize)]
struct GameExited {
    slug: String,
    code: Option<i32>,
}

#[tauri::command]
pub async fn play(app: AppHandle, state: State<'_, AppState>, slug: String) -> CmdResult<()> {
    start_game(app, &state, slug, None)
}

/// Join a friend's multiplayer lobby: hand it to the game if it's already running, otherwise
/// launch the game and let it join once it connects.
#[tauri::command]
pub async fn join_lobby(app: AppHandle, state: State<'_, AppState>, slug: String, lobby: String) -> CmdResult<()> {
    if app.state::<Arc<GameApi>>().request_join(&slug, &lobby) {
        return Ok(());
    }
    if state.running.lock().unwrap().contains_key(&slug) {
        return Err("The game is running but isn't connected to Condensate multiplayer.".into());
    }
    if read_installed(&state.game_dir(&slug)).ok().flatten().is_none() {
        return Err("Install the game first, then join again.".into());
    }
    start_game(app, &state, slug, Some(lobby))
}

fn start_game(app: AppHandle, state: &AppState, slug: String, join: Option<String>) -> CmdResult<()> {
    if state.running.lock().unwrap().contains_key(&slug) {
        return Err("already running".into());
    }
    let dir = state.game_dir(&slug);
    let api = app.state::<Arc<GameApi>>().inner().clone();
    let (launch_token, env) = api.launch_env(&slug, join);
    let mut child = match condensate_sdk::launch::launch(&dir, &env) {
        Ok(child) => child,
        Err(e) => {
            api.forget_launch(&launch_token);
            return Err(err(e));
        }
    };
    state.running.lock().unwrap().insert(slug.clone(), child.id());
    let _ = app.emit("game-started", &slug);
    crate::realtime::send(&app, WsCommand::Playing { game: Some(slug.clone()) });
    let client = state.client().ok();
    let started = std::time::Instant::now();
    std::thread::spawn(move || {
        let code = child.wait().ok().and_then(|s| s.code());
        api.forget_launch(&launch_token);
        let seconds = started.elapsed().as_secs() as i64;
        let state = app.state::<AppState>();
        state.running.lock().unwrap().remove(&slug);
        let still_playing = state.running.lock().unwrap().keys().next().cloned();
        crate::realtime::send(&app, WsCommand::Playing { game: still_playing });
        if let Some(client) = client {
            let slug = slug.clone();
            tauri::async_runtime::spawn(async move {
                let _ = client.report_playtime(&slug, seconds).await;
            });
        }
        let _ = app.emit("game-exited", GameExited { slug, code });
    });
    Ok(())
}

#[tauri::command]
pub async fn open_game_folder(app: AppHandle, state: State<'_, AppState>, slug: String) -> CmdResult<()> {
    let dir = state.game_dir(&slug);
    app.opener().open_path(dir.display().to_string(), None::<&str>).map_err(|e| e.to_string())
}

// ---- account ----------------------------------------------------------------------------------

#[tauri::command]
pub async fn create_invite(state: State<'_, AppState>) -> CmdResult<Invite> {
    state.client()?.create_invite().await.map_err(err)
}

#[tauri::command]
pub async fn list_invites(state: State<'_, AppState>) -> CmdResult<Vec<Invite>> {
    state.client()?.list_invites().await.map_err(err)
}

#[tauri::command]
pub async fn update_profile(
    state: State<'_, AppState>,
    display_name: Option<String>,
    bio: Option<String>,
    avatar_image: Option<String>,
) -> CmdResult<User> {
    state.client()?.update_me(&UpdateProfileRequest { display_name, bio, avatar_image }).await.map_err(err)
}

/// Upload an image sent as the raw request body (an ArrayBuffer from the UI); returns its id.
#[tauri::command]
pub async fn upload_image(state: State<'_, AppState>, request: tauri::ipc::Request<'_>) -> CmdResult<String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("expected raw image bytes".into());
    };
    state.client()?.upload_image(bytes.clone()).await.map_err(err)
}

// ---- friends & chat ---------------------------------------------------------------------------

#[tauri::command]
pub async fn friends(state: State<'_, AppState>) -> CmdResult<FriendsResponse> {
    state.client()?.friends().await.map_err(err)
}

#[tauri::command]
pub async fn search_users(state: State<'_, AppState>, query: String) -> CmdResult<Vec<User>> {
    state.client()?.search_users(&query).await.map_err(err)
}

#[tauri::command]
pub async fn add_friend(state: State<'_, AppState>, username: String) -> CmdResult<()> {
    state.client()?.add_friend(&username).await.map_err(err)
}

#[tauri::command]
pub async fn remove_friend(state: State<'_, AppState>, username: String) -> CmdResult<()> {
    state.client()?.remove_friend(&username).await.map_err(err)
}

#[tauri::command]
pub async fn conversations(state: State<'_, AppState>) -> CmdResult<Vec<Conversation>> {
    state.client()?.conversations().await.map_err(err)
}

#[tauri::command]
pub async fn messages(state: State<'_, AppState>, username: String, before: Option<i64>) -> CmdResult<Vec<ChatMessage>> {
    state.client()?.messages(&username, before).await.map_err(err)
}

#[tauri::command]
pub async fn send_message(state: State<'_, AppState>, username: String, body: String) -> CmdResult<ChatMessage> {
    state.client()?.send_message(&username, &body).await.map_err(err)
}

#[tauri::command]
pub async fn mark_read(state: State<'_, AppState>, username: String) -> CmdResult<()> {
    state.client()?.mark_read(&username).await.map_err(err)
}

#[tauri::command]
pub async fn profile(state: State<'_, AppState>, username: String) -> CmdResult<UserProfile> {
    state.client()?.profile(&username).await.map_err(err)
}

// ---- studio (your games) ------------------------------------------------------------------------

#[tauri::command]
pub async fn my_games(state: State<'_, AppState>) -> CmdResult<Vec<GameSummary>> {
    state.client()?.my_games().await.map_err(err)
}

#[tauri::command]
pub async fn create_game(state: State<'_, AppState>, slug: String, title: String) -> CmdResult<GameDetail> {
    let req = CreateGameRequest { slug, title, summary: None, description: None, visibility: None };
    state.client()?.create_game(&req).await.map_err(err)
}

#[tauri::command]
pub async fn update_game(state: State<'_, AppState>, slug: String, changes: UpdateGameRequest) -> CmdResult<GameDetail> {
    state.client()?.update_game(&slug, &changes).await.map_err(err)
}

#[tauri::command]
pub async fn delete_version(state: State<'_, AppState>, slug: String, version: String) -> CmdResult<()> {
    state.client()?.delete_version(&slug, &version).await.map_err(err)
}

#[tauri::command]
pub async fn list_tokens(state: State<'_, AppState>) -> CmdResult<Vec<TokenInfo>> {
    state.client()?.list_tokens().await.map_err(err)
}

#[tauri::command]
pub async fn create_token(state: State<'_, AppState>, name: String, game: Option<String>) -> CmdResult<CreatedToken> {
    state.client()?.create_token(&CreateTokenRequest { name, game }).await.map_err(err)
}

#[tauri::command]
pub async fn revoke_token(state: State<'_, AppState>, id: i64) -> CmdResult<()> {
    state.client()?.revoke_token(id).await.map_err(err)
}

// ---- settings ---------------------------------------------------------------------------------

/// Move the games folder, taking installed games along.
#[tauri::command]
pub async fn set_library_dir(state: State<'_, AppState>, path: String) -> CmdResult<String> {
    if !state.jobs.lock().unwrap().is_empty() || !state.running.lock().unwrap().is_empty() {
        return Err("wait for downloads to finish and close running games first".into());
    }
    let old = state.library_dir();
    let new = std::path::PathBuf::from(&path);
    if new == old {
        return Ok(path);
    }
    if new.starts_with(&old) {
        return Err("pick a folder outside the current games folder".into());
    }
    let (o, n) = (old.clone(), new.clone());
    tauri::async_runtime::spawn_blocking(move || crate::state::move_games(&o, &n))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| format!("couldn't move games: {e:#}"))?;
    state.update_settings(|s| s.library_dir = Some(new))?;
    Ok(path)
}

// ---- voice chat and feedback recordings -------------------------------------------------------

fn game_api(app: &AppHandle) -> Arc<GameApi> {
    app.state::<Arc<GameApi>>().inner().clone()
}

/// The lobby a running game is in, and voice chat (also pushed as `voice-state` events).
#[tauri::command]
pub fn voice_state(app: AppHandle) -> crate::game_api::VoiceState {
    game_api(&app).state()
}

#[tauri::command]
pub fn voice_devices() -> crate::voice::Devices {
    crate::voice::devices()
}

#[tauri::command]
pub fn voice_settings(app: AppHandle) -> crate::voice::VoiceSettings {
    game_api(&app).voice_settings.lock().unwrap().clone()
}

#[tauri::command]
pub fn set_voice_settings(app: AppHandle, state: State<'_, AppState>, settings: crate::voice::VoiceSettings) -> CmdResult<()> {
    let mut settings = settings;
    settings.volume = settings.volume.clamp(0.0, 2.0);
    settings.threshold_db = settings.threshold_db.clamp(-100.0, 0.0);
    state.update_settings(|s| s.voice = settings.clone())?;
    game_api(&app).apply_voice_settings(settings);
    Ok(())
}

/// Run the microphone (meter only) for the settings page.
#[tauri::command]
pub fn mic_test(app: AppHandle, on: bool) {
    game_api(&app).mic_test(on);
}

/// Host only: start or stop a feedback recording of the current lobby.
#[tauri::command]
pub fn set_feedback(app: AppHandle, on: bool) -> CmdResult<()> {
    game_api(&app).set_feedback(on)
}

/// Include or leave out your voice from the current lobby's feedback recording.
#[tauri::command]
pub fn set_record_me(app: AppHandle, on: bool) -> CmdResult<()> {
    game_api(&app).set_record_me(on)
}

#[tauri::command]
pub async fn feedback_list(state: State<'_, AppState>, slug: String) -> CmdResult<Vec<FeedbackSummary>> {
    state.client()?.feedback_list(&slug).await.map_err(err)
}

#[tauri::command]
pub async fn feedback_detail(state: State<'_, AppState>, id: String) -> CmdResult<FeedbackDetail> {
    state.client()?.feedback(&id).await.map_err(err)
}

/// A screenshot's bytes (the server needs a login, so `<img>` can't fetch it directly).
#[tauri::command]
pub async fn feedback_file(state: State<'_, AppState>, id: String, file: String) -> CmdResult<tauri::ipc::Response> {
    let bytes = state.client()?.feedback_file(&id, &file).await.map_err(err)?;
    Ok(tauri::ipc::Response::new(bytes.to_vec()))
}

#[tauri::command]
pub async fn delete_feedback(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.client()?.delete_feedback(&id).await.map_err(err)
}

#[tauri::command]
pub async fn retranscribe_feedback(state: State<'_, AppState>, id: String) -> CmdResult<()> {
    state.client()?.retranscribe_feedback(&id).await.map_err(err)
}

/// Save a recording (transcript, screenshots) into a folder; returns where.
#[tauri::command]
pub async fn export_feedback(state: State<'_, AppState>, id: String, dir: String) -> CmdResult<String> {
    let client = state.client()?;
    let out = condensate_sdk::feedback::export(&client, &id, std::path::Path::new(&dir)).await.map_err(err)?;
    Ok(out.display().to_string())
}
