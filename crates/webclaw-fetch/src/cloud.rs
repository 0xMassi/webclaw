//! Cloud API fallback client for api.webclaw.io.
//!
//! When local fetch hits bot protection or a JS-only SPA, callers can
//! fall back to the hosted API. A Cloudflare challenge can also be solved
//! locally, in a browser this crate launches (see
//! `render::RenderOptions::solve` with the `browser` feature); [`smart_fetch`]
//! tries that first when the client is set up for it. This module is the
//! shared home for that flow: previously
//! duplicated between `webclaw-mcp/src/cloud.rs` and
//! `webclaw-cli/src/cloud.rs`.
//!
//! ## Architecture
//!
//! - [`CloudClient`] — thin reqwest wrapper around the api.webclaw.io
//!   REST surface. Typed errors for the four HTTP failures callers act
//!   on differently (401 / 402 / 429 / other) plus network + parse.
//! - [`is_bot_protected`] / [`needs_js_rendering`] — pure detectors on
//!   response bodies. The detection patterns are public (CF / DataDome
//!   challenge-page signatures) so these live in OSS without leaking
//!   any moat.
//! - [`smart_fetch`] — try-local-then-escalate flow returning an
//!   [`webclaw_core::ExtractionResult`] or raw cloud JSON. Kept on the original
//!   `Result<_, String>` signature so the existing MCP / CLI call
//!   sites work unchanged.
//! - [`smart_fetch_html`] — new convenience for the vertical-extractor
//!   pattern: just give me antibot-bypassed HTML so I can run my own
//!   parser on it. Returns the typed [`CloudError`] so extractors can
//!   emit precise "upgrade your plan" / "invalid key" messages.
//!
//! ## Cloud response shape and `synthesize_html`
//!
//! `api.webclaw.io/v1/scrape` deliberately does **not** return a
//! `html` field even when `formats=["html"]` is requested. By design
//! the cloud API returns a parsed bundle:
//!
//! ```text
//! {
//!   "url":             "https://...",
//!   "metadata":        { title, description, image, site_name, ... },  // OG / meta tags
//!   "structured_data": [ { "@type": "...", ... }, ... ],               // JSON-LD blocks
//!   "markdown":        "# Page Title\n\n...",                          // cleaned markdown
//!   "antibot":         { engine, path, user_agent },                   // bypass telemetry
//!   "cache":           { status, age_seconds }
//! }
//! ```
//!
//! [`CloudClient::fetch_html`] reassembles that bundle back into a
//! minimal synthetic HTML document so the existing local extractor
//! parsers (JSON-LD walkers, OG regex, DOM-regex) run unchanged over
//! cloud output. Each `structured_data` entry becomes a
//! `<script type="application/ld+json">` tag; each `metadata` field
//! becomes a `<meta property="og:...">` tag; `markdown` lands in a
//! `<pre>` inside the body. Callers that walk Schema.org blocks see
//! exactly what they'd see on a real live page.
//!
//! Amazon-style DOM-regex fallbacks (`#productTitle`, `#landingImage`)
//! won't hit on the synthesised HTML — those IDs only exist on live
//! Amazon pages. Extractors that need DOM regex keep OG meta tag
//! fallbacks for that reason.
//!
//! OSS users without `WEBCLAW_API_KEY` get a clear error pointing at
//! signup when a site is blocked; nothing fails silently. Cloud users
//! get the escalation for free.

use std::time::Duration;

use http::HeaderMap;
use serde_json::{Value, json};
use thiserror::Error;
use tracing::{debug, info, warn};

// Client type isn't needed here anymore now that smart_fetch* takes
// `&dyn Fetcher`. Kept as a comment for historical context: this
// module used to import FetchClient directly before v0.5.1.

// ---------------------------------------------------------------------------
// URLs + defaults — keep in one place so "change the signup link" is a
// single-commit edit.
// ---------------------------------------------------------------------------

const API_BASE_DEFAULT: &str = "https://api.webclaw.io/v1";
const DEFAULT_TIMEOUT_SECS: u64 = 120;

const SIGNUP_URL: &str = "https://webclaw.io/signup";
const PRICING_URL: &str = "https://webclaw.io/pricing";
const KEYS_URL: &str = "https://webclaw.io/dashboard/api-keys";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Structured cloud-fallback error. Variants correspond to the HTTP
/// outcomes callers act on differently — a 401 needs a different UX
/// than a 402 which needs a different UX than a network blip.
///
/// Display messages end with an actionable URL so API consumers can
/// surface them to users verbatim.
#[derive(Debug, Error)]
pub enum CloudError {
    /// No `WEBCLAW_API_KEY` configured. Returned by [`smart_fetch_html`]
    /// and friends when they hit bot protection but have no client to
    /// escalate to.
    #[error(
        "this site is behind antibot protection. \
         Set WEBCLAW_API_KEY to unlock automatic cloud bypass. \
         Free tier: {SIGNUP_URL}"
    )]
    NotConfigured,

    /// HTTP 401 — the key is present but rejected.
    #[error(
        "WEBCLAW_API_KEY rejected (HTTP 401). \
         Check or regenerate your key at {KEYS_URL}"
    )]
    Unauthorized,

    /// HTTP 402 — the key is valid but the plan doesn't cover the call.
    #[error(
        "your plan doesn't include this endpoint / site (HTTP 402). \
         Upgrade at {PRICING_URL}"
    )]
    InsufficientPlan,

    /// HTTP 429 — rate limit.
    #[error(
        "cloud API rate limit reached (HTTP 429). \
         Wait a moment or upgrade at {PRICING_URL}"
    )]
    RateLimited,

    /// HTTP 4xx / 5xx the caller probably can't do anything specific
    /// about. Body is truncated to a sensible length for logs.
    #[error("cloud API returned HTTP {status}: {body}")]
    ServerError { status: u16, body: String },

    #[error("cloud request failed: {0}")]
    Network(String),

    #[error("cloud response parse failed: {0}")]
    ParseFailed(String),
}

