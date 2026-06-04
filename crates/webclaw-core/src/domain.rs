/// Domain detection via URL patterns and DOM heuristics.
/// Knowing the domain type lets downstream consumers apply
/// domain-specific prompts or post-processing.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum DomainType {
    Article,
    Documentation,
    GitHub,
    Forum,
    ECommerce,
    Social,
    Generic,
}

/// Detect domain type from URL patterns first, then fall back to DOM heuristics.
pub fn detect(url: Option<&str>, html: &str) -> DomainType {
    if let Some(url) = url
        && let Some(dt) = detect_from_url(url)
    {
        return dt;
    }
    detect_from_dom(html)
}

fn detect_from_url(url: &str) -> Option<DomainType> {
    let lower = url.to_lowercase();

    // GitHub
    if lower.contains("github.com") || lower.contains("gitlab.com") {
        return Some(DomainType::GitHub);
    }

    // Documentation sites
    let doc_patterns = [
        "docs.",
        "readthedocs",
        "gitbook",
        "docusaurus",
        "/docs/",
        "/documentation/",
        "devdocs.io",
        "doc.rust-lang.org",
        "developer.mozilla.org",
        "developer.apple.com/documentation",
    ];
    if doc_patterns.iter().any(|p| lower.contains(p)) {
        return Some(DomainType::Documentation);
    }

    // Forums
    let forum_patterns = [
        "reddit.com",
        "news.ycombinator.com",
        "stackoverflow.com",
        "stackexchange.com",
        "discourse",
        "forum",
        "community.",
    ];
    if forum_patterns.iter().any(|p| lower.contains(p)) {
        return Some(DomainType::Forum);
    }

    // Social
    let social_patterns = [
        "twitter.com",
        "x.com",
        "linkedin.com",
        "facebook.com",
        "instagram.com",
        "mastodon",
        "bsky.app",
    ];
    if social_patterns.iter().any(|p| lower.contains(p)) {
        return Some(DomainType::Social);
    }

    // E-commerce
    let ecommerce_patterns = [
        "amazon.",
        "ebay.",
        "shopify.",
        "etsy.com",
        "/product/",
        "/shop/",
        "/cart",
    ];
    if ecommerce_patterns.iter().any(|p| lower.contains(p)) {
        return Some(DomainType::ECommerce);
    }

    None
}

/// Fallback: check HTML for structural hints when URL isn't enough.
fn detect_from_dom(html: &str) -> DomainType {
    let lower = html.to_lowercase();

    // Article signals: <article> tag, schema.org Article type
    if lower.contains("<article") || lower.contains("schema.org/article") {
        return DomainType::Article;
    }

    // Documentation signals
    if lower.contains("docsearch") || lower.contains("sidebar-nav") || lower.contains("doc-content")
    {
        return DomainType::Documentation;
    }

    DomainType::Generic
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_urls() {
        assert_eq!(
            detect(Some("https://github.com/tokio-rs/tokio"), ""),
            DomainType::GitHub
        );
        assert_eq!(
            detect(Some("https://gitlab.com/foo/bar"), ""),
            DomainType::GitHub
        );
    }

    #[test]
    fn documentation_urls() {
        assert_eq!(
            detect(Some("https://docs.rs/serde/latest"), ""),
            DomainType::Documentation
        );
        assert_eq!(
            detect(Some("https://readthedocs.org/projects/foo"), ""),
            DomainType::Documentation
        );
    }

    #[test]
    fn forum_urls() {
        assert_eq!(
            detect(Some("https://www.reddit.com/r/rust"), ""),
            DomainType::Forum
        );
        assert_eq!(
            detect(Some("https://stackoverflow.com/questions/123"), ""),
            DomainType::Forum
        );
    }

    #[test]
    fn social_urls() {
        assert_eq!(
            detect(Some("https://x.com/elonmusk"), ""),
            DomainType::Social
        );
        assert_eq!(
            detect(Some("https://linkedin.com/in/someone"), ""),
            DomainType::Social
        );
    }

    #[test]
    fn ecommerce_urls() {
        assert_eq!(
            detect(Some("https://amazon.com/dp/B001"), ""),
            DomainType::ECommerce
        );
    }

    #[test]
    fn dom_fallback_article() {
        let html = r#"<html><body><article><p>Hello world</p></article></body></html>"#;
        assert_eq!(detect(None, html), DomainType::Article);
    }

    #[test]
    fn dom_fallback_generic() {
        let html = r#"<html><body><div>Just some div</div></body></html>"#;
        assert_eq!(detect(None, html), DomainType::Generic);
    }
}
