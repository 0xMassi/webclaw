/// LLM-optimized output format.
///
/// Takes an `ExtractionResult` and produces a compact text representation
/// that maximizes information density per token. Strips decorative images,
/// visual-only formatting (bold/italic), and inline link URLs -- moving links
/// to a deduplicated section at the end.
mod body;
mod cleanup;
mod images;
mod links;
mod metadata;

use crate::types::ExtractionResult;

/// Produce a token-optimized text representation of extracted content.
///
/// The output has three sections:
/// 1. Compact metadata header (`> ` prefixed lines)
/// 2. Cleaned body (no images, no bold/italic, links as plain text)
/// 3. Deduplicated links section at the end
pub fn to_llm_text(result: &ExtractionResult, url: Option<&str>) -> String {
    let mut out = String::new();

    // -- 1. Metadata header --
    metadata::build_metadata_header(&mut out, result, url);

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
    let useful: Vec<_> = result
        .structured_data
        .iter()
        .filter(|v| is_useful_structured_data(v))
        .cloned()
        .collect();
    if !useful.is_empty() {
        let serialized = serde_json::to_string_pretty(&useful).unwrap_or_default();
        const STRUCTURED_DATA_MAX_BYTES: usize = 16 * 1024;
        if serialized.len() <= STRUCTURED_DATA_MAX_BYTES {
            out.push_str("\n\n## Structured Data\n\n```json\n");
            out.push_str(&serialized);
            out.push_str("\n```");
        }
    }

    out.trim().to_string()
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
        return false;
    };
    // JSON-LD: @type drives the decision.
    if let Some(t) = obj.get("@type") {
        let type_str = match t {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str())
                .collect::<Vec<_>>()
                .join(","),
            _ => String::new(),
        };
        let lower = type_str.to_ascii_lowercase();
        // Drop low-info chrome types.
        const DROP_TYPES: &[&str] = &["website", "webpage", "sitenavigationelement"];
        if DROP_TYPES.iter().any(|d| lower == *d) {
            return false;
        }
        return !lower.is_empty();
    }
    // Next.js pageProps / SvelteKit data without @type: keep only if compact.
    // Anything over ~4KB is almost certainly hydration state, not content.
    let serialized = serde_json::to_string(v).unwrap_or_default();
    serialized.len() <= 4 * 1024
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
}
