#![allow(dead_code)]
/// CLI entry point -- wires noxa-core and noxa-fetch into a single command.
/// All extraction and fetching logic lives in sibling crates; this is pure plumbing.
mod cloud;
mod config;

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{CommandFactory, FromArgMatches, Parser, ValueEnum};
use serde::Deserialize;
use tracing_subscriber::EnvFilter;
use noxa_core::{
    ChangeStatus, ContentDiff, ExtractionOptions, ExtractionResult, Metadata, extract_with_options,
    to_llm_text,
};
use noxa_fetch::{
    BatchExtractResult, BrowserProfile, CrawlConfig, CrawlResult, Crawler, FetchClient,
    FetchConfig, FetchResult, PageResult, SitemapEntry,
};
use noxa_llm::LlmProvider;
use noxa_pdf::PdfMode;

/// Known anti-bot challenge page titles (case-insensitive prefix match).
const ANTIBOT_TITLES: &[&str] = &[
    "just a moment",
    "attention required",
    "access denied",
    "checking your browser",
    "please wait",
    "one more step",
    "verify you are human",
    "bot verification",
    "security check",
    "ddos protection",
];

/// Detect why a page returned empty content.
enum EmptyReason {
    /// Anti-bot challenge page (Cloudflare, Akamai, etc.)
    Antibot,
    /// JS-only SPA that returns an empty shell without a browser
    JsRequired,
    /// Page has content — not empty
    None,
}

fn detect_empty(result: &ExtractionResult) -> EmptyReason {
    // Has real content — nothing to warn about
    if result.metadata.word_count > 50 || !result.content.markdown.is_empty() {
        return EmptyReason::None;
    }

    // Check for known anti-bot challenge titles
    if let Some(ref title) = result.metadata.title {
        let lower = title.to_lowercase();
        if ANTIBOT_TITLES.iter().any(|t| lower.starts_with(t)) {
            return EmptyReason::Antibot;
        }
    }

    // Empty content with no title or a generic SPA shell = JS-only site
    if result.metadata.word_count == 0 && result.content.links.is_empty() {
        return EmptyReason::JsRequired;
    }

    EmptyReason::None
}

fn warn_empty(url: &str, reason: &EmptyReason) {
    match reason {
        EmptyReason::Antibot => eprintln!(
            "\x1b[33mwarning:\x1b[0m Anti-bot protection detected on {url}\n\
             This site requires CAPTCHA solving or browser rendering.\n\
             Use the noxa Cloud API for automatic bypass: https://noxa.io/pricing"
        ),
        EmptyReason::JsRequired => eprintln!(
            "\x1b[33mwarning:\x1b[0m No content extracted from {url}\n\
             This site requires JavaScript rendering (SPA).\n\
             Use the noxa Cloud API for JS rendering: https://noxa.io/pricing"
        ),
        EmptyReason::None => {}
    }
}

#[derive(Parser)]
#[command(name = "noxa", about = "Extract web content for LLMs", version)]
struct Cli {
    /// Path to config.json (default: ./config.json, override with NOXA_CONFIG env var)
    #[arg(long, global = true)]
    config: Option<String>,

    /// URLs to fetch (multiple allowed)
    #[arg()]
    urls: Vec<String>,

    /// File with URLs (one per line)
    #[arg(long)]
    urls_file: Option<String>,

    /// Output format (markdown, json, text, llm, html)
    #[arg(short, long, default_value = "markdown")]
    format: OutputFormat,

    /// Browser to impersonate
    #[arg(short, long, default_value = "chrome")]
    browser: Browser,

    /// Proxy URL (http://user:pass@host:port or socks5://host:port)
    #[arg(short, long, env = "NOXA_PROXY")]
    proxy: Option<String>,

    /// File with proxies (host:port:user:pass, one per line). Rotates per request.
    #[arg(long, env = "NOXA_PROXY_FILE")]
    proxy_file: Option<String>,

    /// Request timeout in seconds
    #[arg(short, long, default_value = "30")]
    timeout: u64,

    /// Extract from local HTML file instead of fetching
    #[arg(long)]
    file: Option<String>,

    /// Read HTML from stdin
    #[arg(long)]
    stdin: bool,

    /// Include metadata in output (always included in JSON)
    #[arg(long)]
    metadata: bool,

    /// Output raw fetched HTML instead of extracting
    #[arg(long)]
    raw_html: bool,

    /// CSS selectors to include (comma-separated, e.g. "article,.content")
    #[arg(long)]
    include: Option<String>,

    /// CSS selectors to exclude (comma-separated, e.g. "nav,.sidebar,footer")
    #[arg(long)]
    exclude: Option<String>,

    /// Only extract main content (article/main element)
    #[arg(long)]
    only_main_content: bool,

    /// Custom headers (repeatable, e.g. -H "Cookie: foo=bar")
    #[arg(short = 'H', long = "header")]
    headers: Vec<String>,

    /// Cookie string (shorthand for -H "Cookie: ...")
    #[arg(long)]
    cookie: Option<String>,

    /// JSON cookie file (Chrome extension format: [{name, value, domain, ...}])
    #[arg(long)]
    cookie_file: Option<String>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,

    /// Compare against a previous JSON snapshot
    #[arg(long)]
    diff_with: Option<String>,

    /// Watch a URL for changes. Checks at the specified interval and reports diffs.
    #[arg(long)]
    watch: bool,

    /// Watch interval in seconds [default: 300]
    #[arg(long, default_value = "300")]
    watch_interval: u64,

    /// Command to run when changes are detected (receives diff JSON on stdin)
    #[arg(long)]
    on_change: Option<String>,

    /// Webhook URL: POST a JSON payload when an operation completes.
    /// Works with crawl, batch, watch (on change), and single URL modes.
    #[arg(long, env = "NOXA_WEBHOOK_URL")]
    webhook: Option<String>,

    /// Extract brand identity (colors, fonts, logo)
    #[arg(long)]
    brand: bool,

    // -- PDF options --
    /// PDF extraction mode: auto (error on empty) or fast (return whatever text is found)
    #[arg(long, default_value = "auto")]
    pdf_mode: PdfModeArg,

    // -- Crawl options --
    /// Enable recursive crawling of same-domain links
    #[arg(long)]
    crawl: bool,

    /// Max crawl depth [default: 1]
    #[arg(long, default_value = "1")]
    depth: usize,

    /// Max pages to crawl [default: 20]
    #[arg(long, default_value = "20")]
    max_pages: usize,

    /// Max concurrent requests [default: 5]
    #[arg(long, default_value = "5")]
    concurrency: usize,

    /// Delay between requests in ms [default: 100]
    #[arg(long, default_value = "100")]
    delay: u64,

    /// Only crawl URLs matching this path prefix
    #[arg(long)]
    path_prefix: Option<String>,

    /// Glob patterns for crawl URL paths to include (comma-separated, e.g. "/api/*,/guides/**")
    #[arg(long)]
    include_paths: Option<String>,

    /// Glob patterns for crawl URL paths to exclude (comma-separated, e.g. "/changelog/*,/blog/*")
    #[arg(long)]
    exclude_paths: Option<String>,

    /// Path to save/resume crawl state. On Ctrl+C: saves progress. On start: resumes if file exists.
    #[arg(long)]
    crawl_state: Option<PathBuf>,

    /// Seed crawl frontier from sitemap discovery (robots.txt + /sitemap.xml)
    #[arg(long)]
    sitemap: bool,

    /// Discover URLs from sitemap and print them (one per line; JSON array with --format json)
    #[arg(long)]
    map: bool,

    // -- LLM options --
    /// Extract structured JSON using LLM (pass a JSON schema string or @file)
    #[arg(long)]
    extract_json: Option<String>,

    /// Extract using natural language prompt
    #[arg(long)]
    extract_prompt: Option<String>,

    /// Summarize content using LLM (optional: number of sentences, default 3)
    #[arg(long, num_args = 0..=1, default_missing_value = "3")]
    summarize: Option<usize>,

    /// Force a specific LLM provider (gemini, ollama, openai, anthropic)
    #[arg(long, env = "NOXA_LLM_PROVIDER")]
    llm_provider: Option<String>,

    /// Override the LLM model name
    #[arg(long, env = "NOXA_LLM_MODEL")]
    llm_model: Option<String>,

    /// Override the LLM base URL (Ollama or OpenAI-compatible)
    #[arg(long, env = "NOXA_LLM_BASE_URL")]
    llm_base_url: Option<String>,

