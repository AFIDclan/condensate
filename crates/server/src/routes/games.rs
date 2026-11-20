use std::collections::HashSet;

use axum::extract::{Path, Query, State};
use axum::Json;
use condensate_core::api::*;
use condensate_core::{platform, LaunchConfig, Manifest};
use serde::Deserialize;
use sqlx::{FromRow, SqlitePool};

use crate::auth::{AuthUser, TokenKind};
use crate::db::{is_unique_violation, now};
use crate::error::{bad_request, forbidden, not_found, ApiError, ApiResult};
use crate::state::AppState;

/// SQL condition: the game `g` has at least one published version.
pub const HAS_VERSION: &str = "EXISTS (SELECT 1 FROM versions v0 WHERE v0.game_id = g.id)";

/// Extra positional parameters (?2, ?3, ...) for [`summaries_where`]. ?1 is always the caller.
pub enum Bind {
    Int(i64),
    Text(String),
}

#[derive(FromRow)]
struct SummaryRow {
    owner_id: i64,
    slug: String,
    title: String,
    summary: String,
    visibility: String,
    updated_at: i64,
    owner: String,
    latest_version: Option<String>,
    platforms: Option<String>,
    in_library: bool,
}

impl SummaryRow {
    fn into_summary(self) -> GameSummary {
        let mut platforms: Vec<String> = self
            .platforms
            .unwrap_or_default()
            .split(',')
            .filter(|p| !p.is_empty())
            .map(String::from)
            .collect();
        platforms.sort();
        GameSummary {
            slug: self.slug,
            title: self.title,
            summary: self.summary,
            owner: self.owner,
            visibility: self.visibility,
            latest_version: self.latest_version,
            platforms,
            updated_at: self.updated_at,
            in_library: self.in_library,
        }
    }
}

const SUMMARY_SELECT: &str = "SELECT g.owner_id, g.slug, g.title, g.summary, g.visibility, g.updated_at,
       u.username AS owner,
       (SELECT v.version FROM versions v WHERE v.game_id = g.id ORDER BY v.id DESC LIMIT 1) AS latest_version,
       (SELECT group_concat(DISTINCT b.platform) FROM builds b JOIN versions v ON v.id = b.version_id
         WHERE v.game_id = g.id) AS platforms,
       EXISTS (SELECT 1 FROM library l WHERE l.game_id = g.id AND l.user_id = ?1) AS in_library
     FROM games g JOIN users u ON u.id = g.owner_id";

async fn summary_rows(db: &SqlitePool, user_id: i64, clause: &str, binds: Vec<Bind>) -> ApiResult<Vec<SummaryRow>> {
    let sql = format!("{SUMMARY_SELECT} WHERE {clause}");
    let mut q = sqlx::query_as::<_, SummaryRow>(&sql).bind(user_id);
    for b in binds {
        q = match b {
            Bind::Int(i) => q.bind(i),
            Bind::Text(s) => q.bind(s),
        };
    }
    Ok(q.fetch_all(db).await?)
}

/// Game summaries matching `clause` (which may end in ORDER BY / LIMIT).
pub async fn summaries_where(db: &SqlitePool, user_id: i64, clause: &str, binds: Vec<Bind>) -> ApiResult<Vec<GameSummary>> {
    Ok(summary_rows(db, user_id, clause, binds).await?.into_iter().map(SummaryRow::into_summary).collect())
}

#[derive(FromRow)]
pub struct GameRow {
    pub id: i64,
    pub owner_id: i64,
    pub visibility: String,
}

pub fn visible_to(owner_id: i64, visibility: &str, user: &AuthUser) -> bool {
    visibility != "private" || owner_id == user.id || user.is_admin
}

async fn load_game(db: &SqlitePool, slug: &str, user: &AuthUser) -> ApiResult<GameRow> {
    let game: GameRow = sqlx::query_as("SELECT id, owner_id, visibility FROM games WHERE slug = ?")
        .bind(slug)
        .fetch_optional(db)
        .await?
        .ok_or_else(|| not_found(format!("no game called {slug}")))?;
    if !visible_to(game.owner_id, &game.visibility, user) {
        return Err(not_found(format!("no game called {slug}")));
    }
    Ok(game)
}

