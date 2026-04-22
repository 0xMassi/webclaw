//! crates.io structured extractor.
//!
//! Uses the public JSON API at `crates.io/api/v1/crates/{name}`. No
//! auth, no rate limit at normal usage. The response includes both
//! the crate metadata and the full version list, which we summarize
//! down to a count + latest release info to keep the payload small.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "crates_io",
    label: "crates.io package",
    description: "Returns crate metadata: latest version, dependencies, downloads, license, repository.",
    url_patterns: &[
        "https://crates.io/crates/{name}",
        "https://crates.io/crates/{name}/{version}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "crates.io" && host != "www.crates.io" {
        return false;
    }
    url.contains("/crates/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let name = parse_name(url)
        .ok_or_else(|| FetchError::Build(format!("crates.io: cannot parse name from '{url}'")))?;

    let api_url = format!("https://crates.io/api/v1/crates/{name}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "crates.io: crate '{name}' not found"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "crates.io api returned status {}",
            resp.status
        )));
    }

    let body: CratesResponse = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("crates.io parse: {e}")))?;

    let c = body.crate_;
    let latest_version = body
        .versions
        .iter()
        .find(|v| !v.yanked.unwrap_or(false))
        .or_else(|| body.versions.first());

    Ok(json!({
        "url":                 url,
        "name":                c.id,
        "description":         c.description,
        "homepage":            c.homepage,
        "documentation":       c.documentation,
        "repository":          c.repository,
        "max_stable_version":  c.max_stable_version,
        "max_version":         c.max_version,
        "newest_version":      c.newest_version,
        "downloads":           c.downloads,
        "recent_downloads":    c.recent_downloads,
        "categories":          c.categories,
        "keywords":            c.keywords,
        "release_count":       body.versions.len(),
        "latest_release_date": latest_version.and_then(|v| v.created_at.clone()),
        "latest_license":      latest_version.and_then(|v| v.license.clone()),
        "latest_rust_version": latest_version.and_then(|v| v.rust_version.clone()),
        "latest_yanked":       latest_version.and_then(|v| v.yanked),
        "created_at":          c.created_at,
        "updated_at":          c.updated_at,
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

fn parse_name(url: &str) -> Option<String> {
    let after = url.split("/crates/").nth(1)?;
    let stripped = after.split(['?', '#']).next()?.trim_end_matches('/');
    let first = stripped.split('/').find(|s| !s.is_empty())?;
    Some(first.to_string())
}

// ---------------------------------------------------------------------------
// crates.io API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CratesResponse {
    #[serde(rename = "crate")]
    crate_: CrateInfo,
    #[serde(default)]
    versions: Vec<VersionInfo>,
}

#[derive(Deserialize)]
struct CrateInfo {
    id: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    documentation: Option<String>,
    repository: Option<String>,
    max_stable_version: Option<String>,
    max_version: Option<String>,
    newest_version: Option<String>,
    downloads: Option<i64>,
    recent_downloads: Option<i64>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct VersionInfo {
    license: Option<String>,
    rust_version: Option<String>,
    yanked: Option<bool>,
    created_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_crate_pages() {
        assert!(matches("https://crates.io/crates/serde"));
        assert!(matches("https://crates.io/crates/tokio/1.45.0"));
        assert!(!matches("https://crates.io/"));
        assert!(!matches("https://example.com/crates/foo"));
    }

    #[test]
    fn parse_name_handles_versioned_urls() {
        assert_eq!(
            parse_name("https://crates.io/crates/serde"),
            Some("serde".into())
        );
        assert_eq!(
            parse_name("https://crates.io/crates/tokio/1.45.0"),
            Some("tokio".into())
        );
        assert_eq!(
            parse_name("https://crates.io/crates/scraper/?foo=bar"),
            Some("scraper".into())
        );
    }
}
