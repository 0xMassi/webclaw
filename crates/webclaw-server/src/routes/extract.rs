//! POST /v1/extract — LLM-powered structured extraction.
//!
//! Two modes:
//! * `schema` — JSON Schema describing what to extract.
//! * `prompt` — natural-language instructions.
//!
//! At least one must be provided. The provider chain is built per
//! request from env (Ollama -> OpenAI -> Anthropic). Self-hosters
//! get the same fallback behaviour as the CLI.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_llm::{ProviderChain, extract::extract_json, extract::extract_with_prompt};

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ExtractRequest {
    pub url: String,
    pub schema: Option<Value>,
    pub prompt: Option<String>,
    /// Optional override of the provider model name (e.g. `gpt-4o-mini`).
    pub model: Option<String>,
}

pub async fn extract(
    State(state): State<AppState>,
    Json(req): Json<ExtractRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let has_schema = req.schema.is_some();
    let has_prompt = req
        .prompt
        .as_deref()
        .map(|p| !p.trim().is_empty())
        .unwrap_or(false);
    if !has_schema && !has_prompt {
        return Err(ApiError::bad_request(
            "either `schema` or `prompt` is required",
        ));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;

    // Fetch + extract first so we feed the LLM clean markdown instead of
    // raw HTML. Cheaper tokens, better signal.
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

    let model = req.model.as_deref();
    let data = if let Some(schema) = req.schema.as_ref() {
        extract_json(&content, schema, &chain, model).await?
    } else {
        let prompt = req.prompt.as_deref().unwrap_or_default();
        extract_with_prompt(&content, prompt, &chain, model).await?
    };

    Ok(Json(json!({
        "url": req.url,
        "data": data,
    })))
}