    // -- Cloud API options --
    /// Noxa Cloud API key for automatic fallback on bot-protected or JS-rendered sites
    #[arg(long, env = "NOXA_API_KEY")]
    api_key: Option<String>,

    /// Force all requests through the cloud API (skip local extraction)
    #[arg(long)]
    cloud: bool,

    /// Run deep research on a topic via the cloud API. Requires --api-key.
    /// Saves full result (report + sources + findings) to a JSON file.
    #[arg(long)]
    research: Option<String>,

    /// Enable deep research mode (longer, more thorough report). Used with --research.
    #[arg(long)]
    deep: bool,

    /// Output directory: save each page to a separate file instead of stdout.
    /// Works with --crawl, batch (multiple URLs), and single URL mode.
    /// Filenames are derived from URL paths (e.g. /docs/api -> docs/api.md).
    #[arg(long)]
    output_dir: Option<PathBuf>,
}

#[derive(Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum OutputFormat {
    Markdown,
    Json,
    Text,
    Llm,
    Html,
}

#[derive(Clone, Debug, ValueEnum, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Browser {
    Chrome,
    Firefox,
    Random,
}

#[derive(Clone, Debug, ValueEnum, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum PdfModeArg {
    /// Error if PDF has no extractable text (catches scanned PDFs)
    #[default]
    Auto,
    /// Return whatever text is found, even if empty
    Fast,
}

impl From<PdfModeArg> for PdfMode {
    fn from(arg: PdfModeArg) -> Self {
        match arg {
            PdfModeArg::Auto => PdfMode::Auto,
            PdfModeArg::Fast => PdfMode::Fast,
        }
    }
}

impl From<Browser> for BrowserProfile {
    fn from(b: Browser) -> Self {
        match b {
            Browser::Chrome => BrowserProfile::Chrome,
            Browser::Firefox => BrowserProfile::Firefox,
            Browser::Random => BrowserProfile::Random,
        }
    }
}

fn init_logging(verbose: bool) {
    let filter = if verbose {
        EnvFilter::new("noxa=debug")
    } else {
        EnvFilter::try_from_env("NOXA_LOG").unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Build FetchConfig from CLI flags.
///
/// `--proxy` sets a single static proxy (no rotation).
/// `--proxy-file` loads a pool of proxies and rotates per-request.
/// `--proxy` takes priority: if both are set, only the single proxy is used.
fn build_fetch_config(cli: &Cli, resolved: &config::ResolvedConfig) -> FetchConfig {
    let (proxy, proxy_pool) = if cli.proxy.is_some() {
        (cli.proxy.clone(), Vec::new())
    } else if let Some(ref path) = cli.proxy_file {
        match noxa_fetch::parse_proxy_file(path) {
            Ok(pool) => (None, pool),
            Err(e) => {
                eprintln!("warning: {e}");
                (None, Vec::new())
            }
        }
    } else if std::path::Path::new("proxies.txt").exists() {
        // Auto-load proxies.txt from working directory if present
        match noxa_fetch::parse_proxy_file("proxies.txt") {
            Ok(pool) if !pool.is_empty() => {
                eprintln!("loaded {} proxies from proxies.txt", pool.len());
                (None, pool)
            }
            _ => (None, Vec::new()),
        }
    } else {
        (None, Vec::new())
    };

    let mut headers = std::collections::HashMap::from([(
        "Accept-Language".to_string(),
        "en-US,en;q=0.9".to_string(),
    )]);

    // Parse -H "Key: Value" flags
    for h in &cli.headers {
        if let Some((key, val)) = h.split_once(':') {
            headers.insert(key.trim().to_string(), val.trim().to_string());
        }
    }

    // --cookie shorthand
    if let Some(ref cookie) = cli.cookie {
        headers.insert("Cookie".to_string(), cookie.clone());
    }

    // --cookie-file: parse JSON array of {name, value, domain, ...}
    if let Some(ref path) = cli.cookie_file {
        match parse_cookie_file(path) {
            Ok(cookie_str) => {
                // Merge with existing cookies if --cookie was also provided
                if let Some(existing) = headers.get("Cookie") {
                    headers.insert("Cookie".to_string(), format!("{existing}; {cookie_str}"));
                } else {
                    headers.insert("Cookie".to_string(), cookie_str);
                }
            }
            Err(e) => {
                eprintln!("error: failed to parse cookie file: {e}");
                process::exit(1);
            }
        }
    }

    FetchConfig {
        browser: resolved.browser.clone().into(),
        proxy,
        proxy_pool,
        timeout: std::time::Duration::from_secs(resolved.timeout),
        pdf_mode: resolved.pdf_mode.clone().into(),
        headers,
        ..Default::default()
    }
}

/// Parse a JSON cookie file (Chrome extension format) into a Cookie header string.
/// Supports: [{name, value, domain, path, secure, httpOnly, expirationDate, ...}]
fn parse_cookie_file(path: &str) -> Result<String, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    let cookies: Vec<serde_json::Value> =
        serde_json::from_str(&content).map_err(|e| format!("invalid JSON: {e}"))?;

    let pairs: Vec<String> = cookies
        .iter()
        .filter_map(|c| {
            let name = c.get("name")?.as_str()?;
            let value = c.get("value")?.as_str()?;
            Some(format!("{name}={value}"))
        })
        .collect();

    if pairs.is_empty() {
        return Err("no cookies found in file".to_string());
    }

    Ok(pairs.join("; "))
}

fn build_extraction_options(resolved: &config::ResolvedConfig) -> ExtractionOptions {
    ExtractionOptions {
        include_selectors: resolved.include_selectors.clone(),
        exclude_selectors: resolved.exclude_selectors.clone(),
        only_main_content: resolved.only_main_content,
        include_raw_html: resolved.raw_html || matches!(resolved.format, OutputFormat::Html),
    }
}

/// Normalize a URL: prepend `https://` if no scheme is present.
fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();
    if trimmed.contains("://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    }
}

/// Derive a filename from a URL for `--output-dir`.
///
/// Strips the scheme/host, maps the path to a filesystem path, and appends
/// an extension matching the output format.
fn url_to_filename(raw_url: &str, format: &OutputFormat) -> String {
    let ext = match format {
        OutputFormat::Markdown | OutputFormat::Llm => "md",
        OutputFormat::Json => "json",
        OutputFormat::Text => "txt",
        OutputFormat::Html => "html",
    };

    let parsed = url::Url::parse(raw_url);
    let (host, path, query) = match &parsed {
        Ok(u) => (
            u.host_str().unwrap_or("unknown").to_string(),
            u.path().to_string(),
            u.query().map(String::from),
        ),
        Err(_) => (String::new(), String::new(), None),
    };

    let mut stem = path.trim_matches('/').to_string();
    if stem.is_empty() {
        // Use hostname for root URLs to avoid collisions in batch mode
        let clean_host = host.strip_prefix("www.").unwrap_or(&host);
        stem = format!("{}/index", clean_host.replace('.', "_"));
    }

    // Append query params so /p?id=123 doesn't collide with /p?id=456
    if let Some(q) = query {
        stem = format!("{stem}_{q}");
    }

    // Sanitize: keep alphanumeric, dash, underscore, dot, slash
    let sanitized: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | '/') {
                c
            } else {
                '_'
            }
        })
        .collect();

    format!("{sanitized}.{ext}")
}

/// Write extraction output to a file inside `dir`, creating parent dirs as needed.
fn write_to_file(dir: &Path, filename: &str, content: &str) -> Result<(), String> {
    let dest = dir.join(filename);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
    }
    std::fs::write(&dest, content)
        .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    let word_count = content.split_whitespace().count();
    eprintln!("Saved: {} ({word_count} words)", dest.display());
    Ok(())
}

/// Get raw HTML from an extraction result, falling back to markdown if unavailable.
fn raw_html_or_markdown(result: &ExtractionResult) -> &str {
    result
        .content
        .raw_html
        .as_deref()
        .unwrap_or(&result.content.markdown)
}

/// Format an `ExtractionResult` into a string for the given output format.
fn format_output(result: &ExtractionResult, format: &OutputFormat, show_metadata: bool) -> String {
    match format {
        OutputFormat::Markdown => {
            let mut out = String::new();
            if show_metadata {
                out.push_str(&format_frontmatter(&result.metadata));
            }
            out.push_str(&result.content.markdown);
            if !result.structured_data.is_empty() {
                out.push_str("\n\n## Structured Data\n\n```json\n");
                out.push_str(
                    &serde_json::to_string_pretty(&result.structured_data).unwrap_or_default(),
                );
                out.push_str("\n```");
            }
            out
        }
        OutputFormat::Json => serde_json::to_string_pretty(result).expect("serialization failed"),
        OutputFormat::Text => result.content.plain_text.clone(),
        OutputFormat::Llm => to_llm_text(result, result.metadata.url.as_deref()),
        OutputFormat::Html => raw_html_or_markdown(result).to_string(),
    }
}

