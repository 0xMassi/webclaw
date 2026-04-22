//! GitHub repository structured extractor.
//!
//! Uses GitHub's public REST API at `api.github.com/repos/{owner}/{repo}`.
//! Unauthenticated requests get 60/hour per IP, which is fine for users
//! self-hosting and for low-volume cloud usage. Production cloud should
//! set a `GITHUB_TOKEN` to lift to 5,000/hour, but the extractor doesn't
//! depend on it being set — it works open out of the box.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "github_repo",
    label: "GitHub repository",
    description: "Returns repo metadata: stars, forks, topics, license, default branch, recent activity.",
    url_patterns: &["https://github.com/{owner}/{repo}"],
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
    // Path must be exactly /{owner}/{repo} (or with trailing slash). Reject
    // sub-pages (issues, pulls, blob, etc.) so we don't claim URLs the
    // future github_issue / github_pr extractors will handle.
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
    segs.len() == 2 && !RESERVED_OWNERS.contains(&segs[0])
}

/// GitHub uses some top-level paths for non-repo pages.
const RESERVED_OWNERS: &[&str] = &[
    "settings",
    "marketplace",
    "explore",
    "topics",
    "trending",
    "collections",
    "events",
    "sponsors",
    "issues",
    "pulls",
    "notifications",
    "new",
    "organizations",
    "login",
    "join",
    "search",
    "about",
];

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (owner, repo) = parse_owner_repo(url).ok_or_else(|| {
        FetchError::Build(format!("github_repo: cannot parse owner/repo from '{url}'"))
    })?;

    let api_url = format!("https://api.github.com/repos/{owner}/{repo}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "github_repo: repo '{owner}/{repo}' not found"
        )));
    }
    if resp.status == 403 {
        return Err(FetchError::Build(
            "github_repo: rate limited (60/hour unauth). Set GITHUB_TOKEN for 5,000/hour.".into(),
        ));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "github api returned status {}",
            resp.status
        )));
    }

    let r: Repo = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("github api parse: {e}")))?;

    Ok(json!({
        "url":              url,
        "owner":            r.owner.as_ref().map(|o| &o.login),
        "name":             r.name,
        "full_name":        r.full_name,
        "description":      r.description,
        "homepage":         r.homepage,
        "language":         r.language,
        "topics":           r.topics,
        "license":          r.license.as_ref().and_then(|l| l.spdx_id.clone()),
        "license_name":     r.license.as_ref().map(|l| l.name.clone()),
        "default_branch":   r.default_branch,
        "stars":            r.stargazers_count,
        "forks":            r.forks_count,
        "watchers":         r.subscribers_count,
        "open_issues":      r.open_issues_count,
        "size_kb":          r.size,
        "archived":         r.archived,
        "fork":             r.fork,
        "is_template":      r.is_template,
        "has_issues":       r.has_issues,
        "has_wiki":         r.has_wiki,
        "has_pages":        r.has_pages,
        "has_discussions":  r.has_discussions,
        "created_at":       r.created_at,
        "updated_at":       r.updated_at,
        "pushed_at":        r.pushed_at,
        "html_url":         r.html_url,
    }))
}

fn parse_owner_repo(url: &str) -> Option<(String, String)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let owner = segs.next()?.to_string();
    let repo = segs.next()?.to_string();
    Some((owner, repo))
}

// ---------------------------------------------------------------------------
// GitHub API types — only the fields we surface
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Repo {
    name: Option<String>,
    full_name: Option<String>,
    description: Option<String>,
    homepage: Option<String>,
    language: Option<String>,
    #[serde(default)]
    topics: Vec<String>,
    license: Option<License>,
    default_branch: Option<String>,
    stargazers_count: Option<i64>,
    forks_count: Option<i64>,
    subscribers_count: Option<i64>,
    open_issues_count: Option<i64>,
    size: Option<i64>,
    archived: Option<bool>,
    fork: Option<bool>,
    is_template: Option<bool>,
    has_issues: Option<bool>,
    has_wiki: Option<bool>,
    has_pages: Option<bool>,
    has_discussions: Option<bool>,
    created_at: Option<String>,
    updated_at: Option<String>,
    pushed_at: Option<String>,
    html_url: Option<String>,
    owner: Option<Owner>,
}

#[derive(Deserialize)]
struct Owner {
    login: String,
}

#[derive(Deserialize)]
struct License {
    name: String,
    spdx_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_repo_root_only() {
        assert!(matches("https://github.com/rust-lang/rust"));
        assert!(matches("https://github.com/rust-lang/rust/"));
        assert!(!matches("https://github.com/rust-lang/rust/issues"));
        assert!(!matches("https://github.com/rust-lang/rust/pulls/123"));
        assert!(!matches("https://github.com/rust-lang"));
        assert!(!matches("https://github.com/marketplace"));
        assert!(!matches("https://github.com/topics/rust"));
        assert!(!matches("https://example.com/foo/bar"));
    }

    #[test]
    fn parse_owner_repo_handles_trailing_slash_and_query() {
        assert_eq!(
            parse_owner_repo("https://github.com/rust-lang/rust"),
            Some(("rust-lang".into(), "rust".into()))
        );
        assert_eq!(
            parse_owner_repo("https://github.com/rust-lang/rust/?tab=foo"),
            Some(("rust-lang".into(), "rust".into()))
        );
    }
}
