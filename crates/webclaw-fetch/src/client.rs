/// HTTP client with browser TLS fingerprint impersonation.
/// Uses wreq (BoringSSL) for browser-grade TLS + HTTP/2 fingerprinting.
/// Supports single and batch operations with proxy rotation.
/// Automatically detects PDF responses and extracts text via webclaw-pdf.
///
/// Two proxy modes:
/// - **Static**: single proxy (or none) baked into pre-built clients at construction.
/// - **Rotating**: pre-built pool of clients, each with a different proxy + fingerprint.
///   Same-host URLs are routed to the same client for HTTP/2 connection reuse.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use rand::seq::SliceRandom;
use tokio::sync::Semaphore;
use tracing::{debug, instrument, warn};
use webclaw_pdf::PdfMode;

use crate::browser::{self, BrowserProfile, BrowserVariant};
use crate::error::FetchError;

/// Configuration for building a [`FetchClient`].
#[derive(Debug, Clone)]
pub struct FetchConfig {
    pub browser: BrowserProfile,
    /// Single proxy URL. Used when `proxy_pool` is empty.
    pub proxy: Option<String>,
    /// Pool of proxy URLs to rotate through.
    /// When non-empty, each proxy gets a pre-built client with a
    /// random browser fingerprint. Same-host URLs reuse the same client
    /// for HTTP/2 connection multiplexing.
    pub proxy_pool: Vec<String>,
    /// Per-attempt timeout handed to the HTTP client.
    ///
    /// NOTE: the underlying client applies this as two independent budgets —
    /// one for the response head, then a *fresh* one for the body — so a single
    /// attempt can take up to `2 * timeout`. Use [`total_timeout`](Self::total_timeout)
    /// to bound the whole operation.
    pub timeout: Duration,
    /// Ceiling on one complete `fetch`, covering every attempt, redirect and
    /// body read.
    ///
    /// Without this the budget compounds and nothing states the real worst
    /// case: `timeout` is applied twice per attempt by the client, and the
    /// retry loop runs two attempts with a 1s pause, so a `timeout` of 12s
    /// permits 49s. Callers reasonably read `timeout: 12s` as "this returns in
    /// about 12 seconds"; a request that occupies a worker for 49s is a
    /// reliability problem, not a slow page.
    ///
    /// Defaults to `2 * timeout`: one attempt keeps its full head+body budget,
    /// so a slow-but-successful fetch is unaffected, while a retry can no
    /// longer extend past it. Retries exist for transient failures, which fail
    /// fast; retrying after a full-budget timeout rarely succeeds and always
    /// costs the caller another full budget.
    ///
    /// `None` restores the old compounding behaviour.
    pub total_timeout: Option<Duration>,
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub headers: HashMap<String, String>,
    pub pdf_mode: PdfMode,
}

impl Default for FetchConfig {
    fn default() -> Self {
        Self {
            browser: BrowserProfile::Chrome,
            proxy: None,
            proxy_pool: Vec::new(),
            timeout: Duration::from_secs(12),
            total_timeout: Some(Duration::from_secs(24)),
            follow_redirects: true,
            max_redirects: 10,
            headers: HashMap::from([("Accept-Language".to_string(), "en-US,en;q=0.9".to_string())]),
            pdf_mode: PdfMode::default(),
        }
    }
}

/// Result of a successful fetch.
#[derive(Debug, Clone)]
pub struct FetchResult {
    pub html: String,
    pub status: u16,
    /// Final URL after any redirects.
    pub url: String,
    pub headers: http::HeaderMap,
    pub elapsed: Duration,
}

/// Result for a single URL in a batch fetch operation.
#[derive(Debug)]
pub struct BatchResult {
    pub url: String,
    pub result: Result<FetchResult, FetchError>,
}

/// Result for a single URL in a batch fetch-and-extract operation.
#[derive(Debug)]
pub struct BatchExtractResult {
    pub url: String,
    pub result: Result<webclaw_core::ExtractionResult, FetchError>,
}

/// Buffered response that owns its body. Provides the same sync API
/// that webclaw-http::Response used to provide.
struct Response {
    status: u16,
    url: String,
    headers: http::HeaderMap,
    /// Owned as `Vec<u8>` rather than `Bytes` so `into_text` can hand the
    /// allocation straight to `String` instead of copying it (see `into_text`).
    body: Vec<u8>,
}

/// Maximum fetched body size. A single 50 MB HTML document is already
/// several orders of magnitude past any realistic page; larger responses
/// are either malicious (log bomb, zip-bomb decompressed) or streaming
/// bugs. Caps the blast radius of the HTML → markdown conversion
/// downstream (which could otherwise allocate multiple full-size Strings
/// per page in collapse_whitespace + strip_markdown).
const MAX_BODY_BYTES: u64 = 50 * 1024 * 1024;

/// Running decompression-bomb guard: reject as soon as the bytes already
/// buffered plus the next decompressed chunk would cross [`MAX_BODY_BYTES`].
/// Saturating arithmetic so a huge chunk length can't wrap the sum.
fn check_body_ceiling(buffered: usize, next_chunk: usize) -> Result<(), FetchError> {
    let total = (buffered as u64).saturating_add(next_chunk as u64);
    if total > MAX_BODY_BYTES {
        return Err(FetchError::BodyDecode(format!(
            "response body exceeds cap {MAX_BODY_BYTES} bytes (decompressed)"
        )));
    }
    Ok(())
}

