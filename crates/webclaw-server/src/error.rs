//! API error type. Maps internal errors to HTTP status codes + JSON.

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

/// Public-facing API error. Always serializes as `{ "error": "..." }`.
/// Keep messages user-actionable; internal details belong in tracing logs.
///
/// `Unauthorized` / `NotFound` / `Internal` are kept on the enum as
/// stable variants for handlers that don't exist yet (planned: per-key
/// rate-limit responses, dynamic route 404s). Marking them dead-code-OK
/// is preferable to inventing them later in three places.
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{0}")]
    BadRequest(String),

    #[error("unauthorized")]
    Unauthorized,

    #[error("not found")]
    NotFound,

    #[error("upstream fetch failed: {0}")]
    Fetch(String),

    #[error("extraction failed: {0}")]
    Extract(String),

    #[error("LLM provider error: {0}")]
    Llm(String),

    #[error("internal: {0}")]
    Internal(String),
}

impl ApiError {
    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::BadRequest(msg.into())
    }
    #[allow(dead_code)]
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(msg.into())
    }

    fn status(&self) -> StatusCode {
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Fetch(_) => StatusCode::BAD_GATEWAY,
            Self::Extract(_) | Self::Llm(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = Json(json!({ "error": self.to_string() }));
        (self.status(), body).into_response()
    }
}

impl From<webclaw_fetch::FetchError> for ApiError {
    fn from(e: webclaw_fetch::FetchError) -> Self {
        match e {
            webclaw_fetch::FetchError::InvalidUrl(msg) => {
                Self::BadRequest(format!("invalid url: {msg}"))
            }
            other => Self::Fetch(other.to_string()),
        }
    }
}

impl From<webclaw_core::ExtractError> for ApiError {
    fn from(e: webclaw_core::ExtractError) -> Self {
        Self::Extract(e.to_string())
    }
}

impl From<webclaw_llm::LlmError> for ApiError {
    fn from(e: webclaw_llm::LlmError) -> Self {
        Self::Llm(e.to_string())
    }
}
