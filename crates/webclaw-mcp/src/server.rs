/// MCP server implementation for webclaw.
/// Exposes web extraction capabilities as tools for AI agents.
///
/// Uses a local-first architecture: fetches pages directly, then falls back
/// to the webclaw cloud API (api.webclaw.io) when bot protection or
/// JS rendering is detected. Set WEBCLAW_API_KEY for automatic fallback.
use std::fs;
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde_json::{Map, Value, json};
use tracing::{error, info, warn};

use webclaw_capture::cdp::{CaptureOptions, capture_network as run_network_capture};
use webclaw_capture::openapi::write_openapi;
use webclaw_capture::replay::replay_endpoint as run_endpoint_replay;
use webclaw_capture::store::{capture_root, find_endpoint, load_endpoints};
use webclaw_capture::types::{EndpointDefinition, HeaderMap, ReplayOptions};
use webclaw_fetch::cloud::{self, CloudClient, SmartFetchResult};

use crate::tools::*;

pub struct WebclawMcp {
    /// Holds the registered MCP tools. `rmcp >= 1.3` reads this through a
    /// derived trait impl (not by name), so rustc's dead-code lint can't
    /// see the usage and fires a spurious `field tool_router is never
    /// read` on `cargo install`. The field is essential — dropping it
    /// would unregister every tool. See issue #30.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    fetch_client: Arc<webclaw_fetch::FetchClient>,
    /// Lazily-initialized Firefox client, reused across all tool calls that
    /// request the Firefox profile with no cookies. Building a FetchClient
    /// allocates a reqwest pool + TLS state, so tools that repeatedly hit
    /// the same profile used to pay that cost per call.
    ///
    /// We don't cache the default Chrome client (already lives at
    /// `fetch_client`), we don't cache Random (by design rotates per call),
    /// and we don't cache cookie-bearing clients (the cookie header is
    /// part of the shape and varies per call). So a single OnceLock for
    /// Firefox covers the one real cacheable case without a full LRU.
    firefox_client: OnceLock<Arc<webclaw_fetch::FetchClient>>,
    llm_chain: Option<webclaw_llm::ProviderChain>,
    cloud: Option<CloudClient>,
}

/// Parse a browser string into a BrowserProfile.
fn parse_browser(browser: Option<&str>) -> webclaw_fetch::BrowserProfile {
    match browser {
        Some("firefox") => webclaw_fetch::BrowserProfile::Firefox,
        Some("random") => webclaw_fetch::BrowserProfile::Random,
        _ => webclaw_fetch::BrowserProfile::Chrome,
    }
}

/// Validate that a URL is public HTTP(S), matching the fetch-layer SSRF guard.
async fn validate_url(url: &str) -> Result<(), String> {
    webclaw_fetch::url_security::validate_public_http_url(url)
        .await
        .map(|_| ())
        .map_err(|e| format!("Invalid URL: {e}"))
}

/// Timeout for local fetch calls (prevents hanging on tarpitting servers).
const LOCAL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum poll iterations for research jobs (~10 minutes at 3s intervals).
const RESEARCH_MAX_POLLS: u32 = 200;

#[tool_router]
impl WebclawMcp {
    pub async fn new() -> Self {
        let mut config = webclaw_fetch::FetchConfig::default();

        // Load proxy config from env vars or local file
        if let Ok(proxy) = std::env::var("WEBCLAW_PROXY") {
            info!("using single proxy from WEBCLAW_PROXY");
            config.proxy = Some(proxy);
        }

        let proxy_file = std::env::var("WEBCLAW_PROXY_FILE")
            .ok()
            .unwrap_or_else(|| "proxies.txt".to_string());
        if std::path::Path::new(&proxy_file).exists()
            && let Ok(pool) = webclaw_fetch::parse_proxy_file(&proxy_file)
            && !pool.is_empty()
        {
            info!(count = pool.len(), file = %proxy_file, "loaded proxy pool");
            config.proxy_pool = pool;
        }

        let fetch_client = match webclaw_fetch::FetchClient::new(config) {
            Ok(client) => client,
            Err(e) => {
                error!("failed to build FetchClient: {e}");
                std::process::exit(1);
            }
        };

        let chain = webclaw_llm::ProviderChain::default().await;
        let llm_chain = if chain.is_empty() {
            warn!("no LLM providers available -- extract/summarize tools will fail");
            None
        } else {
            info!(providers = chain.len(), "LLM provider chain ready");
            Some(chain)
        };

        let cloud = CloudClient::from_env();
        if cloud.is_some() {
            info!("cloud API fallback enabled (WEBCLAW_API_KEY set)");
        } else {
            warn!(
                "WEBCLAW_API_KEY not set -- bot-protected sites will return challenge pages. \
                 Get a key at https://webclaw.io"
            );
        }

        Self {
            tool_router: Self::tool_router(),
            fetch_client: Arc::new(fetch_client),
            firefox_client: OnceLock::new(),
            llm_chain,
            cloud,
        }
    }

