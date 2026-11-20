use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::Json;
use condensate_core::api::{MissingChunksRequest, MissingChunksResponse, OkResponse};
use condensate_core::chunk::is_valid_hash;

use crate::auth::AuthUser;
use crate::error::{bad_request, not_found, ApiResult};
use crate::state::AppState;

fn check_hash(hash: &str) -> ApiResult<()> {
    if is_valid_hash(hash) {
        Ok(())
    } else {
        Err(bad_request("chunk ids are 64 lowercase hex characters"))
    }
}

/// Which of these chunks does the server not have yet? Lets publishers upload only new data.
pub async fn missing(
    State(st): State<AppState>,
    _user: AuthUser,
    Json(req): Json<MissingChunksRequest>,
) -> ApiResult<Json<MissingChunksResponse>> {
    if req.hashes.len() > 20_000 {
        return Err(bad_request("ask about at most 20000 chunks at a time"));
    }
    let mut missing = Vec::new();
    for h in req.hashes {
        check_hash(&h)?;
        if !st.storage.has_chunk(&h).await {
            missing.push(h);
        }
    }
    Ok(Json(MissingChunksResponse { missing }))
}

/// Body: the zstd-compressed chunk. Verified against the hash before it is stored.
pub async fn upload(
    State(st): State<AppState>,
    _user: AuthUser,
    Path(hash): Path<String>,
    body: Bytes,
) -> ApiResult<Json<OkResponse>> {
    check_hash(&hash)?;
    if !st.storage.has_chunk(&hash).await {
        st.storage.put_chunk(&hash, body).await?;
    }
    Ok(Json(OkResponse { ok: true }))
}

pub async fn download(
    State(st): State<AppState>,
    _user: AuthUser,
    Path(hash): Path<String>,
) -> ApiResult<Response> {
    check_hash(&hash)?;
    let data = match st.storage.read_chunk(&hash).await {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found("no such chunk")),
        Err(e) => return Err(e.into()),
    };
    Ok((
        [(CONTENT_TYPE, "application/zstd"), (CACHE_CONTROL, "private, max-age=31536000, immutable")],
        data,
    )
        .into_response())
}
