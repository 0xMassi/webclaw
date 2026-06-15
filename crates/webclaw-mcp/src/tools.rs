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
// These two helpers accept both forms transparently so callers never see that
// error regardless of which representation their client sends.

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
        Some(NumOrStr::Str(s)) => s
            .trim()
            .parse::<u32>()
            .map(Some)
            .map_err(|_| serde::de::Error::custom(format!("expected a u32, got string \"{s}\""))),
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
        Some(NumOrStr::Str(s)) => s
            .trim()
            .parse::<usize>()
            .map(Some)
            .map_err(|_| {
                serde::de::Error::custom(format!("expected a usize, got string \"{s}\""))
            }),
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
    pub only_main_content: Option<bool>,
    /// Browser profile: "chrome" (default), "firefox", or "random"
    pub browser: Option<String>,
    /// Cookies to send with the request (e.g. ["name=value", "session=abc123"])
    pub cookies: Option<Vec<String>>,
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
    pub use_sitemap: Option<bool>,
    /// Output format for each page: "markdown" (default), "llm", "text"
    pub format: Option<String>,
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
    pub deep: Option<bool>,
    /// Topic hint to guide research focus (e.g. "technology", "finance", "science")
    pub topic: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query
    pub query: String,
    /// Number of results to return (default: 10)
    #[serde(default, deserialize_with = "deser_opt_u32_or_str")]
    pub num_results: Option<u32>,
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

    // ── CrawlParams.depth (u32) ──────────────────────────────────────────────

    #[test]
    fn crawl_depth_from_numeric_string() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","depth":"3"}"#).unwrap();
        assert_eq!(v.depth, Some(3));
    }

    #[test]
    fn crawl_depth_from_number() {
        let v: CrawlParams =
            serde_json::from_str(r#"{"url":"https://x.com","depth":3}"#).unwrap();
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
        let e =
            serde_json::from_str::<CrawlParams>(r#"{"url":"https://x.com","max_pages":"abc"}"#);
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
        let e = serde_json::from_str::<CrawlParams>(
            r#"{"url":"https://x.com","concurrency":"abc"}"#,
        );
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
        let v: SearchParams =
            serde_json::from_str(r#"{"query":"rust","num_results":10}"#).unwrap();
        assert_eq!(v.num_results, Some(10));
    }

    #[test]
    fn search_num_results_absent_is_none() {
        let v: SearchParams = serde_json::from_str(r#"{"query":"rust"}"#).unwrap();
        assert_eq!(v.num_results, None);
    }

    #[test]
    fn search_num_results_non_numeric_string_errors() {
        let e =
            serde_json::from_str::<SearchParams>(r#"{"query":"rust","num_results":"abc"}"#);
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
}