async fn game_detail(db: &SqlitePool, user: &AuthUser, slug: &str) -> ApiResult<GameDetail> {
    let row = summary_rows(db, user.id, "g.slug = ?2", vec![Bind::Text(slug.into())])
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| not_found(format!("no game called {slug}")))?;
    if !visible_to(row.owner_id, &row.visibility, user) {
        return Err(not_found(format!("no game called {slug}")));
    }
    let description: String = sqlx::query_scalar("SELECT description FROM games WHERE slug = ?")
        .bind(slug)
        .fetch_one(db)
        .await?;
    let versions = version_list(db, slug, 50).await?;
    Ok(GameDetail { summary: row.into_summary(), description, versions })
}

async fn version_list(db: &SqlitePool, slug: &str, limit: i64) -> ApiResult<Vec<VersionInfo>> {
    let rows: Vec<(String, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT v.version, v.changelog, v.created_at, u.username,
                (SELECT group_concat(platform) FROM builds WHERE version_id = v.id)
         FROM versions v
         JOIN games g ON g.id = v.game_id
         LEFT JOIN users u ON u.id = v.published_by
         WHERE g.slug = ? ORDER BY v.id DESC LIMIT ?",
    )
    .bind(slug)
    .bind(limit)
    .fetch_all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(version, changelog, created_at, published_by, platforms)| {
            let mut platforms: Vec<String> =
                platforms.unwrap_or_default().split(',').filter(|p| !p.is_empty()).map(String::from).collect();
            platforms.sort();
            VersionInfo { version, changelog, created_at, published_by, platforms }
        })
        .collect())
}

// ---- validation -----------------------------------------------------------------------------

fn validate_slug(slug: &str) -> ApiResult<()> {
    let ok = (2..=40).contains(&slug.len())
        && slug.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !slug.starts_with('-')
        && !slug.ends_with('-');
    if ok {
        Ok(())
    } else {
        Err(bad_request("game id must be 2-40 characters of a-z, 0-9 and -, like my-cool-game"))
    }
}

fn validate_visibility(v: &str) -> ApiResult<()> {
    match v {
        "public" | "unlisted" | "private" => Ok(()),
        _ => Err(bad_request("visibility must be public, unlisted or private")),
    }
}

fn check_len(field: &str, value: &str, min: usize, max: usize) -> ApiResult<()> {
    let n = value.chars().count();
    if n < min || n > max {
        return Err(bad_request(format!("{field} must be {min}-{max} characters")));
    }
    Ok(())
}

fn validate_version(v: &str) -> ApiResult<()> {
    let ok = (1..=32).contains(&v.len())
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'+' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(bad_request("version must be 1-32 characters of letters, numbers and . _ + -"))
    }
}

/// `0.4.2` -> `0.4.3`; `7` -> `8`; nothing published yet -> `0.1.0`.
fn next_version(latest: Option<&str>) -> ApiResult<String> {
    let Some(latest) = latest else { return Ok("0.1.0".into()) };
    let (head, last) = match latest.rsplit_once('.') {
        Some((h, l)) => (Some(h), l),
        None => (None, latest),
    };
    let n: u64 = last.parse().map_err(|_| {
        bad_request(format!("can't auto-increment version {latest:?}; pass a version explicitly"))
    })?;
    Ok(match head {
        Some(h) => format!("{h}.{}", n + 1),
        None => (n + 1).to_string(),
    })
}

// ---- handlers -------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SearchQuery {
    q: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
}

fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_")
}

