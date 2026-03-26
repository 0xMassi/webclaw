#![allow(dead_code)]
/// CLI entry point -- wires webclaw-core and webclaw-fetch into a single command.
/// All extraction and fetching logic lives in sibling crates; this is pure plumbing.
mod cloud;

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use clap::{Parser, ValueEnum};
use tracing_subscriber::EnvFilter;
use webclaw_core::{
    ChangeStatus, ContentDiff, ExtractionOptions, ExtractionResult, Metadata, extract_with_options,
    to_llm_text,
};
use webclaw_fetch::{
    BatchExtractResult, BrowserProfile, CrawlConfig, CrawlResult, Crawler, FetchClient,
    FetchConfig, FetchResult, PageResult, SitemapEntry,
};
use webclaw_llm::LlmProvider;
use webclaw_pdf::PdfMode;

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
             Use the webclaw Cloud API for automatic bypass: https://webclaw.io/pricing"
        ),
        EmptyReason::JsRequired => eprintln!(
            "\x1b[33mwarning:\x1b[0m No content extracted from {url}\n\
             This site requires JavaScript rendering (SPA).\n\
             Use the webclaw Cloud API for JS rendering: https://webclaw.io/pricing"
        ),
        EmptyReason::None => {}
    }
}

#[derive(Parser)]
#[command(name = "webclaw", about = "Extract web content for LLMs", version)]
struct Cli {
    /// URLs to fetch (multiple allowed)
    #[arg()]
    urls: Vec<String>,

    /// File with URLs (one per line)
    #[arg(long)]
    urls_file: Option<String>,

    /// Output format (markdown, json, text, llm)
    #[arg(short, long, default_value = "markdown")]
    format: OutputFormat,

    /// Browser to impersonate
    #[arg(short, long, default_value = "chrome")]
    browser: Browser,

    /// Proxy URL (http://user:pass@host:port or socks5://host:port)
    #[arg(short, long, env = "WEBCLAW_PROXY")]
    proxy: Option<String>,

    /// File with proxies (host:port:user:pass, one per line). Rotates per request.
    #[arg(long, env = "WEBCLAW_PROXY_FILE")]
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
    #[arg(long, env = "WEBCLAW_WEBHOOK_URL")]
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

    /// Force a specific LLM provider (ollama, openai, anthropic)
    #[arg(long, env = "WEBCLAW_LLM_PROVIDER")]
    llm_provider: Option<String>,

    /// Override the LLM model name
    #[arg(long, env = "WEBCLAW_LLM_MODEL")]
    llm_model: Option<String>,

    /// Override the LLM base URL (Ollama or OpenAI-compatible)
    #[arg(long, env = "WEBCLAW_LLM_BASE_URL")]
    llm_base_url: Option<String>,

    // -- Cloud API options --
    /// Webclaw Cloud API key for automatic fallback on bot-protected or JS-rendered sites
    #[arg(long, env = "WEBCLAW_API_KEY")]
    api_key: Option<String>,

    /// Force all requests through the cloud API (skip local extraction)
    #[arg(long)]
    cloud: bool,

    /// Output directory: save each page to a separate file instead of stdout.
    /// Works with --crawl, batch (multiple URLs), and single URL mode.
    /// Filenames are derived from URL paths (e.g. /docs/api -> docs/api.md).
    #[arg(long)]
    output_dir: Option<PathBuf>,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Markdown,
    Json,
    Text,
    Llm,
}

#[derive(Clone, ValueEnum)]
enum Browser {
    Chrome,
    Firefox,
    Random,
}

