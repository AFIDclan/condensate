//! Passwords (argon2id), opaque bearer tokens, and the `AuthUser` request extractor.

use rand::rngs::OsRng;
use rand::RngCore;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use axum::extract::FromRequestParts;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;

use crate::db::now;
use crate::error::{forbidden, ApiError, ApiResult};
use crate::state::AppState;

/// Sessions expire after this long without use.
pub const SESSION_TTL: i64 = 90 * 24 * 3600;

pub const SESSION_PREFIX: &str = "cds_";
pub const PUBLISH_PREFIX: &str = "cdp_";

pub async fn hash_password(password: String) -> anyhow::Result<String> {
    tokio::task::spawn_blocking(move || {
        let salt = SaltString::generate(&mut OsRng);
        Argon2::default()
            .hash_password(password.as_bytes(), &salt)
            .map(|h| h.to_string())
            .map_err(|e| anyhow::anyhow!("hashing password: {e}"))
    })
    .await?
}

pub async fn verify_password(password: String, hash: String) -> bool {
    tokio::task::spawn_blocking(move || {
        PasswordHash::new(&hash)
            .map(|parsed| Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok())
            .unwrap_or(false)
    })
    .await
    .unwrap_or(false)
}

fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// A new random token and the hash to store for it.
pub fn new_token(prefix: &str) -> (String, String) {
    let token = format!("{prefix}{}", random_hex(32));
    let hash = token_hash(&token);
    (token, hash)
}

pub fn token_hash(token: &str) -> String {
    blake3::hash(token.as_bytes()).to_hex().to_string()
}

/// Human-friendly code like `K7QF-M2XD-9PRT` (no 0/O/1/I to avoid misreading).
pub fn new_invite_code() -> String {
    const ALPHABET: &[u8] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
    let mut buf = [0u8; 12];
    OsRng.fill_bytes(&mut buf);
    let chars: Vec<char> = buf.iter().map(|b| ALPHABET[(*b as usize) % ALPHABET.len()] as char).collect();
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

pub fn new_temp_password() -> String {
    new_invite_code().to_lowercase()
}

/// Create a login session for a user, returning the bearer token.
pub async fn create_session(db: &sqlx::SqlitePool, user_id: i64, name: &str) -> ApiResult<String> {
    let (token, hash) = new_token(SESSION_PREFIX);
    let t = now();
    sqlx::query(
        "INSERT INTO tokens (user_id, kind, name, token_hash, created_at, last_used_at, expires_at)
         VALUES (?, 'session', ?, ?, ?, ?, ?)",
    )
    .bind(user_id)
    .bind(name)
    .bind(hash)
    .bind(t)
    .bind(t)
    .bind(t + SESSION_TTL)
    .execute(db)
    .await?;
    Ok(token)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenKind {
    Session,
    Publish,
}

/// The authenticated caller. Add this as a handler argument to require authentication.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub token_id: i64,
    pub kind: TokenKind,
    /// For publish tokens restricted to a single game.
    pub game_scope: Option<i64>,
}

impl AuthUser {
    /// Account management needs a real login, not a publish token handed to a build agent.
    pub fn require_session(&self) -> ApiResult<()> {
        match self.kind {
            TokenKind::Session => Ok(()),
            TokenKind::Publish => Err(forbidden("this needs a login session, not a publish token")),
        }
    }

    pub fn can_publish(&self, game_id: i64, owner_id: i64) -> bool {
        owner_id == self.id && self.game_scope.map_or(true, |g| g == game_id)
    }
}

type TokenRow = (i64, i64, String, Option<i64>, Option<i64>, i64, String, bool);

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .map(str::trim)
            .ok_or(ApiError::Unauthorized)?;

        let row: Option<TokenRow> = sqlx::query_as(
            "SELECT t.id, t.user_id, t.kind, t.game_id, t.expires_at, t.last_used_at, u.username, u.is_admin
             FROM tokens t JOIN users u ON u.id = t.user_id
             WHERE t.token_hash = ?",
        )
        .bind(token_hash(token))
        .fetch_optional(&state.db)
        .await?;
        let Some((token_id, user_id, kind, game_id, expires_at, last_used_at, username, is_admin)) = row else {
            return Err(ApiError::Unauthorized);
        };

        let t = now();
        if expires_at.is_some_and(|e| e < t) {
            return Err(ApiError::Unauthorized);
        }
        // Touch at most once an hour so chunk downloads don't turn into a stream of writes.
        if t - last_used_at > 3600 {
            sqlx::query(
                "UPDATE tokens SET last_used_at = ?,
                 expires_at = CASE WHEN kind = 'session' THEN ? ELSE expires_at END
                 WHERE id = ?",
            )
            .bind(t)
            .bind(t + SESSION_TTL)
            .bind(token_id)
            .execute(&state.db)
            .await?;
        }

        Ok(AuthUser {
            id: user_id,
            username,
            is_admin,
            token_id,
            kind: if kind == "publish" { TokenKind::Publish } else { TokenKind::Session },
            game_scope: game_id,
        })
    }
}