impl Response {
    /// Buffer a wreq response into an owned Response.
    ///
    /// Rejects bodies that advertise a Content-Length beyond
    /// [`MAX_BODY_BYTES`] before we pay any allocation, then streams the
    /// body chunk-by-chunk while enforcing a running ceiling. `chunk()`
    /// yields *post-decompression* bytes (gzip/brotli/zstd/deflate are
    /// negotiated), so a tiny compressed payload that inflates to
    /// gigabytes is aborted as soon as the accumulated size crosses the
    /// cap — it never gets fully buffered in memory.
    async fn from_wreq(resp: wreq::Response) -> Result<Self, FetchError> {
        if let Some(len) = resp.content_length()
            && len > MAX_BODY_BYTES
        {
            return Err(FetchError::BodyDecode(format!(
                "response body {len} bytes exceeds cap {MAX_BODY_BYTES}"
            )));
        }
        let status = resp.status().as_u16();
        let url = resp.uri().to_string();
        let headers = resp.headers().clone();

        // wreq 6.0.0-rc.29 dropped `Response::chunk()`. Stream post-decompression
        // bytes via `bytes_stream()` and keep enforcing the running ceiling so a
        // compression bomb is aborted before it is fully buffered in memory.
        // No capacity hint on purpose — see `into_text` and the note below.
        let mut buf: Vec<u8> = Vec::new();
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| FetchError::BodyDecode(e.to_string()))?;
            check_body_ceiling(buf.len(), chunk.len())?;
            buf.extend_from_slice(&chunk);
        }

        Ok(Self {
            status,
            url,
            headers,
            body: buf,
        })
    }

    fn status(&self) -> u16 {
        self.status
    }
    fn url(&self) -> &str {
        &self.url
    }
    fn headers(&self) -> &http::HeaderMap {
        &self.headers
    }
    fn body(&self) -> &[u8] {
        &self.body
    }

    fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    /// Decode the body to `String`, consuming the buffer.
    ///
    /// The valid-UTF-8 case — effectively every page — takes ownership of the
    /// existing allocation, so this costs no allocation and no copy.
    /// `from_utf8_lossy(&self.body).into_owned()` always allocated a second
    /// full-size buffer and memcpy'd the whole body into it.
    ///
    /// The fallback must decode `e.as_bytes()`, i.e. the bytes handed back by
    /// the failed conversion, so invalid-UTF-8 output stays byte-identical to
    /// the previous implementation. Do NOT add `shrink_to_fit()` — it would
    /// reintroduce exactly the copy this removes.
    ///
    /// Charset handling is unchanged and still ignores `Content-Type: charset`;
    /// a page in a non-UTF-8 encoding produces the same mojibake it did before.
    /// Fixing that is separate work with extraction-quality blast radius.
    fn into_text(self) -> String {
        String::from_utf8(self.body)
            .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned())
    }

    /// Consume the response and return the raw, undecoded body bytes.
    /// Used by [`FetchClient::fetch_raw`] for binary payloads (e.g. gzipped
    /// sitemaps) that must not be run through lossy UTF-8 decoding.
    fn into_body(self) -> bytes::Bytes {
        // `Bytes::from(Vec<u8>)` adopts the allocation; still no copy.
        bytes::Bytes::from(self.body)
    }
}

/// Internal representation of the client pool strategy.
enum ClientPool {
    /// Pre-built clients with a fixed proxy (or no proxy).
    /// Fingerprint rotation still works via the pool when `random` is true.
    Static {
        clients: Vec<wreq::Client>,
        random: bool,
    },
    /// Pre-built pool of clients, each with a different proxy + fingerprint.
    /// Requests pick a client deterministically by host for HTTP/2 connection reuse.
    Rotating { clients: Vec<wreq::Client> },
}

/// HTTP client with browser TLS + HTTP/2 fingerprinting via wreq.
///
/// Operates in two modes:
/// - **Static pool**: pre-built clients, optionally with fingerprint rotation.
///   Used when no `proxy_pool` is configured. Fast (no per-request construction).
/// - **Rotating pool**: pre-built clients, one per proxy in the pool.
///   Same-host URLs are routed to the same client for HTTP/2 multiplexing.
pub struct FetchClient {
    pool: ClientPool,
    pdf_mode: PdfMode,
    /// Optional cloud-fallback client. Extractors that need to
    /// escalate past bot protection call `client.cloud()` to get this
    /// out. Stored as `Arc` so cloning a `FetchClient` (common in
    /// axum state) doesn't clone the underlying reqwest pool.
    cloud: Option<std::sync::Arc<crate::cloud::CloudClient>>,
    /// Ceiling on one complete `fetch`. See [`FetchConfig::total_timeout`].
    total_timeout: Option<Duration>,
}

