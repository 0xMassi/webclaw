//! GitHub release structured extractor.
//!
//! `api.github.com/repos/{owner}/{repo}/releases/tags/{tag}`. Returns
//! the release notes body, asset list with download counts, and
//! prerelease flag.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "github_release",
    label: "GitHub release",
    description: "Returns release metadata: tag, name, body (release notes), assets with download counts.",
    url_patterns: &["https://github.com/{owner}/{repo}/releases/tag/{tag}"],
};

pub fn matches(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    if host != "github.com" && host != "www.github.com" {
        return false;
    }
    parse_release(url).is_some()
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let (owner, repo, tag) = parse_release(url).ok_or_else(|| {
        FetchError::Build(format!("github_release: cannot parse release URL '{url}'"))
    })?;

    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/releases/tags/{tag}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "github_release: release '{owner}/{repo}@{tag}' not found"
        )));
    }
    if resp.status == 403 {
        return Err(FetchError::Build(
            "github_release: rate limited (60/hour unauth). Set GITHUB_TOKEN for 5,000/hour."
                .into(),
        ));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "github api returned status {}",
            resp.status
        )));
    }

    let r: Release = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("github release parse: {e}")))?;

    let assets: Vec<Value> = r
        .assets
        .iter()
        .map(|a| {
            json!({
                "name": a.name,
                "size": a.size,
                "download_count": a.download_count,
                "browser_download_url": a.browser_download_url,
                "content_type": a.content_type,
                "created_at": a.created_at,
                "updated_at": a.updated_at,
            })
        })
        .collect();

    Ok(json!({
        "url":           url,
        "owner":         owner,
        "repo":          repo,
        "tag_name":      r.tag_name,
        "name":          r.name,
        "body":          r.body,
        "draft":         r.draft,
        "prerelease":    r.prerelease,
        "author":        r.author.as_ref().and_then(|u| u.login.clone()),
        "created_at":    r.created_at,
        "published_at":  r.published_at,
        "asset_count":   assets.len(),
        "total_downloads": r.assets.iter().map(|a| a.download_count.unwrap_or(0)).sum::<i64>(),
        "assets":        assets,
        "html_url":      r.html_url,
    }))
}

fn parse_release(url: &str) -> Option<(String, String, String)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let segs: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    // /{owner}/{repo}/releases/tag/{tag}
    if segs.len() < 5 {
        return None;
    }
    if segs[2] != "releases" || segs[3] != "tag" {
        return None;
    }
    Some((
        segs[0].to_string(),
        segs[1].to_string(),
        segs[4].to_string(),
    ))
}

// ---------------------------------------------------------------------------
// GitHub Release API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Release {
    tag_name: Option<String>,
    name: Option<String>,
    body: Option<String>,
    draft: Option<bool>,
    prerelease: Option<bool>,
    author: Option<UserRef>,
    created_at: Option<String>,
    published_at: Option<String>,
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct UserRef {
    login: Option<String>,
}

#[derive(Deserialize)]
struct Asset {
    name: Option<String>,
    size: Option<i64>,
    download_count: Option<i64>,
    browser_download_url: Option<String>,
    content_type: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_release_urls() {
        assert!(matches(
            "https://github.com/rust-lang/rust/releases/tag/1.85.0"
        ));
        assert!(matches(
            "https://github.com/0xMassi/webclaw/releases/tag/v0.4.0"
        ));
        assert!(!matches("https://github.com/rust-lang/rust"));
        assert!(!matches("https://github.com/rust-lang/rust/releases"));
        assert!(!matches("https://github.com/rust-lang/rust/pull/100"));
    }

    #[test]
    fn parse_release_extracts_owner_repo_tag() {
        assert_eq!(
            parse_release("https://github.com/0xMassi/webclaw/releases/tag/v0.4.0"),
            Some(("0xMassi".into(), "webclaw".into(), "v0.4.0".into()))
        );
        assert_eq!(
            parse_release("https://github.com/rust-lang/rust/releases/tag/1.85.0/?foo=bar"),
            Some(("rust-lang".into(), "rust".into(), "1.85.0".into()))
        );
    }
}
