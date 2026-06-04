//! Input handling and fetching: config building, URL/cookie parsing, empty-page
//! detection, output-file writing, and the fetch+extract entry points (local,
//! remote, and cloud fallback).

use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process;

use webclaw_core::{ExtractionOptions, ExtractionResult, extract_with_options};
use webclaw_fetch::{FetchClient, FetchConfig, FetchResult};

use crate::cli::Cli;

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

/// URL host/path fragments that indicate a GDPR/cookie consent redirect.
const CONSENT_URL_FRAGMENTS: &[&str] = &[
    "://consent.",
    "/consent?",
    "/consent/",
    "collectconsent",
    "consentcheck",
    "/cmp/",
    "guce.advertising.com",
];

/// English consent-wall title prefixes. Many providers localize this page, so
/// this is a best-effort secondary signal. URL shape is the primary signal.
const CONSENT_TITLES: &[&str] = &[
    "before you continue",
    "your privacy choices",
    "we value your privacy",
    "we care about your privacy",
    "cookie consent",
    "consent required",
];

/// Detect why a page returned empty or near-empty content.
#[derive(Debug, PartialEq, Eq)]
pub enum EmptyReason {
    /// Anti-bot challenge page (Cloudflare, Akamai, etc.)
    Antibot,
    /// GDPR/cookie consent redirect.
    ConsentWall,
    /// JS-only SPA that returns an empty shell without a browser
    JsRequired,
    /// Page has content.
    None,
}

pub fn detect_empty(result: &ExtractionResult) -> EmptyReason {
    // Consent walls can have a tiny body, so check before the content
    // short-circuit.
    if is_consent_wall(result) {
        return EmptyReason::ConsentWall;
    }

    // Has real content. Nothing to warn about.
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

/// A consent wall is identified by either:
/// 1. The final URL pointing at a known consent host/path, or
/// 2. A consent-wall title prefix with a very small body.
fn is_consent_wall(result: &ExtractionResult) -> bool {
    if let Some(ref url) = result.metadata.url {
        let lower = url.to_ascii_lowercase();
        if CONSENT_URL_FRAGMENTS
            .iter()
            .any(|fragment| lower.contains(fragment))
        {
            return true;
        }
    }

    if result.metadata.word_count <= 50
        && let Some(ref title) = result.metadata.title
    {
        let lower = title.to_lowercase();
        if CONSENT_TITLES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return true;
        }
    }

    false
}

pub fn warn_empty(url: &str, reason: &EmptyReason) {
    match reason {
        EmptyReason::Antibot => eprintln!(
            "\x1b[33mwarning:\x1b[0m Anti-bot protection detected on {url}\n\
             This site requires CAPTCHA solving or browser rendering.\n\
             Use the webclaw Cloud API for automatic bypass: https://webclaw.io/pricing"
        ),
        EmptyReason::ConsentWall => eprintln!(
            "\x1b[33mwarning:\x1b[0m GDPR/cookie consent wall detected on {url}\n\
             The site redirected to a consent page and returned no usable content.\n\
             Try a different region via --proxy, or pass a pre-accepted consent cookie\n\
             via --cookie / --cookie-file."
        ),
        EmptyReason::JsRequired => eprintln!(
            "\x1b[33mwarning:\x1b[0m No content extracted from {url}\n\
             This site requires JavaScript rendering (SPA).\n\
             Use the webclaw Cloud API for JS rendering: https://webclaw.io/pricing"
        ),
        EmptyReason::None => {}
    }
}

/// Build FetchConfig from CLI flags.
///
/// `--proxy` sets a single static proxy (no rotation).
/// `--proxy-file` loads a pool of proxies and rotates per-request.
/// `--proxy` takes priority: if both are set, only the single proxy is used.
pub fn build_fetch_config(cli: &Cli) -> FetchConfig {
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
        browser: cli.browser.clone().into(),
        proxy,
        proxy_pool,
        timeout: std::time::Duration::from_secs(cli.timeout),
        pdf_mode: cli.pdf_mode.clone().into(),
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

pub fn build_extraction_options(cli: &Cli) -> ExtractionOptions {
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
        include_raw_html: cli.raw_html || matches!(cli.format, crate::cli::OutputFormat::Html),
    }
}

