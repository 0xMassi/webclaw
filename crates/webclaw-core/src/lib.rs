pub mod brand;
pub(crate) mod data_island;
/// webclaw-core: Pure HTML content extraction engine for LLMs.
///
/// Takes raw HTML + optional URL, returns structured content
/// (metadata, markdown, plain text, links, images, code blocks).
/// Zero network dependencies — WASM-compatible by design.
pub mod diff;
pub mod domain;
pub mod error;
pub mod extractor;
#[cfg(feature = "quickjs")]
pub mod js_eval;
pub mod llm;
pub mod markdown;
pub mod metadata;
#[allow(dead_code)]
pub(crate) mod noise;
pub mod structured_data;
pub mod types;
pub mod youtube;

pub use brand::BrandIdentity;
pub use diff::{ChangeStatus, ContentDiff, MetadataChange};
pub use domain::DomainType;
pub use error::ExtractError;
pub use llm::to_llm_text;
pub use types::{
    CodeBlock, Content, DomainData, ExtractionOptions, ExtractionResult, Image, Link, Metadata,
};

use scraper::Html;
use url::Url;

/// Extract structured content from raw HTML.
///
/// `html` — raw HTML string to parse
/// `url`  — optional source URL, used for resolving relative links and domain detection
pub fn extract(html: &str, url: Option<&str>) -> Result<ExtractionResult, ExtractError> {
    extract_with_options(html, url, &ExtractionOptions::default())
}

/// Extract structured content from raw HTML with configurable options.
///
/// `html`    — raw HTML string to parse
/// `url`     — optional source URL, used for resolving relative links and domain detection
/// `options` — controls include/exclude selectors, main content mode, and raw HTML output
///
/// Spawns extraction on a thread with an 8 MB stack to handle deeply nested
/// HTML (e.g., Express.co.uk live blogs) without overflowing the default 1-2 MB
/// main-thread stack on Windows.
pub fn extract_with_options(
    html: &str,
    url: Option<&str>,
    options: &ExtractionOptions,
) -> Result<ExtractionResult, ExtractError> {
    // The default main-thread stack on Windows is 1 MB, which can overflow
    // on deeply nested pages.  Spawn a worker thread with 8 MB to be safe.
    const STACK_SIZE: usize = 8 * 1024 * 1024; // 8 MB

    let html = html.to_string();
    let url = url.map(|u| u.to_string());
    let options = options.clone();

    std::thread::Builder::new()
        .stack_size(STACK_SIZE)
        .spawn(move || extract_with_options_inner(&html, url.as_deref(), &options))
        .map_err(|_| ExtractError::NoContent)?
        .join()
        .unwrap_or(Err(ExtractError::NoContent))
}

