//! POST /v1/batch — fetch + extract many URLs in parallel.
//!
//! `concurrency` is hard-capped at 20 to avoid hammering targets and
//! to bound memory growth for naive callers. For larger batches use
//! the hosted API.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_core::ExtractionOptions;

use crate::{error::ApiError, state::AppState};

const HARD_MAX_URLS: usize = 100;
const HARD_MAX_CONCURRENCY: usize = 20;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct BatchRequest {
    pub urls: Vec<String>,
    pub concurrency: Option<usize>,
    pub include_selectors: Vec<String>,
    pub exclude_selectors: Vec<String>,
    pub only_main_content: bool,
}

pub async fn batch(
    State(state): State<AppState>,
    Json(req): Json<BatchRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.urls.is_empty() {
        return Err(ApiError::bad_request("`urls` is required"));
    }
    if req.urls.len() > HARD_MAX_URLS {
        return Err(ApiError::bad_request(format!(
            "too many urls: {} (max {HARD_MAX_URLS})",
            req.urls.len()
        )));
    }
    let mut safe_urls = Vec::with_capacity(req.urls.len());
    for url in &req.urls {
        safe_urls.push(
            webclaw_fetch::url_security::validate_public_http_url(url)
                .await?
                .to_string(),
        );
    }

    let concurrency = req.concurrency.unwrap_or(5).clamp(1, HARD_MAX_CONCURRENCY);

    let options = ExtractionOptions {
        include_selectors: req.include_selectors,
        exclude_selectors: req.exclude_selectors,
        only_main_content: req.only_main_content,
        include_raw_html: false,
    };

    let url_refs: Vec<&str> = safe_urls.iter().map(|s| s.as_str()).collect();
    let results = state
        .fetch()
        .fetch_and_extract_batch_with_options(&url_refs, concurrency, &options)
        .await;

    let mut ok = 0usize;
    let mut errors = 0usize;
    let mut out: Vec<Value> = Vec::with_capacity(results.len());
    for r in results {
        match r.result {
            Ok(extraction) => {
                ok += 1;
                out.push(json!({
                    "url": r.url,
                    "metadata": extraction.metadata,
                    "markdown": extraction.content.markdown,
                }));
            }
            Err(e) => {
                errors += 1;
                out.push(json!({
                    "url": r.url,
                    "error": e.to_string(),
                }));
            }
        }
    }

    Ok(Json(json!({
        "total": out.len(),
        "completed": ok,
        "errors": errors,
        "results": out,
    })))
}