    /// Return the cached Firefox client, building it on first use. Any
    /// subsequent call with the same profile (and no cookies) reuses the
    /// pool instead of allocating a fresh reqwest + TLS stack.
    fn firefox_or_build(&self) -> Result<Arc<webclaw_fetch::FetchClient>, String> {
        if let Some(c) = self.firefox_client.get() {
            return Ok(c.clone());
        }
        let config = webclaw_fetch::FetchConfig {
            browser: webclaw_fetch::BrowserProfile::Firefox,
            ..Default::default()
        };
        let client = Arc::new(
            webclaw_fetch::FetchClient::new(config)
                .map_err(|e| format!("Failed to build firefox client: {e}"))?,
        );
        // If a concurrent call raced us here, OnceLock keeps the first
        // winner and drops ours — that's fine, the ref-count just cleans up.
        let _ = self.firefox_client.set(client.clone());
        Ok(self.firefox_client.get().cloned().unwrap_or(client))
    }

    /// Helper: smart fetch with LLM format for extract/summarize tools.
    async fn smart_fetch_llm(&self, url: &str) -> Result<SmartFetchResult, String> {
        cloud::smart_fetch(
            &self.fetch_client,
            self.cloud.as_ref(),
            url,
            &[],
            &[],
            false,
            &["llm", "markdown"],
        )
        .await
    }

    /// Scrape a single URL and extract its content as markdown, LLM-optimized text, plain text, or full JSON.
    /// Automatically falls back to the webclaw cloud API when bot protection or JS rendering is detected.
    #[tool]
    async fn scrape(&self, Parameters(params): Parameters<ScrapeParams>) -> Result<String, String> {
        validate_url(&params.url).await?;
        let format = params.format.as_deref().unwrap_or("markdown");
        let browser = parse_browser(params.browser.as_deref());
        let include = params.include_selectors.unwrap_or_default();
        let exclude = params.exclude_selectors.unwrap_or_default();
        let main_only = params.only_main_content.unwrap_or(false);

        // Build cookie header from params
        let cookie_header = params
            .cookies
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| c.join("; "));

        // Use a custom client if non-default browser or cookies are provided
        // Pick the cheapest route to a FetchClient for this request:
        //   1. Default Chrome + no cookies → reuse the long-lived self.fetch_client
        //   2. Firefox + no cookies        → reuse the lazily-cached self.firefox_client
        //   3. Anything with cookies, or Random                       → build ad-hoc
        let is_default_browser = matches!(browser, webclaw_fetch::BrowserProfile::Chrome);
        let custom_client;
        let cached_firefox;
        let client: &webclaw_fetch::FetchClient = if cookie_header.is_none() && is_default_browser {
            &self.fetch_client
        } else if cookie_header.is_none()
            && matches!(browser, webclaw_fetch::BrowserProfile::Firefox)
        {
            cached_firefox = self.firefox_or_build()?;
            // cached_firefox lives to end of function; auto-deref gives
            // us &FetchClient here.
            &cached_firefox
        } else {
            let mut headers = std::collections::HashMap::new();
            headers.insert("Accept-Language".to_string(), "en-US,en;q=0.9".to_string());
            if let Some(ref cookies) = cookie_header {
                headers.insert("Cookie".to_string(), cookies.clone());
            }
            let config = webclaw_fetch::FetchConfig {
                browser,
                headers,
                ..Default::default()
            };
            custom_client = webclaw_fetch::FetchClient::new(config)
                .map_err(|e| format!("Failed to build client: {e}"))?;
            &custom_client
        };

        let formats = [format];
        let result = cloud::smart_fetch(
            client,
            self.cloud.as_ref(),
            &params.url,
            &include,
            &exclude,
            main_only,
            &formats,
        )
        .await?;

