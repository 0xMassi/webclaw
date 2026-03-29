/// HTTP client with browser TLS fingerprint impersonation.
/// Wraps primp to provide a simple fetch interface with optional
/// content extraction via webclaw-core. Supports single and batch operations.
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

use crate::browser::{self, BrowserProfile, ImpersonateProfile};
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
            timeout: Duration::from_secs(30),
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
    pub headers: HashMap<String, String>,
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

/// Internal representation of the client pool strategy.
enum ClientPool {
    /// Pre-built clients with a fixed proxy (or no proxy).
    /// Fingerprint rotation still works via the pool when `random` is true.
    Static {
        clients: Vec<primp::Client>,
        random: bool,
    },
    /// Pre-built pool of clients, each with a different proxy + fingerprint.
    /// Requests pick a client deterministically by host for HTTP/2 connection reuse.
    Rotating { clients: Vec<primp::Client> },
}

/// HTTP client that impersonates browser TLS fingerprints via primp.
///
/// Operates in two modes:
/// - **Static pool**: pre-built primp clients, optionally with fingerprint rotation.
///   Used when no `proxy_pool` is configured. Fast (no per-request construction).
/// - **Rotating pool**: pre-built primp clients, one per proxy in the pool.
///   Same-host URLs are routed to the same client for HTTP/2 multiplexing.
pub struct FetchClient {
    pool: ClientPool,
    pdf_mode: PdfMode,
}

impl FetchClient {
    /// Build a new client from config.
    ///
    /// When `config.proxy_pool` is non-empty, pre-builds one primp client per proxy,
    /// each with a randomly assigned fingerprint. Same-host URLs get routed to the
    /// same client for HTTP/2 connection reuse.
    ///
    /// When `proxy_pool` is empty, pre-builds primp clients at construction time
    /// (one per fingerprint for `Random` profiles, one for fixed profiles).
    pub fn new(config: FetchConfig) -> Result<Self, FetchError> {
        let profiles = collect_profiles(&config.browser);
        let pdf_mode = config.pdf_mode.clone();

        let pool = if config.proxy_pool.is_empty() {
            let clients = profiles
                .into_iter()
                .map(|p| build_primp_client(&config, &p, config.proxy.as_deref()))
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
                    let p = profiles.choose(&mut rng).unwrap().clone();
                    build_primp_client(&config, &p, Some(proxy))
                })
                .collect::<Result<Vec<_>, _>>()?;

            debug!(
                clients = clients.len(),
                profiles = profiles.len(),
                "fetch client ready (pre-built rotating pool)"
            );