/// Collect all URLs from positional args + --urls-file, normalizing bare domains.
///
/// Returns `(url, optional_custom_filename)` pairs. Custom filenames come from
/// CSV-style lines in `--urls-file`: `url,filename`. Plain lines (no comma) get
/// `None` so the caller auto-generates the filename from the URL.
fn collect_urls(cli: &Cli) -> Result<Vec<(String, Option<String>)>, String> {
    let mut entries: Vec<(String, Option<String>)> =
        cli.urls.iter().map(|u| (normalize_url(u), None)).collect();

    if let Some(ref path) = cli.urls_file {
        let content =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') {
                continue;
            }
            if let Some((url_part, name_part)) = trimmed.split_once(',') {
                let name = name_part.trim();
                let custom = if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                };
                entries.push((normalize_url(url_part.trim()), custom));
            } else {
                entries.push((normalize_url(trimmed), None));
            }
        }
    }

    Ok(entries)
}

/// Result that can be either a local extraction or a cloud API JSON response.
enum FetchOutput {
    Local(Box<ExtractionResult>),
    Cloud(serde_json::Value),
}

impl FetchOutput {
    /// Get the local ExtractionResult, or try to parse it from the cloud response.
    fn into_extraction(self) -> Result<ExtractionResult, String> {
        match self {
            FetchOutput::Local(r) => Ok(*r),
            FetchOutput::Cloud(resp) => {
                // Cloud response has an "extraction" field with the full ExtractionResult
                resp.get("extraction")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .or_else(|| serde_json::from_value(resp.clone()).ok())
                    .ok_or_else(|| "could not parse extraction from cloud response".to_string())
            }
        }
    }
}

/// Fetch a URL and extract content, handling PDF detection automatically.
/// Falls back to cloud API when bot protection or JS rendering is detected.
async fn fetch_and_extract(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
) -> Result<FetchOutput, String> {
    // Local sources: read and extract as HTML
    if cli.stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        let options = build_extraction_options(resolved);
        return extract_with_options(&buf, None, &options)
            .map(|r| FetchOutput::Local(Box::new(r)))
            .map_err(|e| format!("extraction error: {e}"));
    }

    if let Some(ref path) = cli.file {
        let html =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        let options = build_extraction_options(resolved);
        return extract_with_options(&html, None, &options)
            .map(|r| FetchOutput::Local(Box::new(r)))
            .map_err(|e| format!("extraction error: {e}"));
    }

    let raw_url = cli
        .urls
        .first()
        .ok_or("no input provided -- pass a URL, --file, or --stdin")?;
    let url = normalize_url(raw_url);
    let url = url.as_str();

    let cloud_client = cloud::CloudClient::new(cli.api_key.as_deref());

    // --cloud: skip local, go straight to cloud API
    if cli.cloud {
        let c =
            cloud_client.ok_or("--cloud requires NOXA_API_KEY (set via env or --api-key)")?;
        let options = build_extraction_options(resolved);
        let format_str = match resolved.format {
            OutputFormat::Markdown => "markdown",
            OutputFormat::Json => "json",
            OutputFormat::Text => "text",
            OutputFormat::Llm => "llm",
            OutputFormat::Html => "html",
        };
        let resp = c
            .scrape(
                url,
                &[format_str],
                &options.include_selectors,
                &options.exclude_selectors,
                options.only_main_content,
            )
            .await?;
        return Ok(FetchOutput::Cloud(resp));
    }

    // Normal path: try local first
    let client = FetchClient::new(build_fetch_config(cli, resolved))
        .map_err(|e| format!("client error: {e}"))?;
    let options = build_extraction_options(resolved);
    let result = client
        .fetch_and_extract_with_options(url, &options)
        .await
        .map_err(|e| format!("fetch error: {e}"))?;

    // Check if we should fall back to cloud
    let reason = detect_empty(&result);
    if !matches!(reason, EmptyReason::None) {
        if let Some(ref c) = cloud_client {
            eprintln!("\x1b[36minfo:\x1b[0m falling back to cloud API...");
            let format_str = match resolved.format {
                OutputFormat::Markdown => "markdown",
                OutputFormat::Json => "json",
                OutputFormat::Text => "text",
                OutputFormat::Llm => "llm",
                OutputFormat::Html => "html",
            };
            match c
                .scrape(
                    url,
                    &[format_str],
                    &options.include_selectors,
                    &options.exclude_selectors,
                    options.only_main_content,
                )
                .await
            {
                Ok(resp) => return Ok(FetchOutput::Cloud(resp)),
                Err(e) => {
                    eprintln!("\x1b[33mwarning:\x1b[0m cloud fallback failed: {e}");
                    // Fall through to return the local result with a warning
                }
            }
        }
        warn_empty(url, &reason);
    }

    Ok(FetchOutput::Local(Box::new(result)))
}

/// Fetch raw HTML from a URL (no extraction). Used for --raw-html and brand extraction.
async fn fetch_html(cli: &Cli, resolved: &config::ResolvedConfig) -> Result<FetchResult, String> {
    if cli.stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        return Ok(FetchResult {
            html: buf,
            url: String::new(),
            status: 200,
            headers: Default::default(),
            elapsed: Default::default(),
        });
    }

    if let Some(ref path) = cli.file {
        let html =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        return Ok(FetchResult {
            html,
            url: String::new(),
            status: 200,
            headers: Default::default(),
            elapsed: Default::default(),
        });
    }

    let raw_url = cli
        .urls
        .first()
        .ok_or("no input provided -- pass a URL, --file, or --stdin")?;
    let url = normalize_url(raw_url);

    let client = FetchClient::new(build_fetch_config(cli, resolved))
        .map_err(|e| format!("client error: {e}"))?;
    client
        .fetch(&url)
        .await
        .map_err(|e| format!("fetch error: {e}"))
}

/// Fetch external stylesheets referenced in HTML and inject them as `<style>` blocks.
/// This allows brand extraction to see colors/fonts from external CSS files.
async fn enrich_html_with_stylesheets(html: &str, base_url: &str) -> String {
    let base = match url::Url::parse(base_url) {
        Ok(u) => u,
        Err(_) => return html.to_string(),
    };

    // Extract stylesheet hrefs from <link rel="stylesheet" href="...">
    let re = regex::Regex::new(
        r#"<link[^>]+rel=["']stylesheet["'][^>]+href=["']([^"']+)["']|<link[^>]+href=["']([^"']+)["'][^>]+rel=["']stylesheet["']"#
    ).unwrap();

    let hrefs: Vec<String> = re
        .captures_iter(html)
        .filter_map(|cap| {
            let href = cap.get(1).or(cap.get(2))?;
            Some(
                base.join(href.as_str())
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| href.as_str().to_string()),
            )
        })
        .take(10)
        .collect();

    if hrefs.is_empty() {
        return html.to_string();
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap_or_default();

    let mut extra_css = String::new();
    for href in &hrefs {
        if let Ok(resp) = client.get(href).send().await
            && resp.status().is_success()
            && let Ok(body) = resp.text().await
            && !body.trim_start().starts_with("<!")
            && body.len() < 2_000_000
        {
            extra_css.push_str("\n<style>\n");
            extra_css.push_str(&body);
            extra_css.push_str("\n</style>\n");
        }
    }

    if extra_css.is_empty() {
        return html.to_string();
    }

    if let Some(pos) = html.to_lowercase().find("</head>") {
        let mut enriched = String::with_capacity(html.len() + extra_css.len());
        enriched.push_str(&html[..pos]);
        enriched.push_str(&extra_css);
        enriched.push_str(&html[pos..]);
        enriched
    } else {
        format!("{extra_css}{html}")
    }
}

fn format_frontmatter(meta: &Metadata) -> String {
    let mut lines = vec!["---".to_string()];

    if let Some(title) = &meta.title {
        lines.push(format!("title: \"{title}\""));
    }
    if let Some(author) = &meta.author {
        lines.push(format!("author: \"{author}\""));
    }
    if let Some(date) = &meta.published_date {
        lines.push(format!("date: \"{date}\""));
    }
    if let Some(url) = &meta.url {
        lines.push(format!("source: \"{url}\""));
    }
    if meta.word_count > 0 {
        lines.push(format!("word_count: {}", meta.word_count));
    }

    lines.push("---".to_string());
    lines.push(String::new()); // blank line after frontmatter
    lines.join("\n")
}

