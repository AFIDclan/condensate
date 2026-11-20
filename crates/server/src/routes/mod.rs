mod accounts;
mod chunks;
mod games;

use axum::extract::{DefaultBodyLimit, State};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use condensate_core::api::ServerInfo;
use tower_http::trace::TraceLayer;

use crate::error::ApiResult;
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/server", get(server_info))
        // accounts
        .route("/auth/register", post(accounts::register))
        .route("/auth/login", post(accounts::login))
        .route("/auth/logout", post(accounts::logout))
        .route("/me", get(accounts::me).patch(accounts::update_me))
        .route("/me/password", post(accounts::change_password))
        .route("/users/{username}", get(accounts::profile))
        .route("/invites", get(accounts::list_invites).post(accounts::create_invite))
        .route("/tokens", get(accounts::list_tokens).post(accounts::create_token))
        .route("/tokens/{id}", delete(accounts::revoke_token))
        // games
        .route("/games", get(games::search).post(games::create))
        .route("/games/{slug}", get(games::detail).patch(games::update))
        .route("/games/{slug}/versions", get(games::versions).post(games::publish))
        .route("/games/{slug}/builds/{platform}", get(games::latest_build))
        .route("/library", get(games::library))
        .route("/library/{slug}", put(games::library_add).delete(games::library_remove))
        // chunks
        .route("/chunks/missing", post(chunks::missing))
        .route("/chunks/{hash}", get(chunks::download).put(chunks::upload));

    Router::new()
        .route("/", get(|| async { "Condensate server\n" }))
        .nest("/api/v1", api)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn server_info(State(st): State<AppState>) -> ApiResult<Json<ServerInfo>> {
    Ok(Json(ServerInfo {
        name: st.config.name.clone(),
        server_version: env!("CARGO_PKG_VERSION").into(),
        direct_url: st.config.direct_url.clone(),
        needs_invite: true,
    }))
}
