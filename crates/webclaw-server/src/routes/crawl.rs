//! POST /v1/crawl — synchronous BFS crawl.
//!
//! NOTE: this server is stateless — there is no job queue. Crawls run
//! inline and return when complete. `max_pages` is hard-capped at 500
//! to avoid OOM on naive callers. For large crawls + async jobs, use
//! the hosted API at api.webclaw.io.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use std::time::Duration;
use webclaw_fetch::{CrawlConfig, Crawler, FetchConfig};

use crate::{error::ApiError, state::AppState};

const HARD_MAX_PAGES: usize = 500;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CrawlRequest {
    pub url: String,
    pub max_depth: Option<usize>,
    pub max_pages: Option<usize>,
    pub use_sitemap: bool,
    pub concurrency: Option<usize>,
    pub allow_subdomains: bool,
    pub allow_external_links: bool,
    pub include_patterns: Vec<String>,
    pub exclude_patterns: Vec<String>,
}

pub async fn crawl(
    State(_state): State<AppState>,
    Json(req): Json<CrawlRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;
    let max_pages = req.max_pages.unwrap_or(50).min(HARD_MAX_PAGES);
    let max_depth = req.max_depth.unwrap_or(3);
    let concurrency = req.concurrency.unwrap_or(5).min(20);

    let config = CrawlConfig {
        fetch: FetchConfig::default(),
        max_depth,
        max_pages,
        concurrency,
        delay: Duration::from_millis(200),
        path_prefix: None,
        use_sitemap: req.use_sitemap,
        include_patterns: req.include_patterns,
        exclude_patterns: req.exclude_patterns,
        allow_subdomains: req.allow_subdomains,
        allow_external_links: req.allow_external_links,
        progress_tx: None,
        cancel_flag: None,
    };

    let crawler = Crawler::new(url.as_str(), config).map_err(ApiError::from)?;
    let result = crawler.crawl(url.as_str(), None).await;

    let pages: Vec<Value> = result
        .pages
        .iter()
        .map(|p| {
            json!({
                "url": p.url,
                "depth": p.depth,
                "metadata": p.extraction.as_ref().map(|e| &e.metadata),
                "markdown": p.extraction.as_ref().map(|e| e.content.markdown.as_str()).unwrap_or(""),
                "error": p.error,
            })
        })
        .collect();

    Ok(Json(json!({
        "url": req.url,
        "status": "completed",
        "total": result.total,
        "completed": result.ok,
        "errors": result.errors,
        "elapsed_secs": result.elapsed_secs,
        "pages": pages,
    })))
}
