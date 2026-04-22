//! HuggingFace dataset structured extractor.
//!
//! Same shape as the model extractor but hits the dataset endpoint.
//! `huggingface.co/api/datasets/{owner}/{name}`.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "huggingface_dataset",
    label: "HuggingFace dataset",
    description: "Returns dataset metadata: downloads, likes, license, language, task categories, file list.",
    url_patterns: &["https://huggingface.co/datasets/{owner}/{name}"],
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
    // /datasets/{name} (legacy top-level) or /datasets/{owner}/{name} (canonical).
    segs.first().copied() == Some("datasets") && (segs.len() == 2 || segs.len() == 3)
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let dataset_path = parse_dataset_path(url).ok_or_else(|| {
        FetchError::Build(format!(
            "hf_dataset: cannot parse dataset path from '{url}'"
        ))
    })?;

    let api_url = format!("https://huggingface.co/api/datasets/{dataset_path}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "hf_dataset: '{dataset_path}' not found"
        )));
    }
    if resp.status == 401 {
        return Err(FetchError::Build(format!(
            "hf_dataset: '{dataset_path}' requires authentication (gated)"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "hf_dataset api returned status {}",
            resp.status
        )));
    }

    let d: DatasetInfo = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("hf_dataset parse: {e}")))?;

    let files: Vec<Value> = d
        .siblings
        .iter()
        .map(|s| json!({"rfilename": s.rfilename, "size": s.size}))
        .collect();

    Ok(json!({
        "url":             url,
        "id":              d.id,
        "private":         d.private,
        "gated":           d.gated,
        "downloads":       d.downloads,
        "downloads_30d":   d.downloads_all_time,
        "likes":           d.likes,
        "tags":            d.tags,
        "license":         d.card_data.as_ref().and_then(|c| c.license.clone()),
        "language":        d.card_data.as_ref().and_then(|c| c.language.clone()),
        "task_categories": d.card_data.as_ref().and_then(|c| c.task_categories.clone()),
        "size_categories": d.card_data.as_ref().and_then(|c| c.size_categories.clone()),
        "annotations_creators": d.card_data.as_ref().and_then(|c| c.annotations_creators.clone()),
        "configs":         d.card_data.as_ref().and_then(|c| c.configs.clone()),
        "created_at":      d.created_at,
        "last_modified":   d.last_modified,
        "sha":             d.sha,
        "file_count":      d.siblings.len(),
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

/// Returns the part to append to the API URL — either `name` (legacy
/// top-level dataset like `squad`) or `owner/name` (canonical form).
fn parse_dataset_path(url: &str) -> Option<String> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    if segs.next() != Some("datasets") {
        return None;
    }
    let first = segs.next()?.to_string();
    match segs.next() {
        Some(second) => Some(format!("{first}/{second}")),
        None => Some(first),
    }
}

#[derive(Deserialize)]
struct DatasetInfo {
    id: Option<String>,
    private: Option<bool>,
    gated: Option<serde_json::Value>,
    downloads: Option<i64>,
    #[serde(rename = "downloadsAllTime")]
    downloads_all_time: Option<i64>,
    likes: Option<i64>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "createdAt")]
    created_at: Option<String>,
    #[serde(rename = "lastModified")]
    last_modified: Option<String>,
    sha: Option<String>,
    #[serde(rename = "cardData")]
    card_data: Option<DatasetCard>,
    #[serde(default)]
    siblings: Vec<Sibling>,
}

#[derive(Deserialize)]
struct DatasetCard {
    license: Option<serde_json::Value>,
    language: Option<serde_json::Value>,
    task_categories: Option<serde_json::Value>,
    size_categories: Option<serde_json::Value>,
    annotations_creators: Option<serde_json::Value>,
    configs: Option<serde_json::Value>,
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
    fn matches_dataset_pages() {
        assert!(matches("https://huggingface.co/datasets/squad")); // legacy top-level
        assert!(matches("https://huggingface.co/datasets/openai/gsm8k")); // canonical owner/name
        assert!(!matches("https://huggingface.co/openai/whisper-large-v3"));
        assert!(!matches("https://huggingface.co/datasets/"));
    }

    #[test]
    fn parse_dataset_path_works() {
        assert_eq!(
            parse_dataset_path("https://huggingface.co/datasets/squad"),
            Some("squad".into())
        );
        assert_eq!(
            parse_dataset_path("https://huggingface.co/datasets/openai/gsm8k"),
            Some("openai/gsm8k".into())
        );
        assert_eq!(
            parse_dataset_path("https://huggingface.co/datasets/openai/gsm8k/?lib=transformers"),
            Some("openai/gsm8k".into())
        );
    }
}