fn extract_with_options_inner(
    html: &str,
    url: Option<&str>,
    options: &ExtractionOptions,
) -> Result<ExtractionResult, ExtractError> {
    if html.is_empty() {
        return Err(ExtractError::NoContent);
    }

    // YouTube fast path: if the URL is a YouTube video page, try extracting
    // structured metadata from ytInitialPlayerResponse before DOM scoring.
    // This gives LLMs a clean, structured view of video metadata.
    if let Some(u) = url
        && youtube::is_youtube_url(u)
        && let Some(yt_md) = youtube::try_extract(html)
    {
        let doc = Html::parse_document(html);
        let mut meta = metadata::extract(&doc, url);
        meta.word_count = extractor::word_count(&yt_md);

        let plain_text = yt_md
            .lines()
            .filter(|l| !l.starts_with('#') && !l.starts_with("**"))
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_string();

        let domain_data = Some(DomainData {
            domain_type: DomainType::Social,
        });

        let structured_data = structured_data::extract_json_ld(html);

        return Ok(ExtractionResult {
            metadata: meta,
            content: Content {
                markdown: yt_md,
                plain_text,
                links: Vec::new(),
                images: Vec::new(),
                code_blocks: Vec::new(),
                raw_html: None,
            },
            domain_data,
            structured_data,
        });
    }

    let doc = Html::parse_document(html);

    let base_url = url
        .map(|u| Url::parse(u).map_err(|_| ExtractError::InvalidUrl(u.to_string())))
        .transpose()?;

    // Metadata from <head>
    let mut meta = metadata::extract(&doc, url);

    // Main content extraction (Readability-style scoring + markdown conversion)
    let mut content = extractor::extract_content(&doc, base_url.as_ref(), options);
    // Use the higher of plain_text and markdown word counts.
    // Some pages (headings + links) have content in markdown but empty plain_text.
    let pt_wc = extractor::word_count(&content.plain_text);
    let md_wc = extractor::word_count(&content.markdown);
    meta.word_count = pt_wc.max(md_wc);

    // Retry fallback: if extraction captured too little of the page's visible content,
    // retry with wider strategies. The scorer sometimes picks a tiny node (e.g., an
    // <article> with 52 words when the body has 1300 words of real content).
    //
    // Strategy 1: retry without only_main_content restriction
    if options.only_main_content && meta.word_count < 30 {
        let relaxed = ExtractionOptions {
            only_main_content: false,
            ..options.clone()
        };
        let retry = extractor::extract_content(&doc, base_url.as_ref(), &relaxed);
        let retry_wc =
            extractor::word_count(&retry.plain_text).max(extractor::word_count(&retry.markdown));
        if retry_wc > meta.word_count {
            content = retry;
            meta.word_count = retry_wc;
        }
    }

    // Strategy 2: if scored extraction is sparse (<200 words) AND the page has
    // significantly more visible text, retry with include_selectors: ["body"].
    // This bypasses the readability scorer entirely — catches blogs, pricing
    // pages, and modern sites where no single element scores well.
    if meta.word_count < 200 && options.include_selectors.is_empty() {
        let body_opts = ExtractionOptions {
            include_selectors: vec!["body".to_string()],
            exclude_selectors: options.exclude_selectors.clone(),
            only_main_content: false,
            include_raw_html: false,
        };
        let body_content = extractor::extract_content(&doc, base_url.as_ref(), &body_opts);
        let body_wc = extractor::word_count(&body_content.plain_text)
            .max(extractor::word_count(&body_content.markdown));
        // Use body extraction if it captures significantly more content (>2x)
        if body_wc > meta.word_count * 2 && body_wc > 50 {
            content = body_content;
            meta.word_count = body_wc;
        }
    }

    // Fallback: if DOM extraction was sparse, try JSON data islands
    // (React SPAs, Next.js, Contentful CMS embed page data in <script> tags)
    if let Some(island_md) = data_island::try_extract(&doc, meta.word_count, &content.markdown) {
        content.markdown.push_str("\n\n");
        content.markdown.push_str(&island_md);
        meta.word_count = extractor::word_count(&content.markdown);
    }

    // QuickJS: execute inline <script> tags to capture JS-assigned data blobs
    // (e.g., window.__PRELOADED_STATE__, self.__next_f). This supplements the
    // static JSON data island extraction above with runtime-evaluated data.
    #[cfg(feature = "quickjs")]
    {
        let blobs = js_eval::extract_js_data(html);
        if !blobs.is_empty() {
            let js_text = js_eval::extract_readable_text(&blobs);
            if !js_text.is_empty() {
                content.markdown.push_str("\n\n");
                content.markdown.push_str(&js_text);
                meta.word_count = extractor::word_count(&content.markdown);
            }
        }
    }

    // Domain detection from URL patterns and DOM heuristics
    let domain_type = domain::detect(url, html);
    let domain_data = Some(DomainData { domain_type });

    // Structured data: JSON-LD + __NEXT_DATA__ + SvelteKit data islands
    let mut structured_data = structured_data::extract_json_ld(html);
    structured_data.extend(structured_data::extract_next_data(html));
    structured_data.extend(structured_data::extract_sveltekit(html));

    Ok(ExtractionResult {
        metadata: meta,
        content,
        domain_data,
        structured_data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_extraction_pipeline() {
        let html = r#"
        <html lang="en">
        <head>
            <title>Rust is Great</title>
            <meta name="description" content="An article about Rust">
            <meta name="author" content="Bob">
        </head>
        <body>
            <nav><a href="/">Home</a> | <a href="/about">About</a></nav>
            <article>
                <h1>Why Rust is Great</h1>
                <p>Rust gives you <strong>memory safety</strong> without a garbage collector.
                This is achieved through its <em>ownership system</em>.</p>
                <p>Here is an example:</p>
                <pre><code class="language-rust">fn main() {
    println!("Hello, world!");
}</code></pre>
                <p>Learn more at <a href="https://rust-lang.org">rust-lang.org</a>.</p>
            </article>
            <footer>Copyright 2025</footer>
        </body>
        </html>"#;

        let result = extract(html, Some("https://blog.example.com/rust")).unwrap();

        // Metadata
        assert_eq!(result.metadata.title.as_deref(), Some("Rust is Great"));
        assert_eq!(
            result.metadata.description.as_deref(),
            Some("An article about Rust")
        );
        assert_eq!(result.metadata.author.as_deref(), Some("Bob"));
        assert_eq!(result.metadata.language.as_deref(), Some("en"));
        assert!(result.metadata.word_count > 0);

        // Content
        assert!(result.content.markdown.contains("# Why Rust is Great"));
        assert!(result.content.markdown.contains("**memory safety**"));
        assert!(result.content.markdown.contains("```rust"));
        assert!(
            result
                .content
                .links
                .iter()
                .any(|l| l.href == "https://rust-lang.org")
        );
        assert!(!result.content.code_blocks.is_empty());

        // raw_html not populated by default
        assert!(result.content.raw_html.is_none());

        // Domain — blog.example.com has <article> tag
        let dd = result.domain_data.unwrap();
        assert_eq!(dd.domain_type, DomainType::Article);
    }

    #[test]
    fn invalid_url_returns_error() {
        let result = extract("<html></html>", Some("not a url"));
        assert!(matches!(result, Err(ExtractError::InvalidUrl(_))));
    }

    #[test]
    fn empty_html_returns_error() {
        let result = extract("", None);
        assert!(matches!(result, Err(ExtractError::NoContent)));
    }

    #[test]
    fn no_url_is_fine() {
        let result = extract("<html><body><p>Hello</p></body></html>", None);
        assert!(result.is_ok());
    }

    #[test]
    fn serializes_to_json() {
        let result = extract("<html><body><p>Test</p></body></html>", None).unwrap();
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("metadata"));
        assert!(json.contains("content"));
        // raw_html should be absent (skip_serializing_if)
        assert!(!json.contains("raw_html"));
    }

    #[test]
    fn youtube_extraction_produces_structured_markdown() {
        let html = r#"
        <html><head><title>Rust in 100 Seconds - YouTube</title></head>
        <body>
        <script>
        var ytInitialPlayerResponse = {"videoDetails":{"title":"Rust in 100 Seconds","author":"Fireship","viewCount":"5432100","shortDescription":"Learn Rust in 100 seconds. A mass of web developers are mass adopting Rust.","lengthSeconds":"120"},"microformat":{"playerMicroformatRenderer":{"uploadDate":"2023-01-15"}}};
        </script>
        </body></html>
        "#;

        let result = extract(html, Some("https://www.youtube.com/watch?v=5C_HPTJg5ek")).unwrap();

        assert!(result.content.markdown.contains("# Rust in 100 Seconds"));
        assert!(result.content.markdown.contains("**Channel:** Fireship"));
        assert!(result.content.markdown.contains("2:00"));
        assert!(
            result
                .content
                .markdown
                .contains("Learn Rust in 100 seconds")
        );

        // Should be detected as Social domain
        let dd = result.domain_data.unwrap();
        assert_eq!(dd.domain_type, DomainType::Social);
    }

    #[test]
    fn youtube_url_without_player_response_falls_through() {
        // If ytInitialPlayerResponse is missing, fall through to normal extraction
        let html = r#"<html><body><article><h1>Some YouTube Page</h1><p>Content here for testing.</p></article></body></html>"#;
        let result = extract(html, Some("https://www.youtube.com/watch?v=abc123")).unwrap();

        // Should still extract something via normal pipeline
        assert!(result.content.markdown.contains("Some YouTube Page"));
    }

    // --- ExtractionOptions tests ---

    #[test]
    fn test_exclude_selectors() {
        let html = r#"<html><body>
            <nav>Navigation stuff</nav>
            <article><h1>Title</h1><p>Real content here.</p></article>
            <footer>Footer stuff</footer>
        </body></html>"#;

        let options = ExtractionOptions {
            exclude_selectors: vec!["nav".into(), "footer".into()],
            ..Default::default()
        };
        let result = extract_with_options(html, None, &options).unwrap();

        assert!(result.content.markdown.contains("Real content"));
        assert!(
            !result.content.markdown.contains("Navigation stuff"),
            "nav should be excluded"
        );
        assert!(
            !result.content.markdown.contains("Footer stuff"),
            "footer should be excluded"
        );
    }

    #[test]
    fn test_include_selectors() {
        let html = r#"<html><body>
            <nav>Navigation stuff</nav>
            <article><h1>Title</h1><p>Real content here.</p></article>
            <div class="sidebar">Sidebar junk</div>
            <footer>Footer stuff</footer>
        </body></html>"#;

        let options = ExtractionOptions {
            include_selectors: vec!["article".into()],
            ..Default::default()
        };
        let result = extract_with_options(html, None, &options).unwrap();

        assert!(result.content.markdown.contains("Title"));
        assert!(result.content.markdown.contains("Real content"));
        assert!(
            !result.content.markdown.contains("Navigation stuff"),
            "nav should not be included"
        );
        assert!(
            !result.content.markdown.contains("Sidebar junk"),
            "sidebar should not be included"
        );
        assert!(
            !result.content.markdown.contains("Footer stuff"),
            "footer should not be included"
        );
    }

    #[test]
    fn test_include_and_exclude() {
        let html = r#"<html><body>
            <article>
                <h1>Title</h1>
                <p>Real content here.</p>
                <div class="sidebar">Sidebar inside article</div>
            </article>
            <footer>Footer stuff</footer>
        </body></html>"#;

        let options = ExtractionOptions {
            include_selectors: vec!["article".into()],
            exclude_selectors: vec![".sidebar".into()],
            ..Default::default()
        };
        let result = extract_with_options(html, None, &options).unwrap();

        assert!(result.content.markdown.contains("Title"));
        assert!(result.content.markdown.contains("Real content"));
        assert!(
            !result.content.markdown.contains("Sidebar inside article"),
            "sidebar inside article should be excluded"
        );
        assert!(
            !result.content.markdown.contains("Footer stuff"),
            "footer should not be included"
        );
    }

    #[test]
    fn test_only_main_content() {
        let html = r#"<html><body>
            <nav>Navigation</nav>
            <div class="hero"><h1>Big Hero</h1></div>
            <article><h2>Article Title</h2><p>Article content that is long enough to be real.</p></article>
            <div class="sidebar">Sidebar</div>
            <footer>Footer</footer>
        </body></html>"#;

        let options = ExtractionOptions {
            only_main_content: true,
            ..Default::default()
        };
        let result = extract_with_options(html, None, &options).unwrap();

        assert!(
            result.content.markdown.contains("Article Title"),
            "article content should be present"
        );
        assert!(
            result.content.markdown.contains("Article content"),
            "article body should be present"
        );
        // only_main_content picks the article/main element directly, so hero and sidebar
        // should not be in the output
        assert!(
            !result.content.markdown.contains("Sidebar"),
            "sidebar should not be in only_main_content output"
        );
    }

    #[test]
    fn test_include_raw_html() {
        let html = r#"<html><body>
            <article><h1>Title</h1><p>Content here.</p></article>
        </body></html>"#;

        let options = ExtractionOptions {
            include_raw_html: true,
            ..Default::default()
        };
        let result = extract_with_options(html, None, &options).unwrap();

        assert!(
            result.content.raw_html.is_some(),
            "raw_html should be populated"
        );
        let raw = result.content.raw_html.unwrap();
        assert!(
            raw.contains("<article>"),
            "raw_html should contain article tag"
        );
        assert!(raw.contains("<h1>Title</h1>"), "raw_html should contain h1");
    }

    #[test]
    fn test_invalid_selectors() {
        let html = r#"<html><body>
            <article><h1>Title</h1><p>Content here.</p></article>
        </body></html>"#;

        // Invalid selectors should be gracefully skipped
        let options = ExtractionOptions {
            include_selectors: vec!["[invalid[[[".into(), "article".into()],
            exclude_selectors: vec![">>>bad".into()],
            ..Default::default()
        };
        let result = extract_with_options(html, None, &options).unwrap();

        assert!(
            result.content.markdown.contains("Title"),
            "valid selectors should still work"
        );
        assert!(
            result.content.markdown.contains("Content here"),
            "extraction should proceed despite invalid selectors"
        );
    }

    #[test]
    fn test_backward_compat() {
        let html = r#"<html><body>
            <article><h1>Title</h1><p>Content here.</p></article>
        </body></html>"#;

        let result_old = extract(html, None).unwrap();
        let result_new = extract_with_options(html, None, &ExtractionOptions::default()).unwrap();

        assert_eq!(result_old.content.markdown, result_new.content.markdown);
        assert_eq!(result_old.content.plain_text, result_new.content.plain_text);
        assert_eq!(
            result_old.content.links.len(),
            result_new.content.links.len()
        );
    }

    #[test]
    fn test_empty_options() {
        let html = r#"<html><body>
            <article><h1>Title</h1><p>Content here.</p></article>
        </body></html>"#;

        let result_extract = extract(html, None).unwrap();
        let result_options =
            extract_with_options(html, None, &ExtractionOptions::default()).unwrap();

        assert_eq!(
            result_extract.content.markdown, result_options.content.markdown,
            "default ExtractionOptions should produce identical results to extract()"
        );
    }

    #[test]
    fn test_raw_html_not_in_json_when_none() {
        let result = extract("<html><body><p>Test</p></body></html>", None).unwrap();
        let json = serde_json::to_string(&result).unwrap();
        assert!(
            !json.contains("raw_html"),
            "raw_html should be absent from JSON when None"
        );
    }

    #[test]
    fn express_live_blog_no_stack_overflow() {
        // Real-world Express.co.uk live blog that previously caused stack overflow
        let html = include_str!("../testdata/express_test.html");
        let result = extract(
            html,
            Some("https://www.express.co.uk/news/world/2189934/iran-live-donald-trump-uae-dubai-kuwait-attacks"),
        );
        assert!(result.is_ok(), "Should not stack overflow on Express.co.uk live blog");
        let result = result.unwrap();
        assert!(
            result.metadata.word_count > 100,
            "Should extract meaningful content, got {} words",
            result.metadata.word_count
        );
    }

    #[test]
    fn deeply_nested_html_no_stack_overflow() {
        // Simulate deeply nested HTML like Express.co.uk live blogs
        let depth = 500;
        let mut html = String::from("<html><body>");
        for _ in 0..depth {
            html.push_str("<div><span>");
        }
        html.push_str("<p>Deep content here</p>");
        for _ in 0..depth {
            html.push_str("</span></div>");
        }
        html.push_str("</body></html>");

        let result = extract(&html, None);
        assert!(result.is_ok(), "Should not stack overflow on deeply nested HTML");
        let result = result.unwrap();
        assert!(
            result.content.markdown.contains("Deep content"),
            "Should extract content from deep nesting"
        );
    }
}