fn print_output(result: &ExtractionResult, format: &OutputFormat, show_metadata: bool) {
    match format {
        OutputFormat::Markdown => {
            if show_metadata {
                print!("{}", format_frontmatter(&result.metadata));
            }
            println!("{}", result.content.markdown);
            if !result.structured_data.is_empty() {
                println!(
                    "\n## Structured Data\n\n```json\n{}\n```",
                    serde_json::to_string_pretty(&result.structured_data).unwrap_or_default()
                );
            }
        }
        OutputFormat::Json => {
            // serde_json::to_string_pretty won't fail on our types
            println!(
                "{}",
                serde_json::to_string_pretty(result).expect("serialization failed")
            );
        }
        OutputFormat::Text => {
            println!("{}", result.content.plain_text);
        }
        OutputFormat::Llm => {
            println!("{}", to_llm_text(result, result.metadata.url.as_deref()));
        }
        OutputFormat::Html => {
            println!("{}", raw_html_or_markdown(result));
        }
    }
}

/// Print cloud API response in the requested format.
fn print_cloud_output(resp: &serde_json::Value, format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(resp).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            // Cloud response has content.markdown
            if let Some(md) = resp
                .get("content")
                .and_then(|c| c.get("markdown"))
                .and_then(|m| m.as_str())
            {
                println!("{md}");
            } else if let Some(md) = resp.get("markdown").and_then(|m| m.as_str()) {
                println!("{md}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(resp).expect("serialization failed")
                );
            }
        }
        OutputFormat::Text => {
            if let Some(txt) = resp
                .get("content")
                .and_then(|c| c.get("plain_text"))
                .and_then(|t| t.as_str())
            {
                println!("{txt}");
            } else {
                // Fallback to markdown or raw JSON
                print_cloud_output(resp, &OutputFormat::Markdown);
            }
        }
        OutputFormat::Llm => {
            if let Some(llm) = resp
                .get("content")
                .and_then(|c| c.get("llm_text"))
                .and_then(|t| t.as_str())
            {
                println!("{llm}");
            } else {
                print_cloud_output(resp, &OutputFormat::Markdown);
            }
        }
        OutputFormat::Html => {
            if let Some(html) = resp
                .get("content")
                .and_then(|c| c.get("raw_html"))
                .and_then(|h| h.as_str())
            {
                println!("{html}");
            } else {
                print_cloud_output(resp, &OutputFormat::Markdown);
            }
        }
    }
}

fn print_diff_output(diff: &ContentDiff, format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(diff).expect("serialization failed")
            );
        }
        // For markdown/text/llm, show a human-readable summary
        _ => {
            println!("Status: {:?}", diff.status);
            println!("Word count delta: {:+}", diff.word_count_delta);

            if !diff.metadata_changes.is_empty() {
                println!("\nMetadata changes:");
                for change in &diff.metadata_changes {
                    println!(
                        "  {}: {} -> {}",
                        change.field,
                        change.old.as_deref().unwrap_or("(none)"),
                        change.new.as_deref().unwrap_or("(none)"),
                    );
                }
            }

            if !diff.links_added.is_empty() {
                println!("\nLinks added:");
                for link in &diff.links_added {
                    println!("  + {} ({})", link.href, link.text);
                }
            }

            if !diff.links_removed.is_empty() {
                println!("\nLinks removed:");
                for link in &diff.links_removed {
                    println!("  - {} ({})", link.href, link.text);
                }
            }

            if let Some(ref text_diff) = diff.text_diff {
                println!("\n{text_diff}");
            }
        }
    }
}

fn print_crawl_output(result: &CrawlResult, format: &OutputFormat, show_metadata: bool) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("# Page: {}\n", page.url);
                if show_metadata {
                    print!("{}", format_frontmatter(&extraction.metadata));
                }
                println!("{}", extraction.content.markdown);
                println!();
            }
        }
        OutputFormat::Text => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("# Page: {}\n", page.url);
                println!("{}", extraction.content.plain_text);
                println!();
            }
        }
        OutputFormat::Llm => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("{}", to_llm_text(extraction, Some(page.url.as_str())));
                println!();
            }
        }
        OutputFormat::Html => {
            for page in &result.pages {
                let Some(ref extraction) = page.extraction else {
                    continue;
                };
                println!("---");
                println!("<!-- Page: {} -->\n", page.url);
                println!("{}", raw_html_or_markdown(extraction));
                println!();
            }
        }
    }
}

fn print_batch_output(results: &[BatchExtractResult], format: &OutputFormat, show_metadata: bool) {
    match format {
        OutputFormat::Json => {
            // Build a JSON array of {url, result?, error?} objects
            let entries: Vec<serde_json::Value> = results
                .iter()
                .map(|r| match &r.result {
                    Ok(extraction) => serde_json::json!({
                        "url": r.url,
                        "result": extraction,
                    }),
                    Err(e) => serde_json::json!({
                        "url": r.url,
                        "error": e.to_string(),
                    }),
                })
                .collect();
            println!(
                "{}",
                serde_json::to_string_pretty(&entries).expect("serialization failed")
            );
        }
        OutputFormat::Markdown => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("# {}\n", r.url);
                        if show_metadata {
                            print!("{}", format_frontmatter(&extraction.metadata));
                        }
                        println!("{}", extraction.content.markdown);
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
        OutputFormat::Text => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("# {}\n", r.url);
                        println!("{}", extraction.content.plain_text);
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
        OutputFormat::Llm => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("{}", to_llm_text(extraction, Some(r.url.as_str())));
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
        OutputFormat::Html => {
            for r in results {
                match &r.result {
                    Ok(extraction) => {
                        println!("---");
                        println!("<!-- {} -->\n", r.url);
                        println!("{}", raw_html_or_markdown(extraction));
                        println!();
                    }
                    Err(e) => {
                        eprintln!("error: {} -- {}", r.url, e);
                    }
                }
            }
        }
    }
}

fn print_map_output(entries: &[SitemapEntry], format: &OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(entries).expect("serialization failed")
            );
        }
        _ => {
            for entry in entries {
                println!("{}", entry.url);
            }
        }
    }
}

/// Format a streaming progress line for a completed page.
fn format_progress(page: &PageResult, index: usize, max_pages: usize) -> String {
    let status = if page.error.is_some() { "ERR" } else { "OK " };
    let timing = format!("{}ms", page.elapsed.as_millis());
    let detail = if let Some(ref extraction) = page.extraction {
        format!(", {} words", extraction.metadata.word_count)
    } else if let Some(ref err) = page.error {
        format!(" ({err})")
    } else {
        String::new()
    };
    format!(
        "[{index}/{max_pages}] {status} {} ({timing}{detail})",
        page.url
    )
}

