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
//!
//! ## Fallback
//!
//! The API endpoint is rate-limited aggressively on popular publications
//! and occasionally returns 403 on custom domains with Cloudflare in
//! front. When that happens we escalate to an HTML fetch (via
//! `smart_fetch_html`, so antibot-protected custom domains still work)
//! and extract OG tags + Article JSON-LD for a degraded-but-useful
//! payload. The response shape stays stable across both paths; a
//! `data_source` field tells the caller which branch ran.

use std::sync::OnceLock;

use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::cloud::{self, CloudError};
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "substack_post",
    label: "Substack post",
    description: "Returns post HTML, title, subtitle, author, publication, reactions, paywall status via the Substack public API. Falls back to OG + JSON-LD HTML parsing when the API is rate-limited.",
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

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
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

    // 1. Try the public API. 200 = full payload; 404 = real miss; any
    //    other status hands off to the HTML fallback so a transient rate
    //    limit or a hardened custom domain doesn't fail the whole call.
    let resp = client.fetch(&api_url).await?;
    match resp.status {
        200 => match serde_json::from_str::<Post>(&resp.html) {
            Ok(p) => Ok(build_api_payload(url, &api_url, &slug, p)),
            Err(e) => {
                // API returned 200 but the body isn't the Post shape we
                // expect. Could be a custom-domain site that exposes
                // something else at /api/v1/posts/. Fall back to HTML
                // rather than hard-failing.
                html_fallback(
                    client,
                    url,
                    &api_url,
                    &slug,
                    Some(format!(
                        "api returned 200 but body was not Substack JSON ({e})"
                    )),
                )
                .await
            }
        },
        404 => Err(FetchError::Build(format!(
            "substack_post: '{slug}' not found on {host} (got 404). \
             If the publication isn't actually on Substack, use /v1/scrape instead."
        ))),
        _ => {
            // Rate limit, 403, 5xx, whatever: try HTML.
            let reason = format!("api returned status {} for {api_url}", resp.status);
            html_fallback(client, url, &api_url, &slug, Some(reason)).await
        }
    }
}

// ---------------------------------------------------------------------------
// API-path payload builder
// ---------------------------------------------------------------------------

fn build_api_payload(url: &str, api_url: &str, slug: &str, p: Post) -> Value {
    json!({
        "url":                  url,
        "api_url":              api_url,
        "data_source":          "api",
        "id":                   p.id,
        "type":                 p.r#type,
        "slug":                 p.slug.or_else(|| Some(slug.to_string())),
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
    })
}

// ---------------------------------------------------------------------------
// HTML fallback: OG + Article JSON-LD
// ---------------------------------------------------------------------------

async fn html_fallback(
    client: &dyn Fetcher,
    url: &str,
    api_url: &str,
    slug: &str,
    fallback_reason: Option<String>,
) -> Result<Value, FetchError> {
    let fetched = cloud::smart_fetch_html(client, client.cloud(), url)
        .await
        .map_err(cloud_to_fetch_err)?;

    let mut data = parse_html(&fetched.html, url, api_url, slug);
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "fetch_source".into(),
            match fetched.source {
                cloud::FetchSource::Local => json!("local"),
                cloud::FetchSource::Cloud => json!("cloud"),
            },
        );
        if let Some(reason) = fallback_reason {
            obj.insert("fallback_reason".into(), json!(reason));
        }
    }
    Ok(data)
}

/// Pure HTML parser. Pulls title, subtitle, description, cover image,
/// publish date, and authors from OG tags and Article JSON-LD. Kept
/// public so tests can exercise it with fixtures.
pub fn parse_html(html: &str, url: &str, api_url: &str, slug: &str) -> Value {
    let article = find_article_jsonld(html);

    let title = article
        .as_ref()
        .and_then(|v| get_text(v, "headline"))
        .or_else(|| og(html, "title"));
    let description = article
        .as_ref()
        .and_then(|v| get_text(v, "description"))
        .or_else(|| og(html, "description"));
    let cover_image = article
        .as_ref()
        .and_then(get_first_image)
        .or_else(|| og(html, "image"));
    let post_date = article
        .as_ref()
        .and_then(|v| get_text(v, "datePublished"))
        .or_else(|| meta_property(html, "article:published_time"));
    let updated_at = article.as_ref().and_then(|v| get_text(v, "dateModified"));
    let publication_name = og(html, "site_name");
    let authors = article.as_ref().map(extract_authors).unwrap_or_default();

    json!({
        "url":                  url,
        "api_url":              api_url,
        "data_source":          "html_fallback",
        "slug":                 slug,
        "title":                title,
        "subtitle":             None::<String>,
        "description":          description,
        "canonical_url":        canonical_url(html).or_else(|| Some(url.to_string())),
        "post_date":            post_date,
        "updated_at":           updated_at,
        "cover_image":          cover_image,
        "body_html":            None::<String>,
        "body_text":            None::<String>,
        "word_count":           None::<i64>,
        "comment_count":        None::<i64>,
        "reactions":            Value::Null,
        "has_paywall":          None::<bool>,
        "is_free_preview":      None::<bool>,
        "publication": json!({
            "name": publication_name,
        }),
        "authors": authors,
    })
}