        match result {
            SmartFetchResult::Local(extraction) => {
                let output = match format {
                    "llm" => webclaw_core::to_llm_text(&extraction, Some(&params.url)),
                    "text" => extraction.content.plain_text,
                    "json" => serde_json::to_string_pretty(&extraction).unwrap_or_default(),
                    _ => extraction.content.markdown,
                };
                Ok(output)
            }
            SmartFetchResult::Cloud(resp) => {
                // Extract the requested format from the API response
                let content = resp
                    .get(format)
                    .or_else(|| resp.get("markdown"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if content.is_empty() {
                    // Return full JSON if no content in the expected format
                    Ok(serde_json::to_string_pretty(&resp).unwrap_or_default())
                } else {
                    Ok(content.to_string())
                }
            }
        }
    }

    /// Crawl a website starting from a seed URL, following links breadth-first up to a configurable depth and page limit.
    #[tool]
    async fn crawl(&self, Parameters(params): Parameters<CrawlParams>) -> Result<String, String> {
        validate_url(&params.url).await?;

        if let Some(max) = params.max_pages
            && max > 500
        {
            return Err("max_pages cannot exceed 500".into());
        }

        let format = params.format.as_deref().unwrap_or("markdown");

        let config = webclaw_fetch::CrawlConfig {
            max_depth: params.depth.unwrap_or(2) as usize,
            max_pages: params.max_pages.unwrap_or(50),
            concurrency: params.concurrency.unwrap_or(5),
            use_sitemap: params.use_sitemap.unwrap_or(false),
            ..Default::default()
        };

        let crawler = webclaw_fetch::Crawler::new(&params.url, config)
            .map_err(|e| format!("Crawler init failed: {e}"))?;

        let result = crawler.crawl(&params.url, None).await;

        let mut output = format!(
            "Crawled {} pages ({} ok, {} errors) in {:.1}s\n\n",
            result.total, result.ok, result.errors, result.elapsed_secs
        );

        for page in &result.pages {
            output.push_str(&format!("--- {} (depth {}) ---\n", page.url, page.depth));
            if let Some(ref extraction) = page.extraction {
                let content = match format {
                    "llm" => webclaw_core::to_llm_text(extraction, Some(&page.url)),
                    "text" => extraction.content.plain_text.clone(),
                    _ => extraction.content.markdown.clone(),
                };
                output.push_str(&content);
            } else if let Some(ref err) = page.error {
                output.push_str(&format!("Error: {err}"));
            }
            output.push_str("\n\n");
        }

        Ok(output)
    }

    /// Discover URLs from a website's sitemaps (robots.txt + sitemap.xml).
    #[tool]
    async fn map(&self, Parameters(params): Parameters<MapParams>) -> Result<String, String> {
        validate_url(&params.url).await?;
        let entries = webclaw_fetch::sitemap::discover(&self.fetch_client, &params.url)
            .await
            .map_err(|e| format!("Sitemap discovery failed: {e}"))?;

        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        Ok(format!(
            "Discovered {} URLs:\n\n{}",
            urls.len(),
            urls.join("\n")
        ))
    }

    /// Extract content from multiple URLs concurrently.
    #[tool]
    async fn batch(&self, Parameters(params): Parameters<BatchParams>) -> Result<String, String> {
        if params.urls.is_empty() {
            return Err("urls must not be empty".into());
        }
        if params.urls.len() > 100 {
            return Err("batch is limited to 100 URLs per request".into());
        }
        for u in &params.urls {
            validate_url(u).await?;
        }

        let format = params.format.as_deref().unwrap_or("markdown");
        let concurrency = params.concurrency.unwrap_or(5);
        let url_refs: Vec<&str> = params.urls.iter().map(String::as_str).collect();

        let results = self
            .fetch_client
            .fetch_and_extract_batch(&url_refs, concurrency)
            .await;

        let mut output = format!("Extracted {} URLs:\n\n", results.len());

        for r in &results {
            output.push_str(&format!("--- {} ---\n", r.url));
            match &r.result {
                Ok(extraction) => {
                    let content = match format {
                        "llm" => webclaw_core::to_llm_text(extraction, Some(&r.url)),
                        "text" => extraction.content.plain_text.clone(),
                        _ => extraction.content.markdown.clone(),
                    };
                    output.push_str(&content);
                }
                Err(e) => {
                    output.push_str(&format!("Error: {e}"));
                }
            }
            output.push_str("\n\n");
        }

        Ok(output)
    }

    /// Extract structured data from a web page using an LLM. Provide either a JSON schema or a natural language prompt.
    /// Falls back to the webclaw cloud API when no local LLM is available or bot protection is detected.
    #[tool]
    async fn extract(
        &self,
        Parameters(params): Parameters<ExtractParams>,
    ) -> Result<String, String> {
        validate_url(&params.url).await?;

        if params.schema.is_none() && params.prompt.is_none() {
            return Err("Either 'schema' or 'prompt' is required for extraction.".into());
        }

        // No local LLM — fall back to cloud API directly
        if self.llm_chain.is_none() {
            let cloud = self.cloud.as_ref().ok_or(
                "No LLM providers available. Set OPENAI_API_KEY, ANTHROPIC_API_KEY, or WEBCLAW_API_KEY for cloud fallback.",
            )?;
            let mut body = json!({"url": params.url});
            if let Some(ref schema) = params.schema {
                body["schema"] = json!(schema);
            }
            if let Some(ref prompt) = params.prompt {
                body["prompt"] = json!(prompt);
            }
            let resp = cloud.post("extract", body).await?;
            return Ok(serde_json::to_string_pretty(&resp).unwrap_or_default());
        }

        let chain = self.llm_chain.as_ref().unwrap();

        let llm_content = match self.smart_fetch_llm(&params.url).await? {
            SmartFetchResult::Local(extraction) => {
                webclaw_core::to_llm_text(&extraction, Some(&params.url))
            }
            SmartFetchResult::Cloud(resp) => resp
                .get("llm")
                .or_else(|| resp.get("markdown"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };

        let data = if let Some(ref schema) = params.schema {
            webclaw_llm::extract::extract_json(&llm_content, schema, chain, None)
                .await
                .map_err(|e| format!("LLM extraction failed: {e}"))?
        } else {
            let prompt = params.prompt.as_deref().unwrap();
            webclaw_llm::extract::extract_with_prompt(&llm_content, prompt, chain, None)
                .await
                .map_err(|e| format!("LLM extraction failed: {e}"))?
        };

        Ok(serde_json::to_string_pretty(&data).unwrap_or_default())
    }

    /// Summarize the content of a web page using an LLM.
    /// Falls back to the webclaw cloud API when no local LLM is available or bot protection is detected.
    #[tool]
    async fn summarize(
        &self,
        Parameters(params): Parameters<SummarizeParams>,
    ) -> Result<String, String> {
        validate_url(&params.url).await?;

        // No local LLM — fall back to cloud API directly
        if self.llm_chain.is_none() {
            let cloud = self.cloud.as_ref().ok_or(
                "No LLM providers available. Set OPENAI_API_KEY, ANTHROPIC_API_KEY, or WEBCLAW_API_KEY for cloud fallback.",
            )?;
            let mut body = json!({"url": params.url});
            if let Some(sentences) = params.max_sentences {
                body["max_sentences"] = json!(sentences);
            }
            let resp = cloud.post("summarize", body).await?;
            let summary = resp.get("summary").and_then(|v| v.as_str()).unwrap_or("");
            if summary.is_empty() {
                return Ok(serde_json::to_string_pretty(&resp).unwrap_or_default());
            }
            return Ok(summary.to_string());
        }

        let chain = self.llm_chain.as_ref().unwrap();

        let llm_content = match self.smart_fetch_llm(&params.url).await? {
            SmartFetchResult::Local(extraction) => {
                webclaw_core::to_llm_text(&extraction, Some(&params.url))
            }
            SmartFetchResult::Cloud(resp) => resp
                .get("llm")
                .or_else(|| resp.get("markdown"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string(),
        };

        webclaw_llm::summarize::summarize(&llm_content, params.max_sentences, chain, None)
            .await
            .map_err(|e| format!("Summarization failed: {e}"))
    }

    /// Compare the current content of a URL against a previous extraction snapshot, showing what changed.
    /// Automatically falls back to the webclaw cloud API when bot protection is detected.
    #[tool]
    async fn diff(&self, Parameters(params): Parameters<DiffParams>) -> Result<String, String> {
        validate_url(&params.url).await?;
        let previous: webclaw_core::ExtractionResult =
            serde_json::from_str(&params.previous_snapshot)
                .map_err(|e| format!("Failed to parse previous_snapshot JSON: {e}"))?;

        let result = cloud::smart_fetch(
            &self.fetch_client,
            self.cloud.as_ref(),
            &params.url,
            &[],
            &[],
            false,
            &["markdown"],
        )
        .await?;

        match result {
            SmartFetchResult::Local(current) => {
                let content_diff = webclaw_core::diff::diff(&previous, &current);
                Ok(serde_json::to_string_pretty(&content_diff).unwrap_or_default())
            }
            SmartFetchResult::Cloud(resp) => {
                // Extract markdown from the cloud response and build a minimal
                // ExtractionResult so we can compute the diff locally.
                let markdown = resp.get("markdown").and_then(|v| v.as_str()).unwrap_or("");

                if markdown.is_empty() {
                    return Err(
                        "Cloud API fallback returned no markdown content; cannot compute diff."
                            .into(),
                    );
                }

                let current = webclaw_core::ExtractionResult {
                    content: webclaw_core::Content {
                        markdown: markdown.to_string(),
                        plain_text: markdown.to_string(),
                        links: Vec::new(),
                        images: Vec::new(),
                        code_blocks: Vec::new(),
                        raw_html: None,
                    },
                    metadata: webclaw_core::Metadata {
                        title: None,
                        description: None,
                        author: None,
                        published_date: None,
                        language: None,
                        url: Some(params.url.clone()),
                        site_name: None,
                        image: None,
                        favicon: None,
                        word_count: markdown.split_whitespace().count(),
                    },
                    domain_data: None,
                    structured_data: Vec::new(),
                };

                let content_diff = webclaw_core::diff::diff(&previous, &current);
                Ok(serde_json::to_string_pretty(&content_diff).unwrap_or_default())
            }
        }
    }

    /// Extract brand identity (colors, fonts, logo, favicon) from a website's HTML and CSS.
    /// Automatically falls back to the webclaw cloud API when bot protection is detected.
    #[tool]
    async fn brand(&self, Parameters(params): Parameters<BrandParams>) -> Result<String, String> {
        validate_url(&params.url).await?;
        let fetch_result =
            tokio::time::timeout(LOCAL_FETCH_TIMEOUT, self.fetch_client.fetch(&params.url))
                .await
                .map_err(|_| format!("Fetch timed out after 30s for {}", params.url))?
                .map_err(|e| format!("Fetch failed: {e}"))?;

        // Check for bot protection before extracting brand
        if cloud::is_bot_protected(&fetch_result.html, &fetch_result.headers) {
            if let Some(ref c) = self.cloud {
                let resp = c
                    .post("brand", serde_json::json!({"url": params.url}))
                    .await?;
                return Ok(serde_json::to_string_pretty(&resp).unwrap_or_default());
            } else {
                return Err(format!(
                    "Bot protection detected on {}. Set WEBCLAW_API_KEY for automatic cloud bypass. \
                     Get a key at https://webclaw.io",
                    params.url
                ));
            }
        }

        let identity =
            webclaw_core::brand::extract_brand(&fetch_result.html, Some(&fetch_result.url));

        Ok(serde_json::to_string_pretty(&identity).unwrap_or_default())
    }

    /// Run a deep research investigation on a topic or question. Requires WEBCLAW_API_KEY.
    /// Saves full result to ~/.webclaw/research/ and returns the file path + key findings.
    /// Checks cache first — same query returns the cached result without spending credits.
    #[tool]
    async fn research(
        &self,
        Parameters(params): Parameters<ResearchParams>,
    ) -> Result<String, String> {
        let cloud = self
            .cloud
            .as_ref()
            .ok_or("Research requires WEBCLAW_API_KEY. Get a key at https://webclaw.io")?;

        let research_dir = research_dir();
        let slug = slugify(&params.query);

        // Check cache first
        if let Some(cached) = load_cached_research(&research_dir, &slug) {
            info!(query = %params.query, "returning cached research");
            return Ok(cached);
        }

        let mut body = json!({ "query": params.query });
        if let Some(deep) = params.deep {
            body["deep"] = json!(deep);
        }
        if let Some(ref topic) = params.topic {
            body["topic"] = json!(topic);
        }

        // Start the research job
        let start_resp = cloud.post("research", body).await?;
        let job_id = start_resp
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or("Research API did not return a job ID")?
            .to_string();

        info!(job_id = %job_id, "research job started, polling for completion");

        // Poll until completed or failed
        for poll in 0..RESEARCH_MAX_POLLS {
            tokio::time::sleep(Duration::from_secs(3)).await;

            let status_resp = cloud.get(&format!("research/{job_id}")).await?;
            let status = status_resp
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            match status {
                "completed" => {
                    // Save full result to file
                    let (report_path, json_path) =
                        save_research(&research_dir, &slug, &status_resp);

                    // Build compact response: file paths + findings (no full report)
                    let sources_count = status_resp
                        .get("sources_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    let findings_count = status_resp
                        .get("findings_count")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);

                    let mut response = json!({
                        "status": "completed",
                        "query": params.query,
                        "report_file": report_path,
                        "json_file": json_path,
                        "sources_count": sources_count,
                        "findings_count": findings_count,
                    });

                    if let Some(findings) = status_resp.get("findings") {
                        response["findings"] = findings.clone();
                    }
                    if let Some(sources) = status_resp.get("sources") {
                        response["sources"] = sources.clone();
                    }

                    return Ok(serde_json::to_string_pretty(&response).unwrap_or_default());
                }
                "failed" => {
                    let error = status_resp
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown error");
                    return Err(format!("Research job failed: {error}"));
                }
                _ => {
                    if poll % 20 == 19 {
                        info!(job_id = %job_id, poll, "research still in progress...");
                    }
                }
            }
        }

        Err(format!(
            "Research job {job_id} timed out after ~10 minutes of polling. \
             Check status manually via the webclaw API: GET /v1/research/{job_id}"
        ))
    }

    /// Search the web for a query and return structured results. Requires WEBCLAW_API_KEY.
    #[tool]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> Result<String, String> {
        let cloud = self
            .cloud
            .as_ref()
            .ok_or("Search requires WEBCLAW_API_KEY. Get a key at https://webclaw.io")?;

        let mut body = json!({ "query": params.query });
        if let Some(num) = params.num_results {
            body["num_results"] = json!(num);
        }

        let resp = cloud.post("search", body).await?;

        // Format results for readability
        if let Some(results) = resp.get("results").and_then(|v| v.as_array()) {
            let mut output = format!("Found {} results:\n\n", results.len());
            for (i, result) in results.iter().enumerate() {
                let title = result.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let url = result.get("url").and_then(|v| v.as_str()).unwrap_or("");
                let snippet = result
                    .get("snippet")
                    .or_else(|| result.get("description"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                output.push_str(&format!(
                    "{}. {}\n   {}\n   {}\n\n",
                    i + 1,
                    title,
                    url,
                    snippet
                ));
            }
            Ok(output)
        } else {
            // Fallback: return raw JSON if unexpected shape
            Ok(serde_json::to_string_pretty(&resp).unwrap_or_default())
        }
    }

    /// Capture browser network traffic from a page and save learned API endpoints for later replay.
    #[tool]
    async fn capture_network(
        &self,
        Parameters(params): Parameters<CaptureNetworkParams>,
    ) -> Result<String, String> {
        let url = normalize_capture_url(&params.url)?;
        validate_url(&url).await?;

        let saved = run_network_capture(CaptureOptions {
            url,
            intent: params.intent,
            wait_ms: params.wait_ms.unwrap_or(3000),
            headed: params.headed.unwrap_or(false),
        })
        .await
        .map_err(|e| format!("capture_network failed: {e}"))?;

        to_pretty_json(&saved)
    }

    /// Return learned endpoint definitions for a saved capture id.
    #[tool]
    async fn discover_endpoints(
        &self,
        Parameters(params): Parameters<DiscoverEndpointsParams>,
    ) -> Result<String, String> {
        let endpoints = load_endpoints(&params.capture_id).map_err(|e| {
            format!(
                "could not load endpoints for capture id {}: {e}",
                params.capture_id
            )
        })?;

        to_pretty_json(&endpoints)
    }

    /// Show one learned endpoint definition by endpoint id.
    #[tool]
    async fn show_endpoint(
        &self,
        Parameters(params): Parameters<ShowEndpointParams>,
    ) -> Result<String, String> {
        let endpoint = find_endpoint(&params.endpoint_id)
            .map_err(|e| format!("could not find endpoint id {}: {e}", params.endpoint_id))?;

        to_pretty_json(&endpoint)
    }

    /// Replay or preview a learned endpoint. Mutating methods default to dry-run unless confirmed.
    #[tool]
    async fn replay_endpoint(
        &self,
        Parameters(params): Parameters<ReplayEndpointParams>,
    ) -> Result<String, String> {
        let endpoint = find_endpoint(&params.endpoint_id)
            .map_err(|e| format!("could not find endpoint id {}: {e}", params.endpoint_id))?;
        let options = replay_options_from_params(&endpoint, &params)?;
        let result = run_endpoint_replay(&endpoint, options)
            .await
            .map_err(|e| format!("replay_endpoint failed: {e}"))?;

        to_pretty_json(&result)
    }

    /// Export a saved capture's learned endpoints as OpenAPI 3.1 JSON.
    #[tool]
    async fn export_openapi(
        &self,
        Parameters(params): Parameters<ExportOpenApiParams>,
    ) -> Result<String, String> {
        let path = write_openapi(&params.capture_id).map_err(|e| {
            format!(
                "could not export OpenAPI for capture id {}: {e}",
                params.capture_id
            )
        })?;

        to_pretty_json(&json!({ "path": path }))
    }

    /// List saved network captures from the configured capture root.
    #[tool]
    async fn list_captures(
        &self,
        Parameters(_params): Parameters<ListCapturesParams>,
    ) -> Result<String, String> {
        to_pretty_json(&list_saved_captures_from_root(&capture_root())?)
    }

    /// List every vertical extractor the server knows about. Returns a
    /// JSON array of `{name, label, description, url_patterns}` entries.
    /// Call this to discover what verticals are available before using
    /// `vertical_scrape`.
    #[tool]
    async fn list_extractors(
        &self,
        Parameters(_params): Parameters<ListExtractorsParams>,
    ) -> Result<String, String> {
        let catalog = webclaw_fetch::extractors::list();
        serde_json::to_string_pretty(&catalog)
            .map_err(|e| format!("failed to serialise extractor catalog: {e}"))
    }

    /// Run a vertical extractor by name and return typed JSON specific
    /// to the target site (title, price, rating, author, etc.), not
    /// generic markdown. Use `list_extractors` to discover available
    /// names. Example names: `reddit`, `github_repo`, `trustpilot_reviews`,
    /// `youtube_video`, `shopify_product`, `pypi`, `npm`, `arxiv`.
    ///
    /// Antibot-gated verticals (amazon_product, ebay_listing,
    /// etsy_listing, trustpilot_reviews) will automatically escalate to
    /// the webclaw cloud API when local fetch hits bot protection,
    /// provided `WEBCLAW_API_KEY` is set.
    #[tool]
    async fn vertical_scrape(
        &self,
        Parameters(params): Parameters<VerticalParams>,
    ) -> Result<String, String> {
        validate_url(&params.url).await?;
        // Use the cached Firefox client, not the default Chrome one.
        // Reddit's `.json` endpoint rejects the wreq-Chrome TLS
        // fingerprint with a 403 even from residential IPs (they
        // ship a fingerprint blocklist that includes common
        // browser-emulation libraries). The wreq-Firefox fingerprint
        // still passes, and Firefox is equally fine for every other
        // vertical in the catalog, so it's a strictly-safer default
        // for `vertical_scrape` than the generic `scrape` tool's
        // Chrome default. Matches the CLI `webclaw vertical`
        // subcommand which already uses Firefox.
        let client = self.firefox_or_build()?;
        let data =
            webclaw_fetch::extractors::dispatch_by_name(client.as_ref(), &params.name, &params.url)
                .await
                .map_err(|e| e.to_string())?;
        serde_json::to_string_pretty(&data)
            .map_err(|e| format!("failed to serialise extractor output: {e}"))
    }
}

#[tool_handler]
impl ServerHandler for WebclawMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("webclaw-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(String::from(
                "Webclaw MCP server -- web content extraction for AI agents. \
                 Tools: scrape, crawl, map, batch, extract, summarize, diff, brand, research, search, \
                 capture_network, discover_endpoints, show_endpoint, replay_endpoint, export_openapi, \
                 list_captures, list_extractors, vertical_scrape.",
            ))
    }
}

fn normalize_capture_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("url must not be empty".to_owned());
    }

    let normalized = if trimmed.contains("://") {
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };

    let parsed = url::Url::parse(&normalized).map_err(|e| format!("invalid URL: {e}"))?;
    match parsed.scheme() {
        "http" | "https" => Ok(normalized),
        scheme => Err(format!(
            "capture_network only supports http and https URLs, got {scheme:?}"
        )),
    }
}

fn replay_options_from_params(
    endpoint: &EndpointDefinition,
    params: &ReplayEndpointParams,
) -> Result<ReplayOptions, String> {
    if let Some(value) = &params.params_json
        && !value.is_object()
    {
        return Err("params_json must be a JSON object".to_owned());
    }

    let confirm_unsafe = params.confirm_unsafe.unwrap_or(false);
    let default_dry_run = endpoint_defaults_to_dry_run(endpoint) && !confirm_unsafe;

    Ok(ReplayOptions {
        dry_run: params.dry_run.unwrap_or(false) || default_dry_run,
        confirm_unsafe,
        params_json: params.params_json.clone(),
        headers: header_map_from_strings(params.headers.as_ref()),
        body_json: params.body_json.clone(),
    })
}

fn endpoint_defaults_to_dry_run(endpoint: &EndpointDefinition) -> bool {
    endpoint.safety.requires_confirmation
        || !endpoint.safety.safe_to_replay
        || !matches!(
            endpoint.method.to_ascii_uppercase().as_str(),
            "GET" | "HEAD" | "OPTIONS"
        )
}

fn header_map_from_strings(
    headers: Option<&std::collections::BTreeMap<String, String>>,
) -> HeaderMap {
    headers
        .into_iter()
        .flat_map(|headers| headers.iter())
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect()
}

fn list_saved_captures_from_root(root: &Path) -> Result<Vec<Value>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }

    let mut captures = Vec::new();
    collect_saved_captures(root, root, &mut captures)?;
    captures.sort_by(|left, right| {
        left.get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .cmp(right.get("id").and_then(Value::as_str).unwrap_or_default())
    });

    Ok(captures)
}

