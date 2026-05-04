//! POST /v1/diff — compare current page content against a prior snapshot.
//!
//! Caller passes either a full prior `ExtractionResult` or the minimal
//! `{ markdown, metadata }` shape used by the hosted API. We re-fetch
//! the URL, extract, and run `webclaw_core::diff::diff` over the pair.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_core::{Content, ExtractionResult, Metadata, diff::diff};

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize)]
pub struct DiffRequest {
    pub url: String,
    pub previous: PreviousSnapshot,
}

/// Either a full prior extraction, or the minimal `{ markdown, metadata }`
/// shape returned by /v1/scrape. Untagged so callers can send whichever
/// they have on hand.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PreviousSnapshot {
    Full(ExtractionResult),
    Minimal {
        #[serde(default)]
        markdown: String,
        #[serde(default)]
        metadata: Option<Metadata>,
    },
}

impl PreviousSnapshot {
    fn into_extraction(self) -> ExtractionResult {
        match self {
            Self::Full(r) => r,
            Self::Minimal { markdown, metadata } => ExtractionResult {
                metadata: metadata.unwrap_or_else(empty_metadata),
                content: Content {
                    markdown,
                    plain_text: String::new(),
                    links: Vec::new(),
                    images: Vec::new(),
                    code_blocks: Vec::new(),
                    raw_html: None,
                },
                domain_data: None,
                structured_data: Vec::new(),
            },
        }
    }
}

fn empty_metadata() -> Metadata {
    Metadata {
        title: None,
        description: None,
        author: None,
        published_date: None,
        language: None,
        url: None,
        site_name: None,
        image: None,
        favicon: None,
        word_count: 0,
    }
}

pub async fn diff_route(
    State(state): State<AppState>,
    Json(req): Json<DiffRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;

    let current = state.fetch().fetch_and_extract(url.as_str()).await?;
    let previous = req.previous.into_extraction();
    let result = diff(&previous, &current);

    Ok(Json(json!({
        "url": req.url,
        "status": result.status,
        "diff": result.text_diff,
        "metadata_changes": result.metadata_changes,
        "links_added": result.links_added,
        "links_removed": result.links_removed,
        "word_count_delta": result.word_count_delta,
    })))
}
