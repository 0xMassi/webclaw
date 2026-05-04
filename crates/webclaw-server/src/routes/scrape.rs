//! POST /v1/scrape — fetch a URL, run extraction, return the requested
//! formats. JSON shape mirrors the hosted-API response where possible so
//! migrating from self-hosted → cloud is a config change, not a code one.

use axum::{Json, extract::State};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_core::{ExtractionOptions, llm::to_llm_text};

use crate::{error::ApiError, state::AppState};

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ScrapeRequest {
    pub url: String,
    /// Output formats. Allowed: "markdown", "text", "llm", "json", "html".
    /// Defaults to ["markdown"]. Accepts a single string ("format")
    /// or an array ("formats") for hosted-API compatibility.
    #[serde(alias = "format")]
    pub formats: ScrapeFormats,
    pub include_selectors: Vec<String>,
    pub exclude_selectors: Vec<String>,
    pub only_main_content: bool,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ScrapeFormats {
    One(String),
    Many(Vec<String>),
}

impl Default for ScrapeFormats {
    fn default() -> Self {
        Self::Many(vec!["markdown".into()])
    }
}

impl ScrapeFormats {
    fn as_vec(&self) -> Vec<String> {
        match self {
            Self::One(s) => vec![s.clone()],
            Self::Many(v) => v.clone(),
        }
    }
}

pub async fn scrape(
    State(state): State<AppState>,
    Json(req): Json<ScrapeRequest>,
) -> Result<Json<Value>, ApiError> {
    if req.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }
    let url = webclaw_fetch::url_security::validate_public_http_url(&req.url).await?;
    let formats = req.formats.as_vec();

    let options = ExtractionOptions {
        include_selectors: req.include_selectors,
        exclude_selectors: req.exclude_selectors,
        only_main_content: req.only_main_content,
        include_raw_html: formats.iter().any(|f| f == "html"),
    };

    let extraction = state
        .fetch()
        .fetch_and_extract_with_options(url.as_str(), &options)
        .await?;

    let mut body = json!({
        "url": extraction.metadata.url.clone().unwrap_or_else(|| url.to_string()),
        "metadata": extraction.metadata,
    });
    let obj = body.as_object_mut().expect("json::object");

    for f in &formats {
        match f.as_str() {
            "markdown" => {
                obj.insert("markdown".into(), json!(extraction.content.markdown));
            }
            "text" => {
                obj.insert("text".into(), json!(extraction.content.plain_text));
            }
            "llm" => {
                let llm = to_llm_text(&extraction, extraction.metadata.url.as_deref());
                obj.insert("llm".into(), json!(llm));
            }
            "html" => {
                if let Some(raw) = &extraction.content.raw_html {
                    obj.insert("html".into(), json!(raw));
                }
            }
            "json" => {
                obj.insert("json".into(), json!(extraction));
            }
            other => {
                return Err(ApiError::bad_request(format!(
                    "unknown format: '{other}' (allowed: markdown, text, llm, html, json)"
                )));
            }
        }
    }

    if !extraction.structured_data.is_empty() {
        obj.insert("structured_data".into(), json!(extraction.structured_data));
    }

    Ok(Json(body))
}
