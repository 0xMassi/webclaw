//! Substack post extractor.
//!
//! Every Substack publication exposes `/api/v1/posts/{slug}` that
//! returns the full post as JSON: body HTML, cover image, author,
//! publication info, reactions, paywall state. No auth on public
//! posts.
//!
//! Works on both `*.substack.com` subdomains and custom domains
//! (e.g. `simonwillison.net` uses Substack too). Detection is
//! "URL has `/p/{slug}`" because that's the canonical Substack post
//! path. Explicit-call only because the `/p/{slug}` URL shape is
//! used by non-Substack sites too.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "substack_post",
    label: "Substack post",
    description: "Returns post HTML, title, subtitle, author, publication, reactions, paywall status via the Substack public API.",
    url_patterns: &[
        "https://{pub}.substack.com/p/{slug}",
        "https://{custom-domain}/p/{slug}",
    ],
};

pub fn matches(url: &str) -> bool {
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    url.contains("/p/")
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let slug = parse_slug(url).ok_or_else(|| {
        FetchError::Build(format!("substack_post: cannot parse slug from '{url}'"))
    })?;
    let host = host_of(url);
    if host.is_empty() {
        return Err(FetchError::Build(format!(
            "substack_post: empty host in '{url}'"
        )));
    }
    let scheme = if url.starts_with("http://") {
        "http"
    } else {
        "https"
    };
    let api_url = format!("{scheme}://{host}/api/v1/posts/{slug}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "substack_post: '{slug}' not found on {host} (got 404). \
             If the publication isn't actually on Substack, use /v1/scrape instead."
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "substack returned status {} for {api_url}",
            resp.status
        )));
    }

    let p: Post = serde_json::from_str(&resp.html).map_err(|e| {
        FetchError::BodyDecode(format!(
            "substack_post: '{host}' didn't return Substack JSON, likely not a Substack ({e})"
        ))
    })?;

    Ok(json!({
        "url":                  url,
        "api_url":              api_url,
        "id":                   p.id,
        "type":                 p.r#type,
        "slug":                 p.slug,
        "title":                p.title,
        "subtitle":             p.subtitle,
        "description":          p.description,
        "canonical_url":        p.canonical_url,
        "post_date":            p.post_date,
        "updated_at":           p.updated_at,
        "audience":             p.audience,
        "has_paywall":          matches!(p.audience.as_deref(), Some("only_paid") | Some("founding")),
        "is_free_preview":      p.is_free_preview,
        "cover_image":          p.cover_image,
        "word_count":           p.wordcount,
        "reactions":            p.reactions,
        "comment_count":        p.comment_count,
        "body_html":            p.body_html,
        "body_text":            p.truncated_body_text.or(p.body_text),
        "publication": json!({
            "id":           p.publication.as_ref().and_then(|pub_| pub_.id),
            "name":         p.publication.as_ref().and_then(|pub_| pub_.name.clone()),
            "subdomain":    p.publication.as_ref().and_then(|pub_| pub_.subdomain.clone()),
            "custom_domain":p.publication.as_ref().and_then(|pub_| pub_.custom_domain.clone()),
        }),
        "authors": p.published_bylines.iter().map(|a| json!({
            "id":     a.id,
            "name":   a.name,
            "handle": a.handle,
            "photo":  a.photo_url,
        })).collect::<Vec<_>>(),
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

fn parse_slug(url: &str) -> Option<String> {
    let after = url.split("/p/").nth(1)?;
    let stripped = after
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/')
        .split('/')
        .next()
        .unwrap_or("");
    if stripped.is_empty() {
        None
    } else {
        Some(stripped.to_string())
    }
}

// ---------------------------------------------------------------------------
// Substack API types (subset)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Post {
    id: Option<i64>,
    r#type: Option<String>,
    slug: Option<String>,
    title: Option<String>,
    subtitle: Option<String>,
    description: Option<String>,
    canonical_url: Option<String>,
    post_date: Option<String>,
    updated_at: Option<String>,
    audience: Option<String>,
    is_free_preview: Option<bool>,
    cover_image: Option<String>,
    wordcount: Option<i64>,
    reactions: Option<serde_json::Value>,
    comment_count: Option<i64>,
    body_html: Option<String>,
    body_text: Option<String>,
    truncated_body_text: Option<String>,
    publication: Option<Publication>,
    #[serde(default, rename = "publishedBylines")]
    published_bylines: Vec<Byline>,
}

#[derive(Deserialize)]
struct Publication {
    id: Option<i64>,
    name: Option<String>,
    subdomain: Option<String>,
    custom_domain: Option<String>,
}

#[derive(Deserialize)]
struct Byline {
    id: Option<i64>,
    name: Option<String>,
    handle: Option<String>,
    photo_url: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_post_urls() {
        assert!(matches(
            "https://stratechery.substack.com/p/the-tech-letter"
        ));
        assert!(matches("https://simonwillison.net/p/2024-08-01-something"));
        assert!(!matches("https://example.com/"));
        assert!(!matches("ftp://example.com/p/foo"));
    }

    #[test]
    fn parse_slug_strips_query_and_trailing_slash() {
        assert_eq!(
            parse_slug("https://example.substack.com/p/my-post"),
            Some("my-post".into())
        );
        assert_eq!(
            parse_slug("https://example.substack.com/p/my-post/"),
            Some("my-post".into())
        );
        assert_eq!(
            parse_slug("https://example.substack.com/p/my-post?ref=123"),
            Some("my-post".into())
        );
    }
}