            ClientPool::Rotating { clients }
        };

        Ok(Self { pool, pdf_mode })
    }

    /// Fetch a URL and return the raw HTML + response metadata.
    ///
    /// Automatically retries on transient failures (network errors, 5xx, 429)
    /// with exponential backoff: 0s, 1s, 3s (3 attempts total).
    #[instrument(skip(self), fields(url = %url))]
    pub async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError> {
        let delays = [
            Duration::ZERO,
            Duration::from_secs(1),
            Duration::from_secs(3),
        ];
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

    /// Single fetch attempt with automatic plain-client fallback.
    ///
    /// If the TLS-impersonated client fails with a connection error or gets a 403,
    /// retries with a plain client (no impersonation). Some sites (e.g. ycombinator.com)
    /// reject forged TLS fingerprints but accept default rustls connections.
    async fn fetch_once(&self, url: &str) -> Result<FetchResult, FetchError> {
        let start = Instant::now();

        let client = match &self.pool {
            ClientPool::Static { clients, random } => {
                if *random {
                    let host = extract_host(url);
                    pick_for_host(clients, &host)
                } else {
                    &clients[0]
                }
            }
            ClientPool::Rotating { clients } => pick_random(clients),
        };

        // Try impersonated client first
        let needs_plain_fallback = match client.get(url).send().await {
            Ok(response) => {
                let status = response.status().as_u16();
                if status == 403 {
                    debug!(url, "impersonated client got 403, trying plain fallback");
                    true
                } else {
                    return Self::response_to_result(response, start).await;
                }
            }
            Err(_e) => {
                debug!(
                    url,
                    "impersonated client connection failed, trying plain fallback"
                );
                true
            }
        };

        // Plain client fallback (no TLS impersonation)
        // danger_accept_invalid_certs: the primp-patched rustls rejects some valid
        // certificates with UnknownCertificateExtension / UnknownIssuer.  For the
        // plain-text fallback (which only runs when the impersonated client already
        // failed) accepting certs unconditionally lets the request succeed while the
        // upstream rustls fork is fixed.
        if needs_plain_fallback {
            let plain = primp::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36")
                .cookie_store(true)
                .timeout(Duration::from_secs(30))
                .danger_accept_invalid_certs(true)
                .build()
                .map_err(|e| FetchError::Build(format!("plain client: {e}")))?;

            let response = plain.get(url).send().await?;
            return Self::response_to_result(response, start).await;
        }

        unreachable!()
    }

    /// Convert a primp Response into a FetchResult.
    async fn response_to_result(
        response: primp::Response,
        start: Instant,
    ) -> Result<FetchResult, FetchError> {
        let status = response.status().as_u16();
        let final_url = response.url().to_string();

        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let html = response
            .text()
            .await
            .map_err(|e| FetchError::BodyDecode(e.to_string()))?;

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

    /// Fetch a URL then extract structured content.
    ///
    /// Automatically detects PDF responses via Content-Type header and routes
    /// to webclaw-pdf for text extraction. HTML responses go through webclaw-core.
    #[instrument(skip(self), fields(url = %url))]
    pub async fn fetch_and_extract(
        &self,
        url: &str,
    ) -> Result<webclaw_core::ExtractionResult, FetchError> {
        self.fetch_and_extract_with_options(url, &webclaw_core::ExtractionOptions::default())
            .await
    }

    /// Fetch a URL then extract structured content with custom extraction options.
    ///
    /// Same as [`fetch_and_extract`] but accepts `ExtractionOptions` for CSS selector
    /// filtering, main-content-only mode, etc. Options only apply to HTML responses;
    /// PDF extraction ignores them (no DOM to filter).
    #[instrument(skip(self, options), fields(url = %url))]
    pub async fn fetch_and_extract_with_options(
        &self,
        url: &str,
        options: &webclaw_core::ExtractionOptions,
    ) -> Result<webclaw_core::ExtractionResult, FetchError> {
        // Reddit fallback: use their JSON API to get post + full comment tree.
        // Uses a plain reqwest client — Reddit's JSON endpoint blocks TLS-fingerprinted clients
        // but accepts standard requests with a browser User-Agent.
        if crate::reddit::is_reddit_url(url) {
            let json_url = crate::reddit::json_url(url);
            debug!("reddit detected, fetching {json_url}");

            let plain = primp::Client::builder()
                .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36")
                .timeout(std::time::Duration::from_secs(15))
                .danger_accept_invalid_certs(true)
                .build()
                .map_err(|e| FetchError::Build(format!("reddit client: {e}")))?;
            let response = plain.get(&json_url).send().await?;
            if response.status().is_success() {
                let bytes = response
                    .bytes()
                    .await
                    .map_err(|e| FetchError::BodyDecode(e.to_string()))?;
                match crate::reddit::parse_reddit_json(&bytes, url) {
                    Ok(result) => return Ok(result),
                    Err(e) => warn!("reddit json fallback failed: {e}, falling back to HTML"),
                }
            }
        }

        let start = Instant::now();
        let client = self.pick_client(url);

        // Try impersonated client, fall back to plain on connection error or 403
        let response = match client.get(url).send().await {
            Ok(resp) if resp.status().as_u16() == 403 => {
                debug!(url, "impersonated client got 403, trying plain fallback");
                let plain = primp::Client::builder()
                    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36")
                    .cookie_store(true)
                    .timeout(Duration::from_secs(30))
                    .danger_accept_invalid_certs(true)
                    .build()
                    .map_err(|e| FetchError::Build(format!("plain fallback: {e}")))?;
                plain.get(url).send().await?
            }
            Ok(resp) => resp,
            Err(_e) => {
                debug!(url, "impersonated client failed, trying plain fallback");
                let plain = primp::Client::builder()
                    .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/142.0.0.0 Safari/537.36")
                    .cookie_store(true)
                    .timeout(Duration::from_secs(30))
                    .danger_accept_invalid_certs(true)
                    .build()
                    .map_err(|e| FetchError::Build(format!("plain fallback: {e}")))?;
                plain.get(url).send().await?
            }
        };

        let status = response.status().as_u16();
        let final_url = response.url().to_string();

        let headers: HashMap<String, String> = response
            .headers()
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_str().unwrap_or("").to_string()))
            .collect();

        let is_pdf = is_pdf_content_type(&headers);

        if is_pdf {
            debug!(status, "detected PDF response, using pdf extraction");

            let bytes = response
                .bytes()
                .await
                .map_err(|e| FetchError::BodyDecode(e.to_string()))?;

            let elapsed = start.elapsed();
            debug!(
                status,
                bytes = bytes.len(),
                elapsed_ms = %elapsed.as_millis(),
                "PDF fetch complete"
            );

            let pdf_result = webclaw_pdf::extract_pdf(&bytes, self.pdf_mode.clone())?;
            Ok(pdf_to_extraction_result(&pdf_result, &final_url))
        } else if let Some(doc_type) =
            crate::document::is_document_content_type(&headers, &final_url)
        {
            debug!(status, doc_type = ?doc_type, "detected document response, extracting");

            let bytes = response
                .bytes()
                .await
                .map_err(|e| FetchError::BodyDecode(e.to_string()))?;

            let elapsed = start.elapsed();
            debug!(
                status,
                bytes = bytes.len(),
                elapsed_ms = %elapsed.as_millis(),
                "document fetch complete"
            );

            let mut result = crate::document::extract_document(&bytes, doc_type)?;
            result.metadata.url = Some(final_url);
            Ok(result)
        } else {
            let html = response
                .text()
                .await
                .map_err(|e| FetchError::BodyDecode(e.to_string()))?;

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

            // YouTube transcript: caption URLs are IP-signed and expire immediately,
            // so the timedtext endpoint returns empty responses. The innertube
            // get_transcript API requires cookies/consent. Transcript extraction
            // will be enabled via the cloud API (JS rendering + cookie jar).
            // The extraction functions exist in webclaw_core::youtube but are not
            // wired up here until we have a reliable fetch path.

            Ok(extraction)
        }
    }

    /// Fetch multiple URLs concurrently with bounded parallelism.
    ///
    /// Spawns one task per URL, bounded by a semaphore. Results are returned
    /// in the same order as the input URLs, regardless of completion order.
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
                let _permit = permit.acquire().await.expect("semaphore closed");
                let result = client.fetch(&url).await;
                (idx, BatchResult { url, result })
            }));
        }

        collect_ordered(handles, urls.len()).await
    }

    /// Fetch and extract multiple URLs concurrently with bounded parallelism.
    ///
    /// Same semantics as [`fetch_batch`] but runs extraction on each response.
    /// Results preserve input URL order.
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
    ///
    /// Same as [`fetch_and_extract_batch`] but applies the given options
    /// (include/exclude selectors, only-main-content, etc.) to each extraction.
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
                let _permit = permit.acquire().await.expect("semaphore closed");
                let result = client.fetch_and_extract_with_options(&url, &opts).await;
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
    fn pick_client(&self, url: &str) -> &primp::Client {
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

/// Collect the impersonation profiles to use based on the browser profile.
fn collect_profiles(profile: &BrowserProfile) -> Vec<ImpersonateProfile> {
    match profile {
        BrowserProfile::Random => {
            let mut profiles = Vec::new();
            profiles.extend(browser::chrome_profiles());
            profiles.extend(browser::firefox_profiles());
            profiles.extend(browser::extra_profiles());
            profiles
        }
        BrowserProfile::Chrome => vec![browser::latest_chrome()],
        BrowserProfile::Firefox => vec![browser::latest_firefox()],
    }
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
fn pick_for_host<'a>(clients: &'a [primp::Client], host: &str) -> &'a primp::Client {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    host.hash(&mut hasher);
    let idx = (hasher.finish() as usize) % clients.len();
    &clients[idx]
}