async fn run_crawl(cli: &Cli, resolved: &config::ResolvedConfig) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--crawl requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    if cli.file.is_some() || cli.stdin {
        return Err("--crawl cannot be used with --file or --stdin".into());
    }

    let include_patterns = resolved.include_paths.clone();
    let exclude_patterns = resolved.exclude_paths.clone();

    // Set up streaming progress channel
    let (progress_tx, mut progress_rx) = tokio::sync::broadcast::channel::<PageResult>(100);

    // Set up cancel flag for Ctrl+C handling
    let cancel_flag = Arc::new(AtomicBool::new(false));

    // Register Ctrl+C handler when --crawl-state is set
    let state_path = cli.crawl_state.clone();
    if state_path.is_some() {
        let flag = Arc::clone(&cancel_flag);
        tokio::spawn(async move {
            tokio::signal::ctrl_c().await.ok();
            flag.store(true, Ordering::Relaxed);
            eprintln!("\nCtrl+C received, saving crawl state...");
        });
    }

    let config = CrawlConfig {
        fetch: build_fetch_config(cli, resolved),
        max_depth: resolved.depth,
        max_pages: resolved.max_pages,
        concurrency: resolved.concurrency,
        delay: std::time::Duration::from_millis(resolved.delay),
        path_prefix: resolved.path_prefix.clone(),
        use_sitemap: resolved.use_sitemap,
        include_patterns,
        exclude_patterns,
        progress_tx: Some(progress_tx),
        cancel_flag: Some(Arc::clone(&cancel_flag)),
    };

    // Load resume state if --crawl-state file exists
    let resume_state = state_path
        .as_ref()
        .and_then(|p| Crawler::load_state(p))
        .inspect(|s| {
            eprintln!(
                "Resuming crawl: {} pages already visited, {} URLs in frontier",
                s.visited.len(),
                s.frontier.len(),
            );
        });

    let max_pages = resolved.max_pages;
    let completed_offset = resume_state.as_ref().map_or(0, |s| s.completed_pages);

    // Spawn background task to print streaming progress to stderr
    let progress_handle = tokio::spawn(async move {
        let mut count = completed_offset;
        while let Ok(page) = progress_rx.recv().await {
            count += 1;
            eprintln!("{}", format_progress(&page, count, max_pages));
        }
    });

    let crawler = Crawler::new(url, config).map_err(|e| format!("crawler error: {e}"))?;
    let result = crawler.crawl(url, resume_state).await;

    // Drop the crawler (and its progress_tx clone) so the progress task finishes
    drop(crawler);
    let _ = progress_handle.await;

    // If cancelled via Ctrl+C and --crawl-state is set, save state for resume
    let was_cancelled = cancel_flag.load(Ordering::Relaxed);
    if was_cancelled {
        if let Some(ref path) = state_path {
            Crawler::save_state(
                path,
                url,
                &result.visited,
                &result.remaining_frontier,
                completed_offset + result.pages.len(),
                resolved.max_pages,
                resolved.depth,
            )?;
            eprintln!(
                "Crawl state saved to {} ({} pages completed). Resume with --crawl-state {}",
                path.display(),
                completed_offset + result.pages.len(),
                path.display(),
            );
        }
    } else if let Some(ref path) = state_path {
        // Crawl completed normally — clean up state file
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
    }

    // Log per-page errors and extraction warnings to stderr
    for page in &result.pages {
        if let Some(ref err) = page.error {
            eprintln!("error: {} -- {}", page.url, err);
        } else if let Some(ref extraction) = page.extraction {
            let reason = detect_empty(extraction);
            if !matches!(reason, EmptyReason::None) {
                warn_empty(&page.url, &reason);
            }
        }
    }

    if let Some(ref dir) = cli.output_dir {
        let mut saved = 0usize;
        for page in &result.pages {
            if let Some(ref extraction) = page.extraction {
                let filename = url_to_filename(&page.url, &resolved.format);
                let content = format_output(extraction, &resolved.format, resolved.metadata);
                write_to_file(dir, &filename, &content)?;
                saved += 1;
            }
        }
        eprintln!("Saved {saved} files to {}", dir.display());
    } else {
        print_crawl_output(&result, &resolved.format, resolved.metadata);
    }

    eprintln!(
        "Crawled {} pages ({} ok, {} errors) in {:.1}s",
        result.total, result.ok, result.errors, result.elapsed_secs,
    );

    // Fire webhook on crawl complete
    if let Some(ref webhook_url) = cli.webhook {
        let urls: Vec<&str> = result.pages.iter().map(|p| p.url.as_str()).collect();
        fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "crawl_complete",
                "total": result.total,
                "ok": result.ok,
                "errors": result.errors,
                "elapsed_secs": result.elapsed_secs,
                "urls": urls,
            }),
        );
        // Brief pause so the async webhook has time to fire
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    if result.errors > 0 {
        Err(format!(
            "{} of {} pages failed",
            result.errors, result.total
        ))
    } else {
        Ok(())
    }
}

async fn run_map(cli: &Cli, resolved: &config::ResolvedConfig) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--map requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    let client = FetchClient::new(build_fetch_config(cli, resolved))
        .map_err(|e| format!("client error: {e}"))?;

    let entries = noxa_fetch::sitemap::discover(&client, url)
        .await
        .map_err(|e| format!("sitemap discovery failed: {e}"))?;

    if entries.is_empty() {
        eprintln!("no sitemap URLs found for {url}");
    } else {
        eprintln!("discovered {} URLs", entries.len());
    }

    print_map_output(&entries, &resolved.format);
    Ok(())
}

async fn run_batch(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
    entries: &[(String, Option<String>)],
) -> Result<(), String> {
    let client = Arc::new(
        FetchClient::new(build_fetch_config(cli, resolved))
            .map_err(|e| format!("client error: {e}"))?,
    );

    let urls: Vec<&str> = entries.iter().map(|(u, _)| u.as_str()).collect();
    let options = build_extraction_options(resolved);
    let results = client
        .fetch_and_extract_batch_with_options(&urls, resolved.concurrency, &options)
        .await;

    let ok = results.iter().filter(|r| r.result.is_ok()).count();
    let errors = results.len() - ok;

    // Log errors and extraction warnings to stderr
    for r in &results {
        if let Err(ref e) = r.result {
            eprintln!("error: {} -- {}", r.url, e);
        } else if let Ok(ref extraction) = r.result {
            let reason = detect_empty(extraction);
            if !matches!(reason, EmptyReason::None) {
                warn_empty(&r.url, &reason);
            }
        }
    }

    // Build a lookup of custom filenames by URL
    let custom_names: std::collections::HashMap<&str, &str> = entries
        .iter()
        .filter_map(|(url, name)| name.as_deref().map(|n| (url.as_str(), n)))
        .collect();

    if let Some(ref dir) = cli.output_dir {
        let mut saved = 0usize;
        for r in &results {
            if let Ok(ref extraction) = r.result {
                let filename = custom_names
                    .get(r.url.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| url_to_filename(&r.url, &resolved.format));
                let content = format_output(extraction, &resolved.format, resolved.metadata);
                write_to_file(dir, &filename, &content)?;
                saved += 1;
            }
        }
        eprintln!("Saved {saved} files to {}", dir.display());
    } else {
        print_batch_output(&results, &resolved.format, resolved.metadata);
    }

    eprintln!(
        "Fetched {} URLs ({} ok, {} errors)",
        results.len(),
        ok,
        errors
    );

    // Fire webhook on batch complete
    if let Some(ref webhook_url) = cli.webhook {
        let urls: Vec<&str> = results.iter().map(|r| r.url.as_str()).collect();
        fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "batch_complete",
                "total": results.len(),
                "ok": ok,
                "errors": errors,
                "urls": urls,
            }),
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    if errors > 0 {
        Err(format!("{errors} of {} URLs failed", results.len()))
    } else {
        Ok(())
    }
}

fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let hours = (now % 86400) / 3600;
    let minutes = (now % 3600) / 60;
    let seconds = now % 60;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}

/// Fire a webhook POST with a JSON payload. Non-blocking — errors logged to stderr.
/// Auto-detects Discord and Slack webhook URLs and wraps the payload accordingly.
fn fire_webhook(url: &str, payload: &serde_json::Value) {
    let url = url.to_string();
    let is_discord = url.contains("discord.com/api/webhooks");
    let is_slack = url.contains("hooks.slack.com");

    let body = if is_discord {
        let event = payload
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        let details = serde_json::to_string_pretty(payload).unwrap_or_default();
        serde_json::json!({
            "embeds": [{
                "title": format!("noxa: {event}"),
                "description": format!("```json\n{details}\n```"),
                "color": 5814783
            }]
        })
        .to_string()
    } else if is_slack {
        let event = payload
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        let details = serde_json::to_string_pretty(payload).unwrap_or_default();
        serde_json::json!({
            "text": format!("*noxa: {event}*\n```{details}```")
        })
        .to_string()
    } else {
        serde_json::to_string(payload).unwrap_or_default()
    };
    tokio::spawn(async move {
        match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => match c
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
            {
                Ok(resp) => {
                    eprintln!(
                        "[webhook] POST {} -> {}",
                        &url[..url.len().min(60)],
                        resp.status()
                    );
                }
                Err(e) => eprintln!("[webhook] POST failed: {e}"),
            },
            Err(e) => eprintln!("[webhook] client error: {e}"),
        }
    });
}

async fn run_watch(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
    urls: &[String],
) -> Result<(), String> {
    if urls.is_empty() {
        return Err("--watch requires at least one URL".into());
    }

    let client = Arc::new(
        FetchClient::new(build_fetch_config(cli, resolved))
            .map_err(|e| format!("client error: {e}"))?,
    );
    let options = build_extraction_options(resolved);

    // Ctrl+C handler
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        flag.store(true, Ordering::Relaxed);
    });

    // Single-URL mode: preserve original behavior exactly
    if urls.len() == 1 {
        return run_watch_single(cli, resolved, &client, &options, &urls[0], &cancelled).await;
    }

    // Multi-URL mode: batch fetch, diff each, report aggregate
    run_watch_multi(cli, resolved, &client, &options, urls, &cancelled).await
}

