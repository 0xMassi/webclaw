/// MCP server implementation for webclaw.
/// Exposes web extraction capabilities as tools for AI agents.
///
/// Uses a local-first architecture: fetches pages directly, then falls back
/// to the webclaw cloud API (api.webclaw.io) when bot protection or
/// JS rendering is detected. Set WEBCLAW_API_KEY for automatic fallback.
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Implementation, ServerCapabilities, ServerInfo};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};
use serde_json::json;
use tracing::{error, info, warn};

use webclaw_fetch::cloud::{self, CloudClient, SmartFetchResult};

use crate::tools::*;

#[derive(Clone)]
pub struct WebclawMcp {
    /// Holds the registered MCP tools. `rmcp >= 1.3` reads this through a
    /// derived trait impl (not by name), so rustc's dead-code lint can't
    /// see the usage and fires a spurious `field tool_router is never
    /// read` on `cargo install`. The field is essential — dropping it
    /// would unregister every tool. See issue #30.
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    fetch_config: webclaw_fetch::FetchConfig,
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
    firefox_client: Arc<OnceLock<Arc<webclaw_fetch::FetchClient>>>,
    llm_chain: Option<Arc<webclaw_llm::ProviderChain>>,
    cloud: Option<CloudClient>,
}

fn parse_browser(browser: Option<&str>) -> Result<webclaw_fetch::BrowserProfile, String> {
    use webclaw_fetch::BrowserProfile;
    Ok(match browser {
        None | Some("chrome") => BrowserProfile::Chrome,
        Some("chrome-macos") => BrowserProfile::ChromeMacos,
        Some("firefox") => BrowserProfile::Firefox,
        Some("safari") => BrowserProfile::Safari,
        Some("safari-ios") => BrowserProfile::SafariIos,
        Some("edge") => BrowserProfile::Edge,
        Some("random") => BrowserProfile::Random,
        Some(other) => {
            return Err(format!(
                "unknown browser \"{other}\": use chrome, chrome-macos, firefox, safari, safari-ios, edge or random"
            ));
        }
    })
}

#[cfg(feature = "browser")]
fn render_from_env(
    config: &webclaw_fetch::FetchConfig,
) -> Option<webclaw_fetch::render::RenderConfig> {
    use webclaw_fetch::render::{BrowserSource, RenderConfig, RenderPool};
    let mode = std::env::var("WEBCLAW_RENDER").unwrap_or_default();
    if mode == "never" {
        return None;
    }
    let source = match std::env::var("WEBCLAW_CDP_URL") {
        Ok(endpoint) if !endpoint.is_empty() => BrowserSource::Connect(endpoint),
        _ => BrowserSource::launch_for(config),
    };
    Some(RenderConfig {
        pool: Arc::new(RenderPool::new(source, LOCAL_FETCH_TIMEOUT)),
        always: mode == "always",
        wait_for: None,
        block: webclaw_fetch::Block::default(),
    })
}

#[cfg(feature = "browser")]
fn apply_render(
    config: &mut webclaw_fetch::FetchConfig,
    mode: Option<&str>,
    wait_for: Option<String>,
    block: Option<&str>,
) -> Result<(), String> {
    if !matches!(mode, None | Some("auto" | "always" | "never")) {
        return Err("render must be \"auto\", \"always\" or \"never\"".into());
    }
    let block = block.map(str::parse::<webclaw_fetch::Block>).transpose()?;
    if mode == Some("never") {
        config.render = None;
        return Ok(());
    }
    let Some(render) = config.render.as_mut() else {
        return match (mode, &wait_for) {
            (None | Some("auto"), None) => Ok(()),
            _ => Err("rendering is turned off for this server (WEBCLAW_RENDER=never)".into()),
        };
    };
    if let Some(mode) = mode {
        render.always = mode == "always";
    }
    if wait_for.is_some() {
        render.wait_for = wait_for;
    }
    if let Some(block) = block {
        render.block = block;
    }
    Ok(())
}

