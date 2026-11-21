//! Friends, user search and direct messages.

use axum::extract::{Path, Query, State};
use axum::Json;
use condensate_core::api::*;
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::AuthUser;
use crate::db::now;
use crate::error::{bad_request, forbidden, not_found, ApiResult};
use crate::hub::broadcast_presence;
use crate::state::AppState;

use super::accounts::{UserRow, USER_COLS};

fn pair(a: i64, b: i64) -> (i64, i64) {
    (a.min(b), a.max(b))
}

/// Ids of a user's accepted friends.
pub async fn friend_ids(db: &SqlitePool, user: i64) -> sqlx::Result<Vec<i64>> {
    sqlx::query_scalar(
        "SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END FROM friendships
         WHERE (user_a = ?1 OR user_b = ?1) AND status = 'accepted'",
    )
    .bind(user)
    .fetch_all(db)
    .await
}

async fn are_friends(db: &SqlitePool, a: i64, b: i64) -> sqlx::Result<bool> {
    let (x, y) = pair(a, b);
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM friendships WHERE user_a = ? AND user_b = ? AND status = 'accepted'")
        .bind(x)
        .bind(y)
        .fetch_one(db)
        .await?;
    Ok(n > 0)
}

async fn user_named(db: &SqlitePool, username: &str) -> ApiResult<UserRow> {
    sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE username = ?"))
        .bind(username)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| not_found(format!("no user called {username}")))
}

// ---- users & friends ----------------------------------------------------------------------

#[derive(Deserialize)]
pub struct UserSearch {
    q: Option<String>,
}

pub async fn search_users(State(st): State<AppState>, user: AuthUser, Query(p): Query<UserSearch>) -> ApiResult<Json<Vec<User>>> {
    let q = p.q.unwrap_or_default();
    let pattern = format!("%{}%", q.trim().replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
    let rows: Vec<UserRow> = sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM users
         WHERE id != ?1 AND (username LIKE ?2 ESCAPE '\\' OR display_name LIKE ?2 ESCAPE '\\')
         ORDER BY username COLLATE NOCASE LIMIT 25"
    ))
    .bind(user.id)
    .bind(pattern)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

type FriendRow = (i64, String, i64, i64);

pub async fn list_friends(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<FriendsResponse>> {
    let rows: Vec<FriendRow> = sqlx::query_as(
        "SELECT CASE WHEN user_a = ?1 THEN user_b ELSE user_a END, status, requested_by, COALESCE(accepted_at, created_at)
         FROM friendships WHERE user_a = ?1 OR user_b = ?1",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;

    let mut out = FriendsResponse { friends: vec![], incoming: vec![], outgoing: vec![] };
    for (other, status, requested_by, since) in rows {
        let row: UserRow = sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = ?")).bind(other).fetch_one(&st.db).await?;
        let last_seen = row.last_seen_at;
        let u: User = row.into();
        if status == "accepted" {
            out.friends.push(Friend { presence: st.hub.presence(other, last_seen), user: u, since });
        } else if requested_by == user.id {
            out.outgoing.push(u);
        } else {
            out.incoming.push(u);
        }
    }
    // Online first, then by name.
    out.friends.sort_by(|a, b| {
        let rank = |f: &Friend| match f.presence.status.as_str() {
            "in_game" => 0,
            "online" => 1,
            _ => 2,
        };
        rank(a).cmp(&rank(b)).then_with(|| a.user.display_name.to_lowercase().cmp(&b.user.display_name.to_lowercase()))
    });
    Ok(Json(out))
}

/// Send a friend request, or accept one they already sent you.
pub async fn add_friend(State(st): State<AppState>, user: AuthUser, Path(username): Path<String>) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    let other = user_named(&st.db, &username).await?;
    if other.id == user.id {
        return Err(bad_request("you can't friend yourself"));
    }
    let (a, b) = pair(user.id, other.id);
    let existing: Option<(String, i64)> = sqlx::query_as("SELECT status, requested_by FROM friendships WHERE user_a = ? AND user_b = ?")
        .bind(a)
        .bind(b)
        .fetch_optional(&st.db)
        .await?;
    let mut became_friends = false;
    match existing {
        Some((status, by)) if status == "pending" && by != user.id => {
            sqlx::query("UPDATE friendships SET status = 'accepted', accepted_at = ? WHERE user_a = ? AND user_b = ?")
                .bind(now())
                .bind(a)
                .bind(b)
                .execute(&st.db)
                .await?;
            became_friends = true;
        }
        Some(_) => {} // already friends or already requested
        None => {
            sqlx::query("INSERT INTO friendships (user_a, user_b, requested_by, status, created_at) VALUES (?, ?, ?, 'pending', ?)")
                .bind(a)
                .bind(b)
                .bind(user.id)
                .bind(now())
                .execute(&st.db)
                .await?;
        }
    }
    st.hub.send(other.id, &WsEvent::FriendsChanged);
    st.hub.send(user.id, &WsEvent::FriendsChanged);
    if became_friends {
        // Each now needs to see the other's presence.
        broadcast_presence(&st, user.id).await;
        broadcast_presence(&st, other.id).await;
    }
    Ok(Json(OkResponse { ok: true }))
}

/// Unfriend, decline a request, or cancel one you sent.
pub async fn remove_friend(State(st): State<AppState>, user: AuthUser, Path(username): Path<String>) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    let other = user_named(&st.db, &username).await?;
    let (a, b) = pair(user.id, other.id);
    sqlx::query("DELETE FROM friendships WHERE user_a = ? AND user_b = ?").bind(a).bind(b).execute(&st.db).await?;
    st.hub.send(other.id, &WsEvent::FriendsChanged);
    st.hub.send(user.id, &WsEvent::FriendsChanged);
    Ok(Json(OkResponse { ok: true }))
}

