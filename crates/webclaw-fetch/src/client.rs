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
    pub timeout: Duration,
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
    body: bytes::Bytes,
}

/// Maximum fetched body size. A single 50 MB HTML document is already
/// several orders of magnitude past any realistic page; larger responses
/// are either malicious (log bomb, zip-bomb decompressed) or streaming
/// bugs. Caps the blast radius of the HTML → markdown conversion
/// downstream (which could otherwise allocate multiple full-size Strings
/// per page in collapse_whitespace + strip_markdown).
const MAX_BODY_BYTES: u64 = 50 * 1024 * 1024;

impl Response {
    /// Buffer a wreq response into an owned Response. Rejects bodies that
    /// advertise a Content-Length beyond [`MAX_BODY_BYTES`] before we pay
    /// the allocation, and truncates after the fact as a belt-and-braces
    /// check against a lying server.
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
        let body = resp
            .bytes()
            .await
            .map_err(|e| FetchError::BodyDecode(e.to_string()))?;
        if body.len() as u64 > MAX_BODY_BYTES {
            return Err(FetchError::BodyDecode(format!(
                "response body {} bytes exceeds cap {MAX_BODY_BYTES}",
                body.len()
            )));
        }
        Ok(Self {
            status,
            url,
            headers,
            body,
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
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    fn text(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.body)
    }

    fn into_text(self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
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
}

impl FetchClient {
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
                    crate::tls::build_client(v, config.timeout, &config.headers, Some(proxy))
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

    /// Fetch a URL and return the raw HTML + response metadata.
    ///
    /// Automatically retries on transient failures (network errors, 5xx, 429)
    /// with exponential backoff: 0s, 1s (2 attempts total).
    #[instrument(skip(self), fields(url = %url))]
    pub async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError> {
        let delays = [Duration::ZERO, Duration::from_secs(1)];
        let mut last_err = None;

        for (attempt, delay) in delays.iter().enumerate() {
            if attempt > 0 {
                tokio::time::sleep(*delay).await;
            }

            match self.fetch_once(url).await {
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

        for (attempt, delay) in delays.iter().enumerate() {
            if attempt > 0 {
                tokio::time::sleep(*delay).await;
            }
            match self.fetch_once_with_headers(url, extra).await {
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
        // Reddit fallback: use their JSON API to get post + full comment tree.
        if crate::reddit::is_reddit_url(url) {
            let json_url = crate::reddit::json_url(url);
            debug!("reddit detected, fetching {json_url}");

            let client = self.pick_client(url);
            let resp = client.get(&json_url).send().await?;
            let response = Response::from_wreq(resp).await?;
            if response.is_success() {
                let bytes = response.body();
                match crate::reddit::parse_reddit_json(bytes, url) {
                    Ok(result) => return Ok(result),
                    Err(e) => warn!("reddit json fallback failed: {e}, falling back to HTML"),
                }
            }
        }

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
            let _ = client.get(&homepage).send().await;
            let resp = client.get(url).send().await?;
            response = Response::from_wreq(resp).await?;
            debug!("retried after cookie warmup: status={}", response.status());
        }

        let status = response.status();
        let final_url = response.url().to_string();

        let headers = response.headers().clone();

        let is_pdf = is_pdf_content_type(&headers);

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
            crate::document::is_document_content_type(&headers, &final_url)
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

/// Collect the browser variants to use based on the browser profile.
fn collect_variants(profile: &BrowserProfile) -> Vec<BrowserVariant> {
    match profile {
        BrowserProfile::Random => browser::all_variants(),
        BrowserProfile::Chrome => vec![browser::latest_chrome()],
        BrowserProfile::Firefox => vec![browser::latest_firefox()],
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
    let len = response.body().len();
    if len > 15_000 || len == 0 {
        return false;
    }

    let text = response.text();
    let lower = text.to_lowercase();

    if lower.contains("<title>challenge page</title>") {
        return true;
    }

    if lower.contains("bazadebezolkohpepadr") && len < 5_000 {
        return true;
    }

    false
}

/// Extract the homepage URL (scheme + host) from a full URL.
fn extract_homepage(url: &str) -> Option<String> {
    url::Url::parse(url)
        .ok()
        .map(|u| format!("{}://{}/", u.scheme(), u.host_str().unwrap_or("")))
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