#[cfg(not(feature = "browser"))]
fn apply_render(
    _config: &mut webclaw_fetch::FetchConfig,
    mode: Option<&str>,
    wait_for: Option<String>,
    block: Option<&str>,
) -> Result<(), String> {
    block.map(str::parse::<webclaw_fetch::Block>).transpose()?;
    match (mode, wait_for) {
        (None | Some("auto" | "never"), None) => Ok(()),
        _ => Err(
            "this webclaw-mcp build has no browser support; build it with `--features browser`"
                .into(),
        ),
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
const MAX_CRAWL_QUERY_CHARS: usize = 1000;

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
        #[cfg(feature = "browser")]
        {
            config.render = render_from_env(&config);
        }

        let fetch_client = match webclaw_fetch::FetchClient::new(config.clone()) {
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
            Some(Arc::new(chain))
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
            fetch_config: config,
            fetch_client: Arc::new(fetch_client),
            firefox_client: Arc::new(OnceLock::new()),
            llm_chain,
            cloud,
        }
    }

    /// Shuts down the browser this server launched for rendering, once awaited.
    pub fn close_browser(&self) -> impl std::future::Future<Output = ()> + use<> {
        let config = self.fetch_config.clone();
        async move {
            #[cfg(feature = "browser")]
            if let Some(render) = &config.render {
                render.pool.close().await;
            }
            let _ = config;
        }
    }

    #[cfg(feature = "browser")]
    async fn take_screenshot(
        &self,
        url: &str,
        wait_for: Option<&str>,
        full_page: bool,
        jpeg: bool,
    ) -> Result<CallToolResult, String> {
        let render = self
            .fetch_config
            .render
            .as_ref()
            .ok_or("screenshots need the browser, which is turned off for this server (WEBCLAW_RENDER=never)")?;
        let shot = render
            .pool
            .screenshot(
                url,
                wait_for,
                &[],
                webclaw_fetch::render::ScreenshotOptions { full_page, jpeg },
            )
            .await
            .map_err(|e| format!("Screenshot failed: {e}"))?;
        webclaw_fetch::FetchError::check_status(shot.status).map_err(|e| e.to_string())?;
        let mime = if jpeg { "image/jpeg" } else { "image/png" };
        Ok(CallToolResult::success(vec![
            rmcp::model::ContentBlock::image(shot.base64(), mime),
            rmcp::model::ContentBlock::text(shot.url),
        ]))
    }

    #[cfg(not(feature = "browser"))]
    async fn take_screenshot(
        &self,
        _url: &str,
        _wait_for: Option<&str>,
        _full_page: bool,
        _jpeg: bool,
    ) -> Result<CallToolResult, String> {
        Err(
            "this webclaw-mcp build has no browser support; build it with `--features browser`"
                .into(),
        )
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
            ..self.fetch_config.clone()
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
    /// Pages whose content needs JavaScript are rendered in a local Chrome when this server has browser support; otherwise, and on bot protection, it falls back to the webclaw cloud API when WEBCLAW_API_KEY is set.
    #[tool]
    async fn scrape(&self, Parameters(params): Parameters<ScrapeParams>) -> Result<String, String> {
        validate_url(&params.url).await?;
        let sizes = chunk_sizes(params.chunk, params.chunk_overlap)?;
        let format = params.format.as_deref().unwrap_or("markdown");
        let browser = parse_browser(params.browser.as_deref())?;
        let include = params.include_selectors.unwrap_or_default();
        let exclude = params.exclude_selectors.unwrap_or_default();
        let main_only = params.only_main_content.unwrap_or(false);

        let cookie_header = params
            .cookies
            .as_ref()
            .filter(|c| !c.is_empty())
            .map(|c| c.join("; "));
        let shared = cookie_header.is_none()
            && params.render.is_none()
            && params.wait_for.is_none()
            && params.block.is_none();

        let custom_client;
        let cached_firefox;
        let client: &webclaw_fetch::FetchClient =
            if shared && matches!(browser, webclaw_fetch::BrowserProfile::Chrome) {
                &self.fetch_client
            } else if shared && matches!(browser, webclaw_fetch::BrowserProfile::Firefox) {
                cached_firefox = self.firefox_or_build()?;
                &cached_firefox
            } else {
                let mut headers = std::collections::HashMap::new();
                headers.insert("Accept-Language".to_string(), "en-US,en;q=0.9".to_string());
                if let Some(ref cookies) = cookie_header {
                    headers.insert("Cookie".to_string(), cookies.clone());
                }
                let mut config = webclaw_fetch::FetchConfig {
                    browser,
                    headers,
                    ..self.fetch_config.clone()
                };
                apply_render(
                    &mut config,
                    params.render.as_deref(),
                    params.wait_for.clone(),
                    params.block.as_deref(),
                )?;
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
            SmartFetchResult::Local(mut extraction) => {
                if let Some(ref query) = params.query {
                    webclaw_core::focus_extraction(&mut extraction, query);
                }
                if let Some((size, overlap)) = sizes {
                    let (url, markdown) = (params.url.clone(), extraction.content.markdown);
                    return tokio::task::spawn_blocking(move || {
                        chunks_json(&[(&url, &markdown)], size, overlap)
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                }
                let output = match format {
                    "llm" => webclaw_core::to_llm_text(&extraction, Some(&params.url)),
                    "text" => extraction.content.plain_text,
                    "json" => serde_json::to_string_pretty(&extraction).unwrap_or_default(),
                    _ => extraction.content.markdown,
                };
                Ok(output)
            }
            SmartFetchResult::Cloud(mut resp) => {
                if let Some(ref query) = params.query {
                    webclaw_fetch::cloud::focus_cloud_response(&mut resp, query);
                }
                if let Some((size, overlap)) = sizes {
                    let markdown = resp
                        .get("markdown")
                        .or_else(|| resp.get("content")?.get("markdown"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let url = params.url.clone();
                    return tokio::task::spawn_blocking(move || {
                        chunks_json(&[(&url, &markdown)], size, overlap)
                    })
                    .await
                    .map_err(|e| e.to_string())?;
                }
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

    /// Crawl a website starting from a seed URL, following links breadth-first up to a configurable depth and page limit. With `query`, the links most likely to answer it are followed first and pages come back best match first, each cut to the parts that answer it.
    #[tool]
    async fn crawl(&self, Parameters(params): Parameters<CrawlParams>) -> Result<String, String> {
        validate_url(&params.url).await?;
        let sizes = chunk_sizes(params.chunk, params.chunk_overlap)?;

        if let Some(max) = params.max_pages
            && max > 500
        {
            return Err("max_pages cannot exceed 500".into());
        }

        if params
            .query
            .as_ref()
            .is_some_and(|q| q.chars().count() > MAX_CRAWL_QUERY_CHARS)
        {
            return Err(format!(
                "query cannot exceed {MAX_CRAWL_QUERY_CHARS} characters"
            ));
        }

        let format = params.format.as_deref().unwrap_or("markdown");

        let mut fetch = self.fetch_config.clone();
        apply_render(&mut fetch, None, None, params.block.as_deref())?;
        let config = webclaw_fetch::CrawlConfig {
            fetch,
            max_depth: params.depth.unwrap_or(2) as usize,
            max_pages: match params.max_pages {
                Some(0) => None,
                Some(n) => Some(n),
                None => Some(50),
            },
            concurrency: params.concurrency.unwrap_or(5),
            use_sitemap: params.use_sitemap.unwrap_or(false),
            respect_robots: !params.ignore_robots.unwrap_or(false),
            query: params.query.clone(),
            ..Default::default()
        };

        let crawler = webclaw_fetch::Crawler::new(&params.url, config)
            .map_err(|e| format!("Crawler init failed: {e}"))?;

        let mut result = crawler.crawl(&params.url, None).await;
        result.pages.sort_by(|a, b| {
            b.relevance
                .unwrap_or(0.0)
                .total_cmp(&a.relevance.unwrap_or(0.0))
        });

        if let Some((size, overlap)) = sizes {
            return tokio::task::spawn_blocking(move || crawl_chunks_json(&result, size, overlap))
                .await
                .map_err(|e| e.to_string())?;
        }

        let mut output = format!(
            "Crawled {} pages ({} ok, {} errors, {} disallowed by robots.txt) in {:.1}s\n\n",
            result.total, result.ok, result.errors, result.skipped_by_robots, result.elapsed_secs
        );

        for page in &result.pages {
            match page.relevance {
                Some(r) => output.push_str(&format!(
                    "--- {} (depth {}, relevance {r:.2}) ---\n",
                    page.url, page.depth
                )),
                None => output.push_str(&format!("--- {} (depth {}) ---\n", page.url, page.depth)),
            }
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

    /// Pick values out of a web page with a CSS selector or an XPath 1.0 expression and return them as a JSON array: text by default, or an attribute or HTML with a "::attr(NAME)" or "::html" ending (CSS), or "@attr" (XPath). With `fields`, return one JSON object per match, each field a selector run inside that match. With `tables`, return the page's data tables as headers and rows. With `text` or `regex`, find the elements holding that text instead. With `similar`, take the first element picked as an example and return every element like it, with a CSS selector that picks them all.
    /// Pages whose content needs JavaScript are rendered first when this server has browser support.
    #[tool]
    async fn select(
        &self,
        Parameters(mut params): Parameters<SelectParams>,
    ) -> Result<String, String> {
        let text = check_select(&params)?;
        let similar = params.similar.unwrap_or(false);
        let fields: Vec<(String, String)> = params
            .fields
            .take()
            .unwrap_or_default()
            .into_iter()
            .collect();
        validate_url(&params.url).await?;
        let fetched = tokio::time::timeout(
            LOCAL_FETCH_TIMEOUT,
            self.fetch_client.fetch_smart(&params.url),
        )
        .await
        .map_err(|_| format!("Fetch timed out for {}", params.url))?
        .map_err(|e| format!("Fetch failed: {e}"))?;
        let (page, _) = self
            .fetch_client
            .extract_page(fetched, &webclaw_core::ExtractionOptions::default())
            .await;
        webclaw_fetch::FetchError::check_status(page.status).map_err(|e| e.to_string())?;
        tokio::task::spawn_blocking(move || {
            if params.tables.unwrap_or(false) {
                return serde_json::to_string_pretty(&webclaw_core::tables(&page.html))
                    .map_err(|e| e.to_string());
            }
            pick(
                &params,
                text.as_ref(),
                similar,
                &fields,
                &page.html,
                &page.url,
            )
        })
        .await
        .map_err(|e| e.to_string())?
    }

    /// Take a screenshot of a web page in the local Chrome after its scripts ran: the 1280x720 viewport, or the whole page with `full_page`. Returns the image and the final URL.
    #[tool]
    async fn screenshot(
        &self,
        Parameters(params): Parameters<ScreenshotParams>,
    ) -> Result<CallToolResult, String> {
        validate_url(&params.url).await?;
        let jpeg = match params.format.as_deref() {
            None | Some("png") => false,
            Some("jpeg" | "jpg") => true,
            Some(other) => {
                return Err(format!(
                    "format must be \"png\" or \"jpeg\", got \"{other}\""
                ));
            }
        };
        self.take_screenshot(
            &params.url,
            params.wait_for.as_deref(),
            params.full_page.unwrap_or(false),
            jpeg,
        )
        .await
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
        // No up-front DNS pre-validation: it aborted the whole batch on a
        // single unresolvable URL. The fetch layer applies the same SSRF
        // guard (validate_public_http_url) per URL, so bad entries surface
        // as individual per-URL errors below instead of failing the batch.

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

        let chain = self.llm_chain.as_deref().unwrap();

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

        let chain = self.llm_chain.as_deref().unwrap();

        let llm_content = match self.smart_fetch_llm(&params.url).await? {
            SmartFetchResult::Local(extraction) => {
                webclaw_core::to_llm_text(&extraction, Some(&params.url))
            }
            SmartFetchResult::Cloud(resp) => format!(
                "Source URL: {}\n\n{}",
                params.url,
                resp.get("llm")
                    .or_else(|| resp.get("markdown"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
            ),
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

    /// Search the web for a query and return structured results.
    ///
    /// Resolves the backend in priority order:
    /// 1. `SERPER_API_KEY` set → local Serper.dev search with the user's
    ///    own key (no hosted API needed). Supports `country`, `lang`, and
    ///    `scrape` (fetch + extract each result page).
    /// 2. else `WEBCLAW_API_KEY` set → the hosted webclaw search API.
    /// 3. else → an error explaining both options.
    #[tool]
    async fn search(&self, Parameters(params): Parameters<SearchParams>) -> Result<String, String> {
        // Local path: user's own Serper key. Preferred when present so the
        // tool works without the hosted API and without spending credits.
        if let Ok(serper_key) = std::env::var("SERPER_API_KEY")
            && !serper_key.trim().is_empty()
        {
            let opts = webclaw_fetch::SearchOptions {
                num_results: params.num_results.unwrap_or(5) as usize,
                country: params.country.clone(),
                lang: params.lang.clone(),
                scrape: params.scrape.unwrap_or(false),
            };
            let results = webclaw_fetch::search(
                self.fetch_client.as_ref(),
                &serper_key,
                &params.query,
                &opts,
            )
            .await
            .map_err(|e| format!("search error: {e}"))?;

            let mut output = format!("Found {} results:\n\n", results.len());
            for r in &results {
                output.push_str(&format!("{}. {}\n   {}\n", r.position, r.title, r.link));
                if !r.snippet.is_empty() {
                    output.push_str(&format!("   {}\n", r.snippet));
                }
                if let Some(ref content) = r.content {
                    output.push_str(&format!("\n{content}\n"));
                }
                output.push('\n');
            }
            return Ok(output);
        }

        // Hosted path: the webclaw cloud API.
        let cloud = self.cloud.as_ref().ok_or(
            "Search requires a search backend: set SERPER_API_KEY for local search \
             (get one free at serper.dev), or WEBCLAW_API_KEY for the hosted API.",
        )?;

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

fn chunk_sizes(
    chunk: Option<usize>,
    overlap: Option<usize>,
) -> Result<Option<(usize, usize)>, String> {
    match (chunk, overlap) {
        (None, Some(_)) => Err("chunk_overlap works with chunk".into()),
        (Some(0), _) => Err("chunk must be at least 1 token".into()),
        (size, overlap) => Ok(size.map(|s| (s, overlap.unwrap_or(0)))),
    }
}

fn page_chunks(pages: &[(&str, &str)], size: usize, overlap: usize) -> Vec<webclaw_core::Chunk> {
    pages
        .iter()
        .flat_map(|(url, markdown)| {
            webclaw_core::chunk(markdown, size, overlap)
                .into_iter()
                .map(|mut c| {
                    c.url = Some(url.to_string());
                    c
                })
        })
        .collect()
}

fn chunks_json(pages: &[(&str, &str)], size: usize, overlap: usize) -> Result<String, String> {
    serde_json::to_string_pretty(&page_chunks(pages, size, overlap)).map_err(|e| e.to_string())
}

fn crawl_chunks_json(
    result: &webclaw_fetch::CrawlResult,
    size: usize,
    overlap: usize,
) -> Result<String, String> {
    let pages: Vec<(&str, &str)> = result
        .pages
        .iter()
        .filter_map(|p| {
            Some((
                p.url.as_str(),
                p.extraction.as_ref()?.content.markdown.as_str(),
            ))
        })
        .collect();
    let failed: Vec<serde_json::Value> = result
        .pages
        .iter()
        .filter_map(|p| Some(json!({ "url": p.url, "error": p.error.as_ref()? })))
        .collect();
    serde_json::to_string_pretty(&json!({
        "total": result.total,
        "ok": result.ok,
        "errors": result.errors,
        "skipped_by_robots": result.skipped_by_robots,
        "failed": failed,
        "chunks": page_chunks(&pages, size, overlap),
    }))
    .map_err(|e| e.to_string())
}

#[tool_handler]
impl ServerHandler for WebclawMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("webclaw-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(String::from(
                "Webclaw MCP server -- web content extraction for AI agents. \
                 Tools: scrape, select, screenshot, crawl, map, batch, extract, summarize, diff, brand, \
                 research, search, list_extractors, vertical_scrape.",
            ))
    }
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
    // char-safe truncation: byte slicing panics if char 60 lands
    // mid-codepoint (multibyte queries, e.g. CJK / accented input).
    s.chars().take(60).collect()
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

fn check_select(params: &SelectParams) -> Result<Option<webclaw_core::TextMatch>, String> {
    let tables = params.tables.unwrap_or(false);
    let given = [
        params.selector.is_some(),
        params.xpath.is_some(),
        tables,
        params.text.is_some(),
        params.regex.is_some(),
    ];
    if given.iter().filter(|g| **g).count() != 1 {
        return Err("give exactly one of `selector`, `xpath`, `tables`, `text` and `regex`".into());
    }
    if tables && params.similar.unwrap_or(false) {
        return Err(
            "`similar` works with `selector`, `xpath`, `text` or `regex`, not `tables`".into(),
        );
    }
    if tables && params.fields.as_ref().is_some_and(|f| !f.is_empty()) {
        return Err("`fields` works with `selector`, `xpath` or `similar`, not `tables`".into());
    }
    if params.exact.is_some() && params.text.is_none() {
        return Err("`exact` works with `text`".into());
    }
    if params.case_sensitive.is_some() && params.text.is_none() && params.regex.is_none() {
        return Err("`case_sensitive` works with `text` or `regex`".into());
    }
    let has_fields = params.fields.as_ref().is_some_and(|f| !f.is_empty());
    if (params.text.is_some() || params.regex.is_some())
        && !params.similar.unwrap_or(false)
        && has_fields
    {
        return Err("`fields` works with `selector`, `xpath` or `similar`".into());
    }
    let case_sensitive = params.case_sensitive.unwrap_or(false);
    match (&params.text, &params.regex) {
        (Some(text), _) => {
            webclaw_core::TextMatch::text(text, params.exact.unwrap_or(false), case_sensitive)
                .map(Some)
        }
        (None, Some(regex)) => webclaw_core::TextMatch::regex(regex, case_sensitive).map(Some),
        (None, None) => Ok(None),
    }
    .map_err(|e| e.to_string())
}

fn pick(
    params: &SelectParams,
    text: Option<&webclaw_core::TextMatch>,
    similar: bool,
    fields: &[(String, String)],
    html: &str,
    url: &str,
) -> Result<String, String> {
    let url = Some(url);
    let json = match (&params.selector, &params.xpath, text) {
        _ if similar => {
            let example = match (&params.selector, &params.xpath, text) {
                (Some(css), _, _) => webclaw_core::FindExample::Css(css),
                (_, Some(xpath), _) => webclaw_core::FindExample::XPath(xpath),
                (_, _, Some(text)) => webclaw_core::FindExample::Text(text),
                _ => return Err("`similar` needs an example".into()),
            };
            let similar = webclaw_core::find_similar(html, example)
                .map_err(|e| e.to_string())?
                .ok_or("nothing on the page matches the example")?;
            if fields.is_empty() {
                serde_json::to_string_pretty(&similar)
            } else {
                let records = webclaw_core::select_records(html, url, &similar.selector, fields)
                    .map_err(|e| format!("with `similar`, fields are CSS selectors: {e}"))?;
                serde_json::to_string_pretty(
                    &serde_json::json!({"selector": similar.selector, "records": records}),
                )
            }
        }
        (_, _, Some(text)) => serde_json::to_string_pretty(&webclaw_core::find_text(html, text)),
        (Some(css), _, _) if fields.is_empty() => serde_json::to_string_pretty(
            &webclaw_core::select(html, url, css).map_err(|e| e.to_string())?,
        ),
        (Some(css), _, _) => serde_json::to_string_pretty(
            &webclaw_core::select_records(html, url, css, fields).map_err(|e| e.to_string())?,
        ),
        (_, Some(xpath), _) if fields.is_empty() => serde_json::to_string_pretty(
            &webclaw_core::select_xpath(html, url, xpath).map_err(|e| e.to_string())?,
        ),
        (_, Some(xpath), _) => serde_json::to_string_pretty(
            &webclaw_core::select_xpath_records(html, url, xpath, fields)
                .map_err(|e| e.to_string())?,
        ),
        (None, None, None) => return Err("nothing to pick".into()),
    };
    json.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::{check_select, slugify};

    #[test]
    fn chunk_overlap_needs_chunk_and_chunk_needs_a_size() {
        assert!(super::chunk_sizes(None, Some(10)).is_err());
        assert!(super::chunk_sizes(Some(0), None).is_err());
        assert_eq!(super::chunk_sizes(None, None), Ok(None));
        assert_eq!(super::chunk_sizes(Some(64), None), Ok(Some((64, 0))));
    }

    #[test]
    fn crawl_chunks_keep_the_counts_and_the_failed_pages() {
        let page = |url: &str, html: Option<&str>| webclaw_fetch::PageResult {
            url: url.into(),
            depth: 0,
            extraction: html.map(|h| webclaw_core::extract(h, Some(url)).unwrap()),
            error: html.is_none().then(|| "HTTP 500".to_string()),
            elapsed: std::time::Duration::ZERO,
            relevance: None,
        };
        let result = webclaw_fetch::CrawlResult {
            pages: vec![
                page(
                    "https://a.test/",
                    Some(
                        "<html><body><article><h1>A</h1><p>First page text that is long enough to keep.</p></article></body></html>",
                    ),
                ),
                page("https://a.test/broken", None),
            ],
            total: 2,
            ok: 1,
            errors: 1,
            skipped_by_robots: 0,
            elapsed_secs: 0.5,
            visited: Default::default(),
            remaining_frontier: Vec::new(),
        };
        let out: serde_json::Value =
            serde_json::from_str(&super::crawl_chunks_json(&result, 100, 0).unwrap()).unwrap();
        assert_eq!(
            (
                out["total"].as_u64(),
                out["ok"].as_u64(),
                out["errors"].as_u64()
            ),
            (Some(2), Some(1), Some(1))
        );
        assert_eq!(out["failed"][0]["url"], "https://a.test/broken");
        assert_eq!(out["failed"][0]["error"], "HTTP 500");
        assert_eq!(out["chunks"][0]["url"], "https://a.test/");
    }

    #[test]
    fn chunks_json_tags_every_chunk_with_its_page() {
        let out = super::chunks_json(
            &[
                ("https://a.test/", "# A\n\nOne."),
                ("https://b.test/", "# B\n\nTwo."),
            ],
            100,
            0,
        )
        .unwrap();
        let chunks: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(chunks[0]["url"], "https://a.test/");
        assert_eq!(chunks[1]["url"], "https://b.test/");
        assert_eq!(chunks[1]["index"], 0);
        assert_eq!(chunks[1]["heading_path"][0], "B");
    }

    #[test]
    fn an_unknown_block_type_is_refused_with_or_without_a_browser() {
        let mut config = webclaw_fetch::FetchConfig::default();
        assert!(super::apply_render(&mut config, None, None, Some("bogus")).is_err());
        assert!(super::apply_render(&mut config, None, None, Some("images,fonts")).is_ok());
    }

    #[test]
    fn every_browser_name_picks_its_profile() {
        use webclaw_fetch::BrowserProfile;
        for (name, expected) in [
            (None, BrowserProfile::Chrome),
            (Some("chrome"), BrowserProfile::Chrome),
            (Some("chrome-macos"), BrowserProfile::ChromeMacos),
            (Some("firefox"), BrowserProfile::Firefox),
            (Some("safari"), BrowserProfile::Safari),
            (Some("safari-ios"), BrowserProfile::SafariIos),
            (Some("edge"), BrowserProfile::Edge),
            (Some("random"), BrowserProfile::Random),
        ] {
            let picked = super::parse_browser(name).unwrap();
            assert_eq!(format!("{picked:?}"), format!("{expected:?}"), "{name:?}");
        }
        let err = super::parse_browser(Some("opera")).unwrap_err();
        assert!(err.contains("opera") && err.contains("safari-ios"), "{err}");
    }

    #[test]
    fn tool_catalog_excludes_lead_and_preserves_the_supported_tools() {
        let mut names: Vec<_> = super::WebclawMcp::tool_router()
            .list_all()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "batch",
                "brand",
                "crawl",
                "diff",
                "extract",
                "list_extractors",
                "map",
                "research",
                "scrape",
                "screenshot",
                "search",
                "select",
                "summarize",
                "vertical_scrape",
            ]
        );
    }

    #[test]
    fn slugify_multibyte_query_does_not_panic() {
        // Byte-slicing s[..60] would panic mid-codepoint on multibyte
        // alphanumerics; char-safe truncation must not.
        let q = "日本語のクエリ".repeat(20); // long, 3-byte chars
        let s = slugify(&q);
        assert!(
            s.chars().count() <= 60,
            "slug too long: {}",
            s.chars().count()
        );
    }

    #[test]
    fn slugify_ascii_unchanged_under_limit() {
        assert_eq!(slugify("Hello World Query"), "hello-world-query");
    }

    #[test]
    fn slugify_caps_long_ascii_at_60_chars() {
        let s = slugify(&"word ".repeat(40));
        assert!(s.len() <= 60);
    }

    #[test]
    fn select_options_are_checked_before_anything_runs() {
        let check = |args: serde_json::Value| {
            check_select(&serde_json::from_value(args).unwrap()).map(|text| text.is_some())
        };
        let url = "https://example.com/";
        let err =
            check(serde_json::json!({"url": url, "selector": "a", "regex": "("})).unwrap_err();
        assert!(err.contains("exactly one"), "{err}");
        assert!(check(serde_json::json!({"url": url, "regex": "x", "exact": true})).is_err());
        assert!(check(serde_json::json!({"url": url, "selector": "a", "exact": true})).is_err());
        assert!(
            check(serde_json::json!({"url": url, "selector": "a", "case_sensitive": true}))
                .is_err()
        );
        assert!(check(serde_json::json!({"url": url, "text": "a", "fields": {"x": "b"}})).is_err());
        assert_eq!(
            check(serde_json::json!({"url": url, "text": "a", "exact": true})),
            Ok(true)
        );
        assert_eq!(
            check(serde_json::json!({"url": url, "selector": "a", "similar": true})),
            Ok(false)
        );
        assert_eq!(
            check(serde_json::json!({"url": url, "tables": true})),
            Ok(false)
        );
        assert_eq!(
            check(serde_json::json!({"url": url, "selector": "a", "tables": false})),
            Ok(false)
        );
        for (extra, word) in [
            (serde_json::json!({"selector": "a"}), "exactly one"),
            (serde_json::json!({"xpath": "//a"}), "exactly one"),
            (serde_json::json!({"text": "a"}), "exactly one"),
            (serde_json::json!({"regex": "a"}), "exactly one"),
            (serde_json::json!({"similar": true}), "similar"),
            (serde_json::json!({"fields": {"x": "b"}}), "fields"),
            (serde_json::json!({"exact": true}), "exact"),
            (
                serde_json::json!({"case_sensitive": true}),
                "case_sensitive",
            ),
        ] {
            let mut args = serde_json::json!({"url": url, "tables": true});
            args.as_object_mut()
                .unwrap()
                .extend(extra.as_object().unwrap().clone());
            let err = check(args).unwrap_err();
            assert!(err.contains(word), "{extra} -> {err}");
        }
    }
}