/// Original single-URL watch loop -- backward compatible.
async fn run_watch_single(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
    client: &Arc<FetchClient>,
    options: &ExtractionOptions,
    url: &str,
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let mut previous = client
        .fetch_and_extract_with_options(url, options)
        .await
        .map_err(|e| format!("initial fetch failed: {e}"))?;

    eprintln!(
        "[watch] Initial snapshot: {url} ({} words)",
        previous.metadata.word_count
    );

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(cli.watch_interval)).await;

        if cancelled.load(Ordering::Relaxed) {
            eprintln!("[watch] Stopped");
            break;
        }

        let current = match client.fetch_and_extract_with_options(url, options).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[watch] Fetch error ({}): {e}", timestamp());
                continue;
            }
        };

        let diff = noxa_core::diff::diff(&previous, &current);

        if diff.status == ChangeStatus::Same {
            eprintln!("[watch] No changes ({})", timestamp());
        } else {
            print_diff_output(&diff, &resolved.format);
            eprintln!("[watch] Changes detected! ({})", timestamp());

            if let Some(ref cmd) = cli.on_change {
                let diff_json = serde_json::to_string(&diff).unwrap_or_default();
                eprintln!("[watch] Running: {cmd}");
                match tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(mut child) => {
                        if let Some(mut stdin) = child.stdin.take() {
                            use tokio::io::AsyncWriteExt;
                            let _ = stdin.write_all(diff_json.as_bytes()).await;
                        }
                    }
                    Err(e) => eprintln!("[watch] Failed to run command: {e}"),
                }
            }

            if let Some(ref webhook_url) = cli.webhook {
                fire_webhook(
                    webhook_url,
                    &serde_json::json!({
                        "event": "watch_change",
                        "url": url,
                        "status": format!("{:?}", diff.status),
                        "word_count_delta": diff.word_count_delta,
                        "metadata_changes": diff.metadata_changes.len(),
                        "links_added": diff.links_added.len(),
                        "links_removed": diff.links_removed.len(),
                    }),
                );
            }

            previous = current;
        }
    }

    Ok(())
}

/// Multi-URL watch loop -- batch fetch all URLs, diff each, report aggregate.
async fn run_watch_multi(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
    client: &Arc<FetchClient>,
    options: &ExtractionOptions,
    urls: &[String],
    cancelled: &Arc<AtomicBool>,
) -> Result<(), String> {
    let url_refs: Vec<&str> = urls.iter().map(|u| u.as_str()).collect();

    // Initial pass: fetch all URLs in parallel
    let initial_results = client
        .fetch_and_extract_batch_with_options(&url_refs, resolved.concurrency, options)
        .await;

    let mut snapshots = std::collections::HashMap::new();
    let mut ok_count = 0usize;
    let mut err_count = 0usize;

    for r in initial_results {
        match r.result {
            Ok(extraction) => {
                snapshots.insert(r.url, extraction);
                ok_count += 1;
            }
            Err(e) => {
                eprintln!("[watch] Initial fetch error: {} -- {e}", r.url);
                err_count += 1;
            }
        }
    }

    eprintln!(
        "[watch] Watching {} URLs (interval: {}s)",
        urls.len(),
        cli.watch_interval
    );
    eprintln!("[watch] Initial snapshots: {ok_count} ok, {err_count} errors");

    let mut check_number = 0u64;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(cli.watch_interval)).await;

        if cancelled.load(Ordering::Relaxed) {
            eprintln!("[watch] Stopped");
            break;
        }

        check_number += 1;

        let current_results = client
            .fetch_and_extract_batch_with_options(&url_refs, resolved.concurrency, options)
            .await;

        let mut changed: Vec<serde_json::Value> = Vec::new();
        let mut same_count = 0usize;
        let mut fetch_errors = 0usize;

        for r in current_results {
            match r.result {
                Ok(current) => {
                    if let Some(previous) = snapshots.get(&r.url) {
                        let diff = noxa_core::diff::diff(previous, &current);
                        if diff.status == ChangeStatus::Same {
                            same_count += 1;
                        } else {
                            changed.push(serde_json::json!({
                                "url": r.url,
                                "word_count_delta": diff.word_count_delta,
                            }));
                            snapshots.insert(r.url, current);
                        }
                    } else {
                        // URL failed initially, first successful fetch -- store as baseline
                        snapshots.insert(r.url, current);
                        same_count += 1;
                    }
                }
                Err(e) => {
                    eprintln!("[watch] Fetch error: {} -- {e}", r.url);
                    fetch_errors += 1;
                }
            }
        }

        let ts = timestamp();
        let err_suffix = if fetch_errors > 0 {
            format!(", {fetch_errors} errors")
        } else {
            String::new()
        };

        if changed.is_empty() {
            eprintln!(
                "[watch] Check {check_number} ({ts}): 0 changed, {same_count} same{err_suffix}"
            );
        } else {
            eprintln!(
                "[watch] Check {check_number} ({ts}): {} changed, {same_count} same{err_suffix}",
                changed.len(),
            );
            for entry in &changed {
                let url = entry["url"].as_str().unwrap_or("?");
                let delta = entry["word_count_delta"].as_i64().unwrap_or(0);
                eprintln!("  -> {url} (word delta: {delta:+})");
            }

            // Fire --on-change once with all changes
            if let Some(ref cmd) = cli.on_change {
                let payload = serde_json::json!({
                    "event": "watch_changes",
                    "check_number": check_number,
                    "total_urls": urls.len(),
                    "changed": changed.len(),
                    "same": same_count,
                    "changes": changed,
                });
                let payload_json = serde_json::to_string(&payload).unwrap_or_default();
                eprintln!("[watch] Running: {cmd}");
                match tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(cmd)
                    .stdin(std::process::Stdio::piped())
                    .spawn()
                {
                    Ok(mut child) => {
                        if let Some(mut stdin) = child.stdin.take() {
                            use tokio::io::AsyncWriteExt;
                            let _ = stdin.write_all(payload_json.as_bytes()).await;
                        }
                    }
                    Err(e) => eprintln!("[watch] Failed to run command: {e}"),
                }
            }

            // Fire webhook once with aggregate payload
            if let Some(ref webhook_url) = cli.webhook {
                fire_webhook(
                    webhook_url,
                    &serde_json::json!({
                        "event": "watch_changes",
                        "check_number": check_number,
                        "total_urls": urls.len(),
                        "changed": changed.len(),
                        "same": same_count,
                        "changes": changed,
                    }),
                );
            }
        }
    }

    Ok(())
}

async fn run_diff(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
    snapshot_path: &str,
) -> Result<(), String> {
    // Load previous snapshot
    let snapshot_json = std::fs::read_to_string(snapshot_path)
        .map_err(|e| format!("failed to read snapshot {snapshot_path}: {e}"))?;
    let old: ExtractionResult = serde_json::from_str(&snapshot_json)
        .map_err(|e| format!("failed to parse snapshot JSON: {e}"))?;

    // Extract current version (handles PDF detection for URLs)
    let new_result = fetch_and_extract(cli, resolved).await?.into_extraction()?;

    let diff = noxa_core::diff::diff(&old, &new_result);
    print_diff_output(&diff, &resolved.format);

    Ok(())
}

async fn run_brand(cli: &Cli, resolved: &config::ResolvedConfig) -> Result<(), String> {
    let result = fetch_html(cli, resolved).await?;
    let enriched = enrich_html_with_stylesheets(&result.html, &result.url).await;
    let brand = noxa_core::brand::extract_brand(
        &enriched,
        Some(result.url.as_str()).filter(|s| !s.is_empty()),
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&brand).expect("serialization failed")
    );
    Ok(())
}

