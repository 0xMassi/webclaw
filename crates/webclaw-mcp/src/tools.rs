/// Tool parameter structs for MCP tool inputs.
/// Each struct derives JsonSchema for automatic schema generation,
/// and Deserialize for parsing from MCP tool call arguments.
use schemars::JsonSchema;
use serde::Deserialize;

// ── Coercion helpers ────────────────────────────────────────────────────────
//
// MCP clients (Claude Desktop, VS Code extension, etc.) sometimes pass numeric
// parameters as JSON strings (e.g. `"depth": "3"` instead of `"depth": 3`).
// serde's default u32/usize deserialisers reject strings with:
//
//   "invalid type: string \"3\", expected u32"
//
// These helpers accept both forms transparently so callers never see that
// error regardless of which representation their client sends. The same
// problem hits booleans: clients send `"true"`/`"false"` as JSON strings,
// which serde's default bool deserialiser rejects — `deser_opt_bool_or_str`
// covers that case.

fn deser_opt_u32_or_str<'de, D>(d: D) -> Result<Option<u32>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(u32),
        Str(String),
    }
    match Option::<NumOrStr>::deserialize(d)? {
        None => Ok(None),
        Some(NumOrStr::Num(n)) => Ok(Some(n)),
        Some(NumOrStr::Str(s)) => {
            s.trim().parse::<u32>().map(Some).map_err(|_| {
                serde::de::Error::custom(format!("expected a u32, got string \"{s}\""))
            })
        }
    }
}

fn deser_opt_usize_or_str<'de, D>(d: D) -> Result<Option<usize>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum NumOrStr {
        Num(usize),
        Str(String),
    }
    match Option::<NumOrStr>::deserialize(d)? {
        None => Ok(None),
        Some(NumOrStr::Num(n)) => Ok(Some(n)),
        Some(NumOrStr::Str(s)) => {
            s.trim().parse::<usize>().map(Some).map_err(|_| {
                serde::de::Error::custom(format!("expected a usize, got string \"{s}\""))
            })
        }
    }
}

fn deser_opt_bool_or_str<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum BoolOrStr {
        Bool(bool),
        Str(String),
    }
    match Option::<BoolOrStr>::deserialize(d)? {
        None => Ok(None),
        Some(BoolOrStr::Bool(b)) => Ok(Some(b)),
        // Accept "true"/"false" case-insensitively (trimmed). Reject anything
        // else with a clear message rather than silently coercing it.
        Some(BoolOrStr::Str(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "true" => Ok(Some(true)),
            "false" => Ok(Some(false)),
            _ => Err(serde::de::Error::custom(format!(
                "expected a bool, got string \"{s}\""
            ))),
        },
    }
}

