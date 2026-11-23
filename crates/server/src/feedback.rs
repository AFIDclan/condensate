//! Feedback recordings. While a lobby's host has feedback recording on, the play relay hands
//! every voice frame to the [`Recorder`], which saves each player's speech as separate Ogg Opus
//! files (one per stretch of talking) under `<data>/feedback/<session>/`. Players' screenshots
//! arrive over HTTP. When the recording stops, the [`Transcriber`] runs Whisper (whisper.cpp's
//! `whisper-cli`, via ffmpeg) over each stretch, and the game's owner gets a timeline of who said
//! what, with screenshots in place, as JSON or as Markdown for an LLM.
//!
//! Whisper is optional: set `CONDENSATE_WHISPER` (the whisper-cli binary) and
//! `CONDENSATE_WHISPER_MODEL` (a ggml model file). Without them recordings keep their audio and
//! are marked `untranscribed`; they're transcribed once Whisper is set up and the server restarts.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use axum::body::Bytes;
use axum::extract::{Path as UrlPath, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::Json;
use condensate_core::api::{FeedbackDetail, FeedbackShot, FeedbackSummary, FeedbackUtterance, OkResponse};
use condensate_core::play::VOICE_FRAME;
use ogg::writing::{PacketWriteEndInfo, PacketWriter};
use sqlx::SqlitePool;
use tokio::sync::mpsc;

use crate::auth::AuthUser;
use crate::db::now;
use crate::error::{bad_request, forbidden, not_found, ApiResult};
use crate::state::AppState;

/// A pause longer than this ends a stretch of talking. The app keeps sending for a moment after
/// someone stops, so ordinary pauses between words don't split a sentence.
const GAP: Duration = Duration::from_millis(1200);
/// Shorter stretches are dropped: coughs, clicks and other blips Whisper would invent words for.
const MIN_UTTERANCE_MS: i64 = 500;
/// Opus encoder delay at 48 kHz, written to the Ogg header.
const PRE_SKIP: u16 = 312;
const MAX_SHOT_BYTES: usize = 8 * 1024 * 1024;
/// Files handed to one whisper-cli run (it loads the model once per run).
const WHISPER_BATCH: usize = 40;

/// How to run Whisper. Read from the environment.
#[derive(Debug, Clone)]
pub struct WhisperConfig {
    pub cli: PathBuf,
    pub model: PathBuf,
    pub language: String,
    pub ffmpeg: String,
    pub threads: usize,
}

impl WhisperConfig {
    pub fn from_env() -> Option<Self> {
        let var = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        Some(Self {
            cli: var("CONDENSATE_WHISPER")?.into(),
            model: var("CONDENSATE_WHISPER_MODEL")?.into(),
            language: var("CONDENSATE_WHISPER_LANGUAGE").unwrap_or_else(|| "auto".into()),
            ffmpeg: var("CONDENSATE_FFMPEG").unwrap_or_else(|| "ffmpeg".into()),
            threads: std::thread::available_parallelism().map_or(4, |n| n.get()),
        })
    }
}

pub fn session_dir(data_dir: &Path, session: &str) -> PathBuf {
    data_dir.join("feedback").join(session)
}

// ---- recording ---------------------------------------------------------------------------------

enum RecMsg {
    Start { session: Arc<str>, started: Instant },
    Frame { session: Arc<str>, user: i64, at: Instant, data: Bytes },
    Stop { session: Arc<str> },
}

/// Handle for saving voice. Cheap to clone; sending never blocks.
#[derive(Clone)]
pub struct Recorder {
    tx: mpsc::UnboundedSender<RecMsg>,
}

impl Recorder {
    pub fn start_session(&self, session: Arc<str>, started: Instant) {
        let _ = self.tx.send(RecMsg::Start { session, started });
    }

    pub fn frame(&self, session: Arc<str>, user: i64, data: Bytes) {
        let _ = self.tx.send(RecMsg::Frame { session, user, at: Instant::now(), data });
    }

    pub fn stop_session(&self, session: Arc<str>) {
        let _ = self.tx.send(RecMsg::Stop { session });
    }
}

/// Queue of sessions to transcribe.
#[derive(Clone)]
pub struct Transcriber {
    tx: mpsc::UnboundedSender<String>,
    pub enabled: bool,
}

impl Transcriber {
    pub fn queue(&self, session: String) {
        let _ = self.tx.send(session);
    }
}

/// Start the recorder and transcriber tasks. Recordings interrupted by a restart are finished
/// and queued for transcription.
pub fn start(db: SqlitePool, data_dir: PathBuf) -> (Recorder, Transcriber) {
    let whisper = WhisperConfig::from_env();
    match &whisper {
        Some(w) => tracing::info!("feedback transcription: {} with {}", w.cli.display(), w.model.display()),
        None => tracing::info!("feedback transcription is off (set CONDENSATE_WHISPER and CONDENSATE_WHISPER_MODEL)"),
    }
    let (ttx, trx) = mpsc::unbounded_channel();
    let transcriber = Transcriber { tx: ttx, enabled: whisper.is_some() };
    let (rtx, rrx) = mpsc::unbounded_channel();
    tokio::spawn(record_loop(db.clone(), data_dir.clone(), transcriber.clone(), rrx));
    tokio::spawn(transcribe_loop(db.clone(), data_dir, whisper, trx));
    let t = transcriber.clone();
    tokio::spawn(async move {
        if let Err(e) = resume(&db, &t).await {
            tracing::warn!("feedback: couldn't resume recordings: {e:#}");
        }
    });
    (Recorder { tx: rtx }, transcriber)
}

async fn resume(db: &SqlitePool, transcriber: &Transcriber) -> Result<()> {
    let status = if transcriber.enabled { "transcribing" } else { "untranscribed" };
    sqlx::query("UPDATE feedback_sessions SET ended_at = ?, status = ? WHERE status = 'recording'").bind(now()).bind(status).execute(db).await?;
    if transcriber.enabled {
        let ids: Vec<String> =
            sqlx::query_scalar("SELECT id FROM feedback_sessions WHERE status IN ('transcribing', 'untranscribed') ORDER BY started_at").fetch_all(db).await?;
        for id in ids {
            transcriber.queue(id);
        }
    }
    Ok(())
}

/// An Ogg Opus file being written. The last packet is held back so it can be marked as the end
/// of the stream when the file is closed.
struct OggOpus {
    writer: PacketWriter<'static, BufWriter<File>>,
    granule: u64,
    pending: Option<Vec<u8>>,
}

const SERIAL: u32 = 0x436f_6e64;

impl OggOpus {
    fn create(path: &Path) -> std::io::Result<Self> {
        let mut writer = PacketWriter::new(BufWriter::new(File::create(path)?));
        let mut head = b"OpusHead".to_vec();
        head.push(1); // version
        head.push(1); // channels
        head.extend_from_slice(&PRE_SKIP.to_le_bytes());
        head.extend_from_slice(&48_000u32.to_le_bytes());
        head.extend_from_slice(&0i16.to_le_bytes()); // gain
        head.push(0); // mapping family
        writer.write_packet(head, SERIAL, PacketWriteEndInfo::EndPage, 0)?;
        let vendor = b"Condensate";
        let mut tags = b"OpusTags".to_vec();
        tags.extend_from_slice(&(vendor.len() as u32).to_le_bytes());
        tags.extend_from_slice(vendor);
        tags.extend_from_slice(&0u32.to_le_bytes());
        writer.write_packet(tags, SERIAL, PacketWriteEndInfo::EndPage, 0)?;
        Ok(Self { writer, granule: 0, pending: None })
    }

    fn push(&mut self, packet: Vec<u8>) -> std::io::Result<()> {
        if let Some(p) = self.pending.replace(packet) {
            self.granule += VOICE_FRAME as u64;
            self.writer.write_packet(p, SERIAL, PacketWriteEndInfo::NormalPacket, self.granule)?;
        }
        Ok(())
    }

    fn finish(mut self) -> std::io::Result<()> {
        if let Some(p) = self.pending.take() {
            self.granule += VOICE_FRAME as u64;
            self.writer.write_packet(p, SERIAL, PacketWriteEndInfo::EndStream, self.granule)?;
        }
        use std::io::Write;
        self.writer.inner_mut().flush()
    }
}

/// One player talking.
struct Utterance {
    file: String,
    ogg: OggOpus,
    at_ms: i64,
    frames: i64,
    last: Instant,
}

struct Session {
    started: Instant,
    dir: PathBuf,
    open: HashMap<i64, Utterance>,
}

async fn record_loop(db: SqlitePool, data_dir: PathBuf, transcriber: Transcriber, mut rx: mpsc::UnboundedReceiver<RecMsg>) {
    let mut sessions: HashMap<Arc<str>, Session> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_millis(300));
    loop {
        tokio::select! {
            msg = rx.recv() => match msg {
                None => return,
                Some(RecMsg::Start { session, started }) => {
                    let dir = session_dir(&data_dir, &session);
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        tracing::warn!("feedback: can't create {}: {e}", dir.display());
                    }
                    sessions.insert(session, Session { started, dir, open: HashMap::new() });
                }
                Some(RecMsg::Frame { session, user, at, data }) => {
                    let Some(s) = sessions.get_mut(&session) else { continue };
                    if s.open.get(&user).is_some_and(|u| at.duration_since(u.last) > GAP) {
                        let u = s.open.remove(&user).unwrap();
                        save(&db, &session, &s.dir, user, u).await;
                    }
                    if !s.open.contains_key(&user) {
                        let at_ms = at.duration_since(s.started).as_millis() as i64;
                        let file = format!("u{user}-{at_ms}.opus");
                        match OggOpus::create(&s.dir.join(&file)) {
                            Ok(ogg) => {
                                s.open.insert(user, Utterance { file, ogg, at_ms, frames: 0, last: at });
                            }
                            Err(e) => {
                                tracing::warn!("feedback: can't write {file}: {e}");
                                continue;
                            }
                        }
                    }
                    let u = s.open.get_mut(&user).unwrap();
                    u.last = at;
                    u.frames += 1;
                    if let Err(e) = u.ogg.push(data.to_vec()) {
                        tracing::warn!("feedback: write failed: {e}");
                    }
                }
                Some(RecMsg::Stop { session }) => {
                    let Some(mut s) = sessions.remove(&session) else { continue };
                    for (user, u) in s.open.drain() {
                        save(&db, &session, &s.dir, user, u).await;
                    }
                    let (status, note) = if transcriber.enabled {
                        ("transcribing", None)
                    } else {
                        ("untranscribed", Some("Whisper isn't set up on the server, so the voice wasn't transcribed."))
                    };
                    let res = sqlx::query("UPDATE feedback_sessions SET ended_at = ?, status = ?, note = ? WHERE id = ?")
                        .bind(now())
                        .bind(status)
                        .bind(note)
                        .bind(&*session)
                        .execute(&db)
                        .await;
                    if let Err(e) = res {
                        tracing::warn!("feedback: can't end session {session}: {e}");
                    }
                    if transcriber.enabled {
                        transcriber.queue(session.to_string());
                    }
                }
            },
            _ = tick.tick() => {
                let now = Instant::now();
                for (id, s) in sessions.iter_mut() {
                    let idle: Vec<i64> = s.open.iter().filter(|(_, u)| now.duration_since(u.last) > GAP).map(|(k, _)| *k).collect();
                    for user in idle {
                        let u = s.open.remove(&user).unwrap();
                        save(&db, id, &s.dir, user, u).await;
                    }
                }
            }
        }
    }
}

