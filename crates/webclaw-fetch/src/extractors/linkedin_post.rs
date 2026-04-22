//! LinkedIn post structured extractor.
//!
//! Uses the public embed endpoint `/embed/feed/update/{urn}` which
//! LinkedIn provides for sites that want to render a post inline. No
//! auth required, returns SSR HTML with the full post body, OG tags,
//! image, and a link back to the original post.
//!
//! Accepts both URN forms (`urn:li:share:N` and `urn:li:activity:N`)
//! and pretty post URLs (`/posts/{user}_{slug}-{id}-{suffix}`) by
//! pulling the trailing numeric id and converting to an activity URN.

use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "linkedin_post",
    label: "LinkedIn post",
    description: "Returns post body, author name, image, and original URL via LinkedIn's public embed endpoint.",
    url_patterns: &[
        "https://www.linkedin.com/feed/update/urn:li:share:{id}",
        "https://www.linkedin.com/feed/update/urn:li:activity:{id}",
        "https://www.linkedin.com/posts/{user}_{slug}-{id}-{suffix}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if !matches!(host, "www.linkedin.com" | "linkedin.com") {
        return false;
    }
    url.contains("/feed/update/urn:li:") || url.contains("/posts/")
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let urn = extract_urn(url).ok_or_else(|| {
        FetchError::Build(format!(
            "linkedin_post: cannot extract URN from '{url}' (expected /feed/update/urn:li:... or /posts/{{slug}}-{{id}})"
        ))
    })?;

    let embed_url = format!("https://www.linkedin.com/embed/feed/update/{urn}");
    let resp = client.fetch(&embed_url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "linkedin embed returned status {} for {urn}",
            resp.status
        )));
    }

    let html = &resp.html;
    let og = parse_og_tags(html);
    let body = parse_post_body(html);
    let author = parse_author(html);
    let canonical_url = og.get("url").cloned().unwrap_or_else(|| embed_url.clone());

    Ok(json!({
        "url":               url,
        "embed_url":         embed_url,
        "urn":               urn,
        "canonical_url":     canonical_url,
        "data_completeness": "embed",
        "title":             og.get("title").cloned(),
        "body":              body,
        "author_name":       author,
        "image_url":         og.get("image").cloned(),
        "site_name":         og.get("site_name").cloned().unwrap_or_else(|| "LinkedIn".into()),
    }))
}

// ---------------------------------------------------------------------------
// URN extraction
// ---------------------------------------------------------------------------

/// Pull a `urn:li:share:N` or `urn:li:activity:N` from any LinkedIn URL.
/// `/posts/{slug}-{id}-{suffix}` URLs encode the activity id as the second-
/// to-last `-` separated chunk. Both forms map to a URN we can hit the
/// embed endpoint with.
fn extract_urn(url: &str) -> Option<String> {
    if let Some(idx) = url.find("urn:li:") {
        let tail = &url[idx..];
        let end = tail.find(['/', '?', '#']).unwrap_or(tail.len());
        let urn = &tail[..end];
        // Validate shape: urn:li:{type}:{digits}
        let mut parts = urn.split(':');
        if parts.next() == Some("urn")
            && parts.next() == Some("li")
            && parts.next().is_some()
            && parts
                .next()
                .filter(|p| p.chars().all(|c| c.is_ascii_digit()))
                .is_some()
        {
            return Some(urn.to_string());
        }
    }

    // /posts/{user}_{slug}-{19-digit-id}-{4-char-hash}/ — id is the second-
    // to-last segment after the last `-`.
    if url.contains("/posts/") {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re =
            RE.get_or_init(|| Regex::new(r"/posts/[^/]*?-(\d{15,})-[A-Za-z0-9]{2,}/?").unwrap());
        if let Some(c) = re.captures(url)
            && let Some(id) = c.get(1)
        {
            return Some(format!("urn:li:activity:{}", id.as_str()));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// HTML scraping
// ---------------------------------------------------------------------------

/// Pull `og:foo` → value pairs out of `<meta property="og:..." content="...">`.
/// Returns lowercased keys with leading `og:` stripped.
fn parse_og_tags(html: &str) -> std::collections::HashMap<String, String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    let mut out = std::collections::HashMap::new();
    for c in re.captures_iter(html) {
        let k = c
            .get(1)
            .map(|m| m.as_str().to_lowercase())
            .unwrap_or_default();
        let v = c
            .get(2)
            .map(|m| html_decode(m.as_str()))
            .unwrap_or_default();
        out.entry(k).or_insert(v);
    }
    out
}

/// Extract the post body text from the embed page. LinkedIn renders it
/// inside `<p class="attributed-text-segment-list__content ...">{text}</p>`
/// where the inner content can include nested `<a>` tags for links.
fn parse_post_body(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(
            r#"(?s)<p[^>]+class="[^"]*attributed-text-segment-list__content[^"]*"[^>]*>(.*?)</p>"#,
        )
        .unwrap()
    });
    let inner = re.captures(html).and_then(|c| c.get(1))?.as_str();
    Some(strip_tags(inner).trim().to_string())
}

