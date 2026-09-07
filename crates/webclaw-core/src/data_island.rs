/// Extract content from JSON data islands embedded in `<script>` tags.
///
/// Many modern SPAs (React, Next.js, Nuxt) ship server-rendered page data
/// as JSON inside script tags rather than in visible DOM elements. This module
/// walks those JSON blobs and recovers text content as a fallback when normal
/// DOM extraction yields sparse results.
use std::sync::LazyLock as Lazy;

use scraper::{Html, Selector};
use tracing::debug;

static SCRIPT_JSON_SELECTOR: Lazy<Selector> =
    Lazy::new(|| Selector::parse("script[type='application/json']").unwrap());

/// Below this word count, try data islands for supplemental content.
/// Set high enough to cover marketing homepages with partial SSR (e.g., Notion
/// SSR-renders ~300 words but has ~800 words in __NEXT_DATA__).
const SPARSE_THRESHOLD: usize = 500;

/// Cap total extracted chunks to bound memory and CPU on adversarial inputs.
const MAX_CHUNKS: usize = 1000;

/// A chunk of text extracted from a JSON data island, with optional heading.
#[derive(Debug)]
struct TextChunk {
    heading: Option<String>,
    body: String,
}

/// Try to extract content from JSON data islands when DOM extraction is sparse.
/// Deduplicates against existing markdown so we only add genuinely new content.
/// Handles: application/json script tags, SvelteKit kit.start() data, and
/// other inline JS data patterns.
pub fn try_extract(doc: &Html, dom_word_count: usize, existing_markdown: &str) -> Option<String> {
    if dom_word_count >= SPARSE_THRESHOLD {
        return None;
    }

    let mut all_chunks: Vec<TextChunk> = Vec::new();
    let existing_lower = existing_markdown.to_lowercase();

    // 1. Standard JSON data islands (application/json script tags)
    for script in doc.select(&SCRIPT_JSON_SELECTOR) {
        if is_configuration_key(script.value().attr("id").unwrap_or("")) {
            continue;
        }
        if all_chunks.len() >= MAX_CHUNKS {
            break;
        }

        let json_text = script.text().collect::<String>();
        if json_text.len() < 50 {
            continue;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(&json_text) else {
            continue;
        };

        let mut chunks = Vec::new();
        walk_json(&value, &mut chunks, 0);

        if !chunks.is_empty() {
            debug!(
                script_id = script.value().attr("id").unwrap_or(""),
                data_target = script.value().attr("data-target").unwrap_or(""),
                chunks = chunks.len(),
                "extracted text from data island"
            );
            all_chunks.extend(chunks);
        }
    }

    // Note: SvelteKit data islands are handled in structured_data.rs
    // (extracted as structured JSON, not markdown chunks)

    if all_chunks.is_empty() {
        return None;
    }

    // Enforce limit after collecting from all scripts
    all_chunks.truncate(MAX_CHUNKS);

    // Dedup: remove chunks whose text already appears in DOM markdown
    let mut seen = std::collections::HashSet::new();
    all_chunks.retain(|c| {
        // Must have heading or body
        let key = if !c.body.is_empty() {
            c.body.clone()
        } else if let Some(ref h) = c.heading {
            h.clone()
        } else {
            return false;
        };
        if !seen.insert(key.clone()) {
            return false;
        }
        // Skip if the text already exists in the DOM-extracted content
        !existing_lower.contains(&key.to_lowercase())
    });

    if all_chunks.is_empty() {
        return None;
    }

    let mut md = String::new();
    for chunk in &all_chunks {
        if let Some(ref h) = chunk.heading {
            md.push_str(&format!("\n## {h}\n\n"));
        }
        md.push_str(&chunk.body);
        md.push_str("\n\n");
    }

    let md = md.trim().to_string();
    if md.is_empty() {
        None
    } else {
        debug!(chars = md.len(), "data island content recovered");
        Some(md)
    }
}

/// Application dictionaries and instrumentation are not page content.
pub(crate) fn is_configuration_key(key: &str) -> bool {
    let key = key.trim_matches('_').to_ascii_lowercase();
    // Exact application dictionaries, not arbitrary product fields containing
    // words such as "configuration" or "metrics".
    matches!(
        key.as_str(),
        "i18n"
            | "translations"
            | "dictionary"
            | "telemetry"
            | "initiali18nstore"
            | "delayinitiali18nstore"
            | "privacy_config"
            | "npm_package_config"
            | "region_config"
            | "metricscontext"
    )
}

/// Recursively walk a JSON value and extract text content.
fn walk_json(value: &serde_json::Value, chunks: &mut Vec<TextChunk>, depth: usize) {
    if depth > 15 {
        return;
    }

    match value {
        serde_json::Value::Object(map) => {
            // Contentful rich text node: { "nodeType": "...", "content": [...] }
            if let Some(node_type) = map.get("nodeType").and_then(|v| v.as_str())
                && let Some(text) = extract_contentful_node(map, node_type)
            {
                chunks.push(text);
                return;
            }

            // CMS-style entry with heading + subheading/description
            if is_cms_entry(map)
                && let Some(chunk) = extract_cms_entry(map)
            {
                chunks.push(chunk);
                return;
            }

            // Quote/testimonial pattern
            if let Some(chunk) = extract_quote(map) {
                chunks.push(chunk);
                return;
            }

            // Extract orphaned content strings from known field names
            // before recursing (they won't be caught by CMS/quote patterns)
            extract_orphan_texts(map, chunks);

            // Recurse into all values, skipping image/media/asset fields
            for (key, v) in map {
                if is_media_key(key) || is_configuration_key(key) {
                    continue;
                }
                walk_json(v, chunks, depth + 1);
            }
        }
        serde_json::Value::Array(arr) => {
            // Check for stat-style string arrays (e.g., ["100M+ users", "#1 rated"])
            let content_strings: Vec<&str> = arr
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| s.len() > 10 && s.contains(' '))
                .collect();
            if content_strings.len() >= 2 {
                let body = content_strings.join(" | ");
                chunks.push(TextChunk {
                    heading: None,
                    body,
                });
                return;
            }

            for v in arr {
                walk_json(v, chunks, depth + 1);
            }
        }
        _ => {}
    }
}