fn collect_saved_captures(
    root: &Path,
    current: &Path,
    captures: &mut Vec<Value>,
) -> Result<(), String> {
    let entries = fs::read_dir(current).map_err(|e| {
        format!(
            "could not read capture directory {}: {e}",
            current.display()
        )
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("could not read capture directory entry: {e}"))?;
        let path = entry.path();

        if path.is_dir() {
            collect_saved_captures(root, &path, captures)?;
            continue;
        }

        if path.file_name().and_then(|name| name.to_str()) == Some("metadata.json") {
            captures.push(read_capture_metadata(root, &path)?);
        }
    }

    Ok(())
}

fn read_capture_metadata(root: &Path, metadata_path: &Path) -> Result<Value, String> {
    let contents = fs::read_to_string(metadata_path).map_err(|e| {
        format!(
            "could not read capture metadata {}: {e}",
            metadata_path.display()
        )
    })?;
    let mut metadata = match serde_json::from_str::<Value>(&contents).map_err(|e| {
        format!(
            "could not parse capture metadata {}: {e}",
            metadata_path.display()
        )
    })? {
        Value::Object(metadata) => metadata,
        _ => Map::new(),
    };

    let capture_dir = metadata_path
        .parent()
        .ok_or_else(|| format!("metadata path has no parent: {}", metadata_path.display()))?;
    let capture_id = capture_id_from_dir(root, capture_dir)?;

    metadata
        .entry("id".to_owned())
        .or_insert_with(|| Value::String(capture_id));
    metadata.insert(
        "capture_dir".to_owned(),
        Value::String(capture_dir.display().to_string()),
    );

    Ok(Value::Object(metadata))
}

