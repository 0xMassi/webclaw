/// Tool parameter structs for MCP tool inputs.
/// Each struct derives JsonSchema for automatic schema generation,
/// and Deserialize for parsing from MCP tool call arguments.
use schemars::JsonSchema;
use serde::Deserialize;

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
    pub depth: Option<u32>,
    /// Maximum number of pages to crawl (default: 50)
    pub max_pages: Option<usize>,
    /// Number of concurrent requests (default: 5)
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
    pub num_results: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct CaptureNetworkParams {
    /// URL to open in Chromium and capture network traffic from.
    pub url: String,
    /// Optional natural-language purpose for the capture.
    pub intent: Option<String>,
    /// Milliseconds to wait after navigation while collecting network events.
    pub wait_ms: Option<u64>,
    /// Run the browser in headed mode for debugging.
    pub headed: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct DiscoverEndpointsParams {
    /// Saved capture id, for example `example.com/2026-05-16T12-00-00Z`.
    pub capture_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct ShowEndpointParams {
    /// Learned endpoint id to load from saved captures.
    pub endpoint_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct ReplayEndpointParams {
    /// Learned endpoint id to replay or preview.
    pub endpoint_id: String,
    /// Path/query parameter values to substitute into the learned endpoint.
    pub params_json: Option<serde_json::Value>,
    /// Preview the replay request without sending network traffic.
    pub dry_run: Option<bool>,
    /// Allow mutating methods such as POST, PUT, PATCH, and DELETE to execute.
    pub confirm_unsafe: Option<bool>,
    /// Additional non-secret request headers to include in the replay.
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    /// JSON request body override for replay.
    pub body_json: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct ExportOpenApiParams {
    /// Saved capture id whose learned endpoints should be exported.
    pub capture_id: String,
}

/// `list_captures` takes no arguments but uses a struct so rmcp can generate
/// a schema and parse the empty JSON-RPC params.
#[derive(Debug, Deserialize, JsonSchema)]
#[allow(dead_code)]
pub struct ListCapturesParams {}

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