/// Close a stretch of talking and record it, or drop it if it's only a blip.
async fn save(db: &SqlitePool, session: &str, dir: &Path, user: i64, u: Utterance) {
    let duration_ms = u.frames * 20;
    let path = dir.join(&u.file);
    if let Err(e) = u.ogg.finish() {
        tracing::warn!("feedback: can't finish {}: {e}", path.display());
    }
    if duration_ms < MIN_UTTERANCE_MS {
        let _ = std::fs::remove_file(&path);
        return;
    }
    let res = sqlx::query("INSERT INTO feedback_utterances (session_id, user_id, at_ms, duration_ms, file) VALUES (?, ?, ?, ?, ?)")
        .bind(session)
        .bind(user)
        .bind(u.at_ms)
        .bind(duration_ms)
        .bind(&u.file)
        .execute(db)
        .await;
    if let Err(e) = res {
        tracing::warn!("feedback: can't save utterance: {e}");
    }
}

// ---- transcription -----------------------------------------------------------------------------

async fn transcribe_loop(db: SqlitePool, data_dir: PathBuf, whisper: Option<WhisperConfig>, mut rx: mpsc::UnboundedReceiver<String>) {
    while let Some(session) = rx.recv().await {
        let Some(w) = &whisper else { continue };
        let _ = sqlx::query("UPDATE feedback_sessions SET status = 'transcribing', note = NULL WHERE id = ?").bind(&session).execute(&db).await;
        let started = Instant::now();
        let result = transcribe(&db, &session_dir(&data_dir, &session), w, &session).await;
        let (status, note) = match &result {
            Ok(()) => ("ready", None),
            Err(e) => ("failed", Some(format!("{e:#}"))),
        };
        match &result {
            Ok(()) => tracing::info!("feedback: transcribed {session} in {:.0?}", started.elapsed()),
            Err(e) => tracing::warn!("feedback: transcribing {session} failed: {e:#}"),
        }
        let _ = sqlx::query("UPDATE feedback_sessions SET status = ?, note = ? WHERE id = ?").bind(status).bind(note).bind(&session).execute(&db).await;
    }
}