fn capture_id_from_dir(root: &Path, capture_dir: &Path) -> Result<String, String> {
    let relative = capture_dir.strip_prefix(root).map_err(|e| {
        format!(
            "capture directory {} is not under root {}: {e}",
            capture_dir.display(),
            root.display()
        )
    })?;
    let parts = relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();

    if parts.is_empty() {
        Err(format!(
            "capture directory {} does not contain a capture id",
            capture_dir.display()
        ))
    } else {
        Ok(parts.join("/"))
    }
}

fn to_pretty_json<T: serde::Serialize>(value: &T) -> Result<String, String> {
    serde_json::to_string_pretty(value).map_err(|e| format!("JSON encode failed: {e}"))
}

// ---------------------------------------------------------------------------
// Research file helpers
// ---------------------------------------------------------------------------

fn research_dir() -> std::path::PathBuf {
    let dir = dirs::home_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(".webclaw")
        .join("research");
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn slugify(query: &str) -> String {
    let s: String = query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("-")
        .to_lowercase();
    if s.len() > 60 { s[..60].to_string() } else { s }
}

/// Check for a cached research result. Returns the compact response if found.
fn load_cached_research(dir: &std::path::Path, slug: &str) -> Option<String> {
    let json_path = dir.join(format!("{slug}.json"));
    let report_path = dir.join(format!("{slug}.md"));

    if !json_path.exists() || !report_path.exists() {
        return None;
    }

    let json_str = std::fs::read_to_string(&json_path).ok()?;
    let data: serde_json::Value = serde_json::from_str(&json_str).ok()?;

    // Build compact response from cache
    let mut response = json!({
        "status": "completed",
        "cached": true,
        "query": data.get("query").cloned().unwrap_or(json!("")),
        "report_file": report_path.to_string_lossy(),
        "json_file": json_path.to_string_lossy(),
        "sources_count": data.get("sources_count").cloned().unwrap_or(json!(0)),
        "findings_count": data.get("findings_count").cloned().unwrap_or(json!(0)),
    });

    if let Some(findings) = data.get("findings") {
        response["findings"] = findings.clone();
    }
    if let Some(sources) = data.get("sources") {
        response["sources"] = sources.clone();
    }

    Some(serde_json::to_string_pretty(&response).unwrap_or_default())
}

/// Save research result to disk. Returns (report_path, json_path) as strings.
fn save_research(dir: &std::path::Path, slug: &str, data: &serde_json::Value) -> (String, String) {
    let json_path = dir.join(format!("{slug}.json"));
    let report_path = dir.join(format!("{slug}.md"));

    // Save full JSON
    if let Ok(json_str) = serde_json::to_string_pretty(data) {
        std::fs::write(&json_path, json_str).ok();
    }

    // Save report as markdown
    if let Some(report) = data.get("report").and_then(|v| v.as_str()) {
        std::fs::write(&report_path, report).ok();
    }

    (
        report_path.to_string_lossy().to_string(),
        json_path.to_string_lossy().to_string(),
    )
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::fs;

    use serde_json::json;
    use webclaw_capture::types::{EndpointDefinition, EndpointSafety};

    use super::*;

    fn endpoint(
        method: &str,
        safe_to_replay: bool,
        requires_confirmation: bool,
    ) -> EndpointDefinition {
        EndpointDefinition {
            id: format!("{}_example", method.to_ascii_lowercase()),
            method: method.to_owned(),
            origin: "https://example.test".to_owned(),
            path_template: "/api/items".to_owned(),
            query_params: BTreeMap::new(),
            request_schema: None,
            response_schema: None,
            auth_evidence: Vec::new(),
            safety: EndpointSafety {
                safe_to_replay,
                requires_confirmation,
                reason: "test".to_owned(),
            },
            examples: Vec::new(),
        }
    }

    #[test]
    fn normalize_capture_url_adds_https_and_rejects_non_http_schemes() {
        assert_eq!(
            normalize_capture_url("example.test/path").unwrap(),
            "https://example.test/path"
        );

        assert!(normalize_capture_url("file:///C:/secret.txt").is_err());
    }

    #[test]
    fn replay_options_default_unsafe_methods_to_dry_run_unless_confirmed() {
        let unsafe_endpoint = endpoint("POST", false, true);
        let params = ReplayEndpointParams {
            endpoint_id: unsafe_endpoint.id.clone(),
            params_json: Some(json!({"id": "123"})),
            dry_run: None,
            confirm_unsafe: None,
            headers: Some(BTreeMap::from([("X-Test".to_owned(), "ok".to_owned())])),
            body_json: Some(json!({"name": "tool"})),
        };

        let options = replay_options_from_params(&unsafe_endpoint, &params).unwrap();
        assert!(options.dry_run);
        assert!(!options.confirm_unsafe);
        assert_eq!(options.params_json, Some(json!({"id": "123"})));
        assert_eq!(options.headers.get("X-Test"), Some(&json!("ok")));

        let confirmed = ReplayEndpointParams {
            confirm_unsafe: Some(true),
            ..params
        };
        let options = replay_options_from_params(&unsafe_endpoint, &confirmed).unwrap();
        assert!(!options.dry_run);
        assert!(options.confirm_unsafe);
    }

    #[test]
    fn replay_options_leave_safe_gets_executable_by_default() {
        let safe_endpoint = endpoint("GET", true, false);
        let params = ReplayEndpointParams {
            endpoint_id: safe_endpoint.id.clone(),
            params_json: None,
            dry_run: None,
            confirm_unsafe: None,
            headers: None,
            body_json: None,
        };

        let options = replay_options_from_params(&safe_endpoint, &params).unwrap();
        assert!(!options.dry_run);
        assert!(!options.confirm_unsafe);
    }

    #[test]
    fn list_saved_captures_from_root_returns_metadata_with_capture_id() {
        let root = std::env::temp_dir().join(format!(
            "webclaw-mcp-list-captures-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let capture_dir = root.join("example.test").join("2026-05-16T12-00-00Z");
        fs::create_dir_all(&capture_dir).unwrap();
        fs::write(
            capture_dir.join("metadata.json"),
            serde_json::to_string(&json!({
                "source_url": "https://example.test",
                "endpoint_count": 2
            }))
            .unwrap(),
        )
        .unwrap();

        let captures = list_saved_captures_from_root(&root).unwrap();
        fs::remove_dir_all(&root).ok();

        assert_eq!(captures.len(), 1);
        assert_eq!(captures[0]["id"], "example.test/2026-05-16T12-00-00Z");
        assert_eq!(captures[0]["endpoint_count"], 2);
        assert!(
            captures[0]["capture_dir"]
                .as_str()
                .unwrap()
                .contains("example.test")
        );
    }
}
