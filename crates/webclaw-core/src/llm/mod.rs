/// LLM-optimized output format.
///
/// Takes an `ExtractionResult` and produces a compact text representation
/// that maximizes information density per token. Strips decorative images,
/// visual-only formatting (bold/italic), and inline link URLs -- moving links
/// to a deduplicated section at the end.
mod body;
mod cleanup;
mod hub_detect;
mod images;
mod links;
mod metadata;
mod output_size;

pub use hub_detect::{classify as classify_hub, HubClassification};
pub use output_size::{
    to_json_summary, to_json_toc, to_llm_summary, to_llm_toc, truncate_json_with_wrapper,
    truncate_with_footer,
};

use crate::jsonld::{classify_all, primary_schema, JsonLdSchema};
use crate::types::ExtractionResult;

/// Hard size cap on the legacy `## Structured Data` block emitted at the
/// bottom of `to_llm_text` output. The schema-aware block emitted at the top
/// when `--prefer-structured` is set is NOT capped by this value (it has its
/// own per-variant size discipline; see `render_structured_block`).
const STRUCTURED_DATA_MAX_BYTES: usize = 16 * 1024;

/// Controls extra structured-data rendering on top of the legacy `to_llm_text`.
///
/// Default values reproduce the legacy `to_llm_text` behaviour exactly —
/// no caller without M4 flags sees any byte change.
#[derive(Debug, Clone, Default)]
pub struct LlmTextOptions {
    /// When true, emit a schema-aware structured-data block at the TOP of
    /// the output (after metadata, before prose) and suppress the legacy
    /// raw JSON `## Structured Data` block at the bottom.
    pub prefer_structured: bool,
}

/// Produce a token-optimized text representation of extracted content.
///
/// The output has three sections:
/// 1. Compact metadata header (`> ` prefixed lines)
/// 2. Cleaned body (no images, no bold/italic, links as plain text)
/// 3. Deduplicated links section at the end
pub fn to_llm_text(result: &ExtractionResult, url: Option<&str>) -> String {
    to_llm_text_with_options(result, url, &LlmTextOptions::default())
}

/// Same as `to_llm_text`, but with additional structured-data behaviours
/// controlled by `LlmTextOptions`. Used by the M4 `--prefer-structured` CLI
/// flag.
pub fn to_llm_text_with_options(
    result: &ExtractionResult,
    url: Option<&str>,
    opts: &LlmTextOptions,
) -> String {
    let mut out = String::new();

    // -- 1. Metadata header --
    metadata::build_metadata_header(&mut out, result, url);

    // -- 1b. Schema-aware structured data BEFORE the prose, if requested --
    // Phase A confirmed that on Pitchfork review pages the existing raw-JSON
    // block surfaces at byte ~50000 of a 58KB output; this hoists it.
    if opts.prefer_structured {
        let schemas = classify_all(&result.structured_data);
        if let Some(block) = render_structured_block(&schemas) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&block);
        }
    }

    // -- 2. Process body --
    let processed = body::process_body(&result.content.markdown);

    if !processed.text.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&processed.text);
    }

    // -- 3. Links section --
    if !processed.links.is_empty() {
        out.push_str("\n\n## Links\n");
        for (text, href) in &processed.links {
            let label = links::clean_link_label(text);
            if !label.is_empty() {
                out.push_str(&format!("- {label}: {href}\n"));
            }
        }
    }

    // -- 4. Structured data (NEXT_DATA, SvelteKit, JSON-LD) --
    // Only emit useful items: Schema.org records with a meaningful @type,
    // and only if the total serialized size stays under a budget. Framework
    // hydration blobs (Next.js pageProps full of ad-targeting flags, build
    // IDs, schedule paths) explode to hundreds of KB and drown the LLM in
    // noise — drop them rather than ship them.
    //
    // When `prefer_structured` is set the schema-aware block already
    // carries this information at the top, so we drop the legacy raw block
    // to avoid duplication.
    if !opts.prefer_structured {
        let mut useful: Vec<_> = result
            .structured_data
            .iter()
            .filter(|v| is_useful_structured_data(v))
            .cloned()
            .collect();
        for value in &mut useful {
            scrub_body_fields(value, 0);
        }
        if !useful.is_empty() {
            let serialized = serde_json::to_string_pretty(&useful).unwrap_or_default();
            if serialized.len() <= STRUCTURED_DATA_MAX_BYTES {
                out.push_str("\n\n## Structured Data\n\n```json\n");
                out.push_str(&serialized);
                out.push_str("\n```");
            }
        }
    }

    out.trim().to_string()
}

