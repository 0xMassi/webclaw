//! Hacker News structured extractor.
//!
//! Uses Algolia's HN API (`hn.algolia.com/api/v1/items/{id}`) which
//! returns the full post + recursive comment tree in a single request.
//! The official Firebase API at `hacker-news.firebaseio.com` requires
//! N+1 fetches per comment, so we'd hit either timeout or rate-limit
//! on any non-trivial thread.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "hackernews",
    label: "Hacker News story",
    description: "Returns post + nested comment tree for a Hacker News item.",
    url_patterns: &[
        "https://news.ycombinator.com/item?id=N",
        "https://hn.algolia.com/items/N",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    if host == "news.ycombinator.com" {
        return url.contains("item?id=") || url.contains("item%3Fid=");
    }
    if host == "hn.algolia.com" {
        return url.contains("/items/");
    }
    false
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let id = parse_item_id(url).ok_or_else(|| {
        FetchError::Build(format!("hackernews: cannot parse item id from '{url}'"))
    })?;

    let api_url = format!("https://hn.algolia.com/api/v1/items/{id}");
    let resp = client.fetch(&api_url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "hn algolia returned status {}",
            resp.status
        )));
    }

    let item: AlgoliaItem = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("hn algolia parse: {e}")))?;

    let post = post_json(&item);
    let comments: Vec<Value> = item.children.iter().filter_map(comment_json).collect();

    Ok(json!({
        "url": url,
        "post": post,
        "comments": comments,
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pull the numeric id out of a HN URL. Handles `item?id=N` and the
/// Algolia mirror's `/items/N` form.
fn parse_item_id(url: &str) -> Option<u64> {
    if let Some(after) = url.split("id=").nth(1) {
        let n = after.split('&').next().unwrap_or(after);
        if let Ok(id) = n.parse::<u64>() {
            return Some(id);
        }
    }
    if let Some(after) = url.split("/items/").nth(1) {
        let n = after.split(['/', '?', '#']).next().unwrap_or(after);
        if let Ok(id) = n.parse::<u64>() {
            return Some(id);
        }
    }
    None
}

fn post_json(item: &AlgoliaItem) -> Value {
    json!({
        "id":              item.id,
        "type":            item.r#type,
        "title":           item.title,
        "url":             item.url,
        "author":          item.author,
        "points":          item.points,
        "text":            item.text,                 // populated for ask/show/tell
        "created_at":      item.created_at,
        "created_at_unix": item.created_at_i,
        "comment_count":   count_descendants(item),
        "permalink":       item.id.map(|i| format!("https://news.ycombinator.com/item?id={i}")),
    })
}

fn comment_json(item: &AlgoliaItem) -> Option<Value> {
    if !matches!(item.r#type.as_deref(), Some("comment")) {
        return None;
    }
    // Dead/deleted comments still appear in the tree; surface them honestly.
    let replies: Vec<Value> = item.children.iter().filter_map(comment_json).collect();
    Some(json!({
        "id":              item.id,
        "author":          item.author,
        "text":            item.text,
        "created_at":      item.created_at,
        "created_at_unix": item.created_at_i,
        "parent_id":       item.parent_id,
        "story_id":        item.story_id,
        "replies":         replies,
    }))
}

fn count_descendants(item: &AlgoliaItem) -> usize {
    item.children
        .iter()
        .filter(|c| matches!(c.r#type.as_deref(), Some("comment")))
        .map(|c| 1 + count_descendants(c))
        .sum()
}

// ---------------------------------------------------------------------------
// Algolia API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AlgoliaItem {
    id: Option<u64>,
    r#type: Option<String>,
    title: Option<String>,
    url: Option<String>,
    author: Option<String>,
    points: Option<i64>,
    text: Option<String>,
    created_at: Option<String>,
    created_at_i: Option<i64>,
    parent_id: Option<u64>,
    story_id: Option<u64>,
    #[serde(default)]
    children: Vec<AlgoliaItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_hn_item_urls() {
        assert!(matches("https://news.ycombinator.com/item?id=1"));
        assert!(matches("https://news.ycombinator.com/item?id=12345"));
        assert!(matches("https://hn.algolia.com/items/1"));
    }

    #[test]
    fn rejects_non_item_urls() {
        assert!(!matches("https://news.ycombinator.com/"));
        assert!(!matches("https://news.ycombinator.com/news"));
        assert!(!matches("https://example.com/item?id=1"));
    }

    #[test]
    fn parse_item_id_handles_both_forms() {
        assert_eq!(
            parse_item_id("https://news.ycombinator.com/item?id=1"),
            Some(1)
        );
        assert_eq!(
            parse_item_id("https://news.ycombinator.com/item?id=12345&p=2"),
            Some(12345)
        );
        assert_eq!(parse_item_id("https://hn.algolia.com/items/999"), Some(999));
        assert_eq!(parse_item_id("https://example.com/foo"), None);
    }
}