async fn transcribe(db: &SqlitePool, dir: &Path, w: &WhisperConfig, session: &str) -> Result<()> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, file FROM feedback_utterances WHERE session_id = ? AND text IS NULL ORDER BY at_ms").bind(session).fetch_all(db).await?;
    let work = dir.join("whisper");
    tokio::fs::create_dir_all(&work).await?;
    for batch in rows.chunks(WHISPER_BATCH) {
        // Whisper wants 16 kHz mono WAV.
        for (id, file) in batch {
            let out = tokio::process::Command::new(&w.ffmpeg)
                .args(["-y", "-loglevel", "error", "-i"])
                .arg(dir.join(file))
                .args(["-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"])
                .arg(work.join(format!("{id}.wav")))
                .output()
                .await
                .with_context(|| format!("couldn't run {}", w.ffmpeg))?;
            if !out.status.success() {
                bail!("ffmpeg failed on {file}: {}", String::from_utf8_lossy(&out.stderr).trim());
            }
        }
        let mut cmd = tokio::process::Command::new(&w.cli);
        cmd.arg("-m").arg(&w.model).args(["-l", &w.language, "-t", &w.threads.to_string(), "-oj", "-np"]);
        for (id, _) in batch {
            cmd.arg("-f").arg(work.join(format!("{id}.wav"))).arg("-of").arg(work.join(id.to_string()));
        }
        let out = cmd.output().await.with_context(|| format!("couldn't run {}", w.cli.display()))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("whisper failed: {}", err.lines().rev().take(3).collect::<Vec<_>>().join(" / "));
        }
        for (id, _) in batch {
            let json = tokio::fs::read(work.join(format!("{id}.json"))).await.context("whisper wrote no output")?;
            let text = whisper_text(&json)?;
            sqlx::query("UPDATE feedback_utterances SET text = ? WHERE id = ?").bind(text).bind(id).execute(db).await?;
        }
    }
    let _ = tokio::fs::remove_dir_all(&work).await;
    Ok(())
}