impl CloudError {
    /// Build from a non-success HTTP response, routing well-known
    /// statuses to dedicated variants.
    fn from_status_and_body(status: u16, body: String) -> Self {
        match status {
            401 => Self::Unauthorized,
            402 => Self::InsufficientPlan,
            429 => Self::RateLimited,
            _ => Self::ServerError {
                status,
                body: truncate(&body, 500).to_string(),
            },
        }
    }
}

impl From<reqwest::Error> for CloudError {
    fn from(e: reqwest::Error) -> Self {
        Self::Network(e.to_string())
    }
}

/// Backwards-compatibility bridge: a lot of pre-existing MCP / CLI call
/// sites `use .await?` into functions returning `Result<_, String>`.
/// Having this `From` impl means those sites keep compiling while we
/// migrate them to the typed error over time.
impl From<CloudError> for String {
    fn from(e: CloudError) -> Self {
        e.to_string()
    }
}

fn truncate(text: &str, max: usize) -> &str {
    match text.char_indices().nth(max) {
        Some((byte_pos, _)) => &text[..byte_pos],
        None => text,
    }
}

// ---------------------------------------------------------------------------
// CloudClient
// ---------------------------------------------------------------------------

/// Thin reqwest client around api.webclaw.io. Cloneable cheaply — the
/// inner `reqwest::Client` already refcounts its connection pool.
#[derive(Clone)]
pub struct CloudClient {
    api_key: String,
    base_url: String,
    http: reqwest::Client,
}

impl CloudClient {
    /// Build from an explicit key (e.g. a `--api-key` CLI flag) or fall
    /// back to the `WEBCLAW_API_KEY` env var. Returns `None` when
    /// neither is set / both are empty.
    ///
    /// This is the function call sites should use by default — it's
    /// what both the CLI and MCP want.
    pub fn new(explicit_key: Option<&str>) -> Option<Self> {
        explicit_key
            .map(String::from)
            .or_else(|| std::env::var("WEBCLAW_API_KEY").ok())
            .filter(|k| !k.trim().is_empty())
            .map(Self::with_key)
    }

    /// Build from `WEBCLAW_API_KEY` env only. Thin wrapper kept for
    /// readability at call sites that never accept a flag.
    pub fn from_env() -> Option<Self> {
        Self::new(None)
    }

    /// Build with an explicit key. Useful when the caller already has
    /// a key from somewhere other than env or a flag (e.g. loaded from
    /// config). `WEBCLAW_API_URL` overrides the API base, including `/v1`.
    pub fn with_key(api_key: impl Into<String>) -> Self {
        let base = std::env::var("WEBCLAW_API_URL")
            .ok()
            .filter(|url| !url.trim().is_empty())
            .unwrap_or_else(|| API_BASE_DEFAULT.into());
        Self::with_key_and_base(api_key, base)
    }

