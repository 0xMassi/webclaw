//! dev.to article structured extractor.
//!
//! `dev.to/api/articles/{username}/{slug}` returns the full article body,
//! tags, reaction count, comment count, and reading time. Anonymous
//! access works fine for published posts.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "dev_to",
    label: "dev.to article",
    description: "Returns article metadata + body: title, body markdown, tags, reactions, comments, reading time.",
    url_patterns: &["https://dev.to/{username}/{slug}"],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "dev.to" && host != "www.dev.to" {
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
    // Need exactly /{username}/{slug}, with username starting with non-reserved.
    segs.len() == 2 && !RESERVED_FIRST_SEGS.contains(&segs[0])
}

const RESERVED_FIRST_SEGS: &[&str] = &[
    "api",
    "tags",
    "search",
    "settings",
    "enter",
    "signup",
    "about",
    "code-of-conduct",
    "privacy",
    "terms",
    "contact",
    "sponsorships",
    "sponsors",
    "shop",
    "videos",
    "listings",
    "podcasts",
    "p",
    "t",
];

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let (username, slug) = parse_username_slug(url).ok_or_else(|| {
        FetchError::Build(format!("dev_to: cannot parse username/slug from '{url}'"))
    })?;

    let api_url = format!("https://dev.to/api/articles/{username}/{slug}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "dev_to: article '{username}/{slug}' not found"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "dev.to api returned status {}",
            resp.status
        )));
    }

    let a: Article = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("dev.to parse: {e}")))?;

    Ok(json!({
        "url":               url,
        "id":                a.id,
        "title":             a.title,
        "description":       a.description,
        "body_markdown":     a.body_markdown,
        "url_canonical":     a.canonical_url,
        "published_at":      a.published_at,
        "edited_at":         a.edited_at,
        "reading_time_min":  a.reading_time_minutes,
        "tags":              a.tag_list,
        "positive_reactions": a.positive_reactions_count,
        "public_reactions":  a.public_reactions_count,
        "comments_count":    a.comments_count,
        "page_views_count":  a.page_views_count,
        "cover_image":       a.cover_image,
        "author": json!({
            "username":  a.user.as_ref().and_then(|u| u.username.clone()),
            "name":      a.user.as_ref().and_then(|u| u.name.clone()),
            "twitter":   a.user.as_ref().and_then(|u| u.twitter_username.clone()),
            "github":    a.user.as_ref().and_then(|u| u.github_username.clone()),
            "website":   a.user.as_ref().and_then(|u| u.website_url.clone()),
        }),
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

fn parse_username_slug(url: &str) -> Option<(String, String)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let username = segs.next()?;
    let slug = segs.next()?;
    Some((username.to_string(), slug.to_string()))
}

// ---------------------------------------------------------------------------
// dev.to API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Article {
    id: Option<i64>,
    title: Option<String>,
    description: Option<String>,
    body_markdown: Option<String>,
    canonical_url: Option<String>,
    published_at: Option<String>,
    edited_at: Option<String>,
    reading_time_minutes: Option<i64>,
    tag_list: Option<serde_json::Value>, // string OR array depending on endpoint
    positive_reactions_count: Option<i64>,
    public_reactions_count: Option<i64>,
    comments_count: Option<i64>,
    page_views_count: Option<i64>,
    cover_image: Option<String>,
    user: Option<UserRef>,
}

#[derive(Deserialize)]
struct UserRef {
    username: Option<String>,
    name: Option<String>,
    twitter_username: Option<String>,
    github_username: Option<String>,
    website_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_article_urls() {
        assert!(matches("https://dev.to/ben/welcome-thread"));
        assert!(matches("https://dev.to/0xmassi/some-post-1abc"));
        assert!(!matches("https://dev.to/"));
        assert!(!matches("https://dev.to/api/articles/foo/bar"));
        assert!(!matches("https://dev.to/tags/rust"));
        assert!(!matches("https://dev.to/ben")); // user profile, not article
        assert!(!matches("https://example.com/ben/post"));
    }

    #[test]
    fn parse_pulls_username_and_slug() {
        assert_eq!(
            parse_username_slug("https://dev.to/ben/welcome-thread"),
            Some(("ben".into(), "welcome-thread".into()))
        );
        assert_eq!(
            parse_username_slug("https://dev.to/0xmassi/some-post-1abc/?foo=bar"),
            Some(("0xmassi".into(), "some-post-1abc".into()))
        );
    }
}
