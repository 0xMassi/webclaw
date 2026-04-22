//! Reddit structured extractor — returns the full post + comment tree
//! as typed JSON via Reddit's `.json` API.
//!
//! The same trick the markdown extractor in `crate::reddit` uses:
//! appending `.json` to any post URL returns the data the new SPA
//! frontend would load client-side. Zero antibot, zero JS rendering.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "reddit",
    label: "Reddit thread",
    description: "Returns post + nested comment tree with scores, authors, and timestamps.",
    url_patterns: &[
        "https://www.reddit.com/r/*/comments/*",
        "https://reddit.com/r/*/comments/*",
        "https://old.reddit.com/r/*/comments/*",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    let is_reddit_host = matches!(
        host,
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "np.reddit.com" | "new.reddit.com"
    );
    is_reddit_host && url.contains("/comments/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let json_url = build_json_url(url);
    let resp = client.fetch(&json_url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "reddit api returned status {}",
            resp.status
        )));
    }

    let listings: Vec<Listing> = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("reddit json parse: {e}")))?;

    if listings.is_empty() {
        return Err(FetchError::BodyDecode("reddit response empty".into()));
    }

    // First listing = the post (single t3 child).
    let post = listings
        .first()
        .and_then(|l| l.data.children.first())
        .filter(|t| t.kind == "t3")
        .map(|t| post_json(&t.data))
        .unwrap_or(Value::Null);

    // Second listing = the comment tree.
    let comments: Vec<Value> = listings
        .get(1)
        .map(|l| l.data.children.iter().filter_map(comment_json).collect())
        .unwrap_or_default();

    Ok(json!({
        "url": url,
        "post": post,
        "comments": comments,
    }))
}

// ---------------------------------------------------------------------------
// JSON shapers
// ---------------------------------------------------------------------------

fn post_json(d: &ThingData) -> Value {
    json!({
        "id":               d.id,
        "title":            d.title,
        "author":           d.author,
        "subreddit":        d.subreddit_name_prefixed,
        "permalink":        d.permalink.as_ref().map(|p| format!("https://www.reddit.com{p}")),
        "url":              d.url_overridden_by_dest,
        "is_self":          d.is_self,
        "selftext":         d.selftext,
        "score":            d.score,
        "upvote_ratio":     d.upvote_ratio,
        "num_comments":     d.num_comments,
        "created_utc":      d.created_utc,
        "link_flair_text":  d.link_flair_text,
        "over_18":          d.over_18,
        "spoiler":          d.spoiler,
        "stickied":         d.stickied,
        "locked":           d.locked,
    })
}

/// Render a single comment + its reply tree. Returns `None` for non-t1
/// kinds (the trailing `more` placeholder Reddit injects at depth limits).
fn comment_json(thing: &Thing) -> Option<Value> {
    if thing.kind != "t1" {
        return None;
    }
    let d = &thing.data;
    let replies: Vec<Value> = match &d.replies {
        Some(Replies::Listing(l)) => l.data.children.iter().filter_map(comment_json).collect(),
        _ => Vec::new(),
    };
    Some(json!({
        "id":             d.id,
        "author":         d.author,
        "body":           d.body,
        "score":          d.score,
        "created_utc":    d.created_utc,
        "is_submitter":   d.is_submitter,
        "stickied":       d.stickied,
        "depth":          d.depth,
        "permalink":      d.permalink.as_ref().map(|p| format!("https://www.reddit.com{p}")),
        "replies":        replies,
    }))
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Build the Reddit JSON URL. We keep the original host (`www.reddit.com`
/// or `old.reddit.com` as the caller gave us). Routing through
/// `old.reddit.com` unconditionally looks appealing but that host has
/// stricter UA-based blocking than `www.reddit.com`, while the main
/// host accepts our Chrome-fingerprinted client fine.
fn build_json_url(url: &str) -> String {
    let clean = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    format!("{clean}.json?raw_json=1")
}

// ---------------------------------------------------------------------------
// Reddit JSON types — only fields we render. Everything else is dropped.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Listing {
    data: ListingData,
}

#[derive(Deserialize)]
struct ListingData {
    children: Vec<Thing>,
}

#[derive(Deserialize)]
struct Thing {
    kind: String,
    data: ThingData,
}

#[derive(Deserialize, Default)]
struct ThingData {
    // post (t3)
    id: Option<String>,
    title: Option<String>,
    selftext: Option<String>,
    subreddit_name_prefixed: Option<String>,
    url_overridden_by_dest: Option<String>,
    is_self: Option<bool>,
    upvote_ratio: Option<f64>,
    num_comments: Option<i64>,
    over_18: Option<bool>,
    spoiler: Option<bool>,
    stickied: Option<bool>,
    locked: Option<bool>,
    link_flair_text: Option<String>,

    // comment (t1)
    author: Option<String>,
    body: Option<String>,
    score: Option<i64>,
    created_utc: Option<f64>,
    is_submitter: Option<bool>,
    depth: Option<i64>,
    permalink: Option<String>,

    // recursive
    replies: Option<Replies>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Replies {
    Listing(Listing),
    #[allow(dead_code)]
    Empty(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_reddit_post_urls() {
        assert!(matches(
            "https://www.reddit.com/r/rust/comments/abc123/some_title/"
        ));
        assert!(matches(
            "https://reddit.com/r/rust/comments/abc123/some_title"
        ));
        assert!(matches("https://old.reddit.com/r/rust/comments/abc123/x/"));
    }

    #[test]
    fn rejects_non_post_reddit_urls() {
        assert!(!matches("https://www.reddit.com/r/rust"));
        assert!(!matches("https://www.reddit.com/user/foo"));
        assert!(!matches("https://example.com/r/rust/comments/x"));
    }

    #[test]
    fn json_url_appends_suffix_and_drops_query() {
        assert_eq!(
            build_json_url("https://www.reddit.com/r/rust/comments/abc/x/?utm=foo"),
            "https://www.reddit.com/r/rust/comments/abc/x.json?raw_json=1"
        );
    }
}