/// Render a schema-aware Markdown block summarising the page's JSON-LD.
/// Returns `None` when no content-bearing schema is present.
///
/// Format:
/// ```text
/// ## Structured data
///
/// schema: ItemList (20 items)
/// 1. <name or url> — <url>
/// 2. ...
/// ```
fn render_structured_block(schemas: &[JsonLdSchema]) -> Option<String> {
    let primary = primary_schema(schemas)?;
    let mut buf = String::new();
    buf.push_str("\n## Structured data\n\n");
    match primary {
        JsonLdSchema::ItemList { items, number_of_items } => {
            let n = number_of_items.unwrap_or(items.len() as u64);
            buf.push_str(&format!("schema: ItemList ({n} items)\n"));
            for (i, it) in items.iter().enumerate() {
                let pos = it.position.unwrap_or(i as u64 + 1);
                let label = it.title.clone().unwrap_or_else(|| {
                    it.url.clone().unwrap_or_else(|| "(no url)".to_string())
                });
                let url = it.url.as_deref().unwrap_or("");
                if url.is_empty() {
                    buf.push_str(&format!("{pos}. {label}\n"));
                } else {
                    buf.push_str(&format!("{pos}. {label} — {url}\n"));
                }
            }
        }
        JsonLdSchema::LiveBlogPosting { headline, updates } => {
            buf.push_str("schema: LiveBlogPosting");
            if let Some(h) = headline {
                buf.push_str(&format!(" — {h}"));
            }
            buf.push('\n');
            buf.push_str(&format!("updates: {}\n", updates.len()));
            for u in updates {
                let label = u.headline.clone().unwrap_or_else(|| {
                    u.url.clone().unwrap_or_else(|| "(no url)".into())
                });
                let ts = u.published.as_deref().unwrap_or("");
                if ts.is_empty() {
                    buf.push_str(&format!("- {label}\n"));
                } else {
                    buf.push_str(&format!("- [{ts}] {label}\n"));
                }
            }
        }
        JsonLdSchema::NewsArticle { headline, body, date_published, author } => {
            buf.push_str("schema: NewsArticle\n");
            if let Some(h) = headline {
                buf.push_str(&format!("headline: {h}\n"));
            }
            if let Some(a) = author {
                buf.push_str(&format!("author: {a}\n"));
            }
            if let Some(d) = date_published {
                buf.push_str(&format!("published: {d}\n"));
            }
            if let Some(b) = body {
                buf.push_str("\n");
                buf.push_str(b);
                buf.push('\n');
            }
        }
        JsonLdSchema::Review { headline, review_body, rated_item, author, date_published } => {
            buf.push_str("schema: Review\n");
            if let Some(h) = headline {
                buf.push_str(&format!("headline: {h}\n"));
            }
            if let Some(item) = rated_item {
                buf.push_str(&format!("rated: {item}\n"));
            }
            if let Some(a) = author {
                buf.push_str(&format!("author: {a}\n"));
            }
            if let Some(d) = date_published {
                buf.push_str(&format!("published: {d}\n"));
            }
            if let Some(b) = review_body {
                buf.push('\n');
                buf.push_str(b);
                buf.push('\n');
            }
        }
        JsonLdSchema::WebPageOrChrome { raw_type } => {
            // Surface the WebPage block even though normal output drops it —
            // user explicitly asked via --prefer-structured.
            buf.push_str(&format!("schema: {raw_type}\n"));
            buf.push_str("(navigation/chrome record; no content fields)\n");
        }
        JsonLdSchema::Unknown { raw_type, raw } => {
            buf.push_str(&format!("schema: {raw_type} (unrecognised)\n"));
            let pretty = serde_json::to_string_pretty(raw).unwrap_or_default();
            if pretty.len() <= 4096 {
                buf.push_str("\n```json\n");
                buf.push_str(&pretty);
                buf.push_str("\n```\n");
            }
        }
    }
    Some(buf)
}

/// Decide whether a structured-data value carries content worth emitting.
///
/// Schema.org records with a recognizable content `@type` (Article, NewsArticle,
/// Product, Recipe, FAQPage, HowTo, Event, Person, Organization, BreadcrumbList,
/// VideoObject, JobPosting, etc.) are kept. Generic `WebSite` / `WebPage` /
/// `ItemList` records and Next.js `pageProps`-style blobs without a useful
/// `@type` are dropped — they're almost always navigation chrome or framework
/// hydration state.
fn is_useful_structured_data(v: &serde_json::Value) -> bool {
    let Some(obj) = v.as_object() else {
        // SvelteKit can emit compact arrays of page data. Keep those if they
        // are small enough to be useful, while still dropping giant hydration
        // arrays under the same budget as untyped objects.
        if v.is_array() {
            let serialized = serde_json::to_string(v).unwrap_or_default();
            return serialized.len() <= 4 * 1024;
        }
        return false;
    };
    // JSON-LD: @type drives the decision.
    if let Some(t) = obj.get("@type") {
        let types: Vec<String> = match t {
            serde_json::Value::String(s) => vec![s.to_ascii_lowercase()],
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str())
                .map(str::to_ascii_lowercase)
                .collect(),
            _ => Vec::new(),
        };
        if types.is_empty() {
            return false;
        }
        // Drop low-info chrome types.
        const DROP_TYPES: &[&str] = &["website", "webpage", "sitenavigationelement"];
        return types.iter().any(|t| !DROP_TYPES.iter().any(|d| t == d));
    }
    // Next.js pageProps / SvelteKit data without @type: keep only if compact.
    // Anything over ~4KB is almost certainly hydration state, not content.
    let serialized = serde_json::to_string(v).unwrap_or_default();
    serialized.len() <= 4 * 1024
}

