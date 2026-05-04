//! `POST /v1/scrape/{vertical}` and `GET /v1/extractors`.
//!
//! Vertical extractors return typed JSON instead of generic markdown.
//! See `webclaw_fetch::extractors` for the catalog and per-site logic.

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_fetch::extractors::{self, ExtractorDispatchError};

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct ScrapeRequest {
    pub url: String,
}

/// Map dispatcher errors to ApiError so users get clean HTTP statuses
/// instead of opaque 500s.
impl From<ExtractorDispatchError> for ApiError {
    fn from(e: ExtractorDispatchError) -> Self {
        match e {
            ExtractorDispatchError::UnknownVertical(_) => ApiError::NotFound,
            ExtractorDispatchError::UrlMismatch { .. } => ApiError::bad_request(e.to_string()),
            ExtractorDispatchError::Fetch(f) => ApiError::from(f),
        }
    }
}

/// `GET /v1/extractors` — catalog of all available verticals.
pub async fn list_extractors() -> Json<Value> {
    Json(json!({
        "extractors": extractors::list(),
    }))
}

/// `POST /v1/scrape/{vertical}` — explicit vertical, e.g. /v1/scrape/reddit.
pub async fn scrape_vertical(
    State(state): State<AppState>,
    Path(vertical): Path<String>,
    Json(req): Json<ScrapeRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;
    let data = extractors::dispatch_by_name(state.fetch(), &vertical, url.as_str()).await?;
    Ok(Json(json!({
        "vertical": vertical,
        "url": req.url,
        "data": data,
    })))
}