fn extract_authors(v: &Value) -> Vec<Value> {
    let Some(a) = v.get("author") else {
        return Vec::new();
    };
    let one = |val: &Value| -> Option<Value> {
        match val {
            Value::String(s) => Some(json!({"name": s})),
            Value::Object(_) => {
                let name = val.get("name").and_then(|n| n.as_str())?;
                let handle = val
                    .get("url")
                    .and_then(|u| u.as_str())
                    .and_then(handle_from_author_url);
                Some(json!({
                    "name":   name,
                    "handle": handle,
                }))
            }
            _ => None,
        }
    };
    match a {
        Value::Array(arr) => arr.iter().filter_map(one).collect(),
        _ => one(a).into_iter().collect(),
    }
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

/// Extract the Substack handle from an author URL like
/// `https://substack.com/@handle` or `https://pub.substack.com/@handle`.
///
/// Returns `None` when the URL has no `@` segment (e.g. a non-Substack
/// author page) so we don't synthesise a fake handle.
fn handle_from_author_url(u: &str) -> Option<String> {
    let after = u.rsplit_once('@').map(|(_, tail)| tail)?;
    let clean = after.split(['/', '?', '#']).next()?;
    if clean.is_empty() {
        None
    } else {
        Some(clean.to_string())
    }
}

// ---------------------------------------------------------------------------
// HTML tag helpers
// ---------------------------------------------------------------------------

fn og(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

/// Pull `<meta property="article:published_time" content="...">` and
/// similar structured meta tags.
fn meta_property(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="([^"]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

fn canonical_url(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE
        .get_or_init(|| Regex::new(r#"(?i)<link[^>]+rel="canonical"[^>]+href="([^"]+)""#).unwrap());
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

// ---------------------------------------------------------------------------
// JSON-LD walkers (Article / NewsArticle)
// ---------------------------------------------------------------------------

fn find_article_jsonld(html: &str) -> Option<Value> {
    let blocks = webclaw_core::structured_data::extract_json_ld(html);
    for b in blocks {
        if let Some(found) = find_article_in(&b) {
            return Some(found);
        }
    }
    None
}

fn find_article_in(v: &Value) -> Option<Value> {
    if is_article_type(v) {
        return Some(v.clone());
    }
    if let Some(graph) = v.get("@graph").and_then(|g| g.as_array()) {
        for item in graph {
            if let Some(found) = find_article_in(item) {
                return Some(found);
            }
        }
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(found) = find_article_in(item) {
                return Some(found);
            }
        }
    }
    None
}

fn is_article_type(v: &Value) -> bool {
    let Some(t) = v.get("@type") else {
        return false;
    };
    let is_art = |s: &str| {
        matches!(
            s,
            "Article" | "NewsArticle" | "BlogPosting" | "SocialMediaPosting"
        )
    };
    match t {
        Value::String(s) => is_art(s),
        Value::Array(arr) => arr.iter().any(|x| x.as_str().is_some_and(is_art)),
        _ => false,
    }
}

fn get_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| match x {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn get_first_image(v: &Value) -> Option<String> {
    match v.get("image")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => arr.iter().find_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Object(_) => x.get("url").and_then(|u| u.as_str()).map(String::from),
            _ => None,
        }),
        Value::Object(o) => o.get("url").and_then(|u| u.as_str()).map(String::from),
        _ => None,
    }
}

fn cloud_to_fetch_err(e: CloudError) -> FetchError {
    FetchError::Build(e.to_string())
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

    #[test]
    fn parse_html_extracts_from_og_tags() {
        let html = r##"
<html><head>
<meta property="og:title" content="My Great Post">
<meta property="og:description" content="A short summary.">
<meta property="og:image" content="https://cdn.substack.com/cover.jpg">
<meta property="og:site_name" content="My Publication">
<meta property="article:published_time" content="2025-09-01T10:00:00Z">
<link rel="canonical" href="https://mypub.substack.com/p/my-post">
</head></html>"##;
        let v = parse_html(
            html,
            "https://mypub.substack.com/p/my-post",
            "https://mypub.substack.com/api/v1/posts/my-post",
            "my-post",
        );
        assert_eq!(v["data_source"], "html_fallback");
        assert_eq!(v["title"], "My Great Post");
        assert_eq!(v["description"], "A short summary.");
        assert_eq!(v["cover_image"], "https://cdn.substack.com/cover.jpg");
        assert_eq!(v["post_date"], "2025-09-01T10:00:00Z");
        assert_eq!(v["publication"]["name"], "My Publication");
        assert_eq!(v["canonical_url"], "https://mypub.substack.com/p/my-post");
    }

    #[test]
    fn parse_html_prefers_jsonld_when_present() {
        let html = r##"
<html><head>
<meta property="og:title" content="OG Title">
<script type="application/ld+json">
{"@context":"https://schema.org","@type":"NewsArticle",
 "headline":"JSON-LD Title",
 "description":"JSON-LD desc.",
 "image":"https://cdn.substack.com/hero.jpg",
 "datePublished":"2025-10-12T08:30:00Z",
 "dateModified":"2025-10-12T09:00:00Z",
 "author":[{"@type":"Person","name":"Alice Author","url":"https://substack.com/@alice"}]}
</script>
</head></html>"##;
        let v = parse_html(
            html,
            "https://example.com/p/a",
            "https://example.com/api/v1/posts/a",
            "a",
        );
        assert_eq!(v["title"], "JSON-LD Title");
        assert_eq!(v["description"], "JSON-LD desc.");
        assert_eq!(v["cover_image"], "https://cdn.substack.com/hero.jpg");
        assert_eq!(v["post_date"], "2025-10-12T08:30:00Z");
        assert_eq!(v["updated_at"], "2025-10-12T09:00:00Z");
        assert_eq!(v["authors"][0]["name"], "Alice Author");
        assert_eq!(v["authors"][0]["handle"], "alice");
    }

    #[test]
    fn handle_from_author_url_pulls_handle() {
        assert_eq!(
            handle_from_author_url("https://substack.com/@alice"),
            Some("alice".into())
        );
        assert_eq!(
            handle_from_author_url("https://mypub.substack.com/@bob/"),
            Some("bob".into())
        );
        assert_eq!(
            handle_from_author_url("https://not-substack.com/author/carol"),
            None
        );
    }
}