/// Pick a random client from the pool for per-request rotation.
fn pick_random(clients: &[primp::Client]) -> &primp::Client {
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

fn is_pdf_content_type(headers: &HashMap<String, String>) -> bool {
    headers
        .get("content-type")
        .map(|ct| {
            let mime = ct.split(';').next().unwrap_or("").trim();
            mime.eq_ignore_ascii_case("application/pdf")
        })
        .unwrap_or(false)
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

/// Build a single primp Client from config + impersonation profile + optional proxy.
fn build_primp_client(
    config: &FetchConfig,
    profile: &ImpersonateProfile,
    proxy: Option<&str>,
) -> Result<primp::Client, FetchError> {
    let redirect_policy = if config.follow_redirects {
        primp::redirect::Policy::limited(config.max_redirects as usize)
    } else {
        primp::redirect::Policy::none()
    };

    let mut headers = primp::header::HeaderMap::new();
    for (k, v) in &config.headers {
        if let (Ok(name), Ok(val)) = (
            primp::header::HeaderName::from_bytes(k.as_bytes()),
            primp::header::HeaderValue::from_str(v),
        ) {
            headers.insert(name, val);
        }
    }

    let mut builder = primp::Client::builder()
        .impersonate(profile.browser)
        .impersonate_os(profile.os)
        .cookie_store(true)
        .timeout(config.timeout)
        .redirect(redirect_policy)
        .default_headers(headers);

    if let Some(proxy_url) = proxy {
        builder = builder
            .proxy(primp::Proxy::all(proxy_url).map_err(|e| FetchError::Build(e.to_string()))?);
    }

    builder
        .build()
        .map_err(|e| FetchError::Build(e.to_string()))
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
                headers: HashMap::new(),
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
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/pdf".to_string());
        assert!(is_pdf_content_type(&headers));

        headers.insert(
            "content-type".to_string(),
            "application/pdf; charset=utf-8".to_string(),
        );
        assert!(is_pdf_content_type(&headers));

        headers.insert("content-type".to_string(), "Application/PDF".to_string());
        assert!(is_pdf_content_type(&headers));

        headers.insert("content-type".to_string(), "text/html".to_string());
        assert!(!is_pdf_content_type(&headers));

        let empty: HashMap<String, String> = HashMap::new();
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