pub async fn search(
    State(st): State<AppState>,
    user: AuthUser,
    Query(p): Query<SearchQuery>,
) -> ApiResult<Json<Vec<GameSummary>>> {
    let pattern = format!("%{}%", escape_like(p.q.as_deref().unwrap_or("").trim()));
    let clause = format!(
        "(g.visibility = 'public' OR g.owner_id = ?1)
         AND (g.owner_id = ?1 OR {HAS_VERSION})
         AND (g.title LIKE ?2 ESCAPE '\\' OR g.slug LIKE ?2 ESCAPE '\\' OR g.summary LIKE ?2 ESCAPE '\\')
         ORDER BY g.updated_at DESC LIMIT ?3 OFFSET ?4"
    );
    let binds = vec![
        Bind::Text(pattern),
        Bind::Int(p.limit.unwrap_or(50).clamp(1, 200)),
        Bind::Int(p.offset.unwrap_or(0).max(0)),
    ];
    Ok(Json(summaries_where(&st.db, user.id, &clause, binds).await?))
}

pub async fn create(
    State(st): State<AppState>,
    user: AuthUser,
    Json(req): Json<CreateGameRequest>,
) -> ApiResult<Json<GameDetail>> {
    if user.kind == TokenKind::Publish && user.game_scope.is_some() {
        return Err(forbidden("this token can only publish one specific game"));
    }
    validate_slug(&req.slug)?;
    check_len("title", req.title.trim(), 1, 80)?;
    let summary = req.summary.unwrap_or_default();
    check_len("summary", &summary, 0, 200)?;
    let description = req.description.unwrap_or_default();
    check_len("description", &description, 0, 20_000)?;
    let visibility = req.visibility.unwrap_or_else(|| "public".into());
    validate_visibility(&visibility)?;

    let t = now();
    let mut tx = st.db.begin().await?;
    let game_id: i64 = sqlx::query_scalar(
        "INSERT INTO games (slug, title, owner_id, summary, description, visibility, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(&req.slug)
    .bind(req.title.trim())
    .bind(user.id)
    .bind(summary.trim())
    .bind(description.trim())
    .bind(&visibility)
    .bind(t)
    .bind(t)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            ApiError::Conflict(format!("a game called {} already exists", req.slug))
        } else {
            e.into()
        }
    })?;
    sqlx::query("INSERT INTO library (user_id, game_id, added_at) VALUES (?, ?, ?)")
        .bind(user.id)
        .bind(game_id)
        .bind(t)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Json(game_detail(&st.db, &user, &req.slug).await?))
}

