pub mod brand;
pub(crate) mod data_island;
/// webclaw-core: Pure HTML content extraction engine for LLMs.
///
/// Takes raw HTML + optional URL, returns structured content
/// (metadata, markdown, plain text, links, images, code blocks).
/// Zero network dependencies — WASM-compatible by design.
pub mod diff;
pub mod domain;
pub mod endpoints;
pub mod error;
pub mod extractor;
#[cfg(all(feature = "quickjs", not(target_arch = "wasm32")))]
pub mod js_eval;
pub mod jsonld;
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
pub use jsonld::{
    classify_all as classify_jsonld_all, classify_value as classify_jsonld_value, primary_schema,
    ArticleRef, JsonLdSchema, LiveUpdate,
};
pub use llm::{
    body_word_count, classify_hub, classify_thin_body, collect_section_links, to_json_sections,
    to_json_summary, to_json_toc, to_llm_sections, to_llm_summary, to_llm_text,
    to_llm_text_with_options, to_llm_toc, truncate_json_with_wrapper, truncate_with_footer,
    HubClassification, LlmTextOptions, ThinBodyClassification,
};
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
/// On native targets, spawns extraction on a thread with an 8 MB stack to
/// handle deeply nested HTML (e.g., Express.co.uk live blogs) without
/// overflowing the default 1-2 MB main-thread stack on Windows.
///
/// On `wasm32`, threads are unavailable (`std::thread::spawn` panics at
/// runtime), so extraction runs inline on the caller's stack.
#[cfg(not(target_arch = "wasm32"))]
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

/// WASM has no threads; run extraction directly on the caller's stack.
#[cfg(target_arch = "wasm32")]
pub fn extract_with_options(
    html: &str,
    url: Option<&str>,
    options: &ExtractionOptions,
) -> Result<ExtractionResult, ExtractError> {
    extract_with_options_inner(html, url, options)
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
        // M12: YouTube fast path emits structured video metadata only
        // (title, channel, view count, description). No chrome / nav /
        // ads in the output — all words are "article" by definition.
        meta.word_count_article = meta.word_count;
        meta.word_count_chrome = 0;

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
    #[cfg(all(feature = "quickjs", not(target_arch = "wasm32")))]
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

    // M12 (issue #7): split the total word_count into an article-body
    // portion and a chrome portion. Computed once, here, AFTER all the
    // word_count update paths above (data island, QuickJS, retry strategies)
    // have settled. Sourced from JSON-LD articleBody/reviewBody when
    // present, else the M2-style body word count on the extracted markdown.
    let (article_wc, chrome_wc) =
        compute_word_count_breakdown(&content.markdown, &structured_data, meta.word_count);
    meta.word_count_article = article_wc;
    meta.word_count_chrome = chrome_wc;

    Ok(ExtractionResult {
        metadata: meta,
        content,
        domain_data,
        structured_data,
    })
}

