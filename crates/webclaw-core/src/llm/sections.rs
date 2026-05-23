/// Section / nav-URL discovery for hub and aggregator pages.
///
/// M8 (issue #14, subsumes #16) — `--mode sections` returns only the
/// navigation/section URLs the page links to, suitable for an LLM caller
/// that wants to drill into a category (Economía, Política, Sport,
/// Health, ...) without first parsing a full extraction.
///
/// Approach: this is a pure FILTER over the (label, href) list that
/// `body::process_body` already produces for the page. No new fetch, no
/// new HTML parse — the heuristic walks the in-memory link list once and
/// keeps only links that look like section/nav entries by URL shape +
/// label shape + same-host + denylist signals (see `is_section_link`).
///
/// The `OutputMode::Sections` arm in the CLI calls `to_llm_sections` /
/// `to_json_sections`. The metadata header is built with
/// `include_status=false` (mirrors summary/toc — M7 status line is not
/// useful in a section list).
use url::Url;

use crate::types::ExtractionResult;

use super::body;
use super::links;
use super::metadata::build_metadata_header_with_opts;

// ---------------------------------------------------------------------------
// Section-detection heuristic
// ---------------------------------------------------------------------------

/// First-segment denylist. Any URL whose path starts with one of these
/// segments is rejected as a non-section (price tickers, user pages, auth
/// flows, comment threads, search). Catches Decrypt's 248-row `/price/*`
/// ticker ribbon cheaply, plus generic chrome across many sites.
const DENY_FIRST_SEGMENTS: &[&str] = &[
    "price",
    "prices",
    "quote",
    "quotes",
    "comments",
    "user",
    "users",
    "auth",
    "login",
    "logout",
    "register",
    "signin",
    "signup",
    "subscribe",
    "subscription",
    "share",
    "tag",
    "tags",
    "search",
    "cart",
    "checkout",
    "account",
    "profile",
];

/// Maximum number of path segments a section URL may have. Section paths
/// are 1 segment (`/sport`) or 2 (`/news/business`); article URLs are
/// typically 3+ (`/news/articles/<id>`, `/2024/05/23/<slug>`).
const MAX_PATH_SEGMENTS: usize = 2;

/// Maximum length of a single path segment. Article slugs are usually
/// longer (`big-news-headline-about-some-topic`); section names are
/// short (`business`, `health`, `editors-picks`).
const MAX_SEGMENT_LEN: usize = 30;

/// Decide whether a path segment looks like an article ID rather than a
/// section name. Article-ID heuristic: length >= 6 chars AND contains at
/// least 2 ASCII digits AND mixes letters with digits. Matches BBC
/// `crmp121z3z8o` style and CMS IDs; doesn't trip on `editors-picks` (no
/// digits) or `2024` (all digits, no letters).
fn looks_like_article_id(segment: &str) -> bool {
    if segment.len() < 6 {
        return false;
    }
    let mut digits = 0usize;
    let mut letters = 0usize;
    for c in segment.chars() {
        if c.is_ascii_digit() {
            digits += 1;
        } else if c.is_ascii_alphabetic() {
            letters += 1;
        }
    }
    digits >= 2 && letters >= 1
}

/// Test whether a URL path is shaped like a section path.
///
/// Accepts:
///   - `/`                (rare — site root link, used by some "Home" nav)
///   - `/sport`
///   - `/news/business`
///   - `/editors-picks`
///   - `/news/business/` (trailing slash)
///
/// Rejects: 3+ segment paths, segments with article-ID shape, segments
/// matching the denylist, segments containing non-`[a-z0-9-]` chars (case
/// insensitive on the alpha side), segments longer than 30 chars.
fn is_section_path(path: &str) -> bool {
    // Drop leading + trailing slash for segment count.
    let trimmed = path.trim_start_matches('/').trim_end_matches('/');
    if trimmed.is_empty() {
        // Root path "/" — treat as a section (e.g. BBC "Home" link).
        return true;
    }
    let segments: Vec<&str> = trimmed.split('/').collect();
    if segments.len() > MAX_PATH_SEGMENTS {
        return false;
    }
    for (i, seg) in segments.iter().enumerate() {
        if seg.is_empty() {
            return false;
        }
        if seg.len() > MAX_SEGMENT_LEN {
            return false;
        }
        // First-segment denylist (price ribbons, user/auth pages, search).
        if i == 0 && DENY_FIRST_SEGMENTS.contains(&seg.to_ascii_lowercase().as_str()) {
            return false;
        }
        // Article-ID-shaped segment rejection.
        if looks_like_article_id(seg) {
            return false;
        }
        // Only ASCII alpha-numeric + hyphen. Underscores, dots, digits-only
        // segments (year-paths like `/2024/`) are not sections.
        let mut has_alpha = false;
        for c in seg.chars() {
            if c.is_ascii_alphabetic() {
                has_alpha = true;
            } else if c.is_ascii_digit() || c == '-' {
                // Allowed.
            } else {
                return false;
            }
        }
        if !has_alpha {
            // Pure-digit segments (`/2024`) are not sections.
            return false;
        }
    }
    true
}