/// Author name lives in the `<title>` like:
///   "55 founding members are in… | Orc Dev"
/// The chunk after the final `|` is the author display name. Falls back
/// to the og:title minus the post body if there's no title.
fn parse_author(html: &str) -> Option<String> {
    static RE_TITLE: OnceLock<Regex> = OnceLock::new();
    let re = RE_TITLE.get_or_init(|| Regex::new(r"<title>([^<]+)</title>").unwrap());
    let title = re.captures(html).and_then(|c| c.get(1))?.as_str();
    title
        .rsplit_once('|')
        .map(|(_, name)| html_decode(name.trim()))
}

/// Replace the small set of HTML entities LinkedIn (and Instagram, etc.)
/// stuff into OG content attributes.
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

/// Crude HTML tag stripper for the post body. Preserves text inside
/// nested anchors so URLs don't disappear, and collapses runs of
/// whitespace introduced by line wrapping.
fn strip_tags(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap());
    let no_tags = re.replace_all(html, "").to_string();
    html_decode(&no_tags)
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_li_post_urls() {
        assert!(matches(
            "https://www.linkedin.com/feed/update/urn:li:share:7452618582213144577/"
        ));
        assert!(matches(
            "https://www.linkedin.com/feed/update/urn:li:activity:7452618583290892288"
        ));
        assert!(matches(
            "https://www.linkedin.com/posts/somebody_some-slug-7452618583290892288-aB1c"
        ));
        assert!(!matches("https://www.linkedin.com/in/foo"));
        assert!(!matches("https://www.linkedin.com/"));
        assert!(!matches("https://example.com/feed/update/urn:li:share:1"));
    }

    #[test]
    fn extract_urn_from_share_url() {
        assert_eq!(
            extract_urn("https://www.linkedin.com/feed/update/urn:li:share:7452618582213144577/"),
            Some("urn:li:share:7452618582213144577".into())
        );
    }

    #[test]
    fn extract_urn_from_pretty_post_url() {
        assert_eq!(
            extract_urn(
                "https://www.linkedin.com/posts/somebody_some-slug-7452618583290892288-aB1c/"
            ),
            Some("urn:li:activity:7452618583290892288".into())
        );
    }

    #[test]
    fn parse_og_tags_basic() {
        let html = r#"<meta property="og:image" content="https://x.com/a.png">
<meta property="og:url" content="https://example.com/x">"#;
        let og = parse_og_tags(html);
        assert_eq!(
            og.get("image").map(String::as_str),
            Some("https://x.com/a.png")
        );
        assert_eq!(
            og.get("url").map(String::as_str),
            Some("https://example.com/x")
        );
    }

    #[test]
    fn parse_post_body_strips_anchor_tags() {
        let html = r#"<p class="attributed-text-segment-list__content text-color-text" dir="ltr">Hello <a href="x">link</a> world</p>"#;
        assert_eq!(parse_post_body(html).as_deref(), Some("Hello link world"));
    }

    #[test]
    fn html_decode_handles_common_entities() {
        assert_eq!(html_decode("AT&amp;T &#064;jane"), "AT&T @jane");
    }
}