/// Extract text from a Contentful rich text node.
/// Handles: document, paragraph, heading-1..6, blockquote, etc.
fn extract_contentful_node(
    map: &serde_json::Map<String, serde_json::Value>,
    node_type: &str,
) -> Option<TextChunk> {
    match node_type {
        "document" => {
            // Top-level document — collect children
            let content = map.get("content")?.as_array()?;
            let mut parts = Vec::new();
            for child in content {
                if let Some(chunk) = child
                    .as_object()
                    .and_then(|m| m.get("nodeType").and_then(|v| v.as_str()))
                    .and_then(|nt| extract_contentful_node(child.as_object().unwrap(), nt))
                {
                    if let Some(h) = &chunk.heading {
                        parts.push(format!("## {h}"));
                    }
                    if !chunk.body.is_empty() {
                        parts.push(chunk.body);
                    }
                }
            }
            if parts.is_empty() {
                return None;
            }
            Some(TextChunk {
                heading: None,
                body: parts.join("\n\n"),
            })
        }
        "paragraph" | "text" => {
            let text = collect_text_content(map);
            if is_content_text(&text) {
                Some(TextChunk {
                    heading: None,
                    body: text,
                })
            } else {
                None
            }
        }
        nt if nt.starts_with("heading-") => {
            let text = collect_text_content(map);
            if text.is_empty() {
                None
            } else {
                Some(TextChunk {
                    heading: Some(text),
                    body: String::new(),
                })
            }
        }
        "blockquote" => {
            let text = collect_text_content(map);
            if is_content_text(&text) {
                Some(TextChunk {
                    heading: None,
                    body: format!("> {text}"),
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Recursively collect plain text from a Contentful rich text node tree.
fn collect_text_content(map: &serde_json::Map<String, serde_json::Value>) -> String {
    let mut text = String::new();

    if let Some(v) = map.get("value").and_then(|v| v.as_str()) {
        text.push_str(v);
    }

    if let Some(content) = map.get("content").and_then(|v| v.as_array()) {
        for child in content {
            if let Some(child_map) = child.as_object() {
                let child_text = collect_text_content(child_map);
                text.push_str(&child_text);
            }
        }
    }

    text.trim().to_string()
}

/// Check if a JSON object looks like a CMS entry with heading + description.
fn is_cms_entry(map: &serde_json::Map<String, serde_json::Value>) -> bool {
    let has_heading =
        map.contains_key("heading") || map.contains_key("title") || map.contains_key("headline");
    let has_body = map.contains_key("description")
        || map.contains_key("subheading")
        || map.contains_key("body")
        || map.contains_key("text");
    has_heading && has_body
}

/// Extract heading + body from a CMS-style entry.
fn extract_cms_entry(map: &serde_json::Map<String, serde_json::Value>) -> Option<TextChunk> {
    let heading = extract_text_field(map, "heading")
        .or_else(|| extract_text_field(map, "title"))
        .or_else(|| extract_text_field(map, "headline"))
        .filter(|h| !is_cms_internal_title(h) && h.len() > 5)?;

    let body = extract_text_field(map, "description")
        .or_else(|| extract_text_field(map, "subheading"))
        .or_else(|| extract_text_field(map, "body"))
        .or_else(|| extract_text_field(map, "text"))
        .unwrap_or_default();

    if !is_content_text(&heading) && !is_content_text(&body) {
        return None;
    }

    Some(TextChunk {
        heading: Some(heading),
        body,
    })
}

/// Extract a quote/testimonial from a JSON object.
fn extract_quote(map: &serde_json::Map<String, serde_json::Value>) -> Option<TextChunk> {
    let quote =
        extract_text_field(map, "quote").or_else(|| extract_text_field(map, "quoteText"))?;
    if !is_content_text(&quote) {
        return None;
    }

    let attribution = extract_text_field(map, "position")
        .or_else(|| extract_text_field(map, "author"))
        .or_else(|| extract_text_field(map, "name"))
        .unwrap_or_default();

    let body = if attribution.is_empty() {
        format!("> {quote}")
    } else {
        format!("> {quote}\n> — {attribution}")
    };

    Some(TextChunk {
        heading: None,
        body,
    })
}

/// Extract standalone content strings from known field names that weren't
/// caught by the CMS entry or quote patterns. These are body/description/
/// subheading/eyebrow fields on objects that lack a paired heading, or
/// headline fields on objects that lack a body.
fn extract_orphan_texts(
    map: &serde_json::Map<String, serde_json::Value>,
    chunks: &mut Vec<TextChunk>,
) {
    const BODY_KEYS: &[&str] = &["body", "description", "subheading", "eyebrow", "children"];
    const HEADING_KEYS: &[&str] = &["heading", "title", "headline"];

    // Don't extract if this object was already handled as a CMS entry
    if is_cms_entry(map) {
        return;
    }

    // Try extracting a standalone heading (without body)
    for key in HEADING_KEYS {
        if let Some(text) = extract_text_field(map, key)
            && is_content_text(&text)
        {
            chunks.push(TextChunk {
                heading: Some(text),
                body: String::new(),
            });
            return;
        }
    }

    // Try extracting a standalone body field
    for key in BODY_KEYS {
        if let Some(text) = extract_text_field(map, key)
            && is_content_text(&text)
        {
            chunks.push(TextChunk {
                heading: None,
                body: text,
            });
            return;
        }
    }
}

/// Extract a text value from a JSON field, handling both plain strings and
/// Contentful rich text objects.
fn extract_text_field(
    map: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<String> {
    let value = map.get(key)?;

    // Plain string
    if let Some(s) = value.as_str() {
        let s = s.trim().to_string();
        return if s.is_empty() { None } else { Some(s) };
    }

    // Contentful rich text object: { "content": [{ "content": [{ "value": "..." }] }] }
    if let Some(obj) = value.as_object() {
        let text = collect_text_content(obj);
        return if text.is_empty() { None } else { Some(text) };
    }

    None
}

/// JSON keys that hold image/media/asset data — skip recursing into these
/// to avoid extracting CMS alt text as content.
fn is_media_key(key: &str) -> bool {
    let k = key.to_lowercase();
    k == "alt"
        || k.contains("image")
        || k.contains("poster")
        || k.contains("video")
        || k.contains("thumbnail")
        || k.contains("icon")
        || k.contains("logo")
        || k == "src"
        || k == "url"
        || k == "href"
}

/// CMS internal titles like "/home Customer Stories: Logo" or
/// "Copilot agent mode hero poster desktop" are editorial labels, not user-facing text.
fn is_cms_internal_title(s: &str) -> bool {
    // Contentful path-style titles
    if s.starts_with("/home ") || s.starts_with("/page ") {
        return true;
    }
    // Titles that look like asset/component labels (short words, no sentence structure)
    let words: Vec<&str> = s.split_whitespace().collect();
    if words.len() >= 3 {
        let has_label_keyword = words
            .iter()
            .any(|w| ["poster", "logo", "image", "icon", "asset", "thumbnail"].contains(w));
        if has_label_keyword {
            return true;
        }
    }
    false
}

/// Heuristic: is this string actual content (not an ID, URL, class name, etc.)?
fn is_content_text(s: &str) -> bool {
    let s = s.trim();
    if s.len() < 15 {
        return false;
    }
    // Skip URLs, IDs, technical strings
    if s.starts_with("http") || s.starts_with('/') || s.starts_with('{') || s.starts_with('[') {
        return false;
    }
    // Must contain spaces (prose), not just a single technical token
    if !s.contains(' ') {
        return false;
    }
    // Skip strings that are mostly hex/base64 (hashes, IDs)
    let alnum_ratio = s.chars().filter(|c| c.is_alphanumeric()).count() as f64 / s.len() as f64;
    if alnum_ratio < 0.6 {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_contentful_rich_text() {
        let html = r#"<html><body>
        <script type="application/json" data-target="react-app.embeddedData">
        {"payload":{"contentfulRawJsonResponse":{"includes":{"Entry":[
            {"fields":{
                "heading":"Ship faster with secure CI/CD",
                "subheading":{"content":[{"content":[{"value":"Automate builds, tests, and deployments."}]}]}
            }},
            {"fields":{
                "heading":"Built-in application security",
                "description":{"content":[{"content":[{"value":"Use AI to find and fix vulnerabilities so your team can ship more secure software faster."}]}]}
            }}
        ]}}}}
        </script>
        </body></html>"#;

        let doc = Html::parse_document(html);
        let result = try_extract(&doc, 0, "").unwrap();

        assert!(result.contains("Ship faster with secure CI/CD"));
        assert!(result.contains("Automate builds, tests, and deployments"));
        assert!(result.contains("Built-in application security"));
        assert!(result.contains("find and fix vulnerabilities"));
    }

    #[test]
    fn skips_when_dom_has_enough_content() {
        let html = r#"<html><body>
        <script type="application/json">{"heading":"Foo","description":"Some long description here."}</script>
        </body></html>"#;

        let doc = Html::parse_document(html);
        assert!(try_extract(&doc, 500, "").is_none());
    }

    #[test]
    fn skips_non_content_strings() {
        assert!(!is_content_text("abc123"));
        assert!(!is_content_text("https://example.com/foo/bar"));
        assert!(!is_content_text("/home Customer Stories: Logo"));
        assert!(!is_content_text("a1b2c3d4e5f6a1b2c3d4e5f6"));
        assert!(is_content_text(
            "Automate builds, tests, and deployments with CI/CD."
        ));
    }

    #[test]
    fn extracts_quotes() {
        let html = r#"<html><body>
        <script type="application/json">
        {"fields":{"quote":{"content":[{"content":[{"value":"GitHub frees us from maintaining our own infrastructure."}]}]},"position":"CTO at Example Corp"}}
        </script>
        </body></html>"#;

        let doc = Html::parse_document(html);
        let result = try_extract(&doc, 0, "").unwrap();
        assert!(result.contains("> GitHub frees us from maintaining our own infrastructure."));
        assert!(result.contains("CTO at Example Corp"));
    }

    #[test]
    fn skips_content_already_in_dom() {
        let html = r#"<html><body>
        <script type="application/json">
        {"fields":{"heading":"Already in DOM heading","description":"This text already appears in the DOM markdown output."}}
        </script>
        </body></html>"#;

        let doc = Html::parse_document(html);
        let existing =
            "# Already in DOM heading\n\nThis text already appears in the DOM markdown output.";
        assert!(try_extract(&doc, 10, existing).is_none());
    }

    #[test]
    fn deduplicates_chunks() {
        let html = r#"<html><body>
        <script type="application/json">
        {"a":{"heading":"Same heading here","description":"Same body content across multiple entries."},
         "b":{"heading":"Same heading here","description":"Same body content across multiple entries."}}
        </script>
        </body></html>"#;

        let doc = Html::parse_document(html);
        let result = try_extract(&doc, 0, "").unwrap();
        // Should appear only once
        assert_eq!(
            result
                .matches("Same body content across multiple entries")
                .count(),
            1
        );
    }
}
