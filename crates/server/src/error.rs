use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use condensate_core::api::ApiErrorBody;

#[derive(Debug)]
pub enum ApiError {
    BadRequest(String),
    Unauthorized,
    Forbidden(String),
    NotFound(String),
    Conflict(String),
    TooManyRequests(String),
    Internal(anyhow::Error),
}

pub type ApiResult<T> = Result<T, ApiError>;

pub fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError::BadRequest(msg.into())
}

pub fn forbidden(msg: impl Into<String>) -> ApiError {
    ApiError::Forbidden(msg.into())
}

pub fn not_found(msg: impl Into<String>) -> ApiError {
    ApiError::NotFound(msg.into())
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, "bad_request", m),
            ApiError::Unauthorized => {
                (StatusCode::UNAUTHORIZED, "unauthorized", "not logged in or session expired".into())
            }
            ApiError::Forbidden(m) => (StatusCode::FORBIDDEN, "forbidden", m),
            ApiError::NotFound(m) => (StatusCode::NOT_FOUND, "not_found", m),
            ApiError::Conflict(m) => (StatusCode::CONFLICT, "conflict", m),
            ApiError::TooManyRequests(m) => (StatusCode::TOO_MANY_REQUESTS, "rate_limited", m),
            ApiError::Internal(e) => {
                tracing::error!("internal error: {e:#}");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal", "internal server error".into())
            }
        };
        (status, Json(ApiErrorBody { error: code.into(), message })).into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        ApiError::Internal(e.into())
    }
}

impl From<std::io::Error> for ApiError {
    fn from(e: std::io::Error) -> Self {
        ApiError::Internal(e.into())
    }
}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        ApiError::Internal(e)
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(e: tokio::task::JoinError) -> Self {
        ApiError::Internal(e.into())
    }
}
