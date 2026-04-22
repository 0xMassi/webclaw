//! GitHub issue structured extractor.
//!
//! Mirror of `github_pr` but on `/issues/{number}`. Uses
//! `api.github.com/repos/{owner}/{repo}/issues/{number}`. Returns the
//! issue body + comment count + labels + milestone + author /
//! assignees. Full per-comment bodies would be another call; kept for
//! a follow-up.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "github_issue",
    label: "GitHub issue",
    description: "Returns issue metadata: title, body, state, author, labels, assignees, milestone, comment count.",
    url_patterns: &["https://github.com/{owner}/{repo}/issues/{number}"],
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
    parse_issue(url).is_some()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (owner, repo, number) = parse_issue(url).ok_or_else(|| {
        FetchError::Build(format!("github_issue: cannot parse issue URL '{url}'"))
    })?;

    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/issues/{number}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "github_issue: issue '{owner}/{repo}#{number}' not found"
        )));
    }
    if resp.status == 403 {
        return Err(FetchError::Build(
            "github_issue: rate limited (60/hour unauth). Set GITHUB_TOKEN for 5,000/hour.".into(),
        ));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "github api returned status {}",
            resp.status
        )));
    }

    let issue: Issue = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("github issue parse: {e}")))?;

    // The same endpoint returns PRs too; reject if we got one so the caller
    // uses /v1/scrape/github_pr instead of getting a half-shaped payload.
    if issue.pull_request.is_some() {
        return Err(FetchError::Build(format!(
            "github_issue: '{owner}/{repo}#{number}' is a pull request, use /v1/scrape/github_pr"
        )));
    }

    Ok(json!({
        "url":         url,
        "owner":       owner,
        "repo":        repo,
        "number":      issue.number,
        "title":       issue.title,
        "body":        issue.body,
        "state":       issue.state,
        "state_reason":issue.state_reason,
        "author":      issue.user.as_ref().and_then(|u| u.login.clone()),
        "labels":      issue.labels.iter().filter_map(|l| l.name.clone()).collect::<Vec<_>>(),
        "assignees":   issue.assignees.iter().filter_map(|u| u.login.clone()).collect::<Vec<_>>(),
        "milestone":   issue.milestone.as_ref().and_then(|m| m.title.clone()),
        "comments":    issue.comments,
        "locked":      issue.locked,
        "created_at":  issue.created_at,
        "updated_at":  issue.updated_at,
        "closed_at":   issue.closed_at,
        "html_url":    issue.html_url,
    }))
}

fn parse_issue(url: &str) -> Option<(String, String, u64)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let segs: Vec<&str> = stripped.split('/').filter(|s| !s.is_empty()).collect();
    if segs.len() < 4 || segs[2] != "issues" {
        return None;
    }
    let number: u64 = segs[3].parse().ok()?;
    Some((segs[0].to_string(), segs[1].to_string(), number))
}

// ---------------------------------------------------------------------------
// GitHub issue API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Issue {
    number: Option<i64>,
    title: Option<String>,
    body: Option<String>,
    state: Option<String>,
    state_reason: Option<String>,
    locked: Option<bool>,
    comments: Option<i64>,
    created_at: Option<String>,
    updated_at: Option<String>,
    closed_at: Option<String>,
    html_url: Option<String>,
    user: Option<UserRef>,
    #[serde(default)]
    labels: Vec<LabelRef>,
    #[serde(default)]
    assignees: Vec<UserRef>,
    milestone: Option<Milestone>,
    /// Present when this "issue" is actually a pull request. The REST
    /// API overloads the issues endpoint for PRs.
    pull_request: Option<serde_json::Value>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_issue_urls() {
        assert!(matches("https://github.com/rust-lang/rust/issues/100"));
        assert!(matches("https://github.com/rust-lang/rust/issues/100/"));
        assert!(!matches("https://github.com/rust-lang/rust"));
        assert!(!matches("https://github.com/rust-lang/rust/pull/100"));
        assert!(!matches("https://github.com/rust-lang/rust/issues"));
    }

    #[test]
    fn parse_issue_extracts_owner_repo_number() {
        assert_eq!(
            parse_issue("https://github.com/rust-lang/rust/issues/100"),
            Some(("rust-lang".into(), "rust".into(), 100))
        );
        assert_eq!(
            parse_issue("https://github.com/rust-lang/rust/issues/100/?foo=bar"),
            Some(("rust-lang".into(), "rust".into(), 100))
        );
    }
}