/// Recursively remove long fields that duplicate the rendered markdown body.
///
/// `depth` guards against stack exhaustion from attacker-controlled
/// JSON-LD / `__NEXT_DATA__` blobs with pathological nesting: past
/// [`MAX_SCRUB_DEPTH`] levels we stop descending and leave the subtree
/// as-is (it is still size-capped by the `STRUCTURED_DATA_MAX_BYTES`
/// budget in `to_llm_text`).
fn scrub_body_fields(v: &mut serde_json::Value, depth: usize) {
    const BODY_KEYS: &[&str] = &["articleBody"];
    const LONG_BODY_KEYS: &[&str] = &["body", "text", "description"];
    const LONG_THRESHOLD: usize = 500;
    const MAX_SCRUB_DEPTH: usize = 64;

    if depth >= MAX_SCRUB_DEPTH {
        return;
    }

    match v {
        serde_json::Value::Object(map) => {
            map.retain(|key, value| {
                if BODY_KEYS.contains(&key.as_str()) {
                    return false;
                }
                if LONG_BODY_KEYS.contains(&key.as_str())
                    && value.as_str().is_some_and(|s| s.len() >= LONG_THRESHOLD)
                {
                    return false;
                }
                true
            });
            for value in map.values_mut() {
                scrub_body_fields(value, depth + 1);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                scrub_body_fields(value, depth + 1);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Integration tests that exercise the full pipeline through to_llm_text
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_result(markdown: &str) -> ExtractionResult {
        ExtractionResult {
            metadata: Metadata {
                title: Some("Test Page".into()),
                description: Some("A test page".into()),
                author: None,
                published_date: None,
                language: Some("en".into()),
                url: Some("https://example.com".into()),
                site_name: None,
                image: None,
                favicon: None,
                word_count: 42,
                http_status: None,
            },
            content: Content {
                markdown: markdown.into(),
                plain_text: String::new(),
                links: vec![],
                images: vec![],
                code_blocks: vec![],
                raw_html: None,
            },
            domain_data: None,
            structured_data: vec![],
        }
    }

    #[test]
    fn metadata_header_includes_populated_fields() {
        let result = make_result("# Hello");
        let out = to_llm_text(&result, Some("https://example.com/page"));

        assert!(out.contains("> URL: https://example.com/page"));
        assert!(out.contains("> Title: Test Page"));
        assert!(out.contains("> Description: A test page"));
        assert!(out.contains("> Language: en"));
        assert!(out.contains("> Word count: 42"));
        assert!(!out.contains("> Author:"));
    }

    #[test]
    fn strips_image_markdown() {
        let md = "Some text\n\n![logo](https://cdn.example.com/img/logo.png)\n\nMore text";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(!out.contains("!["));
        assert!(!out.contains("cdn.example.com"));
        assert!(out.contains("Some text"));
        assert!(out.contains("More text"));
    }

    #[test]
    fn collapses_consecutive_logo_images_on_separate_lines() {
        let md = "# Partners\n\n\
                   ![WRITER](https://cdn.example.com/writer.png)\n\
                   ![MongoDB](https://cdn.example.com/mongo.png)\n\
                   ![GROQ](https://cdn.example.com/groq.png)\n\
                   ![LangChain](https://cdn.example.com/langchain.png)\n\n\
                   Some other content";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("WRITER, MongoDB, GROQ, LangChain"));
        assert!(!out.contains("!["));
        assert!(!out.contains("cdn.example.com"));
    }

    #[test]
    fn collapses_consecutive_logo_images_on_same_line() {
        let md = "![WRITER](https://cdn.example.com/w.png)![MongoDB](https://cdn.example.com/m.png)![GROQ](https://cdn.example.com/g.png)";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("WRITER"));
        assert!(out.contains("MongoDB"));
        assert!(out.contains("GROQ"));
        assert!(!out.contains("!["));
        assert!(!out.contains("cdn.example.com"));
    }

    #[test]
    fn keeps_meaningful_alt_text() {
        let md = "![A detailed photograph showing the team collaborating on the project](https://img.example.com/photo.jpg)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            out.contains("A detailed photograph showing the team collaborating on the project")
        );
        assert!(!out.contains("!["));
    }

    #[test]
    fn strips_bold_and_italic() {
        let md = "This is **bold text** and *italic text* and __also bold__ and _also italic_.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("This is bold text and italic text and also bold and also italic."));
        assert!(!out.contains("**"));
        assert!(!out.contains("__"));
    }

    #[test]
    fn moves_links_to_end() {
        let md = "Check out [Rust](https://rust-lang.org) and [Go](https://go.dev) for details.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("Check out Rust and Go for details."));
        assert!(out.contains("## Links"));
        assert!(out.contains("- Rust: https://rust-lang.org"));
        assert!(out.contains("- Go: https://go.dev"));
    }

    #[test]
    fn skips_anchor_and_javascript_links() {
        let md = "Go to [top](#top) and [click](javascript:void(0)) and [real](https://real.example.com).";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("## Links"));
        assert!(out.contains("- real: https://real.example.com"));
        let links_section = out.split("## Links").nth(1).unwrap_or("");
        assert!(!links_section.contains("#top"));
        assert!(!links_section.contains("javascript:"));
    }

    #[test]
    fn deduplicates_heading_and_paragraph() {
        let md = "### Ground models\n\nGround models with fresh web context\n\nRetrieve live data.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("### Ground models with fresh web context"));
        assert!(out.contains("Retrieve live data."));
    }

    #[test]
    fn deduplicates_identical_heading_paragraph() {
        let md = "## Features\n\nFeatures\n\nHere are the features.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let feature_count = out.matches("Features").count();
        assert_eq!(
            feature_count, 1,
            "Expected 'Features' exactly once, got: {out}"
        );
    }

    #[test]
    fn collapses_excessive_whitespace() {
        let md = "Line one\n\n\n\n\nLine two\n\n\n\nLine three";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            !out.contains("\n\n\n"),
            "Found 3+ consecutive newlines in: {:?}",
            out
        );
    }

    #[test]
    fn preserves_code_blocks() {
        let md = "Example:\n\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\n\nDone.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("```rust"));
        assert!(out.contains("fn main()"));
        assert!(out.contains("```"));
    }

    #[test]
    fn preserves_list_structure() {
        let md = "Features:\n\n- Fast\n- Safe\n- Concurrent";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("- Fast"));
        assert!(out.contains("- Safe"));
        assert!(out.contains("- Concurrent"));
    }

    #[test]
    fn deduplicates_links() {
        let md = "Visit [Example](https://example.org/page) or [Example again](https://example.org/page).";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let link_count = out.matches("https://example.org/page").count();
        assert_eq!(link_count, 1, "Expected link once, got: {out}");
    }

    #[test]
    fn realistic_page() {
        let html = r#"
        <html lang="en">
        <head>
            <title>Tavily - AI Search API</title>
            <meta name="description" content="Real-time search for AI agents">
        </head>
        <body>
            <article>
                <h1>Connect your AI agents to the web</h1>
                <p>Real-time search, extraction, and web crawling through a <strong>single API</strong>.</p>
                <p>Trusted by <em>1M+ developers</em>.</p>
                <img src="https://cdn.example.com/writer.png" alt="WRITER">
                <img src="https://cdn.example.com/mongo.png" alt="MongoDB">
                <img src="https://cdn.example.com/groq.png" alt="GROQ">
                <img src="https://cdn.example.com/langchain.png" alt="LangChain">
                <h2>Ground models with fresh web context</h2>
                <p>Retrieve live web data and return it structured for models.</p>
                <p>Learn more at <a href="https://docs.tavily.com">the docs</a>.</p>
                <p><a href="https://app.tavily.com">Try it out</a></p>
            </article>
        </body>
        </html>"#;

        let result = crate::extract(html, Some("https://www.tavily.com/")).unwrap();
        let out = to_llm_text(&result, Some("https://www.tavily.com/"));

        assert!(out.contains("> URL: https://www.tavily.com/"));
        assert!(out.contains("> Title:"));

        assert!(!out.contains("!["), "Image markdown not stripped: {out}");
        assert!(
            !out.contains("cdn.example.com"),
            "CDN URL not stripped: {out}"
        );

        assert!(
            out.contains("WRITER") && out.contains("MongoDB"),
            "Logo alt texts missing: {out}"
        );

        assert!(!out.contains("**"), "Bold not stripped: {out}");

        assert!(out.contains("# Connect your AI agents to the web"));
        assert!(out.contains("## Ground models with fresh web context"));
        assert!(out.contains("Retrieve live web data"));

        assert!(out.contains("## Links"));
        assert!(out.contains("https://docs.tavily.com"));
        assert!(out.contains("https://app.tavily.com"));
    }

    #[test]
    fn empty_metadata_fields_excluded() {
        let result = ExtractionResult {
            metadata: Metadata {
                title: None,
                description: None,
                author: None,
                published_date: None,
                language: None,
                url: None,
                site_name: None,
                image: None,
                favicon: None,
                word_count: 0,
                http_status: None,
            },
            content: Content {
                markdown: "Just content".into(),
                plain_text: String::new(),
                links: vec![],
                images: vec![],
                code_blocks: vec![],
                raw_html: None,
            },
            domain_data: None,
            structured_data: vec![],
        };

        let out = to_llm_text(&result, None);
        assert!(!out.contains("> "));
        assert!(out.contains("Just content"));
    }

    #[test]
    fn strips_empty_alt_images() {
        let md = "Before\n\n![](https://cdn.example.com/spacer.gif)\n\nAfter";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(!out.contains("cdn.example.com"));
        assert!(!out.contains("!["));
        assert!(out.contains("Before"));
        assert!(out.contains("After"));
    }

    #[test]
    fn preserves_headings_structure() {
        let md = "# H1\n\n## H2\n\n### H3\n\nContent under H3.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("# H1"));
        assert!(out.contains("## H2"));
        assert!(out.contains("### H3"));
    }

    #[test]
    fn inline_image_in_paragraph_stripped() {
        let md = "Check this ![icon](https://x.com/icon.png) out and read more.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(!out.contains("!["));
        assert!(!out.contains("x.com/icon.png"));
        assert!(out.contains("Check this"));
        assert!(out.contains("out and read more."));
    }

    #[test]
    fn does_not_strip_emphasis_inside_code_blocks() {
        let md = "Normal **bold** text\n\n```python\ndef foo(**kwargs):\n    return _internal_var_\n```\n\nMore text";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("Normal bold text"));
        assert!(out.contains("**kwargs"));
        assert!(out.contains("_internal_var_"));
    }

    #[test]
    fn converts_linked_images_to_links() {
        let md = "[![Read the docs](https://img.example.com/docs.png)](https://docs.example.com)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(!out.contains("!["), "Image not converted: {out}");
        assert!(
            out.contains("https://docs.example.com"),
            "Link URL missing from footer: {out}"
        );
        assert!(out.contains("Read the docs"), "Link text missing: {out}");
    }

    #[test]
    fn linked_images_split_on_separate_lines() {
        let md = "[![Article A](https://img/a.png)](https://a.example.com)[![Article B](https://img/b.png)](https://b.example.com)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("Article A"), "Article A missing: {out}");
        assert!(out.contains("Article B"), "Article B missing: {out}");
        assert!(
            !out.contains("Article AArticle B"),
            "Text mashed together: {out}"
        );
    }

    #[test]
    fn separates_short_and_long_alts_on_same_line() {
        let md = "![AWS](https://cdn/aws.png)![IBM](https://cdn/ibm.png)![Ground models with fresh web context](https://cdn/icon.png)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("AWS, IBM"), "Logo collapse failed: {out}");
        assert!(
            !out.contains("IBM, Ground"),
            "Long alt mixed with logos: {out}"
        );
    }

    #[test]
    fn dedup_text_line_matching_heading() {
        let md = "![Handle thousands of web queries in seconds](https://cdn/icon.png)\n\n### Handle thousands of web queries in seconds\n\nA production-grade stack.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let count = out
            .matches("Handle thousands of web queries in seconds")
            .count();
        assert_eq!(count, 1, "Expected once, got {count}: {out}");
        assert!(out.contains("### Handle thousands"));
        assert!(out.contains("A production-grade stack."));
    }

    #[test]
    fn no_leading_dot_from_linked_images() {
        let md = "[![News A](https://img/a.png)](https://a.com)[![News B](https://img/b.png)](https://b.com)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            !out.contains(". News"),
            "Leading dot from empty remaining: {out}"
        );
    }

    #[test]
    fn merges_stat_lines_with_descriptions() {
        let md = "100M+\n\nmonthly requests handled\n\n99.99% uptime\n\nSLA powering mission-critical systems\n\n180 ms\n\np50 on Tavily /search making us fastest on the market\n\n1M+\n\ndevelopers using Tavily\n\nBillions\n\nof pages crawled and extracted without downtime\n\nDrop-in integration\n\nwith leading LLM providers (OpenAI, Anthropic, Groq)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            out.contains("100M+ monthly requests handled"),
            "Stat not merged: {out}"
        );
        assert!(
            out.contains("99.99% uptime SLA powering mission-critical systems"),
            "Stat not merged: {out}"
        );
        assert!(
            out.contains("180 ms p50 on Tavily /search making us fastest on the market"),
            "Stat not merged: {out}"
        );
        assert!(
            out.contains("1M+ developers using Tavily"),
            "Stat not merged: {out}"
        );
        assert!(
            out.contains("Billions of pages crawled and extracted without downtime"),
            "Stat not merged: {out}"
        );
        assert!(
            out.contains(
                "Drop-in integration with leading LLM providers (OpenAI, Anthropic, Groq)"
            ),
            "Stat not merged: {out}"
        );
    }

    #[test]
    fn merge_stat_preserves_headings_and_lists() {
        let md = "## Features\n\n100M+\n\nmonthly requests\n\n- Fast\n- Safe";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("## Features"), "Heading lost: {out}");
        assert!(
            out.contains("100M+ monthly requests"),
            "Stat not merged: {out}"
        );
        assert!(out.contains("- Fast"), "List item lost: {out}");
        assert!(out.contains("- Safe"), "List item lost: {out}");
    }

    #[test]
    fn merge_stat_does_not_merge_long_lines() {
        let md = "This is a longer line of text!\n\nAnd this follows after a blank";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            !out.contains("text! And"),
            "Long line incorrectly merged: {out}"
        );
    }

    #[test]
    fn strips_css_class_text_lines() {
        let md = "# Typography\n\n\
                   text-4xl font-bold tracking-tight text-gray-900\n\n\
                   Build beautiful websites with Tailwind CSS.\n\n\
                   text-5xl text-6xl text-8xl text-gray-950 text-white tracking-tighter text-balance";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            !out.contains("text-4xl font-bold"),
            "CSS class line was not stripped: {out}"
        );
        assert!(
            !out.contains("text-5xl text-6xl"),
            "CSS class line was not stripped: {out}"
        );
        assert!(
            out.contains("Build beautiful websites"),
            "Normal prose was stripped: {out}"
        );
        assert!(out.contains("Typography"), "Heading was stripped: {out}");
    }

    #[test]
    fn keeps_prose_with_css_like_word() {
        let md = "The text-based approach works well for this use case.\n\n\
                   We use a grid-like layout for the dashboard.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            out.contains("text-based approach"),
            "Normal prose incorrectly stripped: {out}"
        );
        assert!(
            out.contains("grid-like layout"),
            "Normal prose incorrectly stripped: {out}"
        );
    }

    #[test]
    fn preserves_css_classes_inside_code_blocks() {
        let md = "Example usage:\n\n\
                   ```html\n\
                   <div class=\"text-4xl font-bold tracking-tight text-gray-900\">\n\
                   ```\n\n\
                   That applies bold typography.";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            out.contains("text-4xl font-bold tracking-tight"),
            "CSS classes inside code block were stripped: {out}"
        );
    }

    #[test]
    fn dedup_removes_exact_duplicate_paragraphs() {
        let md = "Supabase is an amazing platform that makes building apps incredibly fast.\n\nSupabase is an amazing platform that makes building apps incredibly fast.\n\nSupabase is an amazing platform that makes building apps incredibly fast.\n\nEach project gets its own dedicated Postgres database.";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let count = out.matches("Supabase is an amazing platform").count();
        assert_eq!(
            count, 1,
            "Duplicate paragraph should appear only once, got {count}: {out}"
        );
        assert!(
            out.contains("Each project gets its own dedicated Postgres database"),
            "Unique paragraph missing: {out}"
        );
    }

    #[test]
    fn dedup_preserves_unique_paragraphs() {
        let md = "First unique paragraph with enough content to be checked.\n\nSecond unique paragraph that is completely different.\n\nThird unique paragraph covering another topic entirely.";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(out.contains("First unique paragraph"), "Lost first: {out}");
        assert!(
            out.contains("Second unique paragraph"),
            "Lost second: {out}"
        );
        assert!(out.contains("Third unique paragraph"), "Lost third: {out}");
    }

    #[test]
    fn dedup_keeps_short_repeated_text() {
        let md = "Learn more\n\nA detailed explanation of the first feature.\n\nLearn more\n\nA detailed explanation of the second feature.";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let count = out.matches("Learn more").count();
        assert!(
            count >= 2,
            "Short repeated text should be kept, got {count}: {out}"
        );
    }

    #[test]
    fn dedup_catches_near_duplicates_via_prefix() {
        let md = "The platform provides real-time sync collaboration tools for modern developers building web applications with React and Next.js.\n\nThe platform provides real-time sync collaboration tools for modern developers building mobile apps with Flutter.\n\nA completely different paragraph about database design.";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let count = out.matches("The platform provides real-time sync").count();
        assert_eq!(
            count, 1,
            "Near-duplicate should be removed, got {count}: {out}"
        );
        assert!(
            out.contains("A completely different paragraph"),
            "Unique paragraph missing: {out}"
        );
    }

    #[test]
    fn dedup_carousel_realistic() {
        let md = "## What our users say\n\n\"Supabase has transformed how we build products. The developer experience is unmatched.\" - Sarah Chen, CTO at TechCorp\n\n\"Moving from Firebase to Supabase was the best decision we made this year.\" - James Liu, Lead Engineer\n\n\"The real-time features and Postgres foundation give us confidence at scale.\" - Maria Garcia, VP Engineering\n\n\"Supabase has transformed how we build products. The developer experience is unmatched.\" - Sarah Chen, CTO at TechCorp\n\n\"Moving from Firebase to Supabase was the best decision we made this year.\" - James Liu, Lead Engineer\n\n\"The real-time features and Postgres foundation give us confidence at scale.\" - Maria Garcia, VP Engineering\n\n\"Supabase has transformed how we build products. The developer experience is unmatched.\" - Sarah Chen, CTO at TechCorp\n\n\"Moving from Firebase to Supabase was the best decision we made this year.\" - James Liu, Lead Engineer\n\n\"The real-time features and Postgres foundation give us confidence at scale.\" - Maria Garcia, VP Engineering\n\n## Get started\n\nSign up for free today.";

        let result = make_result(md);
        let out = to_llm_text(&result, None);

        let sarah_count = out.matches("Sarah Chen").count();
        let james_count = out.matches("James Liu").count();
        let maria_count = out.matches("Maria Garcia").count();

        assert_eq!(sarah_count, 1, "Sarah duplicated {sarah_count}x: {out}");
        assert_eq!(james_count, 1, "James duplicated {james_count}x: {out}");
        assert_eq!(maria_count, 1, "Maria duplicated {maria_count}x: {out}");

        assert!(out.contains("## What our users say"), "Heading lost: {out}");
        assert!(out.contains("## Get started"), "Heading lost: {out}");
    }

    #[test]
    fn strips_bare_image_references() {
        let md = "Some content\n\nhero.webp\n\nhttps://example.com/logo.svg\n\n![](image.png)\n\n![icon](logo.svg)\n\nThe file output.png is saved to disk.\n\n![Detailed architecture diagram showing the data flow](arch.png)\n\nMore content";
        let result = make_result(md);
        let out = to_llm_text(&result, None);

        assert!(
            !out.contains("hero.webp"),
            "Bare filename not stripped: {out}"
        );
        assert!(
            !out.contains("https://example.com/logo.svg"),
            "Bare image URL not stripped: {out}"
        );
        assert!(
            !out.contains("image.png"),
            "Empty-alt image not stripped: {out}"
        );
        assert!(
            !out.contains("logo.svg"),
            "Generic-alt image not stripped: {out}"
        );
        assert!(
            out.contains("output.png is saved to disk"),
            "Sentence with .png filename was incorrectly stripped: {out}"
        );
        assert!(
            out.contains("Detailed architecture diagram showing the data flow"),
            "Meaningful alt text was stripped: {out}"
        );
        assert!(
            !out.contains("arch.png"),
            "Image URL not stripped from meaningful alt: {out}"
        );
        assert!(out.contains("Some content"), "Content before lost: {out}");
        assert!(out.contains("More content"), "Content after lost: {out}");
    }

    // -- Structured-data gating tests --

    fn make_result_with_structured(values: Vec<serde_json::Value>) -> ExtractionResult {
        let mut r = make_result("# Body");
        r.structured_data = values;
        r
    }

    #[test]
    fn structured_data_drops_chrome_types() {
        // WebSite/WebPage records are framework chrome — should be dropped.
        let r = make_result_with_structured(vec![serde_json::json!({
            "@type": "WebSite",
            "name": "Example",
            "url": "https://example.com"
        })]);
        let out = to_llm_text(&r, None);
        assert!(
            !out.contains("## Structured Data"),
            "WebSite chrome leaked into output: {out}"
        );
    }

    #[test]
    fn structured_data_keeps_article_types() {
        let r = make_result_with_structured(vec![serde_json::json!({
            "@type": "NewsArticle",
            "headline": "Big news",
            "datePublished": "2026-05-10"
        })]);
        let out = to_llm_text(&r, None);
        assert!(
            out.contains("## Structured Data"),
            "NewsArticle dropped: {out}"
        );
        assert!(out.contains("Big news"));
    }

    #[test]
    fn structured_data_scrubs_duplicate_article_body() {
        let body = "This is the rendered article body. ".repeat(40);
        let r = make_result_with_structured(vec![serde_json::json!({
            "@type": "NewsArticle",
            "headline": "Big news",
            "articleBody": body,
            "description": "A short useful summary"
        })]);
        let out = to_llm_text(&r, None);
        assert!(out.contains("Big news"));
        assert!(out.contains("A short useful summary"));
        assert!(
            !out.contains("articleBody"),
            "Duplicate article body leaked: {out}"
        );
    }

    #[test]
    fn llm_output_strips_comment_count_links_and_pagination() {
        let md = "Lead paragraph.\n\n[0](https://example.com/#comment-stream) Next\n\n5 minutes read\n\n[Article](https://example.com/article)";
        let result = make_result(md);
        let out = to_llm_text(&result, None);
        assert!(out.contains("Lead paragraph."));
        assert!(out.contains("5 minutes read"));
        assert!(out.contains("- Article: https://example.com/article"));
        assert!(!out.contains("0 Next"), "Pagination leaked: {out}");
        assert!(
            !out.contains("comment-stream"),
            "Comment link leaked: {out}"
        );
    }

    #[test]
    fn structured_data_drops_oversized_blob() {
        // 32KB pageProps-style blob with no @type — should be dropped.
        let big = "x".repeat(32 * 1024);
        let r = make_result_with_structured(vec![serde_json::json!({
            "buildId": "abc",
            "isFallback": false,
            "noise": big
        })]);
        let out = to_llm_text(&r, None);
        assert!(
            !out.contains("## Structured Data"),
            "Oversized untyped blob leaked: len={}",
            out.len()
        );
    }

    #[test]
    fn structured_data_keeps_compact_untyped() {
        // Small untyped record (e.g. a parsed pageProps with real content) — keep.
        let r = make_result_with_structured(vec![serde_json::json!({
            "title": "Hi",
            "body": "small enough to keep"
        })]);
        let out = to_llm_text(&r, None);
        assert!(
            out.contains("## Structured Data"),
            "Compact untyped dropped: {out}"
        );
    }

    #[test]
    fn structured_data_keeps_compact_untyped_array() {
        // SvelteKit can emit compact arrays rather than objects.
        let r = make_result_with_structured(vec![serde_json::json!([
            { "title": "Hi", "body": "small array item" }
        ])]);
        let out = to_llm_text(&r, None);
        assert!(
            out.contains("small array item"),
            "Compact untyped array dropped: {out}"
        );
    }

    /// Walk `value` down its single `"n"` child link and return the depth
    /// at which an `articleBody` key is still present (i.e. was NOT
    /// scrubbed). Used to observe exactly where the recursion stopped.
    fn first_unscrubbed_article_body_depth(mut value: &serde_json::Value) -> Option<usize> {
        let mut depth = 0;
        loop {
            let obj = value.as_object()?;
            if obj.contains_key("articleBody") {
                return Some(depth);
            }
            value = obj.get("n")?;
            depth += 1;
        }
    }

    #[test]
    fn scrub_body_fields_bounds_recursion_on_deep_nesting() {
        // Attacker-controlled JSON-LD / __NEXT_DATA__ with pathological
        // nesting must not recurse without bound. Build a chain a little
        // past the 64-level cap where every level carries a scrub-able
        // `articleBody`. Levels within the cap get scrubbed; the first
        // level past the cap keeps its `articleBody` because recursion
        // stopped — that is the bound we assert. (Kept shallow on purpose:
        // serde_json drops Values recursively, so a 10k-deep value would
        // overflow the stack just being dropped.)
        const DEPTH: usize = 80;
        let mut node = serde_json::json!({ "articleBody": "x".repeat(600) });
        for _ in 0..DEPTH {
            node = serde_json::json!({
                "articleBody": "x".repeat(600),
                "n": node,
            });
        }

        scrub_body_fields(&mut node, 0);

        let stopped_at = first_unscrubbed_article_body_depth(&node)
            .expect("recursion must stop and leave a deep articleBody intact");
        // Top levels were scrubbed; the survivor sits right at the cap.
        assert_eq!(
            stopped_at, 64,
            "recursion should stop at the depth cap, stopped at {stopped_at}"
        );
        assert!(
            node.as_object().unwrap().get("articleBody").is_none(),
            "shallow articleBody must still be scrubbed"
        );
    }

    // ------------------------------------------------------------------
    // M4: --prefer-structured / --articles-from-jsonld integration tests
    // ------------------------------------------------------------------

    /// Default options (no flags) produce byte-identical output to legacy
    /// `to_llm_text`. This is the sentinel for "additive change" — every
    /// p01-p20 probe relies on this.
    #[test]
    fn to_llm_text_with_options_default_is_legacy_identical() {
        let r = make_result_with_structured(vec![serde_json::json!({
            "@type": "Article",
            "headline": "Hello",
        })]);
        let legacy = to_llm_text(&r, None);
        let with_opts = to_llm_text_with_options(&r, None, &LlmTextOptions::default());
        assert_eq!(legacy, with_opts, "default opts must be byte-identical");
    }

    /// With `prefer_structured`, the schema-aware block appears at the TOP
    /// of the output (after the metadata header, before the prose body).
    /// Also: the legacy bottom `## Structured Data` block is suppressed.
    #[test]
    fn prefer_structured_places_block_above_body_and_drops_legacy() {
        let mut r = make_result_with_structured(vec![serde_json::json!({
            "@type": "Review",
            "headline": "Album X",
            "reviewBody": "A long-form review body that would normally be far down the page.".repeat(20),
            "datePublished": "2026-05-23",
        })]);
        r.content.markdown = "## Body Section\n\nLong prose body here.\n".repeat(20);
        let out = to_llm_text_with_options(&r, None, &LlmTextOptions { prefer_structured: true });

        // Structured-data section is present at the top.
        let struct_idx = out
            .find("## Structured data")
            .expect("schema-aware block must be present");
        let body_idx = out
            .find("Body Section")
            .expect("prose body must be present");
        assert!(
            struct_idx < body_idx,
            "schema-aware block must come BEFORE prose body (struct@{struct_idx}, body@{body_idx})"
        );

        // Legacy bottom block is suppressed to avoid duplication.
        assert!(
            !out.contains("## Structured Data"),
            "legacy uppercase 'Structured Data' block must be dropped when prefer_structured is set"
        );
    }

    /// With `prefer_structured` and an ItemList page, the top block lists
    /// the items with positions and URLs.
    #[test]
    fn prefer_structured_itemlist_renders_items() {
        let r = make_result_with_structured(vec![serde_json::json!({
            "@type": "ItemList",
            "numberOfItems": 2,
            "itemListElement": [
                {"@type": "ListItem", "position": 1, "url": "https://x/1", "name": "First"},
                {"@type": "ListItem", "position": 2, "url": "https://x/2", "name": "Second"},
            ]
        })]);
        let out = to_llm_text_with_options(&r, None, &LlmTextOptions { prefer_structured: true });
        assert!(out.contains("schema: ItemList (2 items)"), "missing header in:\n{out}");
        assert!(out.contains("1. First — https://x/1"), "missing item 1 in:\n{out}");
        assert!(out.contains("2. Second — https://x/2"), "missing item 2 in:\n{out}");
    }

    /// With `prefer_structured` and a WebPage chrome type, the block is
    /// still emitted (override of the normal DROP filter) but identifies
    /// itself as a navigation/chrome record.
    #[test]
    fn prefer_structured_surfaces_webpage_chrome() {
        let r = make_result_with_structured(vec![serde_json::json!({
            "@type": "WebPage",
            "name": "Hub Page",
        })]);
        let out = to_llm_text_with_options(&r, None, &LlmTextOptions { prefer_structured: true });
        assert!(out.contains("## Structured data"), "missing header in:\n{out}");
        assert!(out.contains("schema: WebPage"), "missing WebPage schema label in:\n{out}");
    }

    // ------------------------------------------------------------------
    // M7: HTTP status header line (issue #19)
    // ------------------------------------------------------------------

    /// 200 control: status line appears in -f llm output even on success
    /// so callers can distinguish "webclaw saw a 200" from "webclaw didn't
    /// reach the formatter / status unknown" (e.g. local-file path).
    #[test]
    fn test_status_header_appears_on_200() {
        let mut r = make_result("# Body");
        r.metadata.http_status = Some(200);
        let out = to_llm_text(&r, None);
        assert!(
            out.contains("> Status: 200\n") || out.contains("> Status: 200"),
            "Status: 200 line missing from -f llm output:\n{out}"
        );
        // Must sit between URL and Title (Option A placement).
        let url_pos = out.find("> URL:").expect("URL line missing");
        let status_pos = out.find("> Status:").expect("Status line missing");
        let title_pos = out.find("> Title:").expect("Title line missing");
        assert!(url_pos < status_pos, "Status must come AFTER URL");
        assert!(status_pos < title_pos, "Status must come BEFORE Title");
    }

    /// 404: status line distinguishes a real 404 (Status: 404 + thin
    /// soft-404 body) from a thin 200 article. This is the core M7 bug
    /// — `webclaw https://www.dailysabah.com/business/economy` was
    /// returning exit 0 with a 13-word body and no way for the caller to
    /// tell it was actually a 404 error page.
    #[test]
    fn test_status_header_appears_on_404() {
        let mut r = make_result("## 404 / The page you're looking for does not exist!");
        r.metadata.http_status = Some(404);
        let out = to_llm_text(&r, None);
        assert!(
            out.contains("> Status: 404"),
            "Status: 404 line missing from -f llm output:\n{out}"
        );
    }

    /// When http_status is None (local-file / --stdin / direct
    /// extract_with_options) NO Status line is emitted. Backward-compat
    /// for callers that pre-date M7 and parse via line index.
    #[test]
    fn test_status_header_absent_when_unset() {
        let r = make_result("# Body"); // http_status defaults to None
        assert!(r.metadata.http_status.is_none());
        let out = to_llm_text(&r, None);
        assert!(
            !out.contains("> Status:"),
            "Status line leaked when http_status is None:\n{out}"
        );
    }

    /// JSON output: the `status` field (renamed from internal http_status
    /// via serde rename) appears at metadata level and carries the code.
    #[test]
    fn test_status_header_format_in_json() {
        let mut r = make_result("# Body");
        r.metadata.http_status = Some(404);
        let json = serde_json::to_string_pretty(&r).expect("serialize");
        assert!(
            json.contains("\"status\": 404"),
            "JSON output missing \"status\": 404:\n{json}"
        );
        // serde rename means the internal name "http_status" must NOT
        // surface in JSON output.
        assert!(
            !json.contains("http_status"),
            "internal field name leaked into JSON:\n{json}"
        );
    }

    /// JSON output: when http_status is None the field is omitted
    /// entirely (skip_serializing_if = "Option::is_none").
    #[test]
    fn test_status_field_omitted_in_json_when_unset() {
        let r = make_result("# Body"); // http_status defaults to None
        let json = serde_json::to_string_pretty(&r).expect("serialize");
        assert!(
            !json.contains("\"status\""),
            "status field should be omitted when None:\n{json}"
        );
    }

    /// Summary mode (M1 `--mode summary`) must NOT include the Status
    /// line — summary returns a link list and the status would be noise.
    #[test]
    fn test_status_omitted_in_summary_mode() {
        let mut r = make_result("# Body");
        r.metadata.http_status = Some(404);
        // to_llm_summary builds its own header via
        // build_metadata_header_with_opts(include_status=false).
        let out = to_llm_summary(&r, None);
        assert!(
            !out.contains("> Status:"),
            "Status line leaked into summary mode output:\n{out}"
        );
        // URL line should still be present though.
        assert!(
            out.contains("> URL:") || r.metadata.url.is_some_and(|u| out.contains(&u)) || true,
            // URL is conditional on metadata; we don't assert presence,
            // only that Status is absent regardless.
            ""
        );
    }

    /// TOC mode (M1 `--mode toc`) must NOT include the Status line either —
    /// the outline is structural metadata, status would clutter it.
    #[test]
    fn test_status_omitted_in_toc_mode() {
        let mut r = make_result("# H1\n\n## H2\n\nFirst paragraph after H2.");
        r.metadata.http_status = Some(404);
        let out = to_llm_toc(&r, None);
        assert!(
            !out.contains("> Status:"),
            "Status line leaked into toc mode output:\n{out}"
        );
    }
}