pub async fn detail(
    State(st): State<AppState>,
    user: AuthUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<GameDetail>> {
    Ok(Json(game_detail(&st.db, &user, &slug).await?))
}

pub async fn update(
    State(st): State<AppState>,
    user: AuthUser,
    Path(slug): Path<String>,
    Json(req): Json<UpdateGameRequest>,
) -> ApiResult<Json<GameDetail>> {
    let game = load_game(&st.db, &slug, &user).await?;
    if !user.can_publish(game.id, game.owner_id) {
        return Err(forbidden("only the game's owner can edit it"));
    }
    if let Some(v) = &req.title {
        check_len("title", v.trim(), 1, 80)?;
    }
    if let Some(v) = &req.summary {
        check_len("summary", v, 0, 200)?;
    }
    if let Some(v) = &req.description {
        check_len("description", v, 0, 20_000)?;
    }
    if let Some(v) = &req.visibility {
        validate_visibility(v)?;
    }
    sqlx::query(
        "UPDATE games SET title = COALESCE(?, title), summary = COALESCE(?, summary),
         description = COALESCE(?, description), visibility = COALESCE(?, visibility), updated_at = ?
         WHERE id = ?",
    )
    .bind(req.title.as_deref().map(str::trim))
    .bind(req.summary.as_deref().map(str::trim))
    .bind(req.description.as_deref().map(str::trim))
    .bind(req.visibility)
    .bind(now())
    .bind(game.id)
    .execute(&st.db)
    .await?;
    Ok(Json(game_detail(&st.db, &user, &slug).await?))
}

pub async fn versions(
    State(st): State<AppState>,
    user: AuthUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<Vec<VersionInfo>>> {
    load_game(&st.db, &slug, &user).await?;
    Ok(Json(version_list(&st.db, &slug, 500).await?))
}

pub async fn publish(
    State(st): State<AppState>,
    user: AuthUser,
    Path(slug): Path<String>,
    Json(req): Json<PublishRequest>,
) -> ApiResult<Json<PublishResponse>> {
    let game = load_game(&st.db, &slug, &user).await?;
    if !user.can_publish(game.id, game.owner_id) {
        return Err(forbidden("you can't publish this game"));
    }
    let changelog = req.changelog.trim();
    check_len("changelog", changelog, 1, 10_000)?;
    if req.builds.is_empty() {
        return Err(bad_request("a version needs at least one build"));
    }

    let mut platforms = HashSet::new();
    let mut hashes = HashSet::new();
    for b in &req.builds {
        if !platform::is_known(&b.platform) {
            return Err(bad_request(format!("unknown platform {:?} (expected one of {:?})", b.platform, platform::ALL)));
        }
        if !platforms.insert(b.platform.as_str()) {
            return Err(bad_request(format!("two builds for {}", b.platform)));
        }
        b.manifest.validate().map_err(|e| bad_request(format!("{} manifest: {e}", b.platform)))?;
        b.launch.validate(&b.manifest).map_err(|e| bad_request(format!("{} launch: {e}", b.platform)))?;
        hashes.extend(b.manifest.files.iter().flat_map(|f| f.chunks.iter().map(|c| c.hash.as_str())));
    }
    let mut missing = 0usize;
    for h in &hashes {
        if !st.storage.has_chunk(h).await {
            missing += 1;
        }
    }
    if missing > 0 {
        return Err(bad_request(format!("{missing} chunks have not been uploaded yet")));
    }

    let mut tx = st.db.begin().await?;
    let version = match req.version.as_deref().map(str::trim) {
        Some(v) => {
            validate_version(v)?;
            v.to_string()
        }
        None => {
            let latest: Option<String> =
                sqlx::query_scalar("SELECT version FROM versions WHERE game_id = ? ORDER BY id DESC LIMIT 1")
                    .bind(game.id)
                    .fetch_optional(&mut *tx)
                    .await?;
            next_version(latest.as_deref())?
        }
    };
    let t = now();
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO versions (game_id, version, changelog, published_by, created_at)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(game.id)
    .bind(&version)
    .bind(changelog)
    .bind(user.id)
    .bind(t)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            ApiError::Conflict(format!("version {version} already exists"))
        } else {
            e.into()
        }
    })?;
    for b in &req.builds {
        sqlx::query(
            "INSERT INTO builds (version_id, platform, manifest, manifest_hash, total_size, file_count, launch)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(version_id)
        .bind(&b.platform)
        .bind(serde_json::to_string(&b.manifest).map_err(anyhow::Error::from)?)
        .bind(b.manifest.hash())
        .bind(b.manifest.total_size() as i64)
        .bind(b.manifest.files.len() as i64)
        .bind(serde_json::to_string(&b.launch).map_err(anyhow::Error::from)?)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("UPDATE games SET updated_at = ? WHERE id = ?").bind(t).bind(game.id).execute(&mut *tx).await?;
    tx.commit().await?;

    tracing::info!(game = %slug, %version, by = %user.username, "published");
    Ok(Json(PublishResponse { version, created_at: t }))
}

#[derive(FromRow)]
struct BuildRow {
    version: String,
    changelog: String,
    created_at: i64,
    manifest: String,
    manifest_hash: String,
    total_size: i64,
    launch: String,
}

/// Newest build of a game for one platform. A version that only shipped Windows doesn't hide
/// the previous Linux build.
pub async fn latest_build(
    State(st): State<AppState>,
    user: AuthUser,
    Path((slug, platform)): Path<(String, String)>,
) -> ApiResult<Json<BuildInfo>> {
    let game = load_game(&st.db, &slug, &user).await?;
    let row: BuildRow = sqlx::query_as(
        "SELECT v.version, v.changelog, v.created_at, b.manifest, b.manifest_hash, b.total_size, b.launch
         FROM builds b JOIN versions v ON v.id = b.version_id
         WHERE v.game_id = ? AND b.platform = ? ORDER BY v.id DESC LIMIT 1",
    )
    .bind(game.id)
    .bind(&platform)
    .fetch_optional(&st.db)
    .await?
    .ok_or_else(|| not_found(format!("{slug} has no {platform} build")))?;
    let manifest: Manifest = serde_json::from_str(&row.manifest).map_err(anyhow::Error::from)?;
    let launch: LaunchConfig = serde_json::from_str(&row.launch).map_err(anyhow::Error::from)?;
    Ok(Json(BuildInfo {
        game: slug,
        version: row.version,
        platform,
        changelog: row.changelog,
        created_at: row.created_at,
        manifest_hash: row.manifest_hash,
        total_size: row.total_size as u64,
        launch,
        manifest,
    }))
}

// ---- library --------------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct LibraryQuery {
    platform: Option<String>,
}

