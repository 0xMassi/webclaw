//! GitHub pull request structured extractor.
//!
//! Uses `api.github.com/repos/{owner}/{repo}/pulls/{number}`. Returns
//! the PR metadata + a counted summary of comments and review activity.
//! Full diff and per-comment bodies require additional calls — left for
//! a follow-up enhancement so the v1 stays one network round-trip.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "github_pr",
    label: "GitHub pull request",
    description: "Returns PR metadata: title, body, state, author, labels, additions/deletions, file count.",
    url_patterns: &["https://github.com/{owner}/{repo}/pull/{number}"],
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
    parse_pr(url).is_some()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (owner, repo, number) = parse_pr(url).ok_or_else(|| {
        FetchError::Build(format!("github_pr: cannot parse pull-request URL '{url}'"))
    })?;

    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/pulls/{number}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "github_pr: pull request '{owner}/{repo}#{number}' not found"
        )));
    }
    if resp.status == 403 {
        return Err(FetchError::Build(
            "github_pr: rate limited (60/hour unauth). Set GITHUB_TOKEN for 5,000/hour.".into(),
        ));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "github api returned status {}",
            resp.status
        )));
    }

    let p: PullRequest = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("github pr parse: {e}")))?;

    Ok(json!({
        "url":            url,
        "owner":          owner,
        "repo":           repo,
        "number":         p.number,
        "title":          p.title,
        "body":           p.body,
        "state":          p.state,
        "draft":          p.draft,
        "merged":         p.merged,
        "merged_at":      p.merged_at,
        "merge_commit_sha": p.merge_commit_sha,
        "author":         p.user.as_ref().and_then(|u| u.login.clone()),
        "labels":         p.labels.iter().filter_map(|l| l.name.clone()).collect::<Vec<_>>(),
        "milestone":      p.milestone.as_ref().and_then(|m| m.title.clone()),
        "head_ref":       p.head.as_ref().and_then(|r| r.ref_name.clone()),
        "base_ref":       p.base.as_ref().and_then(|r| r.ref_name.clone()),
        "head_sha":       p.head.as_ref().and_then(|r| r.sha.clone()),
        "additions":      p.additions,
        "deletions":      p.deletions,
        "changed_files":  p.changed_files,
        "commits":        p.commits,
        "comments":       p.comments,
        "review_comments":p.review_comments,
        "created_at":     p.created_at,
        "updated_at":     p.updated_at,
        "closed_at":      p.closed_at,
        "html_url":       p.html_url,
    }))
}

fn parse_pr(url: &str) -> Option<(String, String, u64)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let segs: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    // /{owner}/{repo}/pull/{number} (or /pulls/{number} variant)
    if segs.len() < 4 {
        return None;
    }
    if segs[2] != "pull" && segs[2] != "pulls" {
        return None;
    }
    let number: u64 = segs[3].parse().ok()?;
    Some((segs[0].to_string(), segs[1].to_string(), number))
}

// ---------------------------------------------------------------------------
// GitHub PR API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PullRequest {
    number: Option<i64>,
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    draft: Option<bool>,
    merged: Option<bool>,
    merged_at: Option<String>,
    merge_commit_sha: Option<String>,
    user: Option<UserRef>,
    #[serde(default)]
    labels: Vec<LabelRef>,
    milestone: Option<Milestone>,
    head: Option<GitRef>,
    base: Option<GitRef>,
    additions: Option<i64>,
    deletions: Option<i64>,
    changed_files: Option<i64>,
    commits: Option<i64>,
    comments: Option<i64>,
    review_comments: Option<i64>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closed_at: Option<String>,
    html_url: Option<String>,
}

#[derive(Deserialize)]
struct UserRef {
    login: Option<String>,
}

#[derive(Deserialize)]
struct LabelRef {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Milestone {
    title: Option<String>,
}

#[derive(Deserialize)]
struct GitRef {
    #[serde(rename = "ref")]
    ref_name: Option<String>,
    sha: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_pr_urls() {
        assert!(matches("https://github.com/rust-lang/rust/pull/12345"));
        assert!(matches(
            "https://github.com/rust-lang/rust/pull/12345/files"
        ));
        assert!(!matches("https://github.com/rust-lang/rust"));
        assert!(!matches("https://github.com/rust-lang/rust/issues/100"));
        assert!(!matches("https://github.com/rust-lang"));
    }

    #[test]
    fn parse_pr_extracts_owner_repo_number() {
        assert_eq!(
            parse_pr("https://github.com/rust-lang/rust/pull/12345"),
            Some(("rust-lang".into(), "rust".into(), 12345))
        );
        assert_eq!(
            parse_pr("https://github.com/rust-lang/rust/pull/12345/files"),
            Some(("rust-lang".into(), "rust".into(), 12345))
        );
    }
}