// ---- messages ---------------------------------------------------------------------------------

const MSG_COLS: &str = "m.id, s.username, r.username, m.body, m.created_at, m.read_at";
const MSG_FROM: &str = "FROM messages m JOIN users s ON s.id = m.sender_id JOIN users r ON r.id = m.recipient_id";
type MsgRow = (i64, String, String, String, i64, Option<i64>);

fn to_message((id, from, to, body, created_at, read_at): MsgRow) -> ChatMessage {
    ChatMessage { id, from, to, body, created_at, read_at }
}

pub async fn conversations(State(st): State<AppState>, user: AuthUser) -> ApiResult<Json<Vec<Conversation>>> {
    user.require_session()?;
    let others: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT other, MAX(id) FROM (
             SELECT recipient_id AS other, id FROM messages WHERE sender_id = ?1
             UNION ALL SELECT sender_id AS other, id FROM messages WHERE recipient_id = ?1
         ) GROUP BY other ORDER BY MAX(id) DESC",
    )
    .bind(user.id)
    .fetch_all(&st.db)
    .await?;
    let mut out = Vec::new();
    for (other, last_id) in others {
        let row: UserRow = sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = ?")).bind(other).fetch_one(&st.db).await?;
        let last: MsgRow = sqlx::query_as(&format!("SELECT {MSG_COLS} {MSG_FROM} WHERE m.id = ?")).bind(last_id).fetch_one(&st.db).await?;
        let unread: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM messages WHERE sender_id = ? AND recipient_id = ? AND read_at IS NULL")
            .bind(other)
            .bind(user.id)
            .fetch_one(&st.db)
            .await?;
        out.push(Conversation { user: row.into(), last: Some(to_message(last)), unread });
    }
    Ok(Json(out))
}

#[derive(Deserialize)]
pub struct HistoryQuery {
    before: Option<i64>,
    limit: Option<i64>,
}

pub async fn history(
    State(st): State<AppState>,
    user: AuthUser,
    Path(username): Path<String>,
    Query(p): Query<HistoryQuery>,
) -> ApiResult<Json<Vec<ChatMessage>>> {
    user.require_session()?;
    let other = user_named(&st.db, &username).await?;
    let mut rows: Vec<MsgRow> = sqlx::query_as(&format!(
        "SELECT {MSG_COLS} {MSG_FROM}
         WHERE ((m.sender_id = ?1 AND m.recipient_id = ?2) OR (m.sender_id = ?2 AND m.recipient_id = ?1)) AND m.id < ?3
         ORDER BY m.id DESC LIMIT ?4"
    ))
    .bind(user.id)
    .bind(other.id)
    .bind(p.before.unwrap_or(i64::MAX))
    .bind(p.limit.unwrap_or(60).clamp(1, 200))
    .fetch_all(&st.db)
    .await?;
    rows.reverse();
    Ok(Json(rows.into_iter().map(to_message).collect()))
}

pub async fn send(
    State(st): State<AppState>,
    user: AuthUser,
    Path(username): Path<String>,
    Json(req): Json<SendMessageRequest>,
) -> ApiResult<Json<ChatMessage>> {
    user.require_session()?;
    let other = user_named(&st.db, &username).await?;
    if !are_friends(&st.db, user.id, other.id).await? {
        return Err(forbidden("you can only message friends"));
    }
    let body = req.body.trim();
    if body.is_empty() || body.chars().count() > 4000 {
        return Err(bad_request("messages must be 1-4000 characters"));
    }
    let t = now();
    let id: i64 = sqlx::query_scalar("INSERT INTO messages (sender_id, recipient_id, body, created_at) VALUES (?, ?, ?, ?) RETURNING id")
        .bind(user.id)
        .bind(other.id)
        .bind(body)
        .bind(t)
        .fetch_one(&st.db)
        .await?;
    let message = ChatMessage { id, from: user.username.clone(), to: other.username.clone(), body: body.into(), created_at: t, read_at: None };
    let event = WsEvent::Message { message: message.clone() };
    st.hub.send(other.id, &event);
    st.hub.send(user.id, &event);
    Ok(Json(message))
}

pub async fn mark_read(State(st): State<AppState>, user: AuthUser, Path(username): Path<String>) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    let other = user_named(&st.db, &username).await?;
    sqlx::query("UPDATE messages SET read_at = ? WHERE sender_id = ? AND recipient_id = ? AND read_at IS NULL")
        .bind(now())
        .bind(other.id)
        .bind(user.id)
        .execute(&st.db)
        .await?;
    Ok(Json(OkResponse { ok: true }))
}
