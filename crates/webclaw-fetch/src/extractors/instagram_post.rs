//! Instagram post structured extractor.
//!
//! Uses Instagram's public embed endpoint
//! `/p/{shortcode}/embed/captioned/` which returns SSR HTML with the
//! full caption, author username, and thumbnail. No auth required.
//! The same endpoint serves reels and IGTV under `/reel/{code}` and
//! `/tv/{code}` URLs (we accept all three).

use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "instagram_post",
    label: "Instagram post",
    description: "Returns full caption, author username, thumbnail, and post type (post / reel / tv) via Instagram's public embed.",
    url_patterns: &[
        "https://www.instagram.com/p/{shortcode}/",
        "https://www.instagram.com/reel/{shortcode}/",
        "https://www.instagram.com/tv/{shortcode}/",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if !matches!(host, "www.instagram.com" | "instagram.com") {
        return false;
    }
    parse_shortcode(url).is_some()
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let (kind, shortcode) = parse_shortcode(url).ok_or_else(|| {
        FetchError::Build(format!(
            "instagram_post: cannot parse shortcode from '{url}'"
        ))
    })?;

    // Instagram serves the same embed HTML for posts/reels/tv under /p/.
    let embed_url = format!("https://www.instagram.com/p/{shortcode}/embed/captioned/");
    let resp = client.fetch(&embed_url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "instagram embed returned status {} for {shortcode}",
            resp.status
        )));
    }

    let html = &resp.html;
    let username = parse_username(html);
    let caption = parse_caption(html);
    let thumbnail = parse_thumbnail(html);

    Ok(json!({
        "url":               url,
        "embed_url":         embed_url,
        "shortcode":         shortcode,
        "kind":              kind,
        "data_completeness": "embed",
        "author_username":   username,
        "caption":           caption,
        "thumbnail_url":     thumbnail,
        "canonical_url":     format!("https://www.instagram.com/{}/{shortcode}/", path_segment_for(kind)),
    }))
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Returns `(kind, shortcode)` where kind ∈ {`post`, `reel`, `tv`}.
fn parse_shortcode(url: &str) -> Option<(&'static str, String)> {
    let path = url.split("://").nth(1)?.split_once('/').map(|(_, p)| p)?;
    let stripped = path.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let first = segs.next()?;
    let kind = match first {
        "p" => "post",
        "reel" | "reels" => "reel",
        "tv" => "tv",
        _ => return None,
    };
    let shortcode = segs.next()?;
    if shortcode.is_empty() {
        return None;
    }
    Some((kind, shortcode.to_string()))
}

fn path_segment_for(kind: &str) -> &'static str {
    match kind {
        "reel" => "reel",
        "tv" => "tv",
        _ => "p",
    }
}

// ---------------------------------------------------------------------------
// HTML scraping
// ---------------------------------------------------------------------------

/// Username appears as the anchor text inside `<a class="CaptionUsername">`.
fn parse_username(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?s)class="CaptionUsername"[^>]*>([^<]+)<"#).unwrap());
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| html_decode(m.as_str().trim()))
}

/// Caption sits inside `<div class="Caption">` after the username anchor.
/// We grab the whole Caption block and strip out the username link, time
/// node, and any trailing "Photo by" / "View ... on Instagram" boilerplate.
fn parse_caption(html: &str) -> Option<String> {
    static RE_OUTER: OnceLock<Regex> = OnceLock::new();
    let outer = RE_OUTER
        .get_or_init(|| Regex::new(r#"(?s)<div\s+class="Caption"[^>]*>(.*?)</div>"#).unwrap());
    let block = outer.captures(html)?.get(1)?.as_str();

    // Strip everything wrapped in <a class="CaptionUsername">...</a>.
    static RE_USER: OnceLock<Regex> = OnceLock::new();
    let user_re = RE_USER
        .get_or_init(|| Regex::new(r#"(?s)<a[^>]*class="CaptionUsername"[^>]*>.*?</a>"#).unwrap());
    let stripped = user_re.replace_all(block, "");

    // Then strip anything remaining tagged.
    static RE_TAGS: OnceLock<Regex> = OnceLock::new();
    let tag_re = RE_TAGS.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
    let text = tag_re.replace_all(&stripped, " ");

    let cleaned = collapse_whitespace(&html_decode(text.trim()));
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// Thumbnail is the `<img class="EmbeddedMediaImage">` inside the embed
/// (or the og:image as fallback).
fn parse_thumbnail(html: &str) -> Option<String> {
    static RE_IMG: OnceLock<Regex> = OnceLock::new();
    let img_re = RE_IMG.get_or_init(|| {
        Regex::new(r#"(?s)<img[^>]+class="[^"]*EmbeddedMediaImage[^"]*"[^>]+src="([^"]+)""#)
            .unwrap()
    });
    if let Some(m) = img_re.captures(html).and_then(|c| c.get(1)) {
        return Some(html_decode(m.as_str()));
    }
    static RE_OG: OnceLock<Regex> = OnceLock::new();
    let og_re = RE_OG.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:image"[^>]+content="([^"]+)""#).unwrap()
    });
    og_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| html_decode(m.as_str()))
}

fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#064;", "@")
        .replace("&#x2022;", "•")
        .replace("&hellip;", "…")
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_post_reel_tv_urls() {
        assert!(matches("https://www.instagram.com/p/DT-RICMjeK5/"));
        assert!(matches(
            "https://www.instagram.com/p/DT-RICMjeK5/?img_index=1"
        ));
        assert!(matches("https://www.instagram.com/reel/abc123/"));
        assert!(matches("https://www.instagram.com/tv/abc123/"));
        assert!(!matches("https://www.instagram.com/ticketswave"));
        assert!(!matches("https://www.instagram.com/"));
        assert!(!matches("https://example.com/p/abc/"));
    }

    #[test]
    fn parse_shortcode_reads_each_kind() {
        assert_eq!(
            parse_shortcode("https://www.instagram.com/p/DT-RICMjeK5/?img_index=1"),
            Some(("post", "DT-RICMjeK5".into()))
        );
        assert_eq!(
            parse_shortcode("https://www.instagram.com/reel/abc123/"),
            Some(("reel", "abc123".into()))
        );
        assert_eq!(
            parse_shortcode("https://www.instagram.com/tv/abc123"),
            Some(("tv", "abc123".into()))
        );
    }

    #[test]
    fn parse_username_pulls_anchor_text() {
        let html = r#"<a class="CaptionUsername" href="...">ticketswave</a>"#;
        assert_eq!(parse_username(html).as_deref(), Some("ticketswave"));
    }

    #[test]
    fn parse_caption_strips_username_anchor() {
        let html = r#"<div class="Caption"><a class="CaptionUsername" href="...">ticketswave</a> Some caption text here</div>"#;
        assert_eq!(
            parse_caption(html).as_deref(),
            Some("Some caption text here")
        );
    }
}