// ── Parameter structs ───────────────────────────────────────────────────────

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScrapeParams {
    /// URL to scrape
    pub url: String,
    /// Output format: "markdown" (default), "llm", "text", or "json"
    pub format: Option<String>,
    /// CSS selectors to include (only extract matching elements)
    pub include_selectors: Option<Vec<String>>,
    /// CSS selectors to exclude from output
    pub exclude_selectors: Option<Vec<String>>,
    /// If true, extract only the main content (article/main element)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub only_main_content: Option<bool>,
    /// Browser profile: "chrome" (default), "chrome-macos", "firefox", "safari", "safari-ios", "edge", or "random"
    pub browser: Option<String>,
    /// Cookies to send with the request (e.g. ["name=value", "session=abc123"])
    pub cookies: Option<Vec<String>>,
    /// JavaScript rendering in a local Chrome: "auto" (default) renders pages whose content needs it, "always" renders every page that runs scripts, "never" skips it
    pub render: Option<String>,
    /// When rendering, also wait until an element matching this CSS selector exists
    pub wait_for: Option<String>,
    /// When rendering, skip these resources so pages download less: a comma list of "images", "fonts" and "media", or "none"
    pub block: Option<String>,
    /// A question: keep only the parts of the page that answer it, under their headings
    pub query: Option<String>,
    /// Return the page as a JSON array of chunks for retrieval instead, each under about this many tokens, with url, index, heading_path, tokens and markdown text. Paragraphs, lists, tables and code blocks stay whole
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub chunk: Option<usize>,
    /// With `chunk`, start each chunk with up to this many tokens from the end of the one before (at most half the chunk size)
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub chunk_overlap: Option<usize>,
    /// Steps to run in the local browser once the page has loaded, before it is read. The page is always rendered when this is set. Each step is an object with a "type": {"type": "click", "selector": CSS} with optionally "nth": index from 0, "all": true, or "repeat": N to keep clicking a "load more" button until it is gone; {"type": "type", "selector": CSS, "text": "..."}; {"type": "press", "key": "Enter"}; {"type": "scroll"} to the end, optionally with "selector" for a scrollable element, "by": pixels, and "repeat": N to keep scrolling until the page stops growing; {"type": "wait", "ms": 1000}; {"type": "wait_for", "selector": CSS, "timeout_ms": 10000}; {"type": "eval", "script": "JavaScript expression"} whose value is returned after the content. A failing step fails the call and its number is in the error
    pub actions: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SelectParams {
    /// URL to read
    pub url: String,
    /// CSS selector, optionally ending in "::text" (default), "::attr(NAME)" or "::html", e.g. "h3 a::attr(href)". Give exactly one of `selector`, `xpath`, `tables`, `text` and `regex`
    pub selector: Option<String>,
    /// XPath 1.0 expression instead of a CSS selector, e.g. "//h3/a/@title". Elements and text give their text, attributes their value
    pub xpath: Option<String>,
    /// Find the deepest elements whose visible text contains this, any case. Returns [{"selector", "text"}], each selector picking its element alone
    pub text: Option<String>,
    /// Like `text` with a regular expression, e.g. "^£\d+\.\d\d$"
    pub regex: Option<String>,
    /// With `text`, match the whole text instead of a part (default: false)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub exact: Option<bool>,
    /// With `text` or `regex`, match letter case (default: false)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub case_sensitive: Option<bool>,
    /// Use the first element `selector`, `xpath`, `text` or `regex` picks as an example and return every element like it with one CSS selector that picks them all: {"selector", "matches"}, or with `fields` {"selector", "records"} (default: false)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub similar: Option<bool>,
    /// Return one object per match instead: field name to a selector run inside that match, e.g. {"title": "h3 a::attr(title)", "price": ".price_color"}, or with `xpath` an expression relative to the match, e.g. {"title": ".//h3/a/@title"}. With `similar` the fields are CSS
    pub fields: Option<std::collections::BTreeMap<String, String>>,
    /// Return the page's data tables instead, each as {"caption", "headers", "rows"} with rows of cell text; layout tables are skipped
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub tables: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// URL to capture
    pub url: String,
    /// Capture the whole scrollable page instead of the 1280x720 viewport (default: false)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub full_page: Option<bool>,
    /// Image format: "png" (default) or "jpeg", which is smaller
    pub format: Option<String>,
    /// Also wait until an element matching this CSS selector exists before capturing
    pub wait_for: Option<String>,
    /// Steps to run in the local browser once the page has loaded, before it is captured. Each step is an object with a "type": {"type": "click", "selector": CSS} with optionally "nth": index from 0, "all": true, or "repeat": N to keep clicking a "load more" button until it is gone; {"type": "type", "selector": CSS, "text": "..."}; {"type": "press", "key": "Enter"}; {"type": "scroll"} to the end, optionally with "selector" for a scrollable element, "by": pixels, and "repeat": N to keep scrolling until the page stops growing; {"type": "wait", "ms": 1000}; {"type": "wait_for", "selector": CSS, "timeout_ms": 10000}; {"type": "eval", "script": "JavaScript expression"} whose value is returned after the content. A failing step fails the call and its number is in the error
    pub actions: Option<Vec<serde_json::Value>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrawlParams {
    /// Seed URL to start crawling from
    pub url: String,
    /// Maximum link depth to follow (default: 2)
    #[serde(default, deserialize_with = "deser_opt_u32_or_str")]
    pub depth: Option<u32>,
    /// Maximum number of pages to crawl (default: 50)
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub max_pages: Option<usize>,
    /// Number of concurrent requests (default: 5)
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub concurrency: Option<usize>,
    /// Seed the frontier from sitemap discovery before crawling
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub use_sitemap: Option<bool>,
    /// Crawl URLs that robots.txt disallows (default: false)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub ignore_robots: Option<bool>,
    /// Output format for each page: "markdown" (default), "llm", "text"
    pub format: Option<String>,
    /// When rendering, skip these resources so pages download less: a comma list of "images", "fonts" and "media", or "none"
    pub block: Option<String>,
    /// A question, up to 1000 characters: follow the links most likely to answer it first, keep only the parts of each page that answer it, and list pages best match first
    pub query: Option<String>,
    /// Return chunks for retrieval instead: one JSON object with the crawl counts (total, ok, errors, skipped_by_robots), the failed pages with their error, and the chunks of every page, each under about this many tokens with its page url, index, heading_path, tokens and markdown text
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub chunk: Option<usize>,
    /// With `chunk`, start each chunk with up to this many tokens from the end of the one before (at most half the chunk size)
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub chunk_overlap: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MapParams {
    /// Base URL to discover sitemaps from (e.g. `<https://example.com>`)
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BatchParams {
    /// List of URLs to extract content from
    pub urls: Vec<String>,
    /// Output format: "markdown" (default), "llm", "text"
    pub format: Option<String>,
    /// Number of concurrent requests (default: 5)
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub concurrency: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExtractParams {
    /// URL to fetch and extract structured data from
    pub url: String,
    /// Natural language prompt describing what to extract
    pub prompt: Option<String>,
    /// JSON schema describing the structure to extract
    pub schema: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SummarizeParams {
    /// URL to fetch and summarize
    pub url: String,
    /// Number of sentences in the summary (default: 3)
    #[serde(default, deserialize_with = "deser_opt_usize_or_str")]
    pub max_sentences: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DiffParams {
    /// URL to fetch current content from
    pub url: String,
    /// Previous extraction snapshot as a JSON string (ExtractionResult)
    pub previous_snapshot: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BrandParams {
    /// URL to extract brand identity from
    pub url: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResearchParams {
    /// Research query or question to investigate
    pub query: String,
    /// Enable deep research mode for more thorough investigation (default: false)
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub deep: Option<bool>,
    /// Topic hint to guide research focus (e.g. "technology", "finance", "science")
    pub topic: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query
    pub query: String,
    /// Number of results to return (default: 5, max: 10)
    #[serde(default, deserialize_with = "deser_opt_u32_or_str")]
    pub num_results: Option<u32>,
    /// Country code for localization (e.g. "us", "gb", "it").
    /// Only used by the local Serper path (SERPER_API_KEY).
    pub country: Option<String>,
    /// Language code for localization (e.g. "en", "it").
    /// Only used by the local Serper path (SERPER_API_KEY).
    pub lang: Option<String>,
    /// When true, fetch + extract each result page and include its
    /// markdown. Only used by the local Serper path (SERPER_API_KEY).
    #[serde(default, deserialize_with = "deser_opt_bool_or_str")]
    pub scrape: Option<bool>,
}

/// Parameters for `vertical_scrape`: run a site-specific extractor by name.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VerticalParams {
    /// Name of the vertical extractor. Call `list_extractors` to see all
    /// available names. Examples: "reddit", "github_repo", "pypi",
    /// "trustpilot_reviews", "youtube_video", "shopify_product".
    pub name: String,
    /// URL to extract. Must match the URL patterns the extractor claims;
    /// otherwise the tool returns a clear "URL mismatch" error.
    pub url: String,
}

/// `list_extractors` takes no arguments but we still need an empty struct
/// so rmcp can generate a schema and parse the (empty) JSON-RPC params.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListExtractorsParams {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_sizes_accept_numbers_and_numeric_strings() {
        let v: ScrapeParams =
            serde_json::from_str(r#"{"url":"https://x.com","chunk":"256","chunk_overlap":32}"#)
                .unwrap();
        assert_eq!((v.chunk, v.chunk_overlap), (Some(256), Some(32)));
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","chunk":512}"#).unwrap();
        assert_eq!((v.chunk, v.chunk_overlap), (Some(512), None));
    }

    // ── CrawlParams.depth (u32) ──────────────────────────────────────────────

    #[test]
    fn crawl_depth_from_numeric_string() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","depth":"3"}"#).unwrap();
        assert_eq!(v.depth, Some(3));
    }

    #[test]
    fn crawl_depth_from_number() {
        let v: CrawlParams = serde_json::from_str(r#"{"url":"https://x.com","depth":3}"#).unwrap();
        assert_eq!(v.depth, Some(3));
    }

    #[test]
    fn crawl_depth_absent_is_none() {
        let v: CrawlParams = serde_json::from_str(r#"{"url":"https://x.com"}"#).unwrap();
        assert_eq!(v.depth, None);
    }

    #[test]
    fn crawl_depth_non_numeric_string_errors() {
        let e = serde_json::from_str::<CrawlParams>(r#"{"url":"https://x.com","depth":"abc"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ── CrawlParams.max_pages (usize) ────────────────────────────────────────

    #[test]
    fn crawl_max_pages_from_numeric_string() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","max_pages":"50"}"#).unwrap();
        assert_eq!(v.max_pages, Some(50));
    }

    #[test]
    fn crawl_max_pages_from_number() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","max_pages":50}"#).unwrap();
        assert_eq!(v.max_pages, Some(50));
    }

    #[test]
    fn crawl_max_pages_absent_is_none() {
        let v: CrawlParams = serde_json::from_str(r#"{"url":"https://x.com"}"#).unwrap();
        assert_eq!(v.max_pages, None);
    }

    #[test]
    fn crawl_max_pages_non_numeric_string_errors() {
        let e = serde_json::from_str::<CrawlParams>(r#"{"url":"https://x.com","max_pages":"abc"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ── CrawlParams.concurrency (usize) ──────────────────────────────────────

    #[test]
    fn crawl_concurrency_from_numeric_string() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","concurrency":"5"}"#).unwrap();
        assert_eq!(v.concurrency, Some(5));
    }

    #[test]
    fn crawl_concurrency_from_number() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","concurrency":5}"#).unwrap();
        assert_eq!(v.concurrency, Some(5));
    }

    #[test]
    fn crawl_concurrency_absent_is_none() {
        let v: CrawlParams = serde_json::from_str(r#"{"url":"https://x.com"}"#).unwrap();
        assert_eq!(v.concurrency, None);
    }

    #[test]
    fn crawl_concurrency_non_numeric_string_errors() {
        let e =
            serde_json::from_str::<CrawlParams>(r#"{"url":"https://x.com","concurrency":"abc"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ── BatchParams.concurrency (usize) ──────────────────────────────────────

    #[test]
    fn batch_concurrency_from_numeric_string() {
        let v: BatchParams =
            serde_json::from_str(r#"{"urls":["https://x.com"],"concurrency":"5"}"#).unwrap();
        assert_eq!(v.concurrency, Some(5));
    }

    #[test]
    fn batch_concurrency_from_number() {
        let v: BatchParams =
            serde_json::from_str(r#"{"urls":["https://x.com"],"concurrency":5}"#).unwrap();
        assert_eq!(v.concurrency, Some(5));
    }

    #[test]
    fn batch_concurrency_absent_is_none() {
        let v: BatchParams = serde_json::from_str(r#"{"urls":["https://x.com"]}"#).unwrap();
        assert_eq!(v.concurrency, None);
    }

    #[test]
    fn batch_concurrency_non_numeric_string_errors() {
        let e = serde_json::from_str::<BatchParams>(
            r#"{"urls":["https://x.com"],"concurrency":"abc"}"#,
        );
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ── SearchParams.num_results (u32) ───────────────────────────────────────

    #[test]
    fn search_num_results_from_numeric_string() {
        let v: SearchParams =
            serde_json::from_str(r#"{"query":"rust","num_results":"10"}"#).unwrap();
        assert_eq!(v.num_results, Some(10));
    }

    #[test]
    fn search_num_results_from_number() {
        let v: SearchParams = serde_json::from_str(r#"{"query":"rust","num_results":10}"#).unwrap();
        assert_eq!(v.num_results, Some(10));
    }

    #[test]
    fn search_num_results_absent_is_none() {
        let v: SearchParams = serde_json::from_str(r#"{"query":"rust"}"#).unwrap();
        assert_eq!(v.num_results, None);
    }

    #[test]
    fn search_num_results_non_numeric_string_errors() {
        let e = serde_json::from_str::<SearchParams>(r#"{"query":"rust","num_results":"abc"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ── SummarizeParams.max_sentences (usize) ────────────────────────────────

    #[test]
    fn summarize_max_sentences_from_numeric_string() {
        let v: SummarizeParams =
            serde_json::from_str(r#"{"url":"https://x.com","max_sentences":"3"}"#).unwrap();
        assert_eq!(v.max_sentences, Some(3));
    }

    #[test]
    fn summarize_max_sentences_from_number() {
        let v: SummarizeParams =
            serde_json::from_str(r#"{"url":"https://x.com","max_sentences":3}"#).unwrap();
        assert_eq!(v.max_sentences, Some(3));
    }

    #[test]
    fn summarize_max_sentences_absent_is_none() {
        let v: SummarizeParams = serde_json::from_str(r#"{"url":"https://x.com"}"#).unwrap();
        assert_eq!(v.max_sentences, None);
    }

    #[test]
    fn summarize_max_sentences_non_numeric_string_errors() {
        let e = serde_json::from_str::<SummarizeParams>(
            r#"{"url":"https://x.com","max_sentences":"abc"}"#,
        );
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ── Boolean param string-coercion (issue #62) ───────────────────────────

    // ScrapeParams.only_main_content
    #[test]
    fn scrape_only_main_content_from_bool() {
        let v: ScrapeParams =
            serde_json::from_str(r#"{"url":"https://x.com","only_main_content":true}"#).unwrap();
        assert_eq!(v.only_main_content, Some(true));
    }

    #[test]
    fn scrape_only_main_content_from_string() {
        let t: ScrapeParams =
            serde_json::from_str(r#"{"url":"https://x.com","only_main_content":"true"}"#).unwrap();
        assert_eq!(t.only_main_content, Some(true));
        let f: ScrapeParams =
            serde_json::from_str(r#"{"url":"https://x.com","only_main_content":"false"}"#).unwrap();
        assert_eq!(f.only_main_content, Some(false));
    }

    #[test]
    fn scrape_only_main_content_absent_is_none() {
        let v: ScrapeParams = serde_json::from_str(r#"{"url":"https://x.com"}"#).unwrap();
        assert_eq!(v.only_main_content, None);
    }

    #[test]
    fn scrape_only_main_content_non_bool_string_errors() {
        let e = serde_json::from_str::<ScrapeParams>(
            r#"{"url":"https://x.com","only_main_content":"yes"}"#,
        );
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // CrawlParams.use_sitemap
    #[test]
    fn crawl_use_sitemap_from_bool() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","use_sitemap":false}"#).unwrap();
        assert_eq!(v.use_sitemap, Some(false));
    }

    #[test]
    fn crawl_use_sitemap_from_string() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","use_sitemap":"true"}"#).unwrap();
        assert_eq!(v.use_sitemap, Some(true));
    }

    #[test]
    fn crawl_use_sitemap_absent_is_none() {
        let v: CrawlParams = serde_json::from_str(r#"{"url":"https://x.com"}"#).unwrap();
        assert_eq!(v.use_sitemap, None);
    }

    #[test]
    fn crawl_use_sitemap_non_bool_string_errors() {
        let e =
            serde_json::from_str::<CrawlParams>(r#"{"url":"https://x.com","use_sitemap":"nope"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // ResearchParams.deep
    #[test]
    fn research_deep_from_bool() {
        let v: ResearchParams = serde_json::from_str(r#"{"query":"rust","deep":true}"#).unwrap();
        assert_eq!(v.deep, Some(true));
    }

    #[test]
    fn research_deep_from_string() {
        let v: ResearchParams = serde_json::from_str(r#"{"query":"rust","deep":"true"}"#).unwrap();
        assert_eq!(v.deep, Some(true));
    }

    #[test]
    fn research_deep_absent_is_none() {
        let v: ResearchParams = serde_json::from_str(r#"{"query":"rust"}"#).unwrap();
        assert_eq!(v.deep, None);
    }

    #[test]
    fn research_deep_non_bool_string_errors() {
        // Numeric-looking strings are NOT accepted for bools (avoids ambiguity).
        let e = serde_json::from_str::<ResearchParams>(r#"{"query":"rust","deep":"1"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }

    // SearchParams.scrape
    #[test]
    fn search_scrape_from_bool() {
        let v: SearchParams = serde_json::from_str(r#"{"query":"rust","scrape":true}"#).unwrap();
        assert_eq!(v.scrape, Some(true));
    }

    #[test]
    fn search_scrape_from_string_case_insensitive() {
        let v: SearchParams = serde_json::from_str(r#"{"query":"rust","scrape":"True"}"#).unwrap();
        assert_eq!(v.scrape, Some(true));
    }

    #[test]
    fn search_scrape_absent_is_none() {
        let v: SearchParams = serde_json::from_str(r#"{"query":"rust"}"#).unwrap();
        assert_eq!(v.scrape, None);
    }

    #[test]
    fn search_scrape_non_bool_string_errors() {
        let e = serde_json::from_str::<SearchParams>(r#"{"query":"rust","scrape":"maybe"}"#);
        assert!(e.is_err(), "expected Err, got {e:?}");
    }
}