/// Normalize a URL: prepend `https://` if no scheme is present.
pub fn normalize_url(url: &str) -> String {
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
pub fn url_to_filename(raw_url: &str, format: &crate::cli::OutputFormat) -> String {
    use crate::cli::OutputFormat;
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

    // Drop empty / "." / ".." path segments so a URL path like
    // `/../../etc/passwd` can't climb out of the output directory.
    let cleaned_path: String = path
        .split('/')
        .filter(|seg| !seg.is_empty() && *seg != "." && *seg != "..")
        .collect::<Vec<_>>()
        .join("/");

    let mut stem = cleaned_path;
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

/// Reject a caller-supplied (CSV `url,filename`) name that could escape the
/// output directory: absolute paths, drive prefixes, root, or any `..`
/// component. Returns the validated relative path on success.
fn safe_relative_filename(filename: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(filename);
    use std::path::Component;
    for comp in candidate.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("refusing path with '..' component: {filename}"));
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!("refusing absolute output path: {filename}"));
            }
        }
    }
    if candidate.as_os_str().is_empty() {
        return Err("empty output filename".to_string());
    }
    Ok(candidate.to_path_buf())
}

/// Write extraction output to a file inside `dir`, creating parent dirs as needed.
///
/// `filename` may originate from an attacker-controlled `--urls-file`
/// (`url,filename` CSV). It is validated for traversal, and the canonical
/// destination directory is asserted to stay under the canonical output
/// directory before any write.
pub fn write_to_file(dir: &Path, filename: &str, content: &str) -> Result<(), String> {
    let rel = safe_relative_filename(filename)?;
    let dest = dir.join(&rel);

    std::fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create directory {}: {e}", dir.display()))?;
    let base = dir
        .canonicalize()
        .map_err(|e| format!("failed to resolve output dir {}: {e}", dir.display()))?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("failed to create directory {}: {e}", parent.display()))?;
        let canon_parent = parent
            .canonicalize()
            .map_err(|e| format!("failed to resolve {}: {e}", parent.display()))?;
        if !canon_parent.starts_with(&base) {
            return Err(format!(
                "refusing to write outside output dir: {}",
                dest.display()
            ));
        }
    }

    std::fs::write(&dest, content)
        .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    let word_count = content.split_whitespace().count();
    eprintln!("Saved: {} ({word_count} words)", dest.display());
    Ok(())
}