/// The text of a whisper-cli `-oj` file, without markers such as `[BLANK_AUDIO]`.
fn whisper_text(json: &[u8]) -> Result<String> {
    let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(json))?;
    let text = v["transcription"]
        .as_array()
        .map(|segs| segs.iter().filter_map(|s| s["text"].as_str()).collect::<String>())
        .unwrap_or_default();
    Ok(strip_markers(&text))
}

fn strip_markers(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for c in text.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---- routes ------------------------------------------------------------------------------------

/// The session, if this user may read it (the game's owner, or an admin).
async fn owned_session(st: &AppState, user: &AuthUser, id: &str) -> ApiResult<i64> {
    let owner: Option<i64> =
        sqlx::query_scalar("SELECT g.owner_id FROM feedback_sessions s JOIN games g ON g.id = s.game_id WHERE s.id = ?").bind(id).fetch_optional(&st.db).await?;
    match owner {
        Some(o) if o == user.id || user.is_admin => Ok(o),
        _ => Err(not_found("no such feedback recording")),
    }
}

type SummaryRow = (String, String, String, String, i64, Option<i64>, String, Option<String>, Option<String>, i64, i64);
const SUMMARY_SELECT: &str = "SELECT s.id, g.slug, g.title, u.display_name, s.started_at, s.ended_at, s.status, s.note, s.version,
       (SELECT COUNT(*) FROM feedback_utterances WHERE session_id = s.id),
       (SELECT COUNT(*) FROM feedback_shots WHERE session_id = s.id)
     FROM feedback_sessions s JOIN games g ON g.id = s.game_id JOIN users u ON u.id = s.host_id";

async fn summary(db: &SqlitePool, r: SummaryRow) -> ApiResult<FeedbackSummary> {
    let players: Vec<String> = sqlx::query_scalar(
        "SELECT display_name FROM users WHERE id IN
           (SELECT user_id FROM feedback_utterances WHERE session_id = ?1 UNION SELECT user_id FROM feedback_shots WHERE session_id = ?1)
         ORDER BY display_name COLLATE NOCASE",
    )
    .bind(&r.0)
    .fetch_all(db)
    .await?;
    Ok(FeedbackSummary {
        id: r.0,
        game: r.1,
        game_title: r.2,
        host: r.3,
        started_at: r.4,
        ended_at: r.5,
        status: r.6,
        note: r.7,
        version: r.8,
        players,
        utterances: r.9,
        shots: r.10,
    })
}

async fn detail(db: &SqlitePool, id: &str) -> ApiResult<FeedbackDetail> {
    let row: SummaryRow = sqlx::query_as(&format!("{SUMMARY_SELECT} WHERE s.id = ?")).bind(id).fetch_optional(db).await?.ok_or_else(|| not_found("no such feedback recording"))?;
    let utterances: Vec<(i64, String, String, i64, i64, Option<String>, String)> = sqlx::query_as(
        "SELECT f.id, u.username, u.display_name, f.at_ms, f.duration_ms, f.text, f.file FROM feedback_utterances f JOIN users u ON u.id = f.user_id
         WHERE f.session_id = ? ORDER BY f.at_ms",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    let shots: Vec<(i64, String, String, i64, String, String)> = sqlx::query_as(
        "SELECT f.id, u.username, u.display_name, f.at_ms, f.file, f.meta FROM feedback_shots f JOIN users u ON u.id = f.user_id
         WHERE f.session_id = ? ORDER BY f.at_ms",
    )
    .bind(id)
    .fetch_all(db)
    .await?;
    Ok(FeedbackDetail {
        summary: summary(db, row).await?,
        utterances: utterances
            .into_iter()
            .map(|(id, username, display_name, at_ms, duration_ms, text, file)| FeedbackUtterance { id, username, display_name, at_ms, duration_ms, text, file })
            .collect(),
        shots: shots
            .into_iter()
            .map(|(id, username, display_name, at_ms, file, meta)| FeedbackShot {
                id,
                username,
                display_name,
                at_ms,
                file,
                meta: serde_json::from_str(&meta).unwrap_or_default(),
            })
            .collect(),
    })
}

/// `GET /games/{slug}/feedback`: the game's recordings, newest first (owner only).
pub async fn list(State(st): State<AppState>, user: AuthUser, UrlPath(slug): UrlPath<String>) -> ApiResult<Json<Vec<FeedbackSummary>>> {
    let owner: Option<i64> = sqlx::query_scalar("SELECT owner_id FROM games WHERE slug = ?").bind(&slug).fetch_optional(&st.db).await?;
    match owner {
        Some(o) if o == user.id || user.is_admin => {}
        Some(_) => return Err(forbidden("only the game's owner can see its feedback")),
        None => return Err(not_found(format!("no game called {slug}"))),
    }
    let rows: Vec<SummaryRow> = sqlx::query_as(&format!("{SUMMARY_SELECT} WHERE g.slug = ? ORDER BY s.started_at DESC")).bind(&slug).fetch_all(&st.db).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        out.push(summary(&st.db, r).await?);
    }
    Ok(Json(out))
}

