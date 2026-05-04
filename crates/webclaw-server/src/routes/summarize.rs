//! POST /v1/summarize — LLM-powered page summary.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_llm::{ProviderChain, summarize::summarize};

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct SummarizeRequest {
    pub url: String,
    pub max_sentences: Option<usize>,
    pub model: Option<String>,
}

pub async fn summarize_route(
    State(state): State<AppState>,
    Json(req): Json<SummarizeRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;

    let extraction = state.fetch().fetch_and_extract(url.as_str()).await?;
    let content = if extraction.content.markdown.trim().is_empty() {
        extraction.content.plain_text.clone()
    } else {
        extraction.content.markdown.clone()
    };
    if content.trim().is_empty() {
        return Err(ApiError::Extract(
            "no extractable content on page".to_string(),
        ));
    }

    let chain = ProviderChain::default().await;
    if chain.is_empty() {
        return Err(ApiError::Llm(
            "no LLM providers configured (set OLLAMA_HOST, OPENAI_API_KEY, or ANTHROPIC_API_KEY)"
                .to_string(),
        ));
    }

    let summary = summarize(&content, req.max_sentences, &chain, req.model.as_deref()).await?;

    Ok(Json(json!({
        "url": req.url,
        "summary": summary,
    })))
}
