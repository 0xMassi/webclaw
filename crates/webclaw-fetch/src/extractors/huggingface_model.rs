//! HuggingFace model card structured extractor.
//!
//! Uses the public model API at `huggingface.co/api/models/{owner}/{name}`.
//! Returns metadata + the parsed model card front matter, but does not
//! pull the full README body — those are sometimes 100KB+ and the user
//! can hit /v1/scrape if they want it as markdown.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "huggingface_model",
    label: "HuggingFace model",
    description: "Returns model metadata: downloads, likes, license, pipeline tag, library name, file list.",
    url_patterns: &["https://huggingface.co/{owner}/{name}"],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "huggingface.co" && host != "www.huggingface.co" {
        return false;
    }
    let path = url
        .split("://")
        .nth(1)
        .and_then(|s| s.split_once('/'))
        .map(|(_, p)| p)
        .unwrap_or("");
    let stripped = path
        .split(['?', '#'])
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    let segs: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    // /{owner}/{name} but reject HF-internal sections + sub-pages.
    if segs.len() != 2 {
        return false;
    }
    !RESERVED_NAMESPACES.contains(&segs[0])
}

const RESERVED_NAMESPACES: &[&str] = &[
    "datasets",
    "spaces",
    "blog",
    "docs",
    "api",
    "models",
    "papers",
    "pricing",
    "tasks",
    "join",
    "login",
    "settings",
    "organizations",
    "new",
    "search",
];

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (owner, name) = parse_owner_name(url).ok_or_else(|| {
        FetchError::Build(format!("hf model: cannot parse owner/name from '{url}'"))
    })?;

    let api_url = format!("https://huggingface.co/api/models/{owner}/{name}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "hf model: '{owner}/{name}' not found"
        )));
    }
    if resp.status == 401 {
        return Err(FetchError::Build(format!(
            "hf model: '{owner}/{name}' requires authentication (gated repo)"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "hf api returned status {}",
            resp.status
        )));
    }

    let m: ModelInfo = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("hf api parse: {e}")))?;

    // Surface a flat file list — full siblings can be hundreds of entries
    // for big repos. We keep it as-is because callers want to know about
    // every shard; if it bloats responses too much we'll add pagination.
    let files: Vec<Value> = m
        .siblings
        .iter()
        .map(|s| json!({"rfilename": s.rfilename, "size": s.size}))
        .collect();

    Ok(json!({
        "url":             url,
        "id":              m.id,
        "model_id":        m.model_id,
        "private":         m.private,
        "gated":           m.gated,
        "downloads":       m.downloads,
        "downloads_30d":   m.downloads_all_time,
        "likes":           m.likes,
        "library_name":    m.library_name,
        "pipeline_tag":    m.pipeline_tag,
        "tags":            m.tags,
        "license":         m.card_data.as_ref().and_then(|c| c.license.clone()),
        "language":        m.card_data.as_ref().and_then(|c| c.language.clone()),
        "datasets":        m.card_data.as_ref().and_then(|c| c.datasets.clone()),
        "base_model":      m.card_data.as_ref().and_then(|c| c.base_model.clone()),
        "model_type":      m.card_data.as_ref().and_then(|c| c.model_type.clone()),
        "created_at":      m.created_at,
        "last_modified":   m.last_modified,
        "sha":             m.sha,
        "file_count":      m.siblings.len(),
        "files":           files,
    }))
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

fn parse_owner_name(url: &str) -> Option<(String, String)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let owner = segs.next()?.to_string();
    let name = segs.next()?.to_string();
    Some((owner, name))
}

// ---------------------------------------------------------------------------
// HF API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ModelInfo {
    id: Option<String>,
    #[serde(rename = "modelId")]
    model_id: Option<String>,
    private: Option<bool>,
    gated: Option<serde_json::Value>, // bool or string ("auto" / "manual" / false)
    downloads: Option<i64>,
    #[serde(rename = "downloadsAllTime")]
    downloads_all_time: Option<i64>,
    likes: Option<i64>,
    #[serde(rename = "library_name")]
    library_name: Option<String>,
    #[serde(rename = "pipeline_tag")]
    pipeline_tag: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
    sha: Option<String>,
    #[serde(rename = "cardData")]
    card_data: Option<CardData>,
    #[serde(default)]
    siblings: Vec<Sibling>,
}

#[derive(Deserialize)]
struct CardData {
    license: Option<serde_json::Value>, // string or array
    language: Option<serde_json::Value>,
    datasets: Option<serde_json::Value>,
    #[serde(rename = "base_model")]
    base_model: Option<serde_json::Value>,
    #[serde(rename = "model_type")]
    model_type: Option<String>,
}

#[derive(Deserialize)]
struct Sibling {
    rfilename: String,
    size: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_model_pages() {
        assert!(matches("https://huggingface.co/meta-llama/Meta-Llama-3-8B"));
        assert!(matches("https://huggingface.co/openai/whisper-large-v3"));
        assert!(matches("https://huggingface.co/bert-base-uncased/main")); // owner=bert-base-uncased name=main: false positive but acceptable for v1
    }

    #[test]
    fn rejects_hf_section_pages() {
        assert!(!matches("https://huggingface.co/datasets/squad"));
        assert!(!matches("https://huggingface.co/spaces/foo/bar"));
        assert!(!matches("https://huggingface.co/blog/intro"));
        assert!(!matches("https://huggingface.co/"));
        assert!(!matches("https://huggingface.co/meta-llama"));
    }

    #[test]
    fn parse_owner_name_pulls_both() {
        assert_eq!(
            parse_owner_name("https://huggingface.co/meta-llama/Meta-Llama-3-8B"),
            Some(("meta-llama".into(), "Meta-Llama-3-8B".into()))
        );
        assert_eq!(
            parse_owner_name("https://huggingface.co/openai/whisper-large-v3?library=transformers"),
            Some(("openai".into(), "whisper-large-v3".into()))
        );
    }
}