/// Build an LLM provider based on CLI flags, or fall back to the default chain.
async fn build_llm_provider(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
) -> Result<Box<dyn LlmProvider>, String> {
    if let Some(ref name) = resolved.llm_provider {
        match name.as_str() {
            "gemini" => {
                let provider = noxa_llm::providers::gemini_cli::GeminiCliProvider::new(
                    resolved.llm_model.clone(),
                );
                if !provider.is_available().await {
                    return Err(
                        "gemini CLI not found on PATH -- install it or omit --llm-provider".into(),
                    );
                }
                Ok(Box::new(provider))
            }
            "ollama" => {
                let provider = noxa_llm::providers::ollama::OllamaProvider::new(
                    cli.llm_base_url.clone(),
                    resolved.llm_model.clone(),
                );
                if !provider.is_available().await {
                    return Err("ollama is not running or unreachable".into());
                }
                Ok(Box::new(provider))
            }
            "openai" => {
                let provider = noxa_llm::providers::openai::OpenAiProvider::new(
                    None,
                    cli.llm_base_url.clone(),
                    resolved.llm_model.clone(),
                )
                .ok_or("OPENAI_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            "anthropic" => {
                let provider = noxa_llm::providers::anthropic::AnthropicProvider::new(
                    None,
                    resolved.llm_model.clone(),
                )
                .ok_or("ANTHROPIC_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            other => Err(format!(
                "unknown LLM provider: {other} (use gemini, ollama, openai, or anthropic)"
            )),
        }
    } else {
        let chain = noxa_llm::ProviderChain::default().await;
        if chain.is_empty() {
            return Err(
                "no LLM providers available -- install the gemini CLI, start Ollama, or set OPENAI_API_KEY / ANTHROPIC_API_KEY"
                    .into(),
            );
        }
        Ok(Box::new(chain))
    }
}

async fn run_llm(cli: &Cli, resolved: &config::ResolvedConfig) -> Result<(), String> {
    // Extract content from source first (handles PDF detection for URLs)
    let result = fetch_and_extract(cli, resolved).await?.into_extraction()?;

    let provider = build_llm_provider(cli, resolved).await?;
    let model = resolved.llm_model.as_deref();

    if let Some(ref schema_input) = cli.extract_json {
        // Support @file syntax for loading schema from file
        let schema_str = if let Some(path) = schema_input.strip_prefix('@') {
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read schema file {path}: {e}"))?
        } else {
            schema_input.clone()
        };

        let schema: serde_json::Value =
            serde_json::from_str(&schema_str).map_err(|e| format!("invalid JSON schema: {e}"))?;

        let t = std::time::Instant::now();
        let extracted = noxa_llm::extract::extract_json(
            &result.content.plain_text,
            &schema,
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM extraction failed: {e}"))?;
        eprintln!("LLM: {:.1}s", t.elapsed().as_secs_f64());

        println!(
            "{}",
            serde_json::to_string_pretty(&extracted).expect("serialization failed")
        );
    } else if let Some(ref prompt) = cli.extract_prompt {
        let t = std::time::Instant::now();
        let extracted = noxa_llm::extract::extract_with_prompt(
            &result.content.plain_text,
            prompt,
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM extraction failed: {e}"))?;
        eprintln!("LLM: {:.1}s", t.elapsed().as_secs_f64());

        println!(
            "{}",
            serde_json::to_string_pretty(&extracted).expect("serialization failed")
        );
    } else if let Some(sentences) = cli.summarize {
        let t = std::time::Instant::now();
        let summary = noxa_llm::summarize::summarize(
            &result.content.plain_text,
            Some(sentences),
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM summarization failed: {e}"))?;
        eprintln!("LLM: {:.1}s", t.elapsed().as_secs_f64());

        println!("{summary}");
    }

    Ok(())
}

/// Batch LLM extraction: fetch each URL, run LLM on extracted content, save/print results.
/// URLs are processed sequentially to respect LLM provider rate limits.
async fn run_batch_llm(
    cli: &Cli,
    resolved: &config::ResolvedConfig,
    entries: &[(String, Option<String>)],
) -> Result<(), String> {
    let client = FetchClient::new(build_fetch_config(cli, resolved))
        .map_err(|e| format!("client error: {e}"))?;
    let options = build_extraction_options(resolved);
    let provider = build_llm_provider(cli, resolved).await?;
    let model = resolved.llm_model.as_deref();

    // Pre-parse schema once if --extract-json is used
    let schema = if let Some(ref schema_input) = cli.extract_json {
        let schema_str = if let Some(path) = schema_input.strip_prefix('@') {
            std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read schema file {path}: {e}"))?
        } else {
            schema_input.clone()
        };
        Some(
            serde_json::from_str::<serde_json::Value>(&schema_str)
                .map_err(|e| format!("invalid JSON schema: {e}"))?,
        )
    } else {
        None
    };

    // Build custom filename lookup from entries
    let custom_names: std::collections::HashMap<&str, &str> = entries
        .iter()
        .filter_map(|(url, name)| name.as_deref().map(|n| (url.as_str(), n)))
        .collect();

    let total = entries.len();
    let mut ok = 0usize;
    let mut errors = 0usize;
    let mut all_results: Vec<serde_json::Value> = Vec::with_capacity(total);

    for (i, (url, _)) in entries.iter().enumerate() {
        let idx = i + 1;
        eprint!("[{idx}/{total}] {url} ");

        // Fetch and extract page content
        let extraction = match client.fetch_and_extract_with_options(url, &options).await {
            Ok(r) => r,
            Err(e) => {
                errors += 1;
                let msg = format!("fetch failed: {e}");
                eprintln!("-> error: {msg}");
                all_results.push(serde_json::json!({ "url": url, "error": msg }));
                continue;
            }
        };

        let text = &extraction.content.plain_text;

        // Run the appropriate LLM operation
        let llm_start = std::time::Instant::now();
        let llm_result = if let Some(ref schema) = schema {
            noxa_llm::extract::extract_json(text, schema, provider.as_ref(), model)
                .await
                .map(LlmOutput::Json)
        } else if let Some(ref prompt) = cli.extract_prompt {
            noxa_llm::extract::extract_with_prompt(text, prompt, provider.as_ref(), model)
                .await
                .map(LlmOutput::Json)
        } else if let Some(sentences) = cli.summarize {
            noxa_llm::summarize::summarize(text, Some(sentences), provider.as_ref(), model)
                .await
                .map(LlmOutput::Text)
        } else {
            unreachable!("run_batch_llm called without LLM flags")
        };
        let llm_elapsed = llm_start.elapsed();

        match llm_result {
            Ok(output) => {
                ok += 1;

                let (output_str, result_json) = match &output {
                    LlmOutput::Json(v) => {
                        let s = serde_json::to_string_pretty(v).expect("serialization failed");
                        let j = serde_json::json!({ "url": url, "result": v });
                        (s, j)
                    }
                    LlmOutput::Text(s) => {
                        let j = serde_json::json!({ "url": url, "result": s });
                        (s.clone(), j)
                    }
                };

                // Count top-level fields/items for progress display
                let detail = match &output {
                    LlmOutput::Json(v) => match v {
                        serde_json::Value::Object(m) => format!("{} fields", m.len()),
                        serde_json::Value::Array(a) => format!("{} items", a.len()),
                        _ => "done".to_string(),
                    },
                    LlmOutput::Text(s) => {
                        let words = s.split_whitespace().count();
                        format!("{words} words")
                    }
                };
                eprintln!("-> extracted {detail} ({:.1}s)", llm_elapsed.as_secs_f64());

                if let Some(ref dir) = cli.output_dir {
                    let filename = custom_names
                        .get(url.as_str())
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| url_to_filename(url, &OutputFormat::Json));
                    write_to_file(dir, &filename, &output_str)?;
                } else {
                    println!("--- {url}");
                    println!("{output_str}");
                    println!();
                }

                all_results.push(result_json);
            }
            Err(e) => {
                errors += 1;
                let msg = format!("LLM extraction failed: {e}");
                eprintln!("-> error: {msg}");
                all_results.push(serde_json::json!({ "url": url, "error": msg }));
            }
        }
    }

    eprintln!("Processed {total} URLs ({ok} ok, {errors} errors)");

    if let Some(ref webhook_url) = cli.webhook {
        fire_webhook(
            webhook_url,
            &serde_json::json!({
                "event": "batch_llm_complete",
                "total": total,
                "ok": ok,
                "errors": errors,
            }),
        );
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    if errors > 0 {
        Err(format!("{errors} of {total} URLs failed"))
    } else {
        Ok(())
    }
}

/// Intermediate type to hold LLM output before formatting.
enum LlmOutput {
    Json(serde_json::Value),
    Text(String),
}

/// Returns true if any LLM flag is set.
fn has_llm_flags(cli: &Cli) -> bool {
    cli.extract_json.is_some() || cli.extract_prompt.is_some() || cli.summarize.is_some()
}

async fn run_research(cli: &Cli, query: &str) -> Result<(), String> {
    let api_key = cli
        .api_key
        .as_deref()
        .ok_or("--research requires NOXA_API_KEY (set via env or --api-key)")?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("http client error: {e}"))?;

    let mut body = serde_json::json!({ "query": query });
    if cli.deep {
        body["deep"] = serde_json::json!(true);
    }

    eprintln!("Starting research: {query}");
    if cli.deep {
        eprintln!("Deep mode enabled (longer, more thorough)");
    }

    // Start job
    let resp = client
        .post("https://api.noxa.io/v1/research")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("API error: {e}"))?
        .json::<serde_json::Value>()
        .await
        .map_err(|e| format!("parse error: {e}"))?;

    let job_id = resp
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or("API did not return a job ID")?
        .to_string();

    eprintln!("Job started: {job_id}");

    // Poll
    for poll in 0..200 {
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let status_resp = client
            .get(format!("https://api.noxa.io/v1/research/{job_id}"))
            .header("Authorization", format!("Bearer {api_key}"))
            .send()
            .await
            .map_err(|e| format!("poll error: {e}"))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| format!("parse error: {e}"))?;

        let status = status_resp
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        match status {
            "completed" => {
                let report = status_resp
                    .get("report")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                // Save full result to JSON file
                let slug: String = query
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
                let slug = if slug.len() > 50 { &slug[..50] } else { &slug };
                let filename = format!("research-{slug}.json");

                let json = serde_json::to_string_pretty(&status_resp).unwrap_or_default();
                std::fs::write(&filename, &json)
                    .map_err(|e| format!("failed to write {filename}: {e}"))?;

                let elapsed = status_resp
                    .get("elapsed_ms")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let sources = status_resp
                    .get("sources_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let findings = status_resp
                    .get("findings_count")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);

                eprintln!(
                    "Research complete: {sources} sources, {findings} findings, {:.1}s",
                    elapsed as f64 / 1000.0
                );
                eprintln!("Saved to: {filename}");

                // Print report to stdout
                if !report.is_empty() {
                    println!("{report}");
                }

                return Ok(());
            }
            "failed" => {
                let error = status_resp
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                return Err(format!("Research failed: {error}"));
            }
            _ => {
                if poll % 10 == 9 {
                    eprintln!("Still researching... ({:.0}s)", (poll + 1) as f64 * 3.0);
                }
            }
        }
    }

    Err(format!(
        "Research timed out after ~10 minutes. Check status: GET /v1/research/{job_id}"
    ))
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    // Use low-level API to get both typed Cli and ArgMatches for ValueSource detection.
    let matches = Cli::command().get_matches();
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| e.exit());

    // Load config BEFORE init_logging so verbose from config takes effect.
    let cfg = config::NoxaConfig::load(cli.config.as_deref());
    let resolved = config::resolve(&cli, &matches, &cfg);

    init_logging(resolved.verbose);

    // --map: sitemap discovery mode
    if cli.map {
        if let Err(e) = run_map(&cli, &resolved).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --crawl: recursive crawl mode
    if cli.crawl {
        if let Err(e) = run_crawl(&cli, &resolved).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --watch: poll URL(s) for changes
    if cli.watch {
        let watch_urls: Vec<String> = match collect_urls(&cli) {
            Ok(entries) => entries.into_iter().map(|(url, _)| url).collect(),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        };
        if let Err(e) = run_watch(&cli, &resolved, &watch_urls).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --diff-with: change tracking mode
    if let Some(ref snapshot_path) = cli.diff_with {
        if let Err(e) = run_diff(&cli, &resolved, snapshot_path).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --brand: brand identity extraction mode
    if cli.brand {
        if let Err(e) = run_brand(&cli, &resolved).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --research: deep research via cloud API
    if let Some(ref query) = cli.research {
        if let Err(e) = run_research(&cli, query).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // Collect all URLs from args + --urls-file
    let entries = match collect_urls(&cli) {
        Ok(u) => u,
        Err(e) => {
            eprintln!("error: {e}");
            process::exit(1);
        }
    };

    // LLM modes: --extract-json, --extract-prompt, --summarize
    // When multiple URLs are provided, run batch LLM extraction over all of them.
    if has_llm_flags(&cli) {
        if entries.len() > 1 {
            if let Err(e) = run_batch_llm(&cli, &resolved, &entries).await {
                eprintln!("error: {e}");
                process::exit(1);
            }
        } else if let Err(e) = run_llm(&cli, &resolved).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // Multi-URL batch mode
    if entries.len() > 1 {
        if let Err(e) = run_batch(&cli, &resolved, &entries).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --raw-html: skip extraction, dump the fetched HTML
    if resolved.raw_html && resolved.include_selectors.is_empty() && resolved.exclude_selectors.is_empty() {
        match fetch_html(&cli, &resolved).await {
            Ok(r) => println!("{}", r.html),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        return;
    }

    // Single-page extraction (handles both HTML and PDF via content-type detection)
    match fetch_and_extract(&cli, &resolved).await {
        Ok(FetchOutput::Local(result)) => {
            if let Some(ref dir) = cli.output_dir {
                let url = cli
                    .urls
                    .first()
                    .map(|u| normalize_url(u))
                    .unwrap_or_default();
                let custom_name = entries.first().and_then(|(_, name)| name.clone());
                let filename =
                    custom_name.unwrap_or_else(|| url_to_filename(&url, &resolved.format));
                let content = format_output(&result, &resolved.format, resolved.metadata);
                if let Err(e) = write_to_file(dir, &filename, &content) {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            } else {
                print_output(&result, &resolved.format, resolved.metadata);
            }
        }
        Ok(FetchOutput::Cloud(resp)) => {
            print_cloud_output(&resp, &resolved.format);
        }
        Err(e) => {
            eprintln!("{e}");
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_to_filename_root() {
        assert_eq!(
            url_to_filename("https://example.com/", &OutputFormat::Markdown),
            "example_com/index.md"
        );
        assert_eq!(
            url_to_filename("https://example.com", &OutputFormat::Markdown),
            "example_com/index.md"
        );
    }

    #[test]
    fn url_to_filename_path() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Markdown),
            "docs/api.md"
        );
    }

    #[test]
    fn url_to_filename_trailing_slash() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api/", &OutputFormat::Markdown),
            "docs/api.md"
        );
    }

    #[test]
    fn url_to_filename_nested_path() {
        assert_eq!(
            url_to_filename("https://example.com/blog/my-post", &OutputFormat::Markdown),
            "blog/my-post.md"
        );
    }

    #[test]
    fn url_to_filename_query_params() {
        assert_eq!(
            url_to_filename("https://example.com/p?id=123", &OutputFormat::Markdown),
            "p_id_123.md"
        );
    }

    #[test]
    fn url_to_filename_json_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Json),
            "docs/api.json"
        );
    }

    #[test]
    fn url_to_filename_text_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Text),
            "docs/api.txt"
        );
    }

    #[test]
    fn url_to_filename_llm_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Llm),
            "docs/api.md"
        );
    }

    #[test]
    fn url_to_filename_html_format() {
        assert_eq!(
            url_to_filename("https://example.com/docs/api", &OutputFormat::Html),
            "docs/api.html"
        );
    }

    #[test]
    fn url_to_filename_special_chars() {
        // Spaces and special chars get replaced with underscores
        assert_eq!(
            url_to_filename(
                "https://example.com/path%20with%20spaces",
                &OutputFormat::Markdown
            ),
            "path_20with_20spaces.md"
        );
    }

    #[test]
    fn write_to_file_creates_dirs() {
        let dir = std::env::temp_dir().join("noxa_test_output_dir");
        let _ = std::fs::remove_dir_all(&dir);
        write_to_file(&dir, "nested/deep/file.md", "hello").unwrap();
        let content = std::fs::read_to_string(dir.join("nested/deep/file.md")).unwrap();
        assert_eq!(content, "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod enum_deserialize_tests {
    use super::*;

    #[test]
    fn test_output_format_deserialize() {
        let f: OutputFormat = serde_json::from_str("\"llm\"").unwrap();
        assert!(matches!(f, OutputFormat::Llm));
        let f: OutputFormat = serde_json::from_str("\"markdown\"").unwrap();
        assert!(matches!(f, OutputFormat::Markdown));
    }

    #[test]
    fn test_browser_deserialize() {
        let b: Browser = serde_json::from_str("\"firefox\"").unwrap();
        assert!(matches!(b, Browser::Firefox));
    }

    #[test]
    fn test_pdf_mode_deserialize() {
        let p: PdfModeArg = serde_json::from_str("\"fast\"").unwrap();
        assert!(matches!(p, PdfModeArg::Fast));
    }
}