/// Collect all URLs from positional args + --urls-file, normalizing bare domains.
///
/// Returns `(url, optional_custom_filename)` pairs. Custom filenames come from
/// CSV-style lines in `--urls-file`: `url,filename`. Plain lines (no comma) get
/// `None` so the caller auto-generates the filename from the URL.
pub fn collect_urls(cli: &Cli) -> Result<Vec<(String, Option<String>)>, String> {
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
pub enum FetchOutput {
    Local(Box<ExtractionResult>),
    Cloud(serde_json::Value),
}

impl FetchOutput {
    /// Get the local ExtractionResult, or try to parse it from the cloud response.
    pub fn into_extraction(self) -> Result<ExtractionResult, String> {
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
pub async fn fetch_and_extract(cli: &Cli) -> Result<FetchOutput, String> {
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

    let cloud_client = webclaw_fetch::cloud::CloudClient::new(cli.api_key.as_deref());

    // --cloud: skip local, go straight to cloud API
    if cli.cloud {
        let c =
            cloud_client.ok_or("--cloud requires WEBCLAW_API_KEY (set via env or --api-key)")?;
        let options = build_extraction_options(cli);
        let resp = c
            .scrape(
                url,
                &[cli.format.as_api_str()],
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
            match c
                .scrape(
                    url,
                    &[cli.format.as_api_str()],
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
pub async fn fetch_html(cli: &Cli) -> Result<FetchResult, String> {
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
pub async fn enrich_html_with_stylesheets(html: &str, base_url: &str) -> String {
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
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap_or_default();

    let mut extra_css = String::new();
    for href in &hrefs {
        if webclaw_fetch::url_security::validate_public_http_url(href)
            .await
            .is_err()
        {
            continue;
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::OutputFormat;
    use webclaw_core::{Content, Metadata};

    fn empty_result(title: Option<&str>, url: Option<&str>, markdown: &str) -> ExtractionResult {
        let metadata = Metadata::default()
            .with_title(title.map(str::to_string))
            .with_url(url.map(str::to_string))
            .with_word_count(markdown.split_whitespace().count());
        let content = Content::default()
            .with_markdown(markdown.to_string())
            .with_plain_text(markdown.to_string());
        ExtractionResult::new(metadata, content)
    }

    #[test]
    fn detect_empty_identifies_consent_redirect_url() {
        let result = empty_result(
            Some("Yahoo"),
            Some("https://guce.advertising.com/collectIdentifiers?sessionId=abc"),
            "Continue",
        );
        assert_eq!(detect_empty(&result), EmptyReason::ConsentWall);
    }

    #[test]
    fn detect_empty_identifies_short_consent_title() {
        let result = empty_result(
            Some("Before you continue"),
            Some("https://www.google.com/"),
            "Review privacy options",
        );
        assert_eq!(detect_empty(&result), EmptyReason::ConsentWall);
    }

    #[test]
    fn detect_empty_does_not_flag_real_content_with_consent_words() {
        let result = empty_result(
            Some("Cookie consent patterns explained"),
            Some("https://example.com/blog"),
            "This article explains cookie consent patterns for product teams with enough real body text to be useful. It covers consent banners, privacy controls, analytics configuration, regional requirements, product tradeoffs, implementation details, testing flows, debugging notes, accessibility needs, and operational lessons from real teams shipping public websites across multiple markets. It also explains measurement, rollout planning, copy review, support workflows, design constraints, release notes, and how to keep privacy choices understandable for users.",
        );
        assert_eq!(detect_empty(&result), EmptyReason::None);
    }

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
        let dir = std::env::temp_dir().join("webclaw_test_output_dir");
        let _ = std::fs::remove_dir_all(&dir);
        write_to_file(&dir, "nested/deep/file.md", "hello").unwrap();
        let content = std::fs::read_to_string(dir.join("nested/deep/file.md")).unwrap();
        assert_eq!(content, "hello");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn url_to_filename_strips_traversal_segments() {
        // `..` / `.` / empty path segments must not survive into the path.
        let out = url_to_filename(
            "https://example.com/../../etc/passwd",
            &OutputFormat::Markdown,
        );
        assert!(!out.contains(".."), "traversal leaked: {out}");
        assert_eq!(out, "etc/passwd.md");
        let out2 = url_to_filename("https://example.com/a/./b//c", &OutputFormat::Json);
        assert_eq!(out2, "a/b/c.json");
    }

    #[test]
    fn safe_relative_filename_rejects_escapes() {
        assert!(safe_relative_filename("../escape.md").is_err());
        assert!(safe_relative_filename("a/../../b.md").is_err());
        assert!(safe_relative_filename("/etc/passwd").is_err());
        assert!(safe_relative_filename("").is_err());
        // Normal nested relative names stay allowed.
        assert!(safe_relative_filename("nested/deep/file.md").is_ok());
        assert!(safe_relative_filename("./ok.md").is_ok());
    }

    #[test]
    fn write_to_file_refuses_traversal_filename() {
        let dir = std::env::temp_dir().join("webclaw_test_traversal_dir");
        let _ = std::fs::remove_dir_all(&dir);
        // CSV-supplied `url,filename` traversal attempt.
        let err = write_to_file(&dir, "../../tmp/webclaw_pwned.md", "x").unwrap_err();
        assert!(err.contains("refusing"), "unexpected error: {err}");
        assert!(
            !std::path::Path::new("/tmp/webclaw_pwned.md").exists(),
            "traversal write escaped the output dir"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