impl FetchClient {
    /// Run one attempt under the remaining share of the total budget.
    ///
    /// Returns `Err(Timeout)` if the budget is already spent, so the caller
    /// stops retrying rather than starting an attempt it cannot finish.
    async fn within_budget<F, T>(
        &self,
        started: Instant,
        url: &str,
        attempt: F,
    ) -> Result<T, FetchError>
    where
        F: std::future::Future<Output = Result<T, FetchError>>,
    {
        let Some(total) = self.total_timeout else {
            return attempt.await;
        };
        let remaining = total.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(FetchError::Build(format!(
                "fetch budget of {}ms exhausted for {url}",
                total.as_millis()
            )));
        }
        match tokio::time::timeout(remaining, attempt).await {
            Ok(res) => res,
            Err(_) => Err(FetchError::Build(format!(
                "fetch exceeded its {}ms budget for {url}",
                total.as_millis()
            ))),
        }
    }

    /// Build a new client from config.
    pub fn new(config: FetchConfig) -> Result<Self, FetchError> {
        let variants = collect_variants(&config.browser);
        let pdf_mode = config.pdf_mode.clone();

        let pool = if config.proxy_pool.is_empty() {
            let clients = variants
                .into_iter()
                .map(|v| {
                    crate::tls::build_client(
                        v,
                        config.timeout,
                        &config.headers,
                        config.proxy.as_deref(),
                        config.follow_redirects,
                        config.max_redirects,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;

            let random = matches!(config.browser, BrowserProfile::Random);
            debug!(
                count = clients.len(),
                random, "fetch client ready (static pool)"
            );

            ClientPool::Static { clients, random }
        } else {
            let mut rng = rand::thread_rng();

            let clients = config
                .proxy_pool
                .iter()
                .map(|proxy| {
                    let v = *variants.choose(&mut rng).unwrap();
                    crate::tls::build_client(
                        v,
                        config.timeout,
                        &config.headers,
                        Some(proxy),
                        config.follow_redirects,
                        config.max_redirects,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;

            debug!(
                clients = clients.len(),
                "fetch client ready (pre-built rotating pool)"
            );

            ClientPool::Rotating { clients }
        };

        Ok(Self {
            pool,
            pdf_mode,
            cloud: None,
            total_timeout: config.total_timeout,
        })
    }

    /// Attach a cloud-fallback client. Returns `self` so it composes in
    /// a builder-ish way:
    ///
    /// ```ignore
    /// let client = FetchClient::new(config)?
    ///     .with_cloud(CloudClient::from_env()?);
    /// ```
    ///
    /// Extractors that can escalate past bot protection will call
    /// `client.cloud()` internally. Sets the field regardless of
    /// whether `cloud` is configured to bypass anything specific —
    /// attachment is cheap (just wraps in `Arc`).
    pub fn with_cloud(mut self, cloud: crate::cloud::CloudClient) -> Self {
        self.cloud = Some(std::sync::Arc::new(cloud));
        self
    }

    /// Optional cloud-fallback client, if one was attached via
    /// [`Self::with_cloud`]. Extractors that handle antibot sites
    /// pass this into `cloud::smart_fetch_html`.
    pub fn cloud(&self) -> Option<&crate::cloud::CloudClient> {
        self.cloud.as_deref()
    }

    /// Fetch a URL with per-site rescue paths: Reddit URLs redirect to the
    /// `.json` API, and Akamai-style challenge responses trigger a homepage
    /// cookie warmup and a retry. Returns the same `FetchResult` shape as
    /// [`Self::fetch`] so every caller (CLI, MCP, OSS server, production
    /// server) benefits without shape churn.
    ///
    /// This is the method most callers want. Use plain [`Self::fetch`] only
    /// when you need literal no-rescue behavior (e.g. inside the rescue
    /// logic itself to avoid recursion).
    pub async fn fetch_smart(&self, url: &str) -> Result<FetchResult, FetchError> {
        // Reddit: fetch old.reddit.com for stable server-rendered HTML.
        // The JSON API is blocked; old.reddit.com works without JS or auth.
        let owned;
        let url = if crate::reddit::is_reddit_url(url) {
            owned = crate::reddit::to_old_reddit_url(url);
            owned.as_str()
        } else {
            url
        };

        let resp = self.fetch(url).await?;

        // Akamai / bazadebezolkohpepadr challenge: visit the homepage to
        // collect warmup cookies (_abck, bm_sz, etc.), then retry.
        if is_challenge_html(&resp.html)
            && let Some(homepage) = extract_homepage(url)
        {
            debug!("challenge detected, warming cookies via {homepage}");
            let _ = self.fetch(&homepage).await;
            if let Ok(retry) = self.fetch(url).await {
                return Ok(retry);
            }
        }

        Ok(resp)
    }

    /// Fetch a URL and return the raw HTML + response metadata.
    ///
    /// Automatically retries on transient failures (network errors, 5xx, 429)
    /// with exponential backoff: 0s, 1s (2 attempts total). No per-site
    /// rescue logic; use [`Self::fetch_smart`] for that.
    #[instrument(skip(self), fields(url = %url))]
    pub async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError> {
        let delays = [Duration::ZERO, Duration::from_secs(1)];
        let mut last_err = None;
        // Clock starts before the first attempt so the retry pause and every
        // attempt come out of one budget, rather than each getting a fresh one.
        let started = Instant::now();

        for (attempt, delay) in delays.iter().enumerate() {
            if attempt > 0 {
                tokio::time::sleep(*delay).await;
            }

            match self.within_budget(started, url, self.fetch_once(url)).await {
                Ok(result) => {
                    if is_retryable_status(result.status) && attempt < delays.len() - 1 {
                        warn!(
                            url,
                            status = result.status,
                            attempt = attempt + 1,
                            "retryable status, will retry"
                        );
                        last_err = Some(FetchError::Build(format!("HTTP {}", result.status)));
                        continue;
                    }
                    if attempt > 0 {
                        debug!(url, attempt = attempt + 1, "retry succeeded");
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if !is_retryable_error(&e) || attempt == delays.len() - 1 {
                        return Err(e);
                    }
                    warn!(
                        url,
                        error = %e,
                        attempt = attempt + 1,
                        "transient error, will retry"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| FetchError::Build("all retries exhausted".into())))
    }

    /// Single fetch attempt.
    async fn fetch_once(&self, url: &str) -> Result<FetchResult, FetchError> {
        self.fetch_once_with_headers(url, &[]).await
    }

    /// Single fetch attempt with optional per-request headers appended
    /// after the profile defaults. Used by extractors that need to
    /// satisfy site-specific headers (e.g. `x-ig-app-id` for Instagram's
    /// internal API).
    async fn fetch_once_with_headers(
        &self,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<FetchResult, FetchError> {
        let parsed_url = crate::url_security::validate_public_http_url(url).await?;
        let url = parsed_url.as_str();
        let start = Instant::now();
        let client = self.pick_client(url);

        let mut req = client.get(url);
        for (k, v) in extra {
            req = req.header(*k, *v);
        }
        let resp = req.send().await?;
        let response = Response::from_wreq(resp).await?;
        response_to_result(response, start)
    }

    /// Fetch a URL with extra per-request headers appended after the
    /// browser-profile defaults. Same retry semantics as `fetch`.
    ///
    /// Use this when an upstream API requires a header the global
    /// `FetchConfig.headers` shouldn't carry to other hosts (Instagram's
    /// `x-ig-app-id`, GitHub's `Authorization` once we wire `GITHUB_TOKEN`,
    /// Reddit's compliant UA when we add OAuth, etc.).
    #[instrument(skip(self, extra), fields(url = %url, extra_count = extra.len()))]
    pub async fn fetch_with_headers(
        &self,
        url: &str,
        extra: &[(&str, &str)],
    ) -> Result<FetchResult, FetchError> {
        let delays = [Duration::ZERO, Duration::from_secs(1)];
        let mut last_err = None;
        // One budget across the whole call — see `within_budget`.
        let started = Instant::now();

        for (attempt, delay) in delays.iter().enumerate() {
            if attempt > 0 {
                tokio::time::sleep(*delay).await;
            }
            match self
                .within_budget(started, url, self.fetch_once_with_headers(url, extra))
                .await
            {
                Ok(result) => {
                    if is_retryable_status(result.status) && attempt < delays.len() - 1 {
                        warn!(
                            url,
                            status = result.status,
                            attempt = attempt + 1,
                            "retryable status, will retry"
                        );
                        last_err = Some(FetchError::Build(format!("HTTP {}", result.status)));
                        continue;
                    }
                    if attempt > 0 {
                        debug!(url, attempt = attempt + 1, "retry succeeded");
                    }
                    return Ok(result);
                }
                Err(e) => {
                    if !is_retryable_error(&e) || attempt == delays.len() - 1 {
                        return Err(e);
                    }
                    warn!(
                        url,
                        error = %e,
                        attempt = attempt + 1,
                        "transient error, will retry"
                    );
                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| FetchError::Build("all retries exhausted".into())))
    }

    /// Fetch a URL and return the raw, undecoded response body as bytes.
    ///
    /// Unlike [`fetch`](Self::fetch), this does **not** run the body through
    /// `String::from_utf8_lossy`, so binary payloads survive intact. This is
    /// required for gzipped sitemaps (`.xml.gz`): such files are served with
    /// `Content-Type: application/gzip` and *no* `Content-Encoding`, so wreq
    /// never auto-inflates them — the bytes arrive as raw gzip and the lossy
    /// String path would mangle them. Callers detect the gzip magic
    /// (`0x1f 0x8b`) and gunzip before parsing.
    ///
    /// No retry wrapper: callers (sitemap discovery) already tolerate
    /// per-URL failures by skipping. Returns `(status, body)`.
    pub async fn fetch_raw(&self, url: &str) -> Result<(u16, bytes::Bytes), FetchError> {
        let parsed_url = crate::url_security::validate_public_http_url(url).await?;
        let url = parsed_url.as_str();
        let client = self.pick_client(url);
        let resp = client.get(url).send().await?;
        let response = Response::from_wreq(resp).await?;
        Ok((response.status(), response.into_body()))
    }

    /// Fetch a URL then extract structured content.
    #[instrument(skip(self), fields(url = %url))]
    pub async fn fetch_and_extract(
        &self,
        url: &str,
    ) -> Result<webclaw_core::ExtractionResult, FetchError> {
        self.fetch_and_extract_with_options(url, &webclaw_core::ExtractionOptions::default())
            .await
    }

    /// Fetch a URL then extract structured content with custom extraction options.
    #[instrument(skip(self, options), fields(url = %url))]
    pub async fn fetch_and_extract_with_options(
        &self,
        url: &str,
        options: &webclaw_core::ExtractionOptions,
    ) -> Result<webclaw_core::ExtractionResult, FetchError> {
        let parsed_url = crate::url_security::validate_public_http_url(url).await?;
        let url = parsed_url.as_str();

        // Reddit: rewrite to old.reddit.com for stable server-rendered HTML.
        // webclaw-core's Reddit fast path then parses the thread structure.
        let reddit_owned;
        let url = if crate::reddit::is_reddit_url(url) {
            reddit_owned = crate::reddit::to_old_reddit_url(url);
            debug!("reddit: rewriting to {reddit_owned}");
            reddit_owned.as_str()
        } else {
            url
        };

        let start = Instant::now();
        let client = self.pick_client(url);
        let resp = client.get(url).send().await?;
        let mut response = Response::from_wreq(resp).await?;

        // Cookie warmup: if we get a challenge page, visit the homepage first
        // to collect Akamai cookies (_abck, bm_sz, etc.), then retry.
        if is_challenge_response(&response)
            && let Some(homepage) = extract_homepage(url)
        {
            debug!("challenge detected, warming cookies via {homepage}");
            let _ = self.fetch(&homepage).await;
            let resp = client.get(url).send().await?;
            response = Response::from_wreq(resp).await?;
            debug!("retried after cookie warmup: status={}", response.status());
        }

        let status = response.status();
        let final_url = response.url().to_string();

        // Borrowed, not cloned: both consumers below take `&HeaderMap`, and the
        // borrow ends before `response` is moved into `into_text()`.
        let headers = response.headers();

        let is_pdf = is_pdf_content_type(headers);

        if is_pdf {
            debug!(status, "detected PDF response, using pdf extraction");

            let bytes = response.body();

            let elapsed = start.elapsed();
            debug!(
                status,
                bytes = bytes.len(),
                elapsed_ms = %elapsed.as_millis(),
                "PDF fetch complete"
            );

            let pdf_result = webclaw_pdf::extract_pdf(bytes, self.pdf_mode.clone())?;
            Ok(pdf_to_extraction_result(&pdf_result, &final_url))
        } else if let Some(doc_type) =
            crate::document::is_document_content_type(headers, &final_url)
        {
            debug!(status, doc_type = ?doc_type, "detected document response, extracting");

            let bytes = response.body();

            let elapsed = start.elapsed();
            debug!(
                status,
                bytes = bytes.len(),
                elapsed_ms = %elapsed.as_millis(),
                "document fetch complete"
            );

            let mut result = crate::document::extract_document(bytes, doc_type)?;
            result.metadata.url = Some(final_url);
            Ok(result)
        } else {
            let html = response.into_text();

            let elapsed = start.elapsed();
            debug!(status, elapsed_ms = %elapsed.as_millis(), "fetch complete");

            // LinkedIn: extract from embedded <code> JSON blobs
            if crate::linkedin::is_linkedin_post(&final_url) {
                if let Some(result) = crate::linkedin::extract_linkedin_post(&html, &final_url) {
                    debug!("linkedin extraction succeeded");
                    return Ok(result);
                }
                debug!("linkedin extraction failed, falling back to standard");
            }

            let extraction = webclaw_core::extract_with_options(&html, Some(&final_url), options)?;

            Ok(extraction)
        }
    }

    /// Fetch multiple URLs concurrently with bounded parallelism.
    pub async fn fetch_batch(
        self: &Arc<Self>,
        urls: &[&str],
        concurrency: usize,
    ) -> Vec<BatchResult> {
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut handles = Vec::with_capacity(urls.len());

        for (idx, url) in urls.iter().enumerate() {
            let permit = Arc::clone(&semaphore);
            let client = Arc::clone(self);
            let url = url.to_string();

            handles.push(tokio::spawn(async move {
                // Don't panic if the semaphore has been closed under us
                // (adversarial runtime state or shutdown race). Surface a
                // typed error instead so the caller sees one failed URL in
                // the batch instead of a silently-dropped task.
                let result = match permit.acquire().await {
                    Ok(_permit) => client.fetch(&url).await,
                    Err(_) => Err(FetchError::Build("semaphore closed before acquire".into())),
                };
                (idx, BatchResult { url, result })
            }));
        }

        collect_ordered(handles, urls.len()).await
    }

    /// Fetch and extract multiple URLs concurrently with bounded parallelism.
    pub async fn fetch_and_extract_batch(
        self: &Arc<Self>,
        urls: &[&str],
        concurrency: usize,
    ) -> Vec<BatchExtractResult> {
        self.fetch_and_extract_batch_with_options(
            urls,
            concurrency,
            &webclaw_core::ExtractionOptions::default(),
        )
        .await
    }

    /// Fetch and extract multiple URLs concurrently with custom extraction options.
    pub async fn fetch_and_extract_batch_with_options(
        self: &Arc<Self>,
        urls: &[&str],
        concurrency: usize,
        options: &webclaw_core::ExtractionOptions,
    ) -> Vec<BatchExtractResult> {
        let semaphore = Arc::new(Semaphore::new(concurrency));
        let mut handles = Vec::with_capacity(urls.len());

        for (idx, url) in urls.iter().enumerate() {
            let permit = Arc::clone(&semaphore);
            let client = Arc::clone(self);
            let url = url.to_string();
            let opts = options.clone();

            handles.push(tokio::spawn(async move {
                let result = match permit.acquire().await {
                    Ok(_permit) => client.fetch_and_extract_with_options(&url, &opts).await,
                    Err(_) => Err(FetchError::Build("semaphore closed before acquire".into())),
                };
                (idx, BatchExtractResult { url, result })
            }));
        }

        collect_ordered(handles, urls.len()).await
    }

    /// Fetch and extract many URLs, yielding each result as it completes.
    ///
    /// Prefer this over [`fetch_and_extract_batch`](Self::fetch_and_extract_batch)
    /// for large inputs. That one spawns a task per URL up front and collects
    /// everything into a `Vec`, so both task count and memory scale with the
    /// input — which is what forces callers to impose an arbitrary maximum
    /// batch size. This polls at most `concurrency` futures at a time and hands
    /// each result straight to the consumer, so a batch of ten URLs and a batch
    /// of ten thousand cost the same at rest and no cap is needed.
    ///
    /// Results arrive in **completion order**, not input order — that is the
    /// point of streaming. Use `fetch_and_extract_batch` when order matters and
    /// the input is small enough to buffer.
    ///
    /// ```ignore
    /// use futures_util::StreamExt;
    /// let mut s = client.fetch_and_extract_batch_stream(urls, 10, opts);
    /// while let Some(res) = s.next().await {
    ///     // write it out; nothing accumulates
    /// }
    /// ```
    pub fn fetch_and_extract_batch_stream(
        self: &Arc<Self>,
        urls: Vec<String>,
        concurrency: usize,
        options: webclaw_core::ExtractionOptions,
        // `use<>`: the stream clones the Arc internally and borrows nothing,
        // so it must not capture `&self`'s lifetime — otherwise callers cannot
        // return it from the scope that built it (e.g. an axum handler
        // streaming a response body).
    ) -> impl futures_util::Stream<Item = BatchExtractResult> + use<> {
        let client = Arc::clone(self);
        let options = Arc::new(options);
        futures_util::stream::iter(urls.into_iter().map(move |url| {
            let client = Arc::clone(&client);
            let options = Arc::clone(&options);
            async move {
                let result = client.fetch_and_extract_with_options(&url, &options).await;
                BatchExtractResult { url, result }
            }
        }))
        // Bounds in-flight work without materialising a task per URL.
        .buffer_unordered(concurrency.max(1))
    }

    /// Returns the number of proxies in the rotation pool, or 0 if static mode.
    pub fn proxy_pool_size(&self) -> usize {
        match &self.pool {
            ClientPool::Static { .. } => 0,
            ClientPool::Rotating { clients } => clients.len(),
        }
    }

    /// Pick a client from the pool for a given URL.
    fn pick_client(&self, url: &str) -> &wreq::Client {
        match &self.pool {
            ClientPool::Static { clients, random } => {
                if *random {
                    let host = extract_host(url);
                    pick_for_host(clients, &host)
                } else {
                    &clients[0]
                }
            }
            ClientPool::Rotating { clients } => pick_random(clients),
        }
    }
}

// ---------------------------------------------------------------------------
// Fetcher trait implementation
//
// Vertical extractors consume the [`crate::fetcher::Fetcher`] trait
// rather than `FetchClient` directly, which is what lets the production
// API server swap in a tls-sidecar-backed implementation without
// pulling wreq into its dependency graph. For everyone else (CLI, MCP,
// self-hosted OSS server) this impl means "pass the FetchClient you
// already have; nothing changes".
// ---------------------------------------------------------------------------

#[async_trait::async_trait]
impl crate::fetcher::Fetcher for FetchClient {
    async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError> {
        FetchClient::fetch(self, url).await
    }

    async fn fetch_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<FetchResult, FetchError> {
        FetchClient::fetch_with_headers(self, url, headers).await
    }

    fn cloud(&self) -> Option<&crate::cloud::CloudClient> {
        FetchClient::cloud(self)
    }
}

/// Collect the browser variants to use based on the browser profile.
fn collect_variants(profile: &BrowserProfile) -> Vec<BrowserVariant> {
    match profile {
        BrowserProfile::Random => browser::all_variants(),
        BrowserProfile::Chrome => vec![browser::latest_chrome()],
        BrowserProfile::Firefox => vec![browser::latest_firefox()],
        BrowserProfile::SafariIos => vec![BrowserVariant::SafariIos26],
    }
}

/// Convert a buffered Response into a FetchResult.
fn response_to_result(response: Response, start: Instant) -> Result<FetchResult, FetchError> {
    let status = response.status();
    let final_url = response.url().to_string();
    let headers = response.headers().clone();
    let html = response.into_text();
    let elapsed = start.elapsed();

    debug!(status, elapsed_ms = %elapsed.as_millis(), "fetch complete");

    Ok(FetchResult {
        html,
        status,
        url: final_url,
        headers,
        elapsed,
    })
}

/// Extract the host from a URL, returning empty string on parse failure.
fn extract_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(String::from))
        .unwrap_or_default()
}

/// Pick a client deterministically based on a host string.
/// Same host always gets the same client, enabling HTTP/2 connection reuse.
fn pick_for_host<'a>(clients: &'a [wreq::Client], host: &str) -> &'a wreq::Client {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % clients.len();
    &clients[idx]
}

/// Pick a random client from the pool for per-request rotation.
fn pick_random(clients: &[wreq::Client]) -> &wreq::Client {
    use rand::Rng;
    let idx = rand::thread_rng().gen_range(0..clients.len());
    &clients[idx]
}

/// Status codes worth retrying: server errors + rate limiting.
fn is_retryable_status(status: u16) -> bool {
    status == 429
        || status == 502
        || status == 503
        || status == 504
        || status == 520
        || status == 521
        || status == 522
        || status == 523
        || status == 524
}

/// Errors worth retrying: network/connection failures (not client errors).
fn is_retryable_error(err: &FetchError) -> bool {
    matches!(err, FetchError::Request(_) | FetchError::BodyDecode(_))
}

fn is_pdf_content_type(headers: &http::HeaderMap) -> bool {
    headers
        .get("content-type")
        .and_then(|ct| ct.to_str().ok())
        .map(|ct| {
            let mime = ct.split(';').next().unwrap_or("").trim();
            mime.eq_ignore_ascii_case("application/pdf")
        })
        .unwrap_or(false)
}

/// Detect if a response looks like a bot protection challenge page.
fn is_challenge_response(response: &Response) -> bool {
    let body_len = response.body().len();
    if body_len > 15_000 || body_len == 0 {
        return false;
    }
    is_challenge_html(response.text().as_ref())
}

/// Same as `is_challenge_response`, operating on a body string directly
/// so callers holding a `FetchResult` can reuse the heuristic.
fn is_challenge_html(html: &str) -> bool {
    let len = html.len();
    if len > 15_000 || len == 0 {
        return false;
    }
    let lower = html.to_lowercase();
    if lower.contains("<title>challenge page</title>") {
        return true;
    }
    if lower.contains("bazadebezolkohpepadr") && len < 5_000 {
        return true;
    }
    false
}

/// Extract the homepage URL (scheme + host[:port]) from a full URL.
fn extract_homepage(url: &str) -> Option<String> {
    url::Url::parse(url).ok().map(|u| {
        let host = u.host_str().unwrap_or("");
        // `port()` is `Some` only for a non-default port; include it so a
        // host like example.com:8443 is warmed on the right port.
        match u.port() {
            Some(port) => format!("{}://{}:{}/", u.scheme(), host, port),
            None => format!("{}://{}/", u.scheme(), host),
        }
    })
}

/// Convert a webclaw-pdf PdfResult into a webclaw-core ExtractionResult.
fn pdf_to_extraction_result(
    pdf: &webclaw_pdf::PdfResult,
    url: &str,
) -> webclaw_core::ExtractionResult {
    let markdown = webclaw_pdf::to_markdown(pdf);
    let word_count = markdown.split_whitespace().count();

    webclaw_core::ExtractionResult {
        metadata: webclaw_core::Metadata {
            title: pdf.metadata.title.clone(),
            description: pdf.metadata.subject.clone(),
            author: pdf.metadata.author.clone(),
            published_date: None,
            language: None,
            url: Some(url.to_string()),
            site_name: None,
            image: None,
            favicon: None,
            word_count,
        },
        content: webclaw_core::Content {
            markdown,
            plain_text: pdf.text.clone(),
            links: Vec::new(),
            images: Vec::new(),
            code_blocks: Vec::new(),
            raw_html: None,
        },
        domain_data: None,
        structured_data: vec![],
    }
}

/// Collect spawned tasks and reorder results to match input order.
async fn collect_ordered<T>(
    handles: Vec<tokio::task::JoinHandle<(usize, T)>>,
    len: usize,
) -> Vec<T> {
    let mut slots: Vec<Option<T>> = (0..len).map(|_| None).collect();

    for handle in handles {
        match handle.await {
            Ok((idx, result)) => {
                slots[idx] = Some(result);
            }
            Err(e) => {
                warn!(error = %e, "batch task panicked");
            }
        }
    }

    slots.into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `Response` around a raw body so the decode path can be tested
    /// without a network round trip.
    fn resp_with_body(body: Vec<u8>) -> Response {
        Response {
            status: 200,
            url: "https://example.com/".to_string(),
            headers: http::HeaderMap::new(),
            body,
        }
    }

    #[tokio::test]
    async fn batch_stream_yields_every_url_without_buffering_them() {
        use futures_util::StreamExt;

        // All unroutable, so each fails fast — the point here is the stream
        // contract (every input yields exactly one result), not the fetching.
        let urls: Vec<String> = (0..25)
            .map(|i| format!("https://nonexistent-{i}.invalid/"))
            .collect();
        let expected: std::collections::HashSet<String> = urls.iter().cloned().collect();

        let client = Arc::new(FetchClient::new(FetchConfig::default()).expect("build client"));
        let mut seen = std::collections::HashSet::new();
        let mut stream = client.fetch_and_extract_batch_stream(
            urls,
            8,
            webclaw_core::ExtractionOptions::default(),
        );
        while let Some(res) = stream.next().await {
            assert!(
                seen.insert(res.url),
                "each URL must be yielded exactly once"
            );
        }
        assert_eq!(seen, expected, "every input URL must produce a result");
    }

    #[tokio::test]
    async fn batch_stream_survives_zero_concurrency() {
        use futures_util::StreamExt;
        let client = Arc::new(FetchClient::new(FetchConfig::default()).expect("build client"));
        // `buffer_unordered(0)` would stall forever; the API clamps it.
        let mut stream = client.fetch_and_extract_batch_stream(
            vec!["https://nonexistent.invalid/".to_string()],
            0,
            webclaw_core::ExtractionOptions::default(),
        );
        assert!(
            stream.next().await.is_some(),
            "must not deadlock at concurrency 0"
        );
    }

    #[test]
    fn total_timeout_defaults_to_twice_the_per_attempt_timeout() {
        // The client applies `timeout` twice per attempt (head, then a fresh
        // one for the body), so 2x keeps a single slow-but-successful fetch
        // intact while stopping the retry loop from compounding it.
        let c = FetchConfig::default();
        assert_eq!(c.timeout, Duration::from_secs(12));
        assert_eq!(c.total_timeout, Some(Duration::from_secs(24)));
    }

    #[tokio::test]
    async fn within_budget_refuses_an_attempt_once_the_budget_is_spent() {
        let client = FetchClient::new(FetchConfig {
            total_timeout: Some(Duration::from_millis(50)),
            ..Default::default()
        })
        .expect("build client");

        // Pretend the call started long ago: the next attempt must not run.
        let started = Instant::now() - Duration::from_secs(5);
        let ran = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ran2 = std::sync::Arc::clone(&ran);
        let res: Result<(), FetchError> = client
            .within_budget(started, "https://example.com", async move {
                ran2.store(true, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            })
            .await;

        assert!(
            res.is_err(),
            "an exhausted budget must not start an attempt"
        );
        assert!(
            !ran.load(std::sync::atomic::Ordering::SeqCst),
            "the attempt future must never be polled once the budget is gone"
        );
    }

    #[tokio::test]
    async fn within_budget_bounds_a_hanging_attempt() {
        let client = FetchClient::new(FetchConfig {
            total_timeout: Some(Duration::from_millis(80)),
            ..Default::default()
        })
        .expect("build client");

        let began = Instant::now();
        let res: Result<(), FetchError> = client
            .within_budget(Instant::now(), "https://example.com", async {
                // Far longer than the budget.
                tokio::time::sleep(Duration::from_secs(30)).await;
                Ok(())
            })
            .await;

        assert!(res.is_err());
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "must abort at the budget, not run to completion"
        );
    }

    #[tokio::test]
    async fn no_total_timeout_restores_uncapped_behaviour() {
        let client = FetchClient::new(FetchConfig {
            total_timeout: None,
            ..Default::default()
        })
        .expect("build client");
        let started = Instant::now() - Duration::from_secs(600);
        let res: Result<u8, FetchError> = client
            .within_budget(started, "https://example.com", async { Ok(7) })
            .await;
        assert_eq!(res.expect("should run"), 7);
    }

    #[test]
    fn into_text_matches_lossy_decoding_byte_for_byte() {
        // `into_text` swapped `from_utf8_lossy(..).into_owned()` for a
        // consuming `String::from_utf8`. The valid case must be identical and
        // the INVALID case must still produce the same replacement characters
        // in the same positions — that is the whole hazard of the change.
        let mut cases: Vec<Vec<u8>> = vec![
            b"".to_vec(),
            b"plain ascii".to_vec(),
            "héllo wörld — em dash".as_bytes().to_vec(),
            "日本語のテキスト".as_bytes().to_vec(),
            b"\xEF\xBB\xBFleading BOM".to_vec(),
            b"truncated 2-byte: \xC3".to_vec(),
            b"truncated 3-byte: \xE2\x82".to_vec(),
            b"truncated 4-byte: \xF0\x9F\x98".to_vec(),
            b"lone surrogate: \xED\xA0\x80".to_vec(),
            b"overlong NUL: \xC0\x80".to_vec(),
            b"beyond U+10FFFF: \xF5\x80\x80\x80".to_vec(),
            b"windows-1252 quotes: \x93hi\x94".to_vec(),
            vec![0xFF, 0xFE, 0xFD],
        ];
        // A bad byte at several offsets, including around common buffer edges.
        for off in [0usize, 1, 4095, 4096, 4097, 65535] {
            let mut v = vec![b'a'; 70_000];
            v[off] = 0xFF;
            cases.push(v);
        }

        for body in cases {
            let expected = String::from_utf8_lossy(&body).into_owned();
            let got = resp_with_body(body.clone()).into_text();
            assert_eq!(
                got,
                expected,
                "decode diverged for body prefix {:?}",
                &body[..body.len().min(24)]
            );
        }
    }

    #[test]
    fn into_text_reuses_the_buffer_for_valid_utf8() {
        // The point of the change: no second allocation, no copy. Proven by
        // the String landing on the same address the Vec occupied.
        let body = "a reasonably sized valid utf-8 body".repeat(4096);
        let bytes = body.into_bytes();
        let ptr = bytes.as_ptr();
        let text = resp_with_body(bytes).into_text();
        assert_eq!(text.as_ptr(), ptr, "valid UTF-8 must not be re-allocated");
    }

    #[test]
    fn into_body_still_returns_the_bytes_unchanged() {
        let raw = vec![0x1f, 0x8b, 0x08, 0x00, 0xff, 0xfe];
        assert_eq!(resp_with_body(raw.clone()).into_body().as_ref(), &raw[..]);
    }

    #[test]
    fn test_batch_result_struct() {
        let ok = BatchResult {
            url: "https://example.com".to_string(),
            result: Ok(FetchResult {
                html: "<html></html>".to_string(),
                status: 200,
                url: "https://example.com".to_string(),
                headers: http::HeaderMap::new(),
                elapsed: Duration::from_millis(42),
            }),
        };
        assert_eq!(ok.url, "https://example.com");
        assert!(ok.result.is_ok());
        assert_eq!(ok.result.unwrap().status, 200);

        let err = BatchResult {
            url: "https://bad.example".to_string(),
            result: Err(FetchError::InvalidUrl("bad url".into())),
        };
        assert!(err.result.is_err());
    }

    #[test]
    fn body_ceiling_allows_under_cap() {
        assert!(check_body_ceiling(0, 1024).is_ok());
        assert!(check_body_ceiling(MAX_BODY_BYTES as usize - 1, 1).is_ok());
    }

    #[test]
    fn body_ceiling_rejects_at_and_over_cap() {
        // Exactly at the cap is allowed; one byte over is rejected.
        assert!(check_body_ceiling(MAX_BODY_BYTES as usize, 1).is_err());
        // A small buffer plus a huge inflated chunk (decompression bomb)
        // is caught on the very first oversized chunk.
        let err = check_body_ceiling(16, 64 * 1024 * 1024).unwrap_err();
        assert!(matches!(err, FetchError::BodyDecode(_)));
    }

    #[test]
    fn body_ceiling_saturates_on_overflow() {
        // usize::MAX chunk must not wrap the running sum to a small value.
        assert!(check_body_ceiling(usize::MAX, usize::MAX).is_err());
    }

    #[test]
    fn test_batch_extract_result_struct() {
        let err = BatchExtractResult {
            url: "https://example.com".to_string(),
            result: Err(FetchError::BodyDecode("timeout".into())),
        };
        assert_eq!(err.url, "https://example.com");
        assert!(err.result.is_err());
    }

    #[tokio::test]
    async fn test_batch_preserves_order() {
        let handles: Vec<tokio::task::JoinHandle<(usize, String)>> = vec![
            tokio::spawn(async { (2, "c".to_string()) }),
            tokio::spawn(async { (0, "a".to_string()) }),
            tokio::spawn(async { (1, "b".to_string()) }),
        ];

        let results = collect_ordered(handles, 3).await;
        assert_eq!(results, vec!["a", "b", "c"]);
    }

    #[tokio::test]
    async fn test_collect_ordered_handles_gaps() {
        let handles: Vec<tokio::task::JoinHandle<(usize, String)>> = vec![
            tokio::spawn(async { (0, "first".to_string()) }),
            tokio::spawn(async { (2, "third".to_string()) }),
        ];

        let results = collect_ordered(handles, 3).await;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0], "first");
        assert_eq!(results[1], "third");
    }

    #[test]
    fn test_is_pdf_content_type() {
        let mut headers = http::HeaderMap::new();
        headers.insert("content-type", "application/pdf".parse().unwrap());
        assert!(is_pdf_content_type(&headers));

        headers.insert(
            "content-type",
            "application/pdf; charset=utf-8".parse().unwrap(),
        );
        assert!(is_pdf_content_type(&headers));

        headers.insert("content-type", "Application/PDF".parse().unwrap());
        assert!(is_pdf_content_type(&headers));

        headers.insert("content-type", "text/html".parse().unwrap());
        assert!(!is_pdf_content_type(&headers));

        let empty = http::HeaderMap::new();
        assert!(!is_pdf_content_type(&empty));
    }

    #[test]
    fn test_pdf_to_extraction_result() {
        let pdf = webclaw_pdf::PdfResult {
            text: "Hello from PDF.".into(),
            page_count: 2,
            metadata: webclaw_pdf::PdfMetadata {
                title: Some("My Doc".into()),
                author: Some("Author".into()),
                subject: Some("Testing".into()),
                creator: None,
            },
        };

        let result = pdf_to_extraction_result(&pdf, "https://example.com/doc.pdf");

        assert_eq!(result.metadata.title.as_deref(), Some("My Doc"));
        assert_eq!(result.metadata.author.as_deref(), Some("Author"));
        assert_eq!(result.metadata.description.as_deref(), Some("Testing"));
        assert_eq!(
            result.metadata.url.as_deref(),
            Some("https://example.com/doc.pdf")
        );
        assert!(result.content.markdown.contains("# My Doc"));
        assert!(result.content.markdown.contains("Hello from PDF."));
        assert_eq!(result.content.plain_text, "Hello from PDF.");
        assert!(result.content.links.is_empty());
        assert!(result.domain_data.is_none());
        assert!(result.metadata.word_count > 0);
    }

    #[test]
    fn test_static_pool_no_proxy() {
        let config = FetchConfig::default();
        let client = FetchClient::new(config).unwrap();
        assert_eq!(client.proxy_pool_size(), 0);
    }

    #[test]
    fn test_rotating_pool_prebuilds_clients() {
        let config = FetchConfig {
            proxy_pool: vec![
                "http://proxy1:8080".into(),
                "http://proxy2:8080".into(),
                "http://proxy3:8080".into(),
            ],
            ..Default::default()
        };
        let client = FetchClient::new(config).unwrap();
        assert_eq!(client.proxy_pool_size(), 3);
    }

    #[test]
    fn test_pick_for_host_deterministic() {
        let config = FetchConfig {
            browser: BrowserProfile::Random,
            ..Default::default()
        };
        let client = FetchClient::new(config).unwrap();

        let clients = match &client.pool {
            ClientPool::Static { clients, .. } => clients,
            ClientPool::Rotating { clients } => clients,
        };

        let a1 = pick_for_host(clients, "example.com") as *const _;
        let a2 = pick_for_host(clients, "example.com") as *const _;
        let a3 = pick_for_host(clients, "example.com") as *const _;
        assert_eq!(a1, a2);
        assert_eq!(a2, a3);
    }

    #[test]
    fn test_pick_for_host_distributes() {
        let config = FetchConfig {
            proxy_pool: (0..10).map(|i| format!("http://proxy{i}:8080")).collect(),
            ..Default::default()
        };
        let client = FetchClient::new(config).unwrap();

        let clients = match &client.pool {
            ClientPool::Static { clients, .. } | ClientPool::Rotating { clients } => clients,
        };

        let hosts = [
            "example.com",
            "google.com",
            "github.com",
            "rust-lang.org",
            "crates.io",
        ];

        let indices: Vec<usize> = hosts
            .iter()
            .map(|h| {
                let ptr = pick_for_host(clients, h) as *const _;
                clients.iter().position(|c| std::ptr::eq(c, ptr)).unwrap()
            })
            .collect();

        let unique: std::collections::HashSet<_> = indices.iter().collect();
        assert!(
            unique.len() >= 2,
            "expected host distribution across clients, got indices: {indices:?}"
        );
    }

    #[test]
    fn test_extract_host() {
        assert_eq!(extract_host("https://example.com/path"), "example.com");
        assert_eq!(
            extract_host("https://sub.example.com:8080/foo"),
            "sub.example.com"
        );
        assert_eq!(extract_host("not-a-url"), "");
    }

    #[test]
    fn test_default_config_has_empty_proxy_pool() {
        let config = FetchConfig::default();
        assert!(config.proxy_pool.is_empty());
        assert!(config.proxy.is_none());
    }
}