#[derive(Clone, ValueEnum, Default)]
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
        EnvFilter::new("webclaw=debug")
    } else {
        EnvFilter::try_from_env("WEBCLAW_LOG").unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

/// Build FetchConfig from CLI flags.
///
/// `--proxy` sets a single static proxy (no rotation).
/// `--proxy-file` loads a pool of proxies and rotates per-request.
/// `--proxy` takes priority: if both are set, only the single proxy is used.
fn build_fetch_config(cli: &Cli) -> FetchConfig {
    let (proxy, proxy_pool) = if cli.proxy.is_some() {
        (cli.proxy.clone(), Vec::new())
    } else if let Some(ref path) = cli.proxy_file {
        match webclaw_fetch::parse_proxy_file(path) {
            Ok(pool) => (None, pool),
            Err(e) => {
                eprintln!("warning: {e}");
                (None, Vec::new())
            }
        }
    } else if std::path::Path::new("proxies.txt").exists() {
        // Auto-load proxies.txt from working directory if present
        match webclaw_fetch::parse_proxy_file("proxies.txt") {
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

    FetchConfig {
        browser: cli.browser.clone().into(),
        proxy,
        proxy_pool,
        timeout: std::time::Duration::from_secs(cli.timeout),
        pdf_mode: cli.pdf_mode.clone().into(),
        headers,
        ..Default::default()
    }
}

fn build_extraction_options(cli: &Cli) -> ExtractionOptions {
    ExtractionOptions {
        include_selectors: cli
            .include
            .as_deref()
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        exclude_selectors: cli
            .exclude
            .as_deref()
            .map(|s| s.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default(),
        only_main_content: cli.only_main_content,
        include_raw_html: cli.raw_html,
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

/// Format an `ExtractionResult` into a string for the given output format.
fn format_output(result: &ExtractionResult, format: &OutputFormat, show_metadata: bool) -> String {
    match format {
        OutputFormat::Markdown => {
            let mut out = String::new();
            if show_metadata {
                out.push_str(&format_frontmatter(&result.metadata));
            }
            out.push_str(&result.content.markdown);
            out
        }
        OutputFormat::Json => serde_json::to_string_pretty(result).expect("serialization failed"),
        OutputFormat::Text => result.content.plain_text.clone(),
        OutputFormat::Llm => to_llm_text(result, result.metadata.url.as_deref()),
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
async fn fetch_and_extract(cli: &Cli) -> Result<FetchOutput, String> {
    // Local sources: read and extract as HTML
    if cli.stdin {
        let mut buf = String::new();
        io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| format!("failed to read stdin: {e}"))?;
        let options = build_extraction_options(cli);
        return extract_with_options(&buf, None, &options)
            .map(|r| FetchOutput::Local(Box::new(r)))
            .map_err(|e| format!("extraction error: {e}"));
    }

    if let Some(ref path) = cli.file {
        let html =
            std::fs::read_to_string(path).map_err(|e| format!("failed to read {path}: {e}"))?;
        let options = build_extraction_options(cli);
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
            cloud_client.ok_or("--cloud requires WEBCLAW_API_KEY (set via env or --api-key)")?;
        let options = build_extraction_options(cli);
        let format_str = match cli.format {
            OutputFormat::Markdown => "markdown",
            OutputFormat::Json => "json",
            OutputFormat::Text => "text",
            OutputFormat::Llm => "llm",
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
    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;
    let options = build_extraction_options(cli);
    let result = client
        .fetch_and_extract_with_options(url, &options)
        .await
        .map_err(|e| format!("fetch error: {e}"))?;

    // Check if we should fall back to cloud
    let reason = detect_empty(&result);
    if !matches!(reason, EmptyReason::None) {
        if let Some(ref c) = cloud_client {
            eprintln!("\x1b[36minfo:\x1b[0m falling back to cloud API...");
            let format_str = match cli.format {
                OutputFormat::Markdown => "markdown",
                OutputFormat::Json => "json",
                OutputFormat::Text => "text",
                OutputFormat::Llm => "llm",
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
async fn fetch_html(cli: &Cli) -> Result<FetchResult, String> {
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

    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;
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

async fn run_crawl(cli: &Cli) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--crawl requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    if cli.file.is_some() || cli.stdin {
        return Err("--crawl cannot be used with --file or --stdin".into());
    }

    let include_patterns: Vec<String> = cli
        .include_paths
        .as_deref()
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();
    let exclude_patterns: Vec<String> = cli
        .exclude_paths
        .as_deref()
        .map(|s| s.split(',').map(|p| p.trim().to_string()).collect())
        .unwrap_or_default();

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
        fetch: build_fetch_config(cli),
        max_depth: cli.depth,
        max_pages: cli.max_pages,
        concurrency: cli.concurrency,
        delay: std::time::Duration::from_millis(cli.delay),
        path_prefix: cli.path_prefix.clone(),
        use_sitemap: cli.sitemap,
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

    let max_pages = cli.max_pages;
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
                cli.max_pages,
                cli.depth,
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
                let filename = url_to_filename(&page.url, &cli.format);
                let content = format_output(extraction, &cli.format, cli.metadata);
                write_to_file(dir, &filename, &content)?;
                saved += 1;
            }
        }
        eprintln!("Saved {saved} files to {}", dir.display());
    } else {
        print_crawl_output(&result, &cli.format, cli.metadata);
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

async fn run_map(cli: &Cli) -> Result<(), String> {
    let url = cli
        .urls
        .first()
        .ok_or("--map requires a URL argument")
        .map(|u| normalize_url(u))?;
    let url = url.as_str();

    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;

    let entries = webclaw_fetch::sitemap::discover(&client, url)
        .await
        .map_err(|e| format!("sitemap discovery failed: {e}"))?;

    if entries.is_empty() {
        eprintln!("no sitemap URLs found for {url}");
    } else {
        eprintln!("discovered {} URLs", entries.len());
    }

    print_map_output(&entries, &cli.format);
    Ok(())
}

async fn run_batch(cli: &Cli, entries: &[(String, Option<String>)]) -> Result<(), String> {
    let client = Arc::new(
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?,
    );

    let urls: Vec<&str> = entries.iter().map(|(u, _)| u.as_str()).collect();
    let results = client.fetch_and_extract_batch(&urls, cli.concurrency).await;

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
                    .unwrap_or_else(|| url_to_filename(&r.url, &cli.format));
                let content = format_output(extraction, &cli.format, cli.metadata);
                write_to_file(dir, &filename, &content)?;
                saved += 1;
            }
        }
        eprintln!("Saved {saved} files to {}", dir.display());
    } else {
        print_batch_output(&results, &cli.format, cli.metadata);
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
                "title": format!("webclaw: {event}"),
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
            "text": format!("*webclaw: {event}*\n```{details}```")
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

async fn run_watch(cli: &Cli) -> Result<(), String> {
    let raw_url = cli.urls.first().ok_or("--watch requires a URL argument")?;
    let url = normalize_url(raw_url);

    let client =
        FetchClient::new(build_fetch_config(cli)).map_err(|e| format!("client error: {e}"))?;
    let options = build_extraction_options(cli);

    // Initial snapshot
    let mut previous = client
        .fetch_and_extract_with_options(&url, &options)
        .await
        .map_err(|e| format!("initial fetch failed: {e}"))?;

    eprintln!(
        "[watch] Initial snapshot: {url} ({} words)",
        previous.metadata.word_count
    );

    // Ctrl+C handler
    let cancelled = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&cancelled);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        flag.store(true, Ordering::Relaxed);
    });

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(cli.watch_interval)).await;

        if cancelled.load(Ordering::Relaxed) {
            eprintln!("[watch] Stopped");
            break;
        }

        let current = match client.fetch_and_extract_with_options(&url, &options).await {
            Ok(result) => result,
            Err(e) => {
                eprintln!("[watch] Fetch error ({}): {e}", timestamp());
                continue;
            }
        };

        let diff = webclaw_core::diff::diff(&previous, &current);

        if diff.status == ChangeStatus::Same {
            eprintln!("[watch] No changes ({})", timestamp());
        } else {
            print_diff_output(&diff, &cli.format);
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
                        // Pipe diff JSON to stdin, then detach
                        if let Some(mut stdin) = child.stdin.take() {
                            use tokio::io::AsyncWriteExt;
                            let _ = stdin.write_all(diff_json.as_bytes()).await;
                        }
                    }
                    Err(e) => eprintln!("[watch] Failed to run command: {e}"),
                }
            }

            // Fire webhook on change
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

async fn run_diff(cli: &Cli, snapshot_path: &str) -> Result<(), String> {
    // Load previous snapshot
    let snapshot_json = std::fs::read_to_string(snapshot_path)
        .map_err(|e| format!("failed to read snapshot {snapshot_path}: {e}"))?;
    let old: ExtractionResult = serde_json::from_str(&snapshot_json)
        .map_err(|e| format!("failed to parse snapshot JSON: {e}"))?;

    // Extract current version (handles PDF detection for URLs)
    let new_result = fetch_and_extract(cli).await?.into_extraction()?;

    let diff = webclaw_core::diff::diff(&old, &new_result);
    print_diff_output(&diff, &cli.format);

    Ok(())
}

async fn run_brand(cli: &Cli) -> Result<(), String> {
    let result = fetch_html(cli).await?;
    let enriched = enrich_html_with_stylesheets(&result.html, &result.url).await;
    let brand = webclaw_core::brand::extract_brand(
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
async fn build_llm_provider(cli: &Cli) -> Result<Box<dyn LlmProvider>, String> {
    if let Some(ref name) = cli.llm_provider {
        match name.as_str() {
            "ollama" => {
                let provider = webclaw_llm::providers::ollama::OllamaProvider::new(
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                );
                if !provider.is_available().await {
                    return Err("ollama is not running or unreachable".into());
                }
                Ok(Box::new(provider))
            }
            "openai" => {
                let provider = webclaw_llm::providers::openai::OpenAiProvider::new(
                    None,
                    cli.llm_base_url.clone(),
                    cli.llm_model.clone(),
                )
                .ok_or("OPENAI_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            "anthropic" => {
                let provider = webclaw_llm::providers::anthropic::AnthropicProvider::new(
                    None,
                    cli.llm_model.clone(),
                )
                .ok_or("ANTHROPIC_API_KEY not set")?;
                Ok(Box::new(provider))
            }
            other => Err(format!(
                "unknown LLM provider: {other} (use ollama, openai, or anthropic)"
            )),
        }
    } else {
        let chain = webclaw_llm::ProviderChain::default().await;
        if chain.is_empty() {
            return Err(
                "no LLM providers available -- start Ollama or set OPENAI_API_KEY / ANTHROPIC_API_KEY"
                    .into(),
            );
        }
        Ok(Box::new(chain))
    }
}

async fn run_llm(cli: &Cli) -> Result<(), String> {
    // Extract content from source first (handles PDF detection for URLs)
    let result = fetch_and_extract(cli).await?.into_extraction()?;

    let provider = build_llm_provider(cli).await?;
    let model = cli.llm_model.as_deref();

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

        let extracted = webclaw_llm::extract::extract_json(
            &result.content.plain_text,
            &schema,
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM extraction failed: {e}"))?;

        println!(
            "{}",
            serde_json::to_string_pretty(&extracted).expect("serialization failed")
        );
    } else if let Some(ref prompt) = cli.extract_prompt {
        let extracted = webclaw_llm::extract::extract_with_prompt(
            &result.content.plain_text,
            prompt,
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM extraction failed: {e}"))?;

        println!(
            "{}",
            serde_json::to_string_pretty(&extracted).expect("serialization failed")
        );
    } else if let Some(sentences) = cli.summarize {
        let summary = webclaw_llm::summarize::summarize(
            &result.content.plain_text,
            Some(sentences),
            provider.as_ref(),
            model,
        )
        .await
        .map_err(|e| format!("LLM summarization failed: {e}"))?;

        println!("{summary}");
    }

    Ok(())
}

/// Returns true if any LLM flag is set.
fn has_llm_flags(cli: &Cli) -> bool {
    cli.extract_json.is_some() || cli.extract_prompt.is_some() || cli.summarize.is_some()
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let cli = Cli::parse();
    init_logging(cli.verbose);

    // --map: sitemap discovery mode
    if cli.map {
        if let Err(e) = run_map(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --crawl: recursive crawl mode
    if cli.crawl {
        if let Err(e) = run_crawl(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --watch: poll a URL for changes
    if cli.watch {
        if let Err(e) = run_watch(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --diff-with: change tracking mode
    if let Some(ref snapshot_path) = cli.diff_with {
        if let Err(e) = run_diff(&cli, snapshot_path).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --brand: brand identity extraction mode
    if cli.brand {
        if let Err(e) = run_brand(&cli).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // LLM modes: --extract-json, --extract-prompt, --summarize
    if has_llm_flags(&cli) {
        if let Err(e) = run_llm(&cli).await {
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

    // Multi-URL batch mode
    if entries.len() > 1 {
        if let Err(e) = run_batch(&cli, &entries).await {
            eprintln!("error: {e}");
            process::exit(1);
        }
        return;
    }

    // --raw-html: skip extraction, dump the fetched HTML
    if cli.raw_html && cli.include.is_none() && cli.exclude.is_none() {
        match fetch_html(&cli).await {
            Ok(r) => println!("{}", r.html),
            Err(e) => {
                eprintln!("error: {e}");
                process::exit(1);
            }
        }
        return;
    }

    // Single-page extraction (handles both HTML and PDF via content-type detection)
    match fetch_and_extract(&cli).await {
        Ok(FetchOutput::Local(result)) => {
            if let Some(ref dir) = cli.output_dir {
                let url = cli
                    .urls
                    .first()
                    .map(|u| normalize_url(u))
                    .unwrap_or_default();
                let custom_name = entries.first().and_then(|(_, name)| name.clone());
                let filename = custom_name.unwrap_or_else(|| url_to_filename(&url, &cli.format));
                let content = format_output(&result, &cli.format, cli.metadata);
                if let Err(e) = write_to_file(dir, &filename, &content) {
                    eprintln!("error: {e}");
                    process::exit(1);
                }
            } else {
                print_output(&result, &cli.format, cli.metadata);
            }
        }
        Ok(FetchOutput::Cloud(resp)) => {
            print_cloud_output(&resp, &cli.format);
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
        let dir = std::env::temp_dir().join("webclaw_test_output_dir");
        let _ = std::fs::remove_dir_all(&dir);
        write_to_file(&dir, "nested/deep/file.md", "hello").unwrap();
        let content = std::fs::read_to_string(dir.join("nested/deep/file.md")).unwrap();
        assert_eq!(content, "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