/// Same-host check: section links should usually live on the page's own
/// host (subdomains allowed). Prevents cross-domain promo nav from
/// polluting the result. Returns true iff `link_host` equals `page_host`
/// or is a subdomain ending in `.<page_host>`.
fn same_host(link_host: &str, page_host: &str) -> bool {
    if link_host.eq_ignore_ascii_case(page_host) {
        return true;
    }
    // Strip leading "www." from both for the subdomain comparison so
    // `www.bbc.com` matches `bbc.com`.
    let lh = link_host.trim_start_matches("www.").to_ascii_lowercase();
    let ph = page_host.trim_start_matches("www.").to_ascii_lowercase();
    if lh == ph {
        return true;
    }
    lh.ends_with(&format!(".{ph}"))
}

/// Decide whether `(label, href)` is a section link given the page URL.
///
/// Multi-signal AND:
///   1. URL parses with a scheme http/https
///   2. Path matches section shape (`is_section_path`)
///   3. No URL fragment (anchor links like `/news/world#bbc-main` rejected)
///   4. Same-host as the page (or subdomain)
///   5. Label is short (<=40 chars after cleaning) and <=5 words
///   6. Label is not a truncation sentinel (`...` from `clean_link_label`)
fn is_section_link(label: &str, href: &str, page_url: Option<&Url>) -> bool {
    // Label-shape gate.
    if label.is_empty() {
        return false;
    }
    if label.contains("...") {
        // Truncated long-article-title sentinel; not a section.
        return false;
    }
    if label.chars().count() > 40 {
        return false;
    }
    if label.split_whitespace().count() > 5 {
        return false;
    }

    // URL-shape gate.
    let url = match Url::parse(href) {
        Ok(u) => u,
        Err(_) => return false,
    };
    let scheme = url.scheme();
    if scheme != "http" && scheme != "https" {
        return false;
    }
    // Anchor / fragment exclusion — `/news/world#bbc-main` is not a section.
    if url.fragment().is_some() {
        return false;
    }
    // Query string is allowed but uncommon for section links; we don't
    // reject on its presence — many sites carry a `?source=nav` tracker.
    // The path itself must be section-shaped.
    if !is_section_path(url.path()) {
        return false;
    }

    // Same-host gate. If we don't know the page URL, fall through.
    if let Some(page) = page_url
        && let (Some(lh), Some(ph)) = (url.host_str(), page.host_str())
        && !same_host(lh, ph)
    {
        return false;
    }

    true
}

// ---------------------------------------------------------------------------
// Public surface — collectors and formatters
// ---------------------------------------------------------------------------

/// Collect a deduplicated (label, url) list of section links for the
/// page. Reuses the noise-filtered link list `body::process_body`
/// produces; applies the M8 section heuristic on top.
///
/// `page_url` is the canonical URL of the page (used for the same-host
/// gate). When `None`, the same-host gate is skipped.
pub fn collect_section_links(
    result: &ExtractionResult,
    page_url: Option<&str>,
) -> Vec<(String, String)> {
    let parsed_page = page_url.and_then(|u| Url::parse(u).ok());
    let processed = body::process_body(&result.content.markdown);
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen_hrefs: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (text, href) in processed.links {
        let label = links::clean_link_label(&text);
        if !is_section_link(&label, &href, parsed_page.as_ref()) {
            continue;
        }
        if !seen_hrefs.insert(href.clone()) {
            continue;
        }
        out.push((label, href));
    }
    out
}

/// `-f llm` / `-f text` form: metadata header (Status line suppressed)
/// followed by a `## Sections` block of `- [Label](url)` lines.
///
/// When the heuristic returns 0 sections, emits the header plus
/// `## Sections\n_(no sections detected)_` so the caller can
/// distinguish empty-result from a crash / parse failure.
pub fn to_llm_sections(result: &ExtractionResult, url: Option<&str>) -> String {
    let sections = collect_section_links(result, url);
    let mut out = String::new();
    // M7 suppression: section listing is conceptually navigation, not
    // protocol-level outcome.
    build_metadata_header_with_opts(&mut out, result, url, false);
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("## Sections\n");
    if sections.is_empty() {
        out.push_str("_(no sections detected)_");
    } else {
        for (label, href) in &sections {
            out.push_str(&format!("- [{label}]({href})\n"));
        }
    }
    out.trim_end().to_string()
}

