//! POST /v1/map — discover URLs from a site's sitemaps.
//!
//! Walks robots.txt + common sitemap paths, recursively resolves
//! `<sitemapindex>` files, and returns the deduplicated list of URLs.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_fetch::sitemap;

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct MapRequest {
    pub url: String,
    /// When true, return the full SitemapEntry objects (with lastmod,
    /// priority, changefreq). Defaults to false → bare URL strings,
    /// matching the hosted-API shape.
    #[serde(default)]
    pub include_metadata: bool,
}

pub async fn map(
    State(state): State<AppState>,
    Json(req): Json<MapRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;

    let entries = sitemap::discover(state.fetch(), url.as_str()).await?;

    let body = if req.include_metadata {
        json!({
            "url": req.url,
            "count": entries.len(),
            "urls": entries,
        })
    } else {
        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        json!({
            "url": req.url,
            "count": urls.len(),
            "urls": urls,
        })
    };

    Ok(Json(body))
}
