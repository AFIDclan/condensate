use axum::extract::{Path, State};
use axum::Json;
use condensate_core::api::*;
use sqlx::FromRow;

use crate::auth::{self, AuthUser, PUBLISH_PREFIX};
use crate::db::{is_unique_violation, now};
use crate::error::{bad_request, forbidden, not_found, ApiError, ApiResult};
use crate::state::AppState;

use super::games::{summaries_where, Bind, HAS_VERSION};

#[derive(FromRow)]
pub struct UserRow {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub bio: String,
    pub is_admin: bool,
    pub created_at: i64,
    pub avatar_image: Option<String>,
    pub last_seen_at: i64,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> Self {
        User {
            id: r.id,
            username: r.username,
            display_name: r.display_name,
            bio: r.bio,
            is_admin: r.is_admin,
            created_at: r.created_at,
            avatar: r.avatar_image,
        }
    }
}

pub const USER_COLS: &str = "id, username, display_name, bio, is_admin, created_at, avatar_image, last_seen_at";

async fn load_user(db: &sqlx::SqlitePool, id: i64) -> ApiResult<User> {
    let row: UserRow = sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = ?"))
        .bind(id)
        .fetch_one(db)
        .await?;
    Ok(row.into())
}

fn validate_username(name: &str) -> ApiResult<()> {
    let ok_len = (3..=24).contains(&name.len());
    let ok_chars = name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if ok_len && ok_chars {
        Ok(())
    } else {
        Err(bad_request("username must be 3-24 characters: letters, numbers, _ or -"))
    }
}

fn validate_password(pw: &str) -> ApiResult<()> {
    if pw.chars().count() < 8 || pw.len() > 256 {
        return Err(bad_request("password must be at least 8 characters"));
    }
    Ok(())
}

fn clean_display_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(bad_request("display name must be 1-40 characters"));
    }
    Ok(name.to_string())
}

pub async fn register(State(st): State<AppState>, Json(req): Json<RegisterRequest>) -> ApiResult<Json<AuthResponse>> {
    validate_username(&req.username)?;
    validate_password(&req.password)?;
    let display_name = clean_display_name(req.display_name.as_deref().unwrap_or(&req.username))?;
    let password_hash = auth::hash_password(req.password).await?;

    // Every account needs an invite, including the first one: create that with
    // `condensate-server invite` on the server. The first account becomes admin.
    let code = req
        .invite_code
        .as_deref()
        .map(|c| c.trim().to_uppercase())
        .filter(|c| !c.is_empty())
        .ok_or_else(|| forbidden("an invite code is required"))?;

    let mut tx = st.db.begin().await?;
    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users").fetch_one(&mut *tx).await?;
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, display_name, password_hash, is_admin, created_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&req.username)
    .bind(&display_name)
    .bind(&password_hash)
    .bind(user_count == 0)
    .bind(now())
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            ApiError::Conflict("that username is taken".into())
        } else {
            e.into()
        }
    })?;

    // Claim the invite atomically so one code can never create two accounts.
    let claimed = sqlx::query("UPDATE invites SET used_by = ?, used_at = ? WHERE code = ? AND used_by IS NULL")
        .bind(user_id)
        .bind(now())
        .bind(&code)
        .execute(&mut *tx)
        .await?;
    if claimed.rows_affected() == 0 {
        return Err(forbidden("that invite code is invalid or already used"));
    }
    tx.commit().await?;

    let token = auth::create_session(&st.db, user_id, "registration").await?;
    Ok(Json(AuthResponse { token, user: load_user(&st.db, user_id).await? }))
}

