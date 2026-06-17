//! POST /v1/search — web search via Serper.dev using the operator's own key.
//!
//! Enabled only when the server is started with `SERPER_API_KEY` set
//! (get a free key at serper.dev). Without it, this route returns 501 so
//! self-hosters know the capability exists but isn't configured.
//!
//! With `scrape: true`, each result page is fetched + extracted to
//! markdown via the shared [`webclaw_fetch::FetchClient`]. A per-result
//! fetch failure leaves that result's `content` null; it never fails the
//! whole search.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    /// Max results to return (default 5, clamped to 1..=10).
    #[serde(default = "default_num_results")]
    pub num_results: usize,
    /// Country code for localization (e.g. "us", "gb", "it").
    pub country: Option<String>,
    /// Language code for localization (e.g. "en", "it").
    pub lang: Option<String>,
    /// When true, fetch + extract each result page and include its markdown.
    #[serde(default)]
    pub scrape: bool,
}

fn default_num_results() -> usize {
    5
}

pub async fn search(
    State(state): State<AppState>,
    Json(req): Json<SearchRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.query.trim().is_empty() {
        return Err(ApiError::bad_request("`query` is required"));
    }

    let serper_key = state.serper_api_key().ok_or_else(|| {
        ApiError::not_implemented(
            "search is not configured: start the server with SERPER_API_KEY set \
             (get a free key at serper.dev)",
        )
    })?;

    let opts = webclaw_fetch::SearchOptions {
        num_results: req.num_results,
        country: req.country.clone(),
        lang: req.lang.clone(),
        scrape: req.scrape,
    };

    let results = webclaw_fetch::search(state.fetch(), serper_key, &req.query, &opts)
        .await
        .map_err(|e| ApiError::internal(format!("search failed: {e}")))?;

    Ok(Json(json!({
        "query": req.query,
        "count": results.len(),
        "results": results,
    })))
}