/// M12 helper: split a page's total word_count into an article-body portion
/// and a chrome remainder.
///
/// Precedence:
/// 1. JSON-LD `articleBody` (NewsArticle) or `reviewBody` (Review) via
///    [`crate::jsonld::primary_schema`]. When present, the article portion
///    is the word count of that string.
/// 2. Fallback: [`llm::body_word_count`] on the extracted markdown — M2's
///    "words outside markdown link patterns" estimator (same pipeline
///    `hub_detect::count_body_words` uses for hub classification).
///
/// Invariant: returns `(article, chrome)` such that `article + chrome ==
/// total_wc`. `article` is clamped to `total_wc` if the JSON-LD body has
/// more words than the extracted markdown (tokenization differences are
/// expected — the breakdown is a best-effort split, not a perfect
/// partition). When `total_wc == 0`, returns `(0, 0)` so the
/// `skip_serializing_if = "is_zero_usize"` guard on the Metadata fields
/// drops them from JSON output.
fn compute_word_count_breakdown(
    markdown: &str,
    structured_data: &[serde_json::Value],
    total_wc: usize,
) -> (usize, usize) {
    if total_wc == 0 {
        return (0, 0);
    }

    // 1. JSON-LD articleBody / reviewBody — ground truth when present.
    let schemas = jsonld::classify_all(structured_data);
    let jsonld_body: Option<&str> = jsonld::primary_schema(&schemas).and_then(|s| match s {
        jsonld::JsonLdSchema::NewsArticle { body: Some(b), .. } => Some(b.as_str()),
        jsonld::JsonLdSchema::Review {
            review_body: Some(b),
            ..
        } => Some(b.as_str()),
        _ => None,
    });

    let article_raw = if let Some(body_str) = jsonld_body {
        extractor::word_count(body_str)
    } else {
        // 2. Fallback: M2-style body word count on extracted markdown.
        llm::body_word_count(markdown)
    };

    // Clamp so article + chrome == total_wc (invariant for the JSON shape
    // and the header arithmetic). Tokenization mismatches (JSON-LD body
    // vs extractor::word_count) can make article_raw > total_wc; that's
    // not a bug, it's a representation gap — clamp and move on.
    let article = article_raw.min(total_wc);
    let chrome = total_wc - article;
    (article, chrome)
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
            Some(
                "https://www.express.co.uk/news/world/2189934/iran-live-donald-trump-uae-dubai-kuwait-attacks",
            ),
        );
        assert!(
            result.is_ok(),
            "Should not stack overflow on Express.co.uk live blog"
        );
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
        assert!(
            result.is_ok(),
            "Should not stack overflow on deeply nested HTML"
        );
        let result = result.unwrap();
        assert!(
            result.content.markdown.contains("Deep content"),
            "Should extract content from deep nesting"
        );
    }

    #[test]
    fn wasm_direct_call_path_extracts_content() {
        // On wasm32 `extract_with_options` runs `extract_with_options_inner`
        // inline (no thread spawn). Exercise that exact entry point here so
        // the WASM path stays covered on native CI, and assert it produces
        // the same content as the public threaded entry point.
        let html = r#"
        <html lang="en">
        <head><title>WASM Path</title></head>
        <body><article><h1>Heading</h1><p>WASM-safe extraction body content.</p></article></body>
        </html>"#;
        let opts = ExtractionOptions::default();

        let inner = extract_with_options_inner(html, Some("https://example.com"), &opts)
            .expect("inner extraction (wasm path) should succeed");
        assert!(
            inner
                .content
                .markdown
                .contains("WASM-safe extraction body content"),
            "wasm direct-call path should extract body, got: {}",
            inner.content.markdown
        );

        let threaded = extract_with_options(html, Some("https://example.com"), &opts)
            .expect("threaded extraction should succeed");
        assert_eq!(
            inner.content.markdown, threaded.content.markdown,
            "wasm path and threaded path must produce identical content"
        );
    }

    // -----------------------------------------------------------------
    // M12 (issue #7): word-count breakdown — article vs chrome split.
    // Tests the POPULATION logic in `extract_with_options_inner` /
    // `compute_word_count_breakdown`. Formatter behavior is tested in
    // `crate::llm::metadata::m12_tests`.
    // -----------------------------------------------------------------

    /// M12 test 1: a page with a JSON-LD `NewsArticle.articleBody` gets
    /// the article portion sourced from the articleBody string. Chrome
    /// is the remainder. Total invariant: article + chrome == word_count.
    #[test]
    fn test_word_count_breakdown_with_jsonld_article_body() {
        // 20-word articleBody. Wrap in a <p> + nav chrome so the
        // extracted markdown has both article words AND chrome words.
        let html = r#"
        <html lang="en">
        <head>
            <title>Tariffs hit consumers</title>
            <script type="application/ld+json">
            {
              "@context": "https://schema.org",
              "@type": "NewsArticle",
              "headline": "Tariffs hit consumers",
              "articleBody": "Tariffs are taxes on imports paid by consumers in the importing country, not the exporting one, economists explained today again."
            }
            </script>
        </head>
        <body>
            <nav><a href="/">Home</a> | <a href="/markets">Markets</a> | <a href="/world">World</a></nav>
            <article>
                <h1>Tariffs hit consumers</h1>
                <p>Tariffs are taxes on imports paid by consumers in the importing country, not the exporting one, economists explained today again.</p>
            </article>
            <footer>Subscribe to our newsletter for daily updates and breaking-news alerts</footer>
        </body>
        </html>"#;

        let result = extract(html, Some("https://news.example.com/tariffs")).unwrap();
        let m = &result.metadata;

        assert!(m.word_count > 0, "extraction must produce a word count");
        // articleBody is exactly 20 words. The extracted markdown may
        // include more or fewer words depending on what the scorer
        // captured; the invariant we assert is structural, not numeric.
        assert_eq!(
            m.word_count_article + m.word_count_chrome,
            m.word_count,
            "invariant: article + chrome == total. \
             got article={}, chrome={}, total={}",
            m.word_count_article,
            m.word_count_chrome,
            m.word_count
        );
        assert!(
            m.word_count_article > 0,
            "JSON-LD articleBody must populate article portion (>0); \
             got article={}",
            m.word_count_article
        );
    }

    /// M12 test 2: when no JSON-LD body is present, the article portion
    /// falls back to the M2-style body heuristic (`llm::body_word_count`
    /// on extracted markdown). Chrome is the remainder. The article
    /// portion must still be >0 on a real body page; total invariant holds.
    #[test]
    fn test_word_count_breakdown_without_jsonld_falls_back_to_heuristic() {
        // No <script type="application/ld+json"> block — the breakdown
        // must come from the body::process_body fallback.
        let html = r#"
        <html lang="en">
        <head><title>Plain article</title></head>
        <body>
            <article>
                <h1>Plain article</h1>
                <p>The economy expanded last quarter at an annualized rate of three percent
                   driven primarily by consumer spending and a rebound in fixed investment,
                   government statisticians reported on Thursday morning at the usual hour.</p>
                <p>Analysts had broadly expected the print, but the composition of the gain
                   surprised some who had bet that residential housing would drag the headline
                   number into the low twos rather than the comfortable threes.</p>
            </article>
        </body>
        </html>"#;

        let result = extract(html, Some("https://news.example.com/gdp")).unwrap();
        let m = &result.metadata;

        assert!(m.word_count > 0, "extraction must produce a word count");
        assert_eq!(
            m.word_count_article + m.word_count_chrome,
            m.word_count,
            "invariant: article + chrome == total. \
             got article={}, chrome={}, total={}",
            m.word_count_article,
            m.word_count_chrome,
            m.word_count
        );
        assert!(
            m.word_count_article > 0,
            "fallback body heuristic must populate article portion (>0); \
             got article={}",
            m.word_count_article
        );
        // Sanity: structured_data should be empty (no JSON-LD in fixture).
        assert!(
            result.structured_data.is_empty()
                || crate::jsonld::classify_all(&result.structured_data)
                    .iter()
                    .all(|s| !matches!(
                        s,
                        crate::jsonld::JsonLdSchema::NewsArticle { body: Some(_), .. }
                            | crate::jsonld::JsonLdSchema::Review { review_body: Some(_), .. }
                    )),
            "fixture should have no JSON-LD article/review body — \
             this test exercises the fallback path"
        );
    }

    /// M12 test 3: JSON output shape gains `word_count_article` and
    /// `word_count_chrome` fields when populated. The existing
    /// `word_count` field is preserved. The three numbers satisfy
    /// article + chrome == total.
    #[test]
    fn test_word_count_breakdown_json_format_has_three_fields() {
        let html = r#"
        <html lang="en">
        <head><title>JSON shape test</title></head>
        <body>
            <article>
                <h1>JSON shape</h1>
                <p>The body of this article has more than ten words so the
                   fallback heuristic populates a positive article portion.
                   The remaining chrome words come from any nav and footer.</p>
            </article>
        </body>
        </html>"#;

        let result = extract(html, Some("https://example.com/json")).unwrap();
        let json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&result).unwrap()).unwrap();
        let meta = &json["metadata"];

        // Existing field preserved.
        assert!(
            meta.get("word_count").is_some(),
            "json must keep word_count field; got: {meta}"
        );
        // New fields present (because population logic ran and produced
        // non-zero values — `skip_serializing_if = is_zero_usize` would
        // drop them if both were 0).
        let total = meta["word_count"].as_u64().unwrap();
        let article = meta["word_count_article"].as_u64().unwrap_or(0);
        let chrome = meta["word_count_chrome"].as_u64().unwrap_or(0);
        assert_eq!(
            article + chrome,
            total,
            "invariant: article + chrome == word_count in JSON output. \
             got article={article}, chrome={chrome}, total={total}; meta={meta}"
        );
        assert!(
            article > 0 || total == 0,
            "expect at least some article words when total > 0; \
             got article={article}, total={total}"
        );
    }

    /// M12 test 4: --mode summary / toc / sections do NOT call into
    /// `build_metadata_header`, so the breakdown line never appears in
    /// those modes. This pins the modes' contract (link-list outputs
    /// stay clean of metadata noise — see iter-5 / iter-7 carry-forward).
    #[test]
    fn test_word_count_omitted_or_simple_in_summary_mode() {
        let html = r#"
        <html lang="en">
        <head><title>Hub-style page</title></head>
        <body>
            <nav>
                <a href="/a">First section</a>
                <a href="/b">Second section</a>
                <a href="/c">Third section</a>
            </nav>
            <article>
                <p>Short body for hub-style page; the summary mode emits a link list, not a metadata header.</p>
            </article>
        </body>
        </html>"#;

        let result = extract(html, Some("https://example.com/hub")).unwrap();
        let summary = crate::to_llm_summary(&result, Some("https://example.com/hub"));
        let toc = crate::to_llm_toc(&result, Some("https://example.com/hub"));
        let sections = crate::to_llm_sections(&result, Some("https://example.com/hub"));

        for (name, output) in [("summary", &summary), ("toc", &toc), ("sections", &sections)] {
            assert!(
                !output.contains("(article:"),
                "{name} mode must NOT contain the article/chrome breakdown; \
                 got: {output}"
            );
            // toc/summary/sections may or may not have a "Word count:" line
            // depending on their own header conventions, but it must NOT
            // carry the M12 parenthetical when it exists.
        }
    }
}
