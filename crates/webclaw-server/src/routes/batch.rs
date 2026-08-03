//! POST /v1/batch — fetch + extract many URLs in parallel.
//!
//! There is no maximum batch size. There used to be one because the aggregate
//! response is built entirely in memory before anything is sent, so a large
//! batch had to be refused to stop the process growing with the request. That
//! is a property of the *response shape*, not a sensible policy, and capping
//! the count only moved the failure somewhere less obvious.
//!
//! Set `"stream": true` to get NDJSON instead: one JSON object per line,
//! written as each URL completes. Nothing accumulates server-side, so batch
//! size stops mattering and the caller sees progress instead of waiting for
//! the whole set. Results arrive in completion order.
//!
//! `concurrency` is still bounded — that one is politeness toward the sites
//! being fetched, not a limit on the caller.

use axum::body::Body;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Json, extract::State};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_core::ExtractionOptions;

use crate::{error::ApiError, state::AppState};

/// Ceiling on in-flight requests. Politeness toward target sites, not a cap
/// on how many URLs a caller may submit.
const HARD_MAX_CONCURRENCY: usize = 20;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct BatchRequest {
    pub urls: Vec<String>,
    pub concurrency: Option<usize>,
    /// Stream NDJSON (one result per line, in completion order) instead of
    /// buffering the whole batch into a single JSON response.
    pub stream: bool,
    pub include_selectors: Vec<String>,
    pub exclude_selectors: Vec<String>,
    pub only_main_content: bool,
}

pub async fn batch(
    State(state): State<AppState>,
    Json(req): Json<BatchRequest>,
) -> Result<Response, ApiError> {
    if req.urls.is_empty() {
        return Err(ApiError::bad_request("`urls` is required"));
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

    if req.stream {
        // Nothing is collected: each result is serialised and flushed as it
        // lands, so memory is flat regardless of how many URLs were submitted.
        // The stream outlives this handler, so it owns its own client handle
        // rather than borrowing from `state`.
        let client = std::sync::Arc::clone(state.fetch());
        let lines = client
            .fetch_and_extract_batch_stream(safe_urls, concurrency, options)
            .map(|r| {
                let value = match r.result {
                    Ok(extraction) => json!({
                        "url": r.url,
                        "ok": true,
                        "metadata": extraction.metadata,
                        "markdown": extraction.content.markdown,
                    }),
                    Err(e) => json!({ "url": r.url, "ok": false, "error": e.to_string() }),
                };
                // A serialisation failure must not kill the stream mid-batch;
                // emit a parseable error line for that URL and keep going.
                let mut line = serde_json::to_string(&value).unwrap_or_else(|e| {
                    json!({ "ok": false, "error": format!("serialize failed: {e}") }).to_string()
                });
                line.push('\n');
                Ok::<_, std::convert::Infallible>(line)
            });

        return Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "application/x-ndjson")
            .body(Body::from_stream(lines))
            .map_err(|e| ApiError::internal(format!("failed to start stream: {e}")));
    }

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
    }))
    .into_response())
}