    /// Build with an explicit key and base URL. Used by integration
    /// tests and staging deployments.
    pub fn with_key_and_base(api_key: impl Into<String>, base_url: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .build()
            .expect("reqwest client builder failed with default settings");
        Self {
            api_key: api_key.into(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Generic POST. Endpoint may be `"scrape"` or `"/scrape"` — we
    /// normalise the slash.
    pub async fn post(&self, endpoint: &str, body: Value) -> Result<Value, CloudError> {
        let url = format!("{}/{}", self.base_url, endpoint.trim_start_matches('/'));
        let resp = self
            .http
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&body)
            .send()
            .await?;
        parse_cloud_response(resp).await
    }

    /// Generic GET.
    pub async fn get(&self, endpoint: &str) -> Result<Value, CloudError> {
        let url = format!("{}/{}", self.base_url, endpoint.trim_start_matches('/'));
        let resp = self
            .http
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .send()
            .await?;
        parse_cloud_response(resp).await
    }

    /// `POST /v1/scrape` with the caller's extraction options. This is
    /// the public "do everything" surface: the cloud side handles
    /// fetch + antibot + JS render + extraction + formatting.
    pub async fn scrape(
        &self,
        url: &str,
        formats: &[&str],
        include_selectors: &[String],
        exclude_selectors: &[String],
        only_main_content: bool,
    ) -> Result<Value, CloudError> {
        let mut body = json!({ "url": url, "formats": formats });
        if only_main_content {
            body["only_main_content"] = json!(true);
        }
        if !include_selectors.is_empty() {
            body["include_selectors"] = json!(include_selectors);
        }
        if !exclude_selectors.is_empty() {
            body["exclude_selectors"] = json!(exclude_selectors);
        }
        self.post("scrape", body).await
    }

    /// Get antibot-bypassed page data back as a synthetic HTML string.
    ///
    /// `api.webclaw.io/v1/scrape` intentionally does not return raw
    /// HTML: it returns pre-parsed `structured_data` (JSON-LD blocks)
    /// plus `metadata` (title, description, OG tags, image) plus a
    /// `markdown` body. We reassemble those into a minimal HTML doc
    /// that looks enough like the real page for our local extractor
    /// parsers to run unchanged: each JSON-LD block gets emitted as a
    /// `<script type="application/ld+json">` tag, metadata gets
    /// emitted as OG `<meta>` tags, and the markdown lands in the
    /// body. Extractors that walk JSON-LD (ecommerce_product,
    /// trustpilot_reviews, ebay_listing, etsy_listing, amazon_product)
    /// see exactly the same shapes they'd see from a live HTML fetch.
    pub async fn fetch_html(&self, url: &str) -> Result<String, CloudError> {
        let resp = self.scrape(url, &["markdown"], &[], &[], false).await?;
        Ok(synthesize_html(&resp))
    }
}

/// Reassemble a minimal HTML document from a cloud `/v1/scrape`
/// response so existing HTML-based extractor parsers can run against
/// cloud output without a separate code path.
fn synthesize_html(resp: &Value) -> String {
    let mut out = String::with_capacity(8_192);
    out.push_str("<html><head>\n");

    // Metadata → OG meta tags. Keep keys stable with what local
    // extractors read: og:title, og:description, og:image, og:site_name.
    if let Some(meta) = resp.get("metadata").and_then(|m| m.as_object()) {
        for (src_key, og_key) in [
            ("title", "title"),
            ("description", "description"),
            ("image", "image"),
            ("site_name", "site_name"),
        ] {
            if let Some(val) = meta.get(src_key).and_then(|v| v.as_str())
                && !val.is_empty()
            {
                out.push_str(&format!(
                    "<meta property=\"og:{og_key}\" content=\"{}\">\n",
                    html_escape_attr(val)
                ));
            }
        }
    }

    // Structured data blocks → <script type="application/ld+json">.
    // Serialise losslessly so extract_json_ld's parser gets the same
    // shape it would get from a real page.
    if let Some(blocks) = resp.get("structured_data").and_then(|v| v.as_array()) {
        for block in blocks {
            if let Ok(s) = serde_json::to_string(block) {
                out.push_str("<script type=\"application/ld+json\">");
                out.push_str(&s);
                out.push_str("</script>\n");
            }
        }
    }

    out.push_str("</head><body>\n");

    // Markdown body → plaintext in <body>. Extractors that regex over
    // <div> IDs won't hit here, but they won't hit on local cloud
    // bypass either. OK to keep minimal.
    if let Some(md) = resp.get("markdown").and_then(|v| v.as_str()) {
        out.push_str("<pre>");
        out.push_str(&html_escape_text(md));
        out.push_str("</pre>\n");
    }

    out.push_str("</body></html>");
    out
}

fn html_escape_attr(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_escape_text(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

async fn parse_cloud_response(resp: reqwest::Response) -> Result<Value, CloudError> {
    let status = resp.status();
    if status.is_success() {
        return resp
            .json()
            .await
            .map_err(|e| CloudError::ParseFailed(e.to_string()));
    }
    let body = resp.text().await.unwrap_or_default();
    Err(CloudError::from_status_and_body(status.as_u16(), body))
}

// ---------------------------------------------------------------------------
// Detection
// ---------------------------------------------------------------------------

/// Apply [`webclaw_core::focus()`] to the text fields of a cloud scrape
/// response: `markdown`, `llm`, `text` and `plain_text`, at the top level
/// and under `content`.
pub fn focus_cloud_response(resp: &mut Value, query: &str) {
    fn focus_fields(object: &mut Value, query: &str) {
        for key in ["markdown", "llm", "text", "plain_text"] {
            if let Some(Value::String(text)) = object.get_mut(key) {
                *text = webclaw_core::focus(text, query);
            }
        }
    }
    focus_fields(resp, query);
    if let Some(content) = resp.get_mut("content") {
        focus_fields(content, query);
    }
}

/// True when a fetched response body is a bot-protection challenge or
/// block page rather than the content the caller asked for. Conservative:
/// it fires on patterns that mark the whole page as a challenge, and on a
/// Turnstile widget only when the page is small.
pub fn is_bot_protected(html: &str, headers: &HeaderMap) -> bool {
    blocked(html, headers, true)
}

fn blocked(html: &str, headers: &HeaderMap, widgets_block: bool) -> bool {
    let html_lower = html.to_lowercase();
    if is_cloudflare_challenge(&html_lower, headers) || headers.get("cf-mitigated").is_some() {
        return true;
    }
    if (html_lower.contains("just a moment") || html_lower.contains("checking your browser"))
        && html_lower.contains("cf-spinner")
    {
        return true;
    }
    if widgets_block
        && (html_lower.contains("cf-turnstile")
            || html_lower.contains("challenges.cloudflare.com/turnstile"))
        && html.len() < 100_000
    {
        return true;
    }
    if html_lower.contains("geo.captcha-delivery.com")
        || html_lower.contains("captcha-delivery.com/captcha")
    {
        return true;
    }
    if html_lower.contains("awswaf-captcha") || html_lower.contains("aws-waf-client-browser") {
        return true;
    }
    if html_lower.contains("interstitial-spinner")
        && html_lower.contains("verifying your connection")
        && html.len() < 10_000
    {
        return true;
    }
    if html_lower.contains("hcaptcha.com")
        && html_lower.contains("h-captcha")
        && html.len() < 50_000
    {
        return true;
    }
    let has_cf_headers = headers.get("cf-ray").is_some() || headers.get("cf-mitigated").is_some();
    has_cf_headers
        && (html_lower.contains("just a moment") || html_lower.contains("checking your browser"))
}

/// Whether a response is a Cloudflare challenge page: its body carries the
/// challenge options blob (`_cf_chl_opt`) or loads the challenge's
/// orchestrate script, or Cloudflare marked it with `cf-mitigated:
/// challenge`. The script path must include its `h/b` or `h/g` segment:
/// the JS-detection beacon Cloudflare adds to ordinary pages sits under
/// `challenge-platform/scripts/` or `challenge-platform/h/g/scripts/`, and
/// `/orchestrate/` alone is a common word in API paths.
pub fn is_cloudflare_challenge(html: &str, headers: &HeaderMap) -> bool {
    contains_ignore_case(html, "_cf_chl_opt")
        || contains_ignore_case(html, "challenge-platform/h/b/orchestrate/")
        || contains_ignore_case(html, "challenge-platform/h/g/orchestrate/")
        || headers
            .get("cf-mitigated")
            .is_some_and(|value| value == "challenge")
}

/// Whether a page fetched over HTTP probably needs a browser to show its
/// content: it loads scripts, and it has almost no text (under 50 extracted
/// words and under 50 visible words in the whole body, so a thin page with a
/// full menu is left alone), or little text (under 200 words) next to an
/// empty app root (`<div id="root"></div>`) or a `<noscript>` asking for
/// JavaScript. Bundle names alone never count, so server-rendered pages that
/// also ship scripts are left alone.
pub fn needs_js_rendering(word_count: usize, html: &str) -> bool {
    if !contains_ignore_case(html, "<script") {
        return false;
    }
    (word_count < 50 && webclaw_core::quality::visible_word_count(html) < 50)
        || (word_count < 200 && (has_empty_app_root(html) || noscript_asks_for_js(html)))
}

pub(crate) fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|w| w.eq_ignore_ascii_case(needle.as_bytes()))
}

fn has_empty_app_root(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    [
        "id=\"root\"",
        "id=\"app\"",
        "id=\"__next\"",
        "id=\"__nuxt\"",
        "id=\"___gatsby\"",
        "id='root'",
        "id='app'",
    ]
    .iter()
    .any(|marker| {
        lower.match_indices(marker).any(|(at, _)| {
            lower[at..]
                .split_once('>')
                .is_some_and(|(_, inner)| inner.trim_start().starts_with("</"))
        })
    })
}

fn noscript_asks_for_js(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.match_indices("<noscript").any(|(at, _)| {
        let block: String = lower[at..].chars().take(2_000).collect();
        let block = block.split("</noscript>").next().unwrap_or(&block);
        block.contains("javascript")
            && ["enable", "require", "turn on", "need", "must"]
                .iter()
                .any(|verb| block.contains(verb))
    })
}

// ---------------------------------------------------------------------------
// Smart-fetch: classic flow for MCP / CLI (returns either an extraction
// or raw cloud JSON)
// ---------------------------------------------------------------------------

/// Result of [`smart_fetch`]: either a local extraction or the raw
/// cloud API response when we escalated.
pub enum SmartFetchResult {
    Local(Box<webclaw_core::ExtractionResult>),
    Cloud(Value),
}

/// Try local fetch + extract first. On bot protection or detected
/// JS-render, fall back to `cloud.scrape(...)` with the caller's
/// formats. Returns `Err(String)` so existing call sites that expect
/// stringified errors keep compiling.
///
/// Prefer [`smart_fetch_html`] for new callers — it surfaces the typed
/// [`CloudError`] so you can render precise UX.
pub async fn smart_fetch(
    client: &dyn crate::fetcher::Fetcher,
    cloud: Option<&CloudClient>,
    url: &str,
    include_selectors: &[String],
    exclude_selectors: &[String],
    only_main_content: bool,
    formats: &[&str],
) -> Result<SmartFetchResult, String> {
    let mut fetch_result = tokio::time::timeout(Duration::from_secs(30), client.fetch(url))
        .await
        .map_err(|_| format!("Fetch timed out after 30s for {url}"))?
        .map_err(|e| format!("Fetch failed: {e}"))?;

    let mut rendered_already = false;
    let widgets_block = !client.solves_challenges();
    if fetch_result.status >= 400
        || blocked(&fetch_result.html, &fetch_result.headers, widgets_block)
    {
        match client.solve_challenge(&fetch_result).await {
            Some(Ok(solved))
                if solved.status < 400
                    && !is_cloudflare_challenge(&solved.html, &solved.headers) =>
            {
                fetch_result = solved;
                rendered_already = true;
            }
            Some(Ok(page)) if cloud.is_none() => {
                return Err(format!(
                    "solving the Cloudflare challenge on {url} did not reach the page: it ended on HTTP {}",
                    page.status
                ));
            }
            Some(Err(e)) if cloud.is_none() => {
                return Err(format!(
                    "could not get past the Cloudflare challenge on {url}: {e}"
                ));
            }
            None if cloud.is_none()
                && is_cloudflare_challenge(&fetch_result.html, &fetch_result.headers) =>
            {
                return Err(format!(
                    "{url} answered with a Cloudflare challenge: pass solve: true to get past it \
                     in the local browser, or set WEBCLAW_API_KEY for the cloud API"
                ));
            }
            _ => {
                info!(url, "bot protection detected, falling back to cloud API");
                return cloud_scrape_fallback(
                    cloud,
                    url,
                    include_selectors,
                    exclude_selectors,
                    only_main_content,
                    formats,
                )
                .await;
            }
        }
    }

    let options = webclaw_core::ExtractionOptions {
        include_selectors: include_selectors.to_vec(),
        exclude_selectors: exclude_selectors.to_vec(),
        only_main_content,
        include_raw_html: false,
    };
    let mut extraction =
        webclaw_core::extract_with_options(&fetch_result.html, Some(&fetch_result.url), &options)
            .map_err(|e| format!("Extraction failed: {e}"))?;
    let whole_page =
        include_selectors.is_empty() && exclude_selectors.is_empty() && !only_main_content;
    let page_words = if whole_page {
        extraction.metadata.word_count
    } else {
        webclaw_core::extract(&fetch_result.html, Some(&fetch_result.url))
            .map_or(0, |e| e.metadata.word_count)
    };
    if !rendered_already
        && let Some(rendered) = client.render_if_needed(&fetch_result, page_words).await
        && let Ok(better) =
            webclaw_core::extract_with_options(&rendered.html, Some(&rendered.url), &options)
        && better.metadata.word_count > extraction.metadata.word_count
    {
        extraction = better;
        fetch_result = rendered;
    }

    let issue = webclaw_core::quality::content_issue(&extraction).filter(|issue| {
        *issue != webclaw_core::quality::ContentIssue::Empty
            || !webclaw_core::quality::allows_empty_content(&fetch_result.html, &options)
    });
    let looks_unrendered =
        whole_page && needs_js_rendering(extraction.metadata.word_count, &fetch_result.html);
    if issue.is_none() && looks_unrendered && cloud.is_none() {
        return Ok(SmartFetchResult::Local(Box::new(extraction)));
    }
    if issue.is_some() || looks_unrendered {
        info!(
            url,
            word_count = extraction.metadata.word_count,
            html_len = fetch_result.html.len(),
            "JS-rendered page detected, falling back to cloud API"
        );
        return cloud_scrape_fallback(
            cloud,
            url,
            include_selectors,
            exclude_selectors,
            only_main_content,
            formats,
        )
        .await;
    }

    Ok(SmartFetchResult::Local(Box::new(extraction)))
}

async fn cloud_scrape_fallback(
    cloud: Option<&CloudClient>,
    url: &str,
    include_selectors: &[String],
    exclude_selectors: &[String],
    only_main_content: bool,
    formats: &[&str],
) -> Result<SmartFetchResult, String> {
    let Some(c) = cloud else {
        return Err(CloudError::NotConfigured.to_string());
    };
    let resp = c
        .scrape(
            url,
            formats,
            include_selectors,
            exclude_selectors,
            only_main_content,
        )
        .await
        .map_err(|e| e.to_string())?;
    info!(url, "cloud API fallback successful");
    Ok(SmartFetchResult::Cloud(resp))
}

// ---------------------------------------------------------------------------
// Smart-fetch-HTML: for vertical extractors
// ---------------------------------------------------------------------------

/// Where the HTML ultimately came from — useful for callers that want
/// to track "did we fall back?" for logging or pricing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchSource {
    Local,
    Cloud,
}

/// Antibot-aware HTML fetch result. The `html` field is always populated.
pub struct FetchedHtml {
    pub html: String,
    pub final_url: String,
    pub source: FetchSource,
}

/// Try local fetch; on bot protection, escalate to the cloud's
/// `/v1/scrape` with `formats=["html"]` and return the raw HTML.
///
/// Designed for the vertical-extractor pattern where the caller has
/// its own parser and just needs bytes.
pub async fn smart_fetch_html(
    client: &dyn crate::fetcher::Fetcher,
    cloud: Option<&CloudClient>,
    url: &str,
) -> Result<FetchedHtml, CloudError> {
    let resp = client
        .fetch(url)
        .await
        .map_err(|e| CloudError::Network(e.to_string()))?;

    if !is_bot_protected(&resp.html, &resp.headers) {
        return Ok(FetchedHtml {
            html: resp.html,
            final_url: resp.url,
            source: FetchSource::Local,
        });
    }

    let Some(c) = cloud else {
        warn!(url, "bot protection detected + no cloud client configured");
        return Err(CloudError::NotConfigured);
    };
    debug!(url, "bot protection detected, escalating to cloud");
    let html = c.fetch_html(url).await?;
    Ok(FetchedHtml {
        html,
        final_url: url.to_string(),
        source: FetchSource::Cloud,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_headers() -> HeaderMap {
        HeaderMap::new()
    }

    // --- detectors ----------------------------------------------------------

    #[test]
    fn is_bot_protected_detects_cloudflare_challenge() {
        let html = "<html><body>_cf_chl_opt loaded</body></html>";
        assert!(is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_detects_cloudflare_orchestrate_script() {
        let html = r#"<html><body><script
            src="/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1?ray=abc">
            </script></body></html>"#;
        assert!(is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_ignores_hashed_cloudflare_jsd_beacon() {
        // Cloudflare also serves the benign beacon under the `h/{b,g}` prefix
        // with a hash segment. This form is why matching `challenge-platform`
        // — or even `challenge-platform/h/` — is not a valid discriminator.
        let html = r#"<html><head><script
            src="/cdn-cgi/challenge-platform/h/g/scripts/jsd/e4025c85ea63/main.js">
            </script></head><body>a perfectly ordinary page</body></html>"#;
        assert!(!is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_ignores_orchestrate_outside_cloudflare_path() {
        // "orchestrate" is an ordinary word in API/doc paths — verified live
        // on github.com, pypi.org and IBM's watsonx docs. Matching it
        // unanchored flagged all of them as challenges.
        let html = r#"<html><body><nav><a href="/guides/orchestrate/">Orchestrate</a>
            <a href="/docs/orchestrate/schedules">Schedules</a></nav>
            <h1>Orchestrate your pipelines</h1></body></html>"#;
        assert!(!is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_ignores_orchestrate_link_on_cloudflare_fronted_docs() {
        // The hard case: a Cloudflare-fronted docs site carries the benign
        // beacon AND links to `/docs/orchestrate/…`. Testing the two
        // substrings separately would flag it; one contiguous literal does not.
        let html = r#"<html><head><script
            src="/cdn-cgi/challenge-platform/h/g/scripts/jsd/e4025c85ea63/main.js">
            </script></head><body><a href="/docs/orchestrate/schedules">Schedules</a>
            <h1>Orchestrate your pipelines</h1></body></html>"#;
        assert!(!is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_detects_cf_mitigated_header() {
        // Terminal block pages (1020) carry neither `_cf_chl_opt` nor the
        // orchestrate script, but Cloudflare only sets `cf-mitigated` when it
        // actually mitigated — so the header alone is conclusive.
        let mut headers = HeaderMap::new();
        headers.insert("cf-mitigated", "challenge".parse().expect("valid header"));
        let html = "<html><body>Sorry, you have been blocked</body></html>";
        assert!(is_bot_protected(html, &headers));
    }

    #[test]
    fn is_bot_protected_detects_all_orchestrate_challenge_variants() {
        // `chl_page` is not the only orchestration endpoint — `jsch/v1` (JS
        // challenge) and `managed/v1` (managed challenge) are equally real.
        // Each must be caught by the path arm ALONE, without relying on
        // `_cf_chl_opt` also being present.
        for path in [
            "/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1?ray=9a1",
            "/cdn-cgi/challenge-platform/h/g/orchestrate/jsch/v1?ray=9a1",
            "/cdn-cgi/challenge-platform/h/b/orchestrate/managed/v1?ray=9a1",
        ] {
            let html = format!(r#"<html><body><script src="{path}"></script></body></html>"#);
            assert!(
                is_bot_protected(&html, &empty_headers()),
                "challenge variant must be detected: {path}"
            );
        }
    }

    #[test]
    fn is_bot_protected_detects_full_interstitial() {
        // Pins the false-negative property the narrowing is riskiest for: a
        // complete managed-challenge page must still be caught.
        let html = r#"<html><head><script>window._cf_chl_opt={cvId:'3'};</script>
            <script src="/cdn-cgi/challenge-platform/h/b/orchestrate/chl_page/v1?ray=9a1">
            </script></head><body>Just a moment...</body></html>"#;
        assert!(is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_ignores_cloudflare_jsd_beacon() {
        // Regression (chronopost.fr): Cloudflare injects its JS-detection
        // beacon into ordinary 200 pages. This is the real markup that page
        // serves — it is NOT an interstitial, and flagging it escalated a
        // healthy fetch to the cloud API, which then reported an unsolvable
        // challenge that did not exist.
        let html = r#"<html><head><script>window.__CF$cv$params={
            r:'a22db83e59720e29',t:'MTc4NTM0NDg0NA=='};
            var a=document.createElement('script');
            a.src='/cdn-cgi/challenge-platform/scripts/jsd/main.js';
            document.getElementsByTagName('head')[0].appendChild(a);
            </script></head><body>Track your parcel</body></html>"#;
        assert!(!is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_detects_turnstile_on_short_page() {
        let html = "<div class=\"cf-turnstile\"></div>";
        assert!(is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_ignores_turnstile_on_real_content() {
        let html = format!(
            "<html><body>{}<div class=\"cf-turnstile\"></div></body></html>",
            "lots of real content ".repeat(8_000)
        );
        assert!(!is_bot_protected(&html, &empty_headers()));
    }

    #[test]
    fn is_bot_protected_detects_aws_waf_verifying_connection() {
        // The exact shape Trustpilot serves under AWS WAF.
        let html = r#"<div class="container"><div id="loading-state">
            <div class="interstitial-spinner" id="spinner"></div>
            <h1>Verifying your connection...</h1></div></div>"#;
        assert!(is_bot_protected(html, &empty_headers()));
    }

    #[test]
    fn synthesize_html_embeds_jsonld_and_og_tags() {
        let resp = json!({
            "url": "https://example.com/p/1",
            "metadata": {
                "title": "My Product",
                "description": "A nice thing.",
                "image": "https://cdn.example.com/1.jpg",
                "site_name": "Example Shop"
            },
            "structured_data": [
                {"@context":"https://schema.org","@type":"Product",
                 "name":"Widget","offers":{"@type":"Offer","price":"9.99","priceCurrency":"USD"}}
            ],
            "markdown": "# Widget\n\nA nice widget."
        });
        let html = synthesize_html(&resp);
        // OG tags from metadata.
        assert!(html.contains(r#"<meta property="og:title" content="My Product">"#));
        assert!(
            html.contains(r#"<meta property="og:image" content="https://cdn.example.com/1.jpg">"#)
        );
        // JSON-LD block preserved losslessly.
        assert!(html.contains(r#"<script type="application/ld+json">"#));
        assert!(html.contains(r#""@type":"Product""#));
        assert!(html.contains(r#""price":"9.99""#));
        // Body carries markdown.
        assert!(html.contains("A nice widget."));
    }

    #[test]
    fn synthesize_html_handles_missing_fields_gracefully() {
        let resp = json!({"url": "https://example.com", "metadata": {}});
        let html = synthesize_html(&resp);
        // No panic, no stray unclosed tags.
        assert!(html.starts_with("<html><head>"));
        assert!(html.ends_with("</body></html>"));
    }

    #[test]
    fn synthesize_html_escapes_attribute_quotes() {
        let resp = json!({
            "metadata": {"title": r#"She said "hi""#}
        });
        let html = synthesize_html(&resp);
        assert!(html.contains(r#"og:title" content="She said &quot;hi&quot;""#));
    }

    #[test]
    fn is_bot_protected_ignores_phrase_on_real_content() {
        // A real article that happens to mention the phrase in prose
        // should not trigger the short-page detector.
        let html = format!(
            "<html><body>{}<p>Verifying your connection is tricky.</p></body></html>",
            "article text ".repeat(2_000)
        );
        assert!(!is_bot_protected(&html, &empty_headers()));
    }

    #[test]
    fn needs_js_rendering_flags_spa_skeleton() {
        let html = format!(
            "<html><body><div id=\"__next\"></div>{}</body></html>",
            "<script>x</script>".repeat(500)
        );
        assert!(needs_js_rendering(10, &html));
    }

    #[test]
    fn needs_js_rendering_flags_small_app_shells() {
        let todo = r#"<html><head><title>TodoMVC</title></head><body><section class="todoapp"></section><script src="app.js"></script></body></html>"#;
        assert!(needs_js_rendering(0, todo));
        let noscript = format!(
            "<html><body><noscript>You need to enable JavaScript to run this app.</noscript><p>{}</p><script src=\"/main.js\"></script></body></html>",
            "Menu item ".repeat(60)
        );
        assert!(needs_js_rendering(120, &noscript));
        let accented = format!(
            "<html><body><noscript>{} Please enable JavaScript.</noscript><p>{}</p><script></script></body></html>",
            "è".repeat(1_500),
            "parola ".repeat(80)
        );
        assert!(needs_js_rendering(80, &accented));
    }

    #[test]
    fn cloud_responses_are_focused_too() {
        let page = "# Rust\n\n## Tooling\n\nCargo builds it.\n\n## History\n\nGraydon began it.";
        let mut resp = json!({"markdown": page, "content": {"markdown": page}, "url": "https://x"});
        focus_cloud_response(&mut resp, "tooling");
        for text in [&resp["markdown"], &resp["content"]["markdown"]] {
            let text = text.as_str().unwrap();
            assert!(
                text.contains("Cargo") && !text.contains("Graydon"),
                "{text}"
            );
        }
        assert_eq!(resp["url"], "https://x");
    }

    struct StaticPage(&'static str);

    #[async_trait::async_trait]
    impl crate::fetcher::Fetcher for StaticPage {
        async fn fetch(
            &self,
            url: &str,
        ) -> Result<crate::client::FetchResult, crate::error::FetchError> {
            Ok(crate::client::FetchResult {
                html: self.0.to_string(),
                document_bytes: None,
                status: 200,
                url: url.to_string(),
                headers: http::HeaderMap::new(),
                elapsed: std::time::Duration::ZERO,
            })
        }
    }

    struct ScriptBuiltPage;

    #[async_trait::async_trait]
    impl crate::fetcher::Fetcher for ScriptBuiltPage {
        async fn fetch(
            &self,
            url: &str,
        ) -> Result<crate::client::FetchResult, crate::error::FetchError> {
            StaticPage("<html><body><h1>Search</h1><div id=\"results\"></div><script src=\"/app.js\"></script></body></html>").fetch(url).await
        }

        async fn render_if_needed(
            &self,
            fetched: &crate::client::FetchResult,
            _http_words: usize,
        ) -> Option<crate::client::FetchResult> {
            StaticPage("<html><body><h1>Search</h1><div id=\"results\"><p>First result about rendering pages that build their content with scripts.</p></div></body></html>").fetch(&fetched.url).await.ok()
        }
    }

    #[tokio::test]
    async fn smart_fetch_renders_when_selectors_filter_the_page() {
        let result = smart_fetch(
            &ScriptBuiltPage,
            None,
            "https://example.com/search",
            &["#results".to_string()],
            &[],
            false,
            &["markdown"],
        )
        .await
        .expect("local extraction should be returned");
        assert!(
            matches!(result, SmartFetchResult::Local(e) if e.content.markdown.contains("First result"))
        );
    }

    #[tokio::test]
    async fn smart_fetch_keeps_local_content_without_a_cloud_key() {
        let page = StaticPage(
            "<html><body><noscript>Please enable JavaScript.</noscript><main><h1>Plans</h1><p>Starter costs ten dollars a month and includes five projects with unlimited members and email support for every workspace on the plan.</p></main><script src=\"/app.js\"></script></body></html>",
        );
        let result = smart_fetch(
            &page,
            None,
            "https://example.com/",
            &[],
            &[],
            false,
            &["markdown"],
        )
        .await
        .expect("local extraction should be returned");
        assert!(
            matches!(result, SmartFetchResult::Local(e) if e.content.markdown.contains("Starter costs"))
        );
    }

    struct BlockedPage(&'static str);

    #[async_trait::async_trait]
    impl crate::fetcher::Fetcher for BlockedPage {
        async fn fetch(
            &self,
            url: &str,
        ) -> Result<crate::client::FetchResult, crate::error::FetchError> {
            let mut page = StaticPage(self.0).fetch(url).await?;
            page.status = 403;
            Ok(page)
        }

        async fn solve_challenge(
            &self,
            fetched: &crate::client::FetchResult,
        ) -> Option<Result<crate::client::FetchResult, crate::error::FetchError>> {
            if !is_cloudflare_challenge(&fetched.html, &fetched.headers) {
                return None;
            }
            Some(StaticPage("<html><body><main><h1>Prices</h1><p>The solved page lists every plan with its monthly price, the projects it includes and the support that comes with it.</p></main></body></html>").fetch(&fetched.url).await)
        }
    }

    #[tokio::test]
    async fn smart_fetch_reads_the_page_behind_a_solved_challenge() {
        let challenge = BlockedPage(
            "<html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt = {cType: 'managed'};</script></body></html>",
        );
        let result = smart_fetch(
            &challenge,
            None,
            "https://example.com/",
            &[],
            &[],
            false,
            &["markdown"],
        )
        .await
        .expect("the solved page is extracted");
        assert!(
            matches!(result, SmartFetchResult::Local(e) if e.content.markdown.contains("monthly price"))
        );
        let other = BlockedPage(
            "<html><body><script src=\"https://geo.captcha-delivery.com/captcha/\"></script></body></html>",
        );
        let blocked = smart_fetch(
            &other,
            None,
            "https://example.com/",
            &[],
            &[],
            false,
            &["markdown"],
        )
        .await;
        assert!(blocked.is_err());
    }

    const SIGNUP: &str = r#"<html><body><main><h1>Sign up</h1><p>Create an account to follow every plan, compare monthly prices and get support for all your projects.</p></main><div class="cf-turnstile"></div><script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script></body></html>"#;

    const SIGNUP_BEHIND_DATADOME: &str = r#"<html><body><main><h1>Sign up</h1><p>Create an account to follow every plan, compare monthly prices and get support for all your projects.</p></main><div class="cf-turnstile"></div><script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script><script src="https://geo.captcha-delivery.com/captcha/"></script></body></html>"#;

    struct WidgetPage {
        solves: bool,
        html: &'static str,
    }

    #[async_trait::async_trait]
    impl crate::fetcher::Fetcher for WidgetPage {
        async fn fetch(
            &self,
            url: &str,
        ) -> Result<crate::client::FetchResult, crate::error::FetchError> {
            StaticPage(self.html).fetch(url).await
        }

        fn solves_challenges(&self) -> bool {
            self.solves
        }
    }

    #[tokio::test]
    async fn a_turnstile_widget_is_content_when_the_fetcher_solves() {
        let run = |solves, html| async move {
            smart_fetch(
                &WidgetPage { solves, html },
                None,
                "https://example.com/signup",
                &[],
                &[],
                false,
                &["markdown"],
            )
            .await
        };
        assert!(
            matches!(run(true, SIGNUP).await, Ok(SmartFetchResult::Local(e)) if e.content.markdown.contains("Create an account"))
        );
        assert!(run(false, SIGNUP).await.is_err());
        assert!(run(true, SIGNUP_BEHIND_DATADOME).await.is_err());
    }

    struct StuckChallenge;

    #[async_trait::async_trait]
    impl crate::fetcher::Fetcher for StuckChallenge {
        async fn fetch(
            &self,
            url: &str,
        ) -> Result<crate::client::FetchResult, crate::error::FetchError> {
            let mut page = StaticPage(
                "<html><body><script>window._cf_chl_opt = {cType: 'managed'};</script></body></html>",
            )
            .fetch(url)
            .await?;
            page.status = 403;
            Ok(page)
        }

        async fn solve_challenge(
            &self,
            fetched: &crate::client::FetchResult,
        ) -> Option<Result<crate::client::FetchResult, crate::error::FetchError>> {
            Some(Ok(fetched.clone()))
        }
    }

    #[tokio::test]
    async fn a_solve_that_ends_on_an_error_page_says_so() {
        let error = smart_fetch(
            &StuckChallenge,
            None,
            "https://example.com/",
            &[],
            &[],
            false,
            &["markdown"],
        )
        .await
        .err()
        .expect("the page is never reached");
        assert!(
            error.contains("did not reach the page") && error.contains("403"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn a_challenge_without_solving_says_how_to_solve_it() {
        let challenge = StaticPage(
            "<html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt = {cType: 'managed'};</script></body></html>",
        );
        let error = smart_fetch(
            &challenge,
            None,
            "https://example.com/",
            &[],
            &[],
            false,
            &["markdown"],
        )
        .await
        .err()
        .expect("a challenge cannot be read");
        assert!(error.contains("solve: true"), "{error}");
    }

    #[test]
    fn cloudflare_challenges_are_told_from_other_blocks() {
        let mut headers = HeaderMap::new();
        assert!(is_cloudflare_challenge(
            "<script>window._cf_chl_opt = {}</script>",
            &headers
        ));
        assert!(!is_cloudflare_challenge(
            r#"<script src="/cdn-cgi/challenge-platform/h/g/scripts/jsd/e4025c85ea63/main.js"></script>"#,
            &headers
        ));
        headers.insert("cf-mitigated", "block".parse().unwrap());
        assert!(!is_cloudflare_challenge("<html></html>", &headers));
        headers.insert("cf-mitigated", "challenge".parse().unwrap());
        assert!(is_cloudflare_challenge("<html></html>", &headers));
    }

    #[test]
    fn needs_js_rendering_ignores_a_thin_page_with_a_full_menu() {
        let thin = format!(
            "<html><body><nav>{}</nav><p>One book in stock.</p><script src=\"/jquery.js\"></script></body></html>",
            "<a href=\"/c\">Category</a> ".repeat(60)
        );
        assert!(!needs_js_rendering(4, &thin));
    }

    #[test]
    fn needs_js_rendering_ignores_a_populated_app_root() {
        let ssr = format!(
            "<html><body><div id=\"app\"><main><h1>Docs</h1><p>{}</p></main></div><script src=\"/app.js\"></script></body></html>",
            "short server rendered text ".repeat(30)
        );
        assert!(!needs_js_rendering(120, &ssr));
        let empty = format!(
            "<html><body><nav>{}</nav><div id=\"app\">\n  </div><script src=\"/app.js\"></script></body></html>",
            "menu ".repeat(120)
        );
        assert!(needs_js_rendering(120, &empty));
    }

    #[test]
    fn needs_js_rendering_leaves_server_rendered_pages_alone() {
        let ssr = format!(
            "<html><body><div id=\"__next\"><article>{}</article></div><script id=\"__NEXT_DATA__\">{{}}</script></body></html>",
            "Server rendered text ".repeat(150)
        );
        assert!(!needs_js_rendering(450, &ssr));
        assert!(!needs_js_rendering(
            5,
            "<html><body><p>Tiny static page.</p></body></html>"
        ));
    }

    #[test]
    fn needs_js_rendering_passes_real_article() {
        let html = format!(
            "<html><body>{}<script>x</script></body></html>",
            "Real article text ".repeat(5_000)
        );
        assert!(!needs_js_rendering(5_000, &html));
    }

    // --- CloudError mapping -------------------------------------------------

    #[test]
    fn cloud_error_maps_401() {
        let e = CloudError::from_status_and_body(401, "invalid key".into());
        assert!(matches!(e, CloudError::Unauthorized));
        assert!(e.to_string().contains(KEYS_URL));
    }

    #[test]
    fn cloud_error_maps_402() {
        let e = CloudError::from_status_and_body(402, "{}".into());
        assert!(matches!(e, CloudError::InsufficientPlan));
        assert!(e.to_string().contains(PRICING_URL));
    }

    #[test]
    fn cloud_error_maps_429() {
        let e = CloudError::from_status_and_body(429, "slow down".into());
        assert!(matches!(e, CloudError::RateLimited));
        assert!(e.to_string().contains(PRICING_URL));
    }

    #[test]
    fn cloud_error_maps_generic_5xx() {
        let e = CloudError::from_status_and_body(503, "x".repeat(2000));
        match e {
            CloudError::ServerError { status, body } => {
                assert_eq!(status, 503);
                assert!(body.len() <= 500);
            }
            _ => panic!("expected ServerError"),
        }
    }

    #[test]
    fn not_configured_error_points_at_signup() {
        let msg = CloudError::NotConfigured.to_string();
        assert!(msg.contains(SIGNUP_URL));
        assert!(msg.contains("WEBCLAW_API_KEY"));
    }

    // --- CloudClient construction ------------------------------------------

    // `WEBCLAW_API_KEY` is process-global; cargo runs tests in parallel
    // threads. Without serialization, a test that sets the var can race a
    // test asserting it is absent. This lock makes the env-mutating
    // CloudClient tests mutually exclusive (poison-tolerant: a panicking
    // test must not wedge the others).
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn cloud_client_explicit_key_wins_over_env() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: env mutation is serialized by ENV_LOCK; set_var/remove_var
        // are unsafe on the 2024 toolchain. Explicit key must beat the env.
        unsafe {
            std::env::set_var("WEBCLAW_API_KEY", "from-env");
            std::env::set_var("WEBCLAW_API_URL", "http://127.0.0.1:8099/v1/");
        }
        let client = CloudClient::new(Some("from-flag")).expect("client built");
        assert_eq!(client.api_key, "from-flag");
        assert_eq!(client.base_url(), "http://127.0.0.1:8099/v1");
        unsafe {
            std::env::remove_var("WEBCLAW_API_KEY");
            std::env::remove_var("WEBCLAW_API_URL");
        }
    }

    #[test]
    fn cloud_client_none_when_empty() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: env mutation serialized by ENV_LOCK. Clearing the var
        // (incl. any ambient runner value) is what makes this deterministic.
        unsafe {
            std::env::remove_var("WEBCLAW_API_KEY");
        }
        assert!(CloudClient::new(None).is_none());
        assert!(CloudClient::new(Some("")).is_none());
        assert!(CloudClient::new(Some("   ")).is_none());
    }

    #[test]
    fn cloud_client_base_url_strips_trailing_slash() {
        let c = CloudClient::with_key_and_base("k", "https://api.example.com/v1/");
        assert_eq!(c.base_url(), "https://api.example.com/v1");
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        // Ensure we don't slice inside a multi-byte char.
        let s = "a".repeat(10) + "é"; // é is 2 bytes
        let out = truncate(&s, 11);
        assert_eq!(out.chars().count(), 11);
    }
}
