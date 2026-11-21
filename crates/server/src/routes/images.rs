//! Uploaded images (game covers/banners, avatars). Stored once per content hash and served
//! publicly at `/images/<id>`: ids are unguessable hashes and the desktop app's <img> tags
//! can't send auth headers.

use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::Json;
use condensate_core::api::ImageUploaded;

use crate::auth::AuthUser;
use crate::error::{bad_request, not_found, ApiResult};
use crate::state::AppState;

pub const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const TYPES: &[(&str, &str)] = &[("png", "image/png"), ("jpg", "image/jpeg"), ("gif", "image/gif"), ("webp", "image/webp")];

fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else {
        None
    }
}

/// `<32 hex>.<ext>`
pub fn is_valid_id(id: &str) -> bool {
    match id.split_once('.') {
        Some((hash, ext)) => {
            hash.len() == 32 && hash.bytes().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) && TYPES.iter().any(|(e, _)| *e == ext)
        }
        None => false,
    }
}

/// Validate an image id sent by a client: must exist on disk. `""` means "clear".
pub async fn check_id(st: &AppState, id: &str) -> ApiResult<Option<String>> {
    if id.is_empty() {
        return Ok(None);
    }
    if !is_valid_id(id) || !tokio::fs::try_exists(st.storage.images_dir().join(id)).await.unwrap_or(false) {
        return Err(bad_request("unknown image; upload it first"));
    }
    Ok(Some(id.to_string()))
}

pub async fn upload(State(st): State<AppState>, _user: AuthUser, body: Bytes) -> ApiResult<Json<ImageUploaded>> {
    if body.len() > MAX_IMAGE_BYTES {
        return Err(bad_request("images must be 8 MB or smaller"));
    }
    let ext = sniff(&body).ok_or_else(|| bad_request("images must be PNG, JPEG, GIF or WebP"))?;
    let hash = blake3::hash(&body).to_hex();
    let id = format!("{}.{ext}", &hash[..32]);
    let dest = st.storage.images_dir().join(&id);
    if !tokio::fs::try_exists(&dest).await.unwrap_or(false) {
        let tmp = st.storage.tmp_path(&id);
        tokio::fs::write(&tmp, &body).await?;
        tokio::fs::rename(&tmp, &dest).await?;
    }
    Ok(Json(ImageUploaded { id }))
}

pub async fn serve(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult<Response> {
    if !is_valid_id(&id) {
        return Err(not_found("no such image"));
    }
    let data = match tokio::fs::read(st.storage.images_dir().join(&id)).await {
        Ok(d) => d,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(not_found("no such image")),
        Err(e) => return Err(e.into()),
    };
    let ext = id.rsplit('.').next().unwrap_or("");
    let mime = TYPES.iter().find(|(e, _)| *e == ext).map(|(_, m)| *m).unwrap_or("application/octet-stream");
    Ok(([(CONTENT_TYPE, mime), (CACHE_CONTROL, "public, max-age=31536000, immutable")], data).into_response())
}
