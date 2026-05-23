/// Metadata extraction from HTML <head>.
/// Prioritizes Open Graph and Twitter Card tags, falls back to standard meta tags.
use scraper::{Html, Selector};

use crate::types::Metadata;

/// Selectors are cheap to compile but we call them often — cache with once_cell.
macro_rules! selector {
    ($s:expr) => {{
        use once_cell::sync::Lazy;
        static SEL: Lazy<Selector> = Lazy::new(|| Selector::parse($s).unwrap());
        &*SEL
    }};
}

pub fn extract(doc: &Html, url: Option<&str>) -> Metadata {
    let title = og_meta(doc, "og:title")
        .or_else(|| meta_name(doc, "twitter:title"))
        .or_else(|| title_tag(doc));

    let description = og_meta(doc, "og:description")
        .or_else(|| meta_name(doc, "twitter:description"))
        .or_else(|| meta_name(doc, "description"));

    let author = meta_name(doc, "author").or_else(|| og_meta(doc, "article:author"));

    let published_date = og_meta(doc, "article:published_time")
        .or_else(|| meta_name(doc, "date"))
        .or_else(|| meta_name(doc, "publication_date"));

    // Search the whole document for <html lang="..."> — root_element() IS the <html>
    // node in scraper, so selecting "html" from it finds nothing (no nested <html>).
    let language = doc
        .select(selector!("html"))
        .next()
        .and_then(|el| el.value().attr("lang"))
        .map(|s| s.to_string());

    let site_name = og_meta(doc, "og:site_name");
    let image = og_meta(doc, "og:image").or_else(|| meta_name(doc, "twitter:image"));

    let favicon = extract_favicon(doc);

    Metadata {
        title,
        description,
        author,
        published_date,
        language,
        url: url.map(String::from),
        site_name,
        image,
        favicon,
        word_count: 0, // filled later by the extractor
        http_status: None, // filled by webclaw-fetch when reachable; None for local-file / --stdin
    }
}

/// <meta property="og:..." content="...">
fn og_meta(doc: &Html, property: &str) -> Option<String> {
    // OG tags use property= not name=
    doc.select(selector!("meta[property]"))
        .find(|el| el.value().attr("property") == Some(property))
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// <meta name="..." content="...">
fn meta_name(doc: &Html, name: &str) -> Option<String> {
    doc.select(selector!("meta[name]"))
        .find(|el| {
            el.value()
                .attr("name")
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
        })
        .and_then(|el| el.value().attr("content"))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn title_tag(doc: &Html) -> Option<String> {
    doc.select(selector!("title"))
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())
}

fn extract_favicon(doc: &Html) -> Option<String> {
    // <link rel="icon" href="..."> or <link rel="shortcut icon" href="...">
    doc.select(selector!("link[rel]"))
        .find(|el| el.value().attr("rel").is_some_and(|r| r.contains("icon")))
        .and_then(|el| el.value().attr("href"))
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(html: &str) -> Html {
        Html::parse_document(html)
    }

    #[test]
    fn extracts_basic_metadata() {
        let html = r#"
        <html lang="en">
        <head>
            <title>Test Page</title>
            <meta name="description" content="A test page">
            <meta name="author" content="Alice">
            <meta property="og:title" content="OG Title">
            <meta property="og:image" content="https://img.example.com/og.png">
            <meta property="og:site_name" content="Example">
            <meta property="article:published_time" content="2025-01-15">
            <link rel="icon" href="/favicon.ico">
        </head>
        <body></body>
        </html>"#;

        let doc = parse(html);
        let meta = extract(&doc, Some("https://example.com"));

        // OG title wins over <title>
        assert_eq!(meta.title.as_deref(), Some("OG Title"));
        assert_eq!(meta.description.as_deref(), Some("A test page"));
        assert_eq!(meta.author.as_deref(), Some("Alice"));
        assert_eq!(meta.published_date.as_deref(), Some("2025-01-15"));
        assert_eq!(meta.language.as_deref(), Some("en"));
        assert_eq!(meta.site_name.as_deref(), Some("Example"));
        assert_eq!(
            meta.image.as_deref(),
            Some("https://img.example.com/og.png")
        );
        assert_eq!(meta.favicon.as_deref(), Some("/favicon.ico"));
        assert_eq!(meta.url.as_deref(), Some("https://example.com"));
    }

    #[test]
    fn falls_back_to_title_tag() {
        let html = r#"<html><head><title>Fallback Title</title></head><body></body></html>"#;
        let doc = parse(html);
        let meta = extract(&doc, None);
        assert_eq!(meta.title.as_deref(), Some("Fallback Title"));
    }

    #[test]
    fn handles_missing_metadata_gracefully() {
        let html = r#"<html><head></head><body></body></html>"#;
        let doc = parse(html);
        let meta = extract(&doc, None);
        assert!(meta.title.is_none());
        assert!(meta.description.is_none());
        assert!(meta.language.is_none());
    }
}