pub async fn library(
    State(st): State<AppState>,
    user: AuthUser,
    Query(p): Query<LibraryQuery>,
) -> ApiResult<Json<Vec<LibraryEntry>>> {
    let games = summaries_where(
        &st.db,
        user.id,
        "EXISTS (SELECT 1 FROM library l2 WHERE l2.game_id = g.id AND l2.user_id = ?1)
         AND (g.visibility != 'private' OR g.owner_id = ?1)
         ORDER BY g.title COLLATE NOCASE",
        vec![],
    )
    .await?;
    let mut out = Vec::with_capacity(games.len());
    for game in games {
        let latest = match &p.platform {
            Some(platform) => sqlx::query_as::<_, (String, String, i64, i64)>(
                "SELECT v.version, b.manifest_hash, b.total_size, v.created_at
                 FROM builds b JOIN versions v ON v.id = b.version_id JOIN games g ON g.id = v.game_id
                 WHERE g.slug = ? AND b.platform = ? ORDER BY v.id DESC LIMIT 1",
            )
            .bind(&game.slug)
            .bind(platform)
            .fetch_optional(&st.db)
            .await?
            .map(|(version, manifest_hash, total_size, created_at)| LatestBuild {
                version,
                manifest_hash,
                total_size: total_size as u64,
                created_at,
            }),
            None => None,
        };
        out.push(LibraryEntry { game, latest });
    }
    Ok(Json(out))
}

pub async fn library_add(
    State(st): State<AppState>,
    user: AuthUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    let game = load_game(&st.db, &slug, &user).await?;
    sqlx::query("INSERT OR IGNORE INTO library (user_id, game_id, added_at) VALUES (?, ?, ?)")
        .bind(user.id)
        .bind(game.id)
        .bind(now())
        .execute(&st.db)
        .await?;
    Ok(Json(OkResponse { ok: true }))
}

pub async fn library_remove(
    State(st): State<AppState>,
    user: AuthUser,
    Path(slug): Path<String>,
) -> ApiResult<Json<OkResponse>> {
    user.require_session()?;
    let game = load_game(&st.db, &slug, &user).await?;
    sqlx::query("DELETE FROM library WHERE user_id = ? AND game_id = ?")
        .bind(user.id)
        .bind(game.id)
        .execute(&st.db)
        .await?;
    Ok(Json(OkResponse { ok: true }))
}

#[cfg(test)]
mod tests {
    use super::next_version;

    #[test]
    fn bumps_versions() {
        assert_eq!(next_version(None).unwrap(), "0.1.0");
        assert_eq!(next_version(Some("0.4.9")).unwrap(), "0.4.10");
        assert_eq!(next_version(Some("7")).unwrap(), "8");
        assert!(next_version(Some("1.0-beta")).is_err());
    }
}