pub async fn login(State(st): State<AppState>, Json(req): Json<LoginRequest>) -> ApiResult<Json<AuthResponse>> {
    if st.login_limiter.is_blocked(&req.username) {
        return Err(ApiError::TooManyRequests("too many failed logins, try again in 15 minutes".into()));
    }
    let row: Option<(i64, String)> = sqlx::query_as("SELECT id, password_hash FROM users WHERE username = ?")
        .bind(&req.username)
        .fetch_optional(&st.db)
        .await?;
    // Verify against a dummy hash for unknown users so timing doesn't reveal which names exist.
    let (user_id, hash) = row.unwrap_or((
        -1,
        "$argon2id$v=19$m=19456,t=2,p=1$c29tZXNhbHRzb21lc2FsdA$2ZwHQGMlSz5tLMUJnF4/yv8hEJ0yh0nNvHf9SrWkBys".into(),
    ));
    let ok = auth::verify_password(req.password, hash).await;
    if !ok || user_id < 0 {
        st.login_limiter.record_failure(&req.username);
        return Err(forbidden("wrong username or password"));
    }
    st.login_limiter.record_success(&req.username);
    let token = auth::create_session(&st.db, user_id, "login").await?;
    Ok(Json(AuthResponse { token, user: load_user(&st.db, user_id).await? }))
}