/// `GET /feedback/{id}`
pub async fn get(State(st): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<FeedbackDetail>> {
    owned_session(&st, &user, &id).await?;
    Ok(Json(detail(&st.db, &id).await?))
}

/// `GET /feedback/{id}/transcript`: the timeline as Markdown, for reading or for an LLM.
pub async fn transcript(State(st): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<String>) -> ApiResult<Response> {
    owned_session(&st, &user, &id).await?;
    let d = detail(&st.db, &id).await?;
    Ok(([(CONTENT_TYPE, "text/markdown; charset=utf-8")], markdown(&d)).into_response())
}

/// `DELETE /feedback/{id}`
pub async fn delete(State(st): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<OkResponse>> {
    owned_session(&st, &user, &id).await?;
    if st.play.recording(&id) {
        return Err(bad_request("that recording is still going; stop it first"));
    }
    sqlx::query("DELETE FROM feedback_sessions WHERE id = ?").bind(&id).execute(&st.db).await?;
    let _ = tokio::fs::remove_dir_all(session_dir(&st.config.data_dir, &id)).await;
    Ok(Json(OkResponse { ok: true }))
}

/// `POST /feedback/{id}/transcribe`: try transcribing again (after a failure, or after setting
/// Whisper up).
pub async fn retranscribe(State(st): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<String>) -> ApiResult<Json<OkResponse>> {
    owned_session(&st, &user, &id).await?;
    if !st.transcriber.enabled {
        return Err(bad_request("Whisper isn't set up on the server (see docs/FEEDBACK.md)"));
    }
    if st.play.recording(&id) {
        return Err(bad_request("that recording is still going"));
    }
    st.transcriber.queue(id);
    Ok(Json(OkResponse { ok: true }))
}

fn valid_file(name: &str) -> bool {
    !name.is_empty() && name.len() < 100 && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.') && !name.starts_with('.')
}

/// `GET /feedback/{id}/files/{file}`: a screenshot or a stretch of audio.
pub async fn file(State(st): State<AppState>, user: AuthUser, UrlPath((id, name)): UrlPath<(String, String)>) -> ApiResult<Response> {
    owned_session(&st, &user, &id).await?;
    if !valid_file(&name) {
        return Err(not_found("no such file"));
    }
    let data = match tokio::fs::read(session_dir(&st.config.data_dir, &id).join(&name)).await {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found("no such file")),
        Err(e) => return Err(e.into()),
    };
    let mime = match name.rsplit('.').next() {
        Some("jpg") => "image/jpeg",
        Some("png") => "image/png",
        Some("opus") => "audio/ogg",
        _ => "application/octet-stream",
    };
    Ok(([(CONTENT_TYPE, mime), (CACHE_CONTROL, "private, max-age=86400")], data).into_response())
}

/// `POST /feedback/{id}/shots`: a player's screenshot, sent by their Condensate app. The body is
/// `[meta length: u32 LE][meta JSON][JPEG or PNG]`, as the game sent it.
pub async fn upload_shot(State(st): State<AppState>, user: AuthUser, UrlPath(id): UrlPath<String>, body: Bytes) -> ApiResult<Json<serde_json::Value>> {
    let at_ms = st.play.recording_member(&id, user.id).ok_or_else(|| forbidden("you're not in a lobby recording that session"))?;
    if body.len() > MAX_SHOT_BYTES {
        return Err(bad_request("screenshots must be 8 MB or smaller"));
    }
    let (meta, image) = split_shot(&body).ok_or_else(|| bad_request("malformed screenshot"))?;
    let ext = if image.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if image.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else {
        return Err(bad_request("screenshots must be JPEG or PNG"));
    };
    let file = format!("shot-{at_ms}-u{}.{ext}", user.id);
    let dir = session_dir(&st.config.data_dir, &id);
    tokio::fs::create_dir_all(&dir).await?;
    tokio::fs::write(dir.join(&file), image).await?;
    sqlx::query("INSERT INTO feedback_shots (session_id, user_id, at_ms, file, meta) VALUES (?, ?, ?, ?, ?)")
        .bind(&id)
        .bind(user.id)
        .bind(at_ms)
        .bind(&file)
        .bind(meta.to_string())
        .execute(&st.db)
        .await?;
    Ok(Json(serde_json::json!({ "file": file })))
}

/// Split a screenshot frame into its metadata (an object, else `{}`) and image bytes.
fn split_shot(body: &[u8]) -> Option<(serde_json::Value, &[u8])> {
    let len = u32::from_le_bytes(body.get(0..4)?.try_into().ok()?) as usize;
    let meta = body.get(4..4 + len)?;
    let meta: serde_json::Value = serde_json::from_slice(meta).ok().filter(|v: &serde_json::Value| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    let meta = if meta.to_string().len() > 16 * 1024 { serde_json::json!({}) } else { meta };
    Some((meta, &body[4 + len..]))
}

/// Create the database row for a new recording; returns its id.
pub async fn create_session(db: &SqlitePool, game_slug: &str, host: i64) -> Result<String> {
    let (game_id, version): (i64, Option<String>) = sqlx::query_as(
        "SELECT g.id, (SELECT v.version FROM versions v WHERE v.game_id = g.id ORDER BY v.id DESC LIMIT 1) FROM games g WHERE g.slug = ?",
    )
    .bind(game_slug)
    .fetch_one(db)
    .await?;
    let id = crate::play::random_id(12);
    sqlx::query("INSERT INTO feedback_sessions (id, game_id, host_id, started_at, status, version) VALUES (?, ?, ?, ?, 'recording', ?)")
        .bind(&id)
        .bind(game_id)
        .bind(host)
        .bind(now())
        .bind(version)
        .execute(db)
        .await?;
    Ok(id)
}

// ---- Markdown ----------------------------------------------------------------------------------

fn clock(ms: i64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

/// `2026-11-22 21:04 UTC`
pub fn utc(ts: i64) -> String {
    let (y, mo, d, h, mi) = condensate_core::utc_parts(ts);
    format!("{y}-{mo:02}-{d:02} {h:02}:{mi:02} UTC")
}

/// The recording as Markdown: a short header explaining what it is, then every line of speech
/// and every screenshot in time order. Screenshot paths are relative (`shots/<file>`), matching
/// the folder `condensate feedback pull` writes.
pub fn markdown(d: &FeedbackDetail) -> String {
    let s = &d.summary;
    let mut out = format!("# Playtest feedback: {}\n\n", s.game_title);
    let minutes = s.ended_at.map(|e| ((e - s.started_at) as f64 / 60.0).round() as i64);
    out += &format!("- Recorded: {}{}\n", utc(s.started_at), minutes.map(|m| format!(", {m} min")).unwrap_or_else(|| " (still recording)".into()));
    let mut players: Vec<String> = s.players.iter().map(|p| if *p == s.host { format!("{p} (host)") } else { p.clone() }).collect();
    if !s.players.contains(&s.host) {
        players.insert(0, format!("{} (host)", s.host));
    }
    out += &format!("- Players: {}\n", players.join(", "));
    if let Some(v) = &s.version {
        out += &format!("- Game version: {v} (latest published when recording started)\n");
    }
    out += &format!("- Recording id: {}\n", s.id);
    match s.status.as_str() {
        "ready" => {}
        "transcribing" | "recording" => out += "- Note: transcription isn't finished yet; some lines have no text.\n",
        _ => {
            if let Some(n) = &s.note {
                out += &format!("- Note: {n}\n");
            }
        }
    }
    out += "\nThis is the voice chat from a multiplayer playtest, transcribed automatically per player, so expect some \
            misheard words. Times are from the start of the recording. Players pressed a key to take a screenshot when \
            they wanted to point something out; each screenshot is listed where it was taken, with what the game \
            reported about that moment.\n\n## Timeline\n\n";

    enum Item<'a> {
        Said(&'a FeedbackUtterance),
        Shot(&'a FeedbackShot),
    }
    let mut items: Vec<(i64, Item)> = d.utterances.iter().map(|u| (u.at_ms, Item::Said(u))).chain(d.shots.iter().map(|x| (x.at_ms, Item::Shot(x)))).collect();
    items.sort_by_key(|(at, _)| *at);
    if items.is_empty() {
        out += "Nothing was said and no screenshots were taken.\n";
    }
    for (at, item) in items {
        match item {
            Item::Said(u) => {
                let text = match u.text.as_deref() {
                    Some("") => "(nothing intelligible)".to_string(),
                    Some(t) => t.to_string(),
                    None => format!("(not transcribed; {:.0} s of audio)", (u.duration_ms as f64 / 1000.0).max(1.0)),
                };
                out += &format!("[{}] **{}:** {}\n", clock(at), u.display_name, text);
            }
            Item::Shot(x) => {
                let mut about = Vec::new();
                if let Some(scene) = x.meta.get("scene").and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
                    about.push(format!("scene `{scene}`"));
                }
                if let Some(v) = x.meta.get("version").and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
                    about.push(format!("version {v}"));
                }
                if let Some(note) = x.meta.get("note").and_then(|v| v.as_str()).filter(|v| !v.is_empty()) {
                    about.push(format!("note \"{note}\""));
                }
                if let Some(c) = x.meta.get("context").filter(|v| !v.is_null() && v.as_object().is_none_or(|o| !o.is_empty())) {
                    about.push(format!("context `{c}`"));
                }
                let about = if about.is_empty() { String::new() } else { format!(" ({})", about.join(", ")) };
                out += &format!("[{}] *{} took a screenshot:* ![](shots/{}){}\n", clock(at), x.display_name, x.file, about);
            }
        }
        out += "\n";
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(utc(0), "1970-01-01 00:00 UTC");
        assert_eq!(utc(1795382640), "2026-11-22 21:24 UTC");
        assert_eq!(utc(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn markers() {
        assert_eq!(strip_markers(" [BLANK_AUDIO]"), "");
        assert_eq!(strip_markers(" Hello there. (laughs) That jump  is hard."), "Hello there. That jump is hard.");
    }

    #[test]
    fn shot_frames() {
        let mut body = Vec::new();
        let meta = br#"{"scene":"res://a.tscn"}"#;
        body.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        body.extend_from_slice(meta);
        body.extend_from_slice(&[0xFF, 0xD8, 0xFF, 1]);
        let (m, img) = split_shot(&body).unwrap();
        assert_eq!(m["scene"], "res://a.tscn");
        assert_eq!(img, &[0xFF, 0xD8, 0xFF, 1]);
        assert!(split_shot(&[9, 0, 0, 0, 1]).is_none());
    }
}