/// `-f json` form: `{"sections": [{"label": ..., "url": ...}, ...]}`.
/// When 0 sections detected, `sections` is an empty array.
pub fn to_json_sections(result: &ExtractionResult, url: Option<&str>) -> String {
    let sections = collect_section_links(result, url);
    let arr: Vec<serde_json::Value> = sections
        .into_iter()
        .map(|(label, href)| {
            serde_json::json!({
                "label": label,
                "url": href,
            })
        })
        .collect();
    serde_json::to_string_pretty(&serde_json::json!({"sections": arr}))
        .unwrap_or_else(|_| "{\"sections\": []}".to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Content, ExtractionResult, Metadata};

    fn make_result(markdown: &str) -> ExtractionResult {
        ExtractionResult {
            metadata: Metadata {
                title: Some("Test Page".to_string()),
                description: None,
                author: None,
                published_date: None,
                language: None,
                url: Some("https://example.com/".to_string()),
                site_name: None,
                image: None,
                favicon: None,
                word_count: 0,
                http_status: None,
            },
            content: Content {
                markdown: markdown.to_string(),
                plain_text: String::new(),
                links: Vec::new(),
                images: Vec::new(),
                code_blocks: Vec::new(),
                raw_html: None,
            },
            domain_data: None,
            structured_data: Vec::new(),
        }
    }

    // -- heuristic primitives --

    #[test]
    fn test_section_filter_detects_url_pattern_sections() {
        // 5 section-shaped URLs (BBC-style) + 15 article URLs.
        let mut md = String::from("# Page\n\n");
        // 5 section nav links.
        md.push_str("- [Home](https://www.bbc.com/)\n");
        md.push_str("- [Sport](https://www.bbc.com/sport)\n");
        md.push_str("- [Health](https://www.bbc.com/health)\n");
        md.push_str("- [Weather](https://www.bbc.com/weather)\n");
        md.push_str("- [Newsletters](https://www.bbc.com/newsletters)\n");
        // 15 article URLs (3-segment, article-ID shape).
        for i in 0..15 {
            md.push_str(&format!(
                "- [Some long headline number {i}](https://www.bbc.com/news/articles/crmp121z3z{i:01x}o)\n"
            ));
        }
        let r = make_result(&md);
        let out = collect_section_links(&r, Some("https://www.bbc.com/news/world"));
        assert_eq!(out.len(), 5, "expected 5 sections, got {}: {out:?}", out.len());
        let labels: Vec<&str> = out.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"Sport"), "missing Sport: {labels:?}");
        assert!(labels.contains(&"Health"), "missing Health: {labels:?}");
    }

    #[test]
    fn test_section_filter_repetition_signal() {
        // After-dedup behavior: even when a section URL is referenced multiple
        // times in the source markdown, the output contains exactly one entry
        // per unique href. (Phase A: repetition is collapsed upstream by
        // process_body; we verify the final list is deduped.)
        let md = "# Page\n\n\
                  - [Sport](https://www.bbc.com/sport)\n\
                  - [Sport (top)](https://www.bbc.com/sport)\n\
                  - [Sport (footer)](https://www.bbc.com/sport)\n\
                  - [Unique](https://www.bbc.com/health)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://www.bbc.com/"));
        assert_eq!(out.len(), 2, "expected 2 unique sections, got {out:?}");
    }

    #[test]
    fn test_section_filter_combined_signals() {
        // Mix sections, article slugs, denylisted paths, cross-host, anchor links.
        let md = "# Decrypt-style\n\n\
                  - [Business](https://decrypt.co/news/business)\n\
                  - [Markets](https://decrypt.co/news/markets)\n\
                  - [Editors' Picks](https://decrypt.co/news/editors-picks)\n\
                  - [Bitcoin](https://decrypt.co/price/bitcoin)\n\
                  - [Ethereum](https://decrypt.co/price/ethereum)\n\
                  - [Search](https://decrypt.co/search)\n\
                  - [Login](https://decrypt.co/login)\n\
                  - [Cross-host](https://promo.elsewhere.com/sport)\n\
                  - [Skip to content](https://decrypt.co/news/world#main)\n\
                  - [Long article slug here that exceeds limit](https://decrypt.co/news/business/2024/05/some-article)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://decrypt.co/"));
        // Only Business, Markets, Editors' Picks should pass.
        assert_eq!(out.len(), 3, "expected 3 sections, got {out:?}");
        let hrefs: Vec<&str> = out.iter().map(|(_, h)| h.as_str()).collect();
        assert!(hrefs.contains(&"https://decrypt.co/news/business"));
        assert!(hrefs.contains(&"https://decrypt.co/news/markets"));
        assert!(hrefs.contains(&"https://decrypt.co/news/editors-picks"));
        // Explicitly NOT present.
        for bad in [
            "https://decrypt.co/price/bitcoin",
            "https://decrypt.co/search",
            "https://decrypt.co/login",
            "https://promo.elsewhere.com/sport",
        ] {
            assert!(!hrefs.contains(&bad), "{bad} should have been filtered out: {hrefs:?}");
        }
    }

    #[test]
    fn test_article_slug_excluded() {
        // BBC article-id style segment.
        let md = "- [Headline text](https://www.bbc.com/news/articles/crmp121z3z8o)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://www.bbc.com/news/world"));
        assert!(out.is_empty(), "article-ID link should have been dropped: {out:?}");
    }

    #[test]
    fn test_cross_host_link_dropped() {
        let md = "- [Sport](https://promo.bbc.co.uk/sport)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://www.bbc.com/news/world"));
        assert!(out.is_empty(), "cross-host link should have been dropped: {out:?}");
    }

    #[test]
    fn test_subdomain_link_kept() {
        // news.bbc.com is a subdomain of bbc.com — same_host should accept it.
        let md = "- [Sport](https://news.bbc.com/sport)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://www.bbc.com/news/world"));
        assert_eq!(out.len(), 1, "subdomain link should have passed: {out:?}");
    }

    #[test]
    fn test_anchor_fragment_dropped() {
        let md = "- [Skip to content](https://www.bbc.com/news/world#bbc-main)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://www.bbc.com/news/world"));
        assert!(out.is_empty(), "fragment link should have been dropped: {out:?}");
    }

    #[test]
    fn test_no_links_returns_empty() {
        let md = "# Just a heading\n\nNo links at all here.";
        let r = make_result(md);
        let out = collect_section_links(&r, Some("https://example.com/"));
        assert!(out.is_empty(), "expected empty: {out:?}");
    }

    // -- formatter tests --

    #[test]
    fn test_sections_mode_formats_llm_output() {
        let md = "- [Sport](https://www.bbc.com/sport)\n- [Health](https://www.bbc.com/health)\n";
        let mut r = make_result(md);
        r.metadata.url = Some("https://www.bbc.com/news/world".to_string());
        let out = to_llm_sections(&r, Some("https://www.bbc.com/news/world"));
        assert!(out.contains("## Sections"), "missing Sections header: {out}");
        assert!(out.contains("- [Sport](https://www.bbc.com/sport)"), "missing Sport: {out}");
        assert!(out.contains("- [Health](https://www.bbc.com/health)"), "missing Health: {out}");
        // Metadata header URL present, Status line absent (Sections mode passes include_status=false).
        assert!(out.contains("> URL:"));
        assert!(!out.contains("> Status:"));
    }

    #[test]
    fn test_sections_mode_formats_json_output() {
        let md = "- [Sport](https://www.bbc.com/sport)\n- [Health](https://www.bbc.com/health)\n";
        let r = make_result(md);
        let s = to_json_sections(&r, Some("https://www.bbc.com/news/world"));
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        let arr = v["sections"].as_array().expect("sections array present");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["label"].as_str().unwrap(), "Sport");
        assert_eq!(arr[0]["url"].as_str().unwrap(), "https://www.bbc.com/sport");
    }

    #[test]
    fn test_sections_mode_fallback_on_no_nav() {
        // Phase A's chosen fallback: empty list with `_(no sections detected)_`
        // marker in -f llm form, and `{"sections": []}` in -f json form.
        let md = "# Page\n\nNo links here.";
        let r = make_result(md);
        let llm = to_llm_sections(&r, Some("https://example.com/"));
        assert!(llm.contains("## Sections"), "missing header: {llm}");
        assert!(llm.contains("(no sections detected)"), "missing fallback marker: {llm}");
        let json = to_json_sections(&r, Some("https://example.com/"));
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(v["sections"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn test_status_header_suppressed_in_sections_mode() {
        // Parallel to summary/toc behavior — Sections mode passes
        // include_status=false to build_metadata_header_with_opts.
        let mut r = make_result("- [Sport](https://www.bbc.com/sport)\n");
        r.metadata.http_status = Some(404);
        let out = to_llm_sections(&r, Some("https://www.bbc.com/news/world"));
        assert!(
            !out.contains("> Status:"),
            "Status line leaked into sections mode output:\n{out}"
        );
    }

    #[test]
    fn test_no_page_url_skips_same_host_gate() {
        // When page_url is None we don't know the host; the link still
        // passes provided its URL shape is section-like.
        let md = "- [Sport](https://www.bbc.com/sport)\n";
        let r = make_result(md);
        let out = collect_section_links(&r, None);
        assert_eq!(out.len(), 1, "expected 1 section, got {out:?}");
    }
}