pub async fn logout(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<OkResponse>> {
    sqlx::query("DELETE FROM tokens WHERE id = ?").bind(user.token_id).execute(&st.db).await?;
    Ok(Json(OkResponse { ok: true }))
}

pub async fn me(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<User>> {
    Ok(Json(load_user(&st.db, user.id).await?))
}

pub async fn update_me(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<UpdateProfileRequest>,
) -> ApiResult<Json<User>> {
    user.require_session()?;
    if let Some(name) = req.display_name {
        sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
            .bind(clean_display_name(&name)?)
            .bind(user.id)
            .execute(&st.db)
            .await?;
    }
    if let Some(bio) = req.bio {
        if bio.chars().count() > 1000 {
            return Err(bad_request("bio must be at most 1000 characters"));
        }
        sqlx::query("UPDATE users SET bio = ? WHERE id = ?").bind(bio.trim()).bind(user.id).execute(&st.db).await?;
    }
    if let Some(avatar) = req.avatar_image {
        let id = super::images::check_id(&st, &avatar).await?;
        sqlx::query("UPDATE users SET avatar_image = ? WHERE id = ?").bind(id).bind(user.id).execute(&st.db).await?;
    }
    Ok(Json(load_user(&st.db, user.id).await?))
}

pub async fn change_password(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<ChangePasswordRequest>,
) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    validate_password(&req.new_password)?;
    let current: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    if !auth::verify_password(req.current_password, current).await {
        return Err(forbidden("current password is wrong"));
    }
    let hash = auth::hash_password(req.new_password).await?;
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?").bind(hash).bind(user.id).execute(&st.db).await?;
    // Sign out every other session.
    sqlx::query("DELETE FROM tokens WHERE user_id = ? AND kind = 'session' AND id != ?")
        .bind(user.id)
        .bind(user.token_id)
        .execute(&st.db)
        .await?;
    Ok(Json(OkResponse { ok: true }))
}

pub async fn profile(
    State(st): State<AppState>,
    user: AuthUser,
    Path(username): Path<String>,
) -> ApiResult<Json<UserProfile>> {
    let row: UserRow = sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE username = ?"))
        .bind(&username)
        .fetch_optional(&st.db)
        .await?
        .ok_or_else(|| not_found("no such user"))?;
    let owner_id = row.id;
    let games = summaries_where(
        &st.db,
        user.id,
        &format!("g.owner_id = ?2 AND g.visibility = 'public' AND {HAS_VERSION} ORDER BY g.updated_at DESC"),
        vec![Bind::Int(owner_id)],
    )
    .await?;
    Ok(Json(UserProfile { user: row.into(), games }))
}

// ---- invites --------------------------------------------------------------------------------

pub async fn create_invite(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Invite>> {
    user.require_session()?;
    let open: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invites WHERE created_by = ? AND used_by IS NULL")
        .bind(user.id)
        .fetch_one(&st.db)
        .await?;
    if open >= 20 && !user.is_admin {
        return Err(forbidden("you already have 20 unused invites"));
    }
    let code = auth::new_invite_code();
    let t = now();
    sqlx::query("INSERT INTO invites (code, created_by, created_at) VALUES (?, ?, ?)")
        .bind(&code)
        .bind(user.id)
        .bind(t)
        .execute(&st.db)
        .await?;
    Ok(Json(Invite { code, created_at: t, used_by: None, used_at: None }))
}

pub async fn list_invites(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<Invite>>> {
    user.require_session()?;
    let rows: Vec<(String, i64, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT i.code, i.created_at, u.username, i.used_at
         FROM invites i LEFT JOIN users u ON u.id = i.used_by
         WHERE i.created_by = ? ORDER BY i.created_at DESC",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        rows.into_iter()
            .map(|(code, created_at, used_by, used_at)| Invite { code, created_at, used_by, used_at })
            .collect(),
    ))
}

// ---- tokens ---------------------------------------------------------------------------------

#[derive(FromRow)]
struct TokenRow {
    id: i64,
    kind: String,
    name: String,
    game: Option<String>,
    created_at: i64,
    last_used_at: i64,
}

impl From<TokenRow> for TokenInfo {
    fn from(r: TokenRow) -> Self {
        TokenInfo {
            id: r.id,
            kind: r.kind,
            name: r.name,
            game: r.game,
            created_at: r.created_at,
            last_used_at: r.last_used_at,
        }
    }
}

const TOKEN_SELECT: &str = "SELECT t.id, t.kind, t.name, g.slug AS game, t.created_at, t.last_used_at
     FROM tokens t LEFT JOIN games g ON g.id = t.game_id";

pub async fn create_token(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateTokenRequest>,
) -> ApiResult<Json<CreatedToken>> {
    user.require_session()?;
    let name = req.name.trim();
    if name.is_empty() || name.len() > 60 {
        return Err(bad_request("token name must be 1-60 characters"));
    }
    let game_id = match &req.game {
        Some(slug) => {
            let (id, owner): (i64, i64) = sqlx::query_as("SELECT id, owner_id FROM games WHERE slug = ?")
                .bind(slug)
                .fetch_optional(&st.db)
                .await?
                .ok_or_else(|| not_found("no such game"))?;
            if owner != user.id {
                return Err(forbidden("you can only create tokens for your own games"));
            }
            Some(id)
        }
        None => None,
    };
    let (token, hash) = auth::new_token(PUBLISH_PREFIX);
    let t = now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO tokens (user_id, kind, name, token_hash, game_id, created_at, last_used_at)
         VALUES (?, 'publish', ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(user.id)
    .bind(name)
    .bind(hash)
    .bind(game_id)
    .bind(t)
    .bind(t)
    .fetch_one(&st.db)
    .await?;
    let row: TokenRow = sqlx::query_as(&format!("{TOKEN_SELECT} WHERE t.id = ?")).bind(id).fetch_one(&st.db).await?;
    Ok(Json(CreatedToken { token, info: row.into() }))
}

pub async fn list_tokens(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<TokenInfo>>> {
    user.require_session()?;
    let rows: Vec<TokenRow> =
        sqlx::query_as(&format!("{TOKEN_SELECT} WHERE t.user_id = ? ORDER BY t.created_at DESC"))
            .bind(user.id)
            .fetch_all(&st.db)
            .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn revoke_token(
    State(st): State<AppState>,
    user: AuthUser,
    Path(id): Path<i64>,
) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    let res = sqlx::query("DELETE FROM tokens WHERE id = ? AND user_id = ?")
        .bind(id)
        .bind(user.id)
        .execute(&st.db)
        .await?;
    if res.rows_affected() == 0 {
        return Err(not_found("no such token"));
    }
    Ok(Json(OkResponse { ok: true }))
}
