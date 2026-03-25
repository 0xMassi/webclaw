/// Recursive same-origin web crawler built on top of [`FetchClient`].
///
/// Starts from a seed URL, extracts content, discovers links, and follows
/// them breadth-first up to a configurable depth/page limit. Uses a semaphore
/// for bounded concurrency and per-request delays for politeness.
///
/// When `use_sitemap` is enabled, the crawler first discovers URLs from the
/// site's sitemaps and seeds the BFS frontier before crawling.
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};
use url::Url;

use crate::client::{FetchClient, FetchConfig};
use crate::error::FetchError;
use crate::sitemap;

/// Controls crawl scope, depth, concurrency, and politeness.
#[derive(Debug, Clone)]
pub struct CrawlConfig {
    /// Fetch configuration (browser profile, proxy, timeout, etc.)
    pub fetch: FetchConfig,
    /// How deep to follow links. 1 = only immediate links from seed page.
    pub max_depth: usize,
    /// Hard cap on total pages fetched (including the seed).
    pub max_pages: usize,
    /// Max concurrent in-flight requests.
    pub concurrency: usize,
    /// Minimum delay before each request (politeness).
    pub delay: Duration,
    /// Only follow URLs whose path starts with this prefix (e.g. "/docs/").
    pub path_prefix: Option<String>,
    /// Seed BFS frontier from sitemap discovery before crawling.
    pub use_sitemap: bool,
    /// Glob patterns for paths to include. If non-empty, only matching URLs are crawled.
    /// E.g. `["/api/*", "/guides/*"]` — matched against the URL path.
    pub include_patterns: Vec<String>,
    /// Glob patterns for paths to exclude. Checked after include_patterns.
    /// E.g. `["/changelog/*", "/blog/*"]` — matching URLs are skipped.
    pub exclude_patterns: Vec<String>,
    /// Optional channel sender for streaming per-page results as they complete.
    /// When set, each `PageResult` is sent on this channel immediately after extraction.
    pub progress_tx: Option<tokio::sync::broadcast::Sender<PageResult>>,
    /// When set to `true`, the crawler breaks out of the main loop early.
    /// Callers (e.g. a Ctrl+C handler) can flip this to request graceful cancellation.
    pub cancel_flag: Option<Arc<AtomicBool>>,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            fetch: FetchConfig::default(),
            max_depth: 1,
            max_pages: 50,
            concurrency: 5,
            delay: Duration::from_millis(100),
            path_prefix: None,
            use_sitemap: false,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            progress_tx: None,
            cancel_flag: None,
        }
    }
}

/// Aggregated results from a crawl run.
#[derive(Debug, Serialize, Deserialize)]
pub struct CrawlResult {
    pub pages: Vec<PageResult>,
    pub total: usize,
    pub ok: usize,
    pub errors: usize,
    pub elapsed_secs: f64,
    /// URLs visited during this crawl (for resume state).
    #[serde(skip)]
    pub visited: HashSet<String>,
    /// Remaining frontier when crawl was cancelled (for resume state).
    #[serde(skip)]
    pub remaining_frontier: Vec<(String, usize)>,
}

/// Outcome of extracting a single page during the crawl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageResult {
    pub url: String,
    pub depth: usize,
    pub extraction: Option<webclaw_core::ExtractionResult>,
    pub error: Option<String>,
    #[serde(skip)]
    pub elapsed: Duration,
}

/// Serializable crawl state for resume after Ctrl+C cancellation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrawlState {
    pub seed_url: String,
    pub visited: Vec<String>,
    pub frontier: Vec<(String, usize)>,
    pub completed_pages: usize,
    pub max_pages: usize,
    pub max_depth: usize,
}

/// Recursive crawler that wraps a shared [`FetchClient`].
pub struct Crawler {
    client: Arc<FetchClient>,
    config: CrawlConfig,
    seed_origin: String,
}

impl Crawler {
    /// Build a new crawler from a seed URL and config.
    /// Constructs the underlying `FetchClient` from `config.fetch`.
    pub fn new(seed_url: &str, config: CrawlConfig) -> Result<Self, FetchError> {
        let seed = Url::parse(seed_url).map_err(|_| FetchError::InvalidUrl(seed_url.into()))?;
        let seed_origin = origin_key(&seed);

        let client = FetchClient::new(config.fetch.clone())?;

        Ok(Self {
            client: Arc::new(client),
            config,
            seed_origin,
        })
    }

    /// Save current crawl state to a JSON file for later resume.
    pub fn save_state(
        path: &Path,
        seed_url: &str,
        visited: &HashSet<String>,
        frontier: &[(String, usize)],
        completed_pages: usize,
        max_pages: usize,
        max_depth: usize,
    ) -> Result<(), String> {
        let state = CrawlState {
            seed_url: seed_url.to_string(),
            visited: visited.iter().cloned().collect(),
            frontier: frontier.to_vec(),
            completed_pages,
            max_pages,
            max_depth,
        };
        let json =
            serde_json::to_string_pretty(&state).map_err(|e| format!("serialize state: {e}"))?;
        std::fs::write(path, json).map_err(|e| format!("write state to {}: {e}", path.display()))
    }

    /// Load crawl state from a JSON file. Returns `None` if file doesn't exist.
    pub fn load_state(path: &Path) -> Option<CrawlState> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Returns true if the cancel flag has been set.
    fn is_cancelled(&self) -> bool {
        self.config
            .cancel_flag
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Relaxed))
    }

    /// Crawl starting from `start_url`, returning results for every page visited.
    ///
    /// Uses breadth-first traversal: all pages at depth N are fetched (concurrently,
    /// bounded by `config.concurrency`) before moving to depth N+1.
    ///
    /// When `config.use_sitemap` is true, sitemap URLs are discovered first and
    /// added to the initial frontier at depth 0 alongside the seed URL.
    ///
    /// If `resume_state` is provided, the crawl resumes from the saved state
    /// (pre-populated visited set and frontier) instead of starting fresh.
    pub async fn crawl(&self, start_url: &str, resume_state: Option<CrawlState>) -> CrawlResult {
        let start = Instant::now();

        let seed = match Url::parse(start_url) {
            Ok(u) => u,
            Err(_) => {
                return CrawlResult {
                    pages: vec![PageResult {
                        url: start_url.to_string(),
                        depth: 0,
                        extraction: None,
                        error: Some(format!("invalid URL: {start_url}")),
                        elapsed: Duration::ZERO,
                    }],
                    total: 1,
                    ok: 0,
                    errors: 1,
                    elapsed_secs: 0.0,
                    visited: HashSet::new(),
                    remaining_frontier: Vec::new(),
                };
            }
        };

        let semaphore = Arc::new(Semaphore::new(self.config.concurrency));
        let mut visited: HashSet<String>;
        let mut pages: Vec<PageResult> = Vec::new();
        let mut frontier: Vec<(String, usize)>;

        // Resume from saved state or start fresh
        if let Some(state) = resume_state {
            visited = state.visited.into_iter().collect();
            frontier = state.frontier;
            info!(
                visited = visited.len(),
                frontier = frontier.len(),
                "resuming crawl from saved state"
            );
        } else {
            visited = HashSet::new();
            frontier = vec![(normalize(&seed), 0)];

            // Seed frontier from sitemap if enabled
            if self.config.use_sitemap {
                let base_url = format!("{}://{}", seed.scheme(), seed.host_str().unwrap_or(""));
                match sitemap::discover(&self.client, &base_url).await {
                    Ok(entries) => {
                        let before = frontier.len();
                        for entry in entries {
                            if self.qualify_link(&entry.url, &visited).is_some() {
                                let parsed = match Url::parse(&entry.url) {
                                    Ok(u) => u,
                                    Err(_) => continue,
                                };
                                let norm = normalize(&parsed);
                                frontier.push((norm, 0));
                            }
                        }
                        let added = frontier.len() - before;
                        info!(
                            sitemap_urls = added,
                            "seeded frontier from sitemap discovery"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "sitemap discovery failed, continuing with seed URL only");
                    }
                }
            }
        }

        while !frontier.is_empty() && pages.len() < self.config.max_pages {
            // Check cancel flag before processing each batch
            if self.is_cancelled() {
                info!("crawl cancelled by user");
                break;
            }

            // Dedup this level's frontier against the visited set and page cap
            let batch: Vec<(String, usize)> = frontier
                .drain(..)
                .filter(|(url, _)| visited.insert(url.clone()))
                .take(self.config.max_pages.saturating_sub(pages.len()))
                .collect();

            if batch.is_empty() {
                break;
            }

            // Spawn one task per URL, bounded by semaphore
            let mut handles = Vec::with_capacity(batch.len());

            for (url, depth) in &batch {
                let permit = Arc::clone(&semaphore);
                let client = Arc::clone(&self.client);
                let url = url.clone();
                let depth = *depth;
                let delay = self.config.delay;

                handles.push(tokio::spawn(async move {
                    // Acquire permit — blocks if concurrency limit reached
                    let _permit = permit.acquire().await.expect("semaphore closed");
                    tokio::time::sleep(delay).await;

                    let page_start = Instant::now();
                    let result = client.fetch_and_extract(&url).await;
                    let elapsed = page_start.elapsed();

                    match result {
                        Ok(extraction) => {
                            debug!(
                                url = %url, depth,
                                elapsed_ms = %elapsed.as_millis(),
                                "page extracted"
                            );
                            PageResult {
                                url,
                                depth,
                                extraction: Some(extraction),
                                error: None,
                                elapsed,
                            }
                        }
                        Err(e) => {
                            warn!(url = %url, depth, error = %e, "page failed");
                            PageResult {
                                url,
                                depth,
                                extraction: None,
                                error: Some(e.to_string()),
                                elapsed,
                            }
                        }
                    }
                }));
            }

            // Collect results and harvest links for the next depth level
            let mut next_frontier: Vec<(String, usize)> = Vec::new();

            for handle in handles {
                let page = match handle.await {
                    Ok(page) => page,
                    Err(e) => {
                        warn!(error = %e, "crawl task panicked");
                        continue;
                    }
                };
                let depth = page.depth;

                if depth < self.config.max_depth
                    && let Some(ref extraction) = page.extraction
                {
                    for link in &extraction.content.links {
                        if let Some(candidate) = self.qualify_link(&link.href, &visited) {
                            next_frontier.push((candidate, depth + 1));
                        }
                    }
                }

                // Stream progress if a channel is configured
                if let Some(tx) = &self.config.progress_tx {
                    let _ = tx.send(page.clone());
                }

                pages.push(page);

                if pages.len() >= self.config.max_pages {
                    break;
                }

                // Check cancel flag between page results
                if self.is_cancelled() {
                    info!("crawl cancelled by user (mid-batch)");
                    break;
                }
            }

            frontier = next_frontier;
        }

        let total_elapsed = start.elapsed();
        let ok_count = pages.iter().filter(|p| p.extraction.is_some()).count();
        let err_count = pages.len() - ok_count;
        info!(
            total = pages.len(),
            ok = ok_count,
            errors = err_count,
            elapsed_ms = %total_elapsed.as_millis(),
            "crawl complete"
        );

        CrawlResult {
            total: pages.len(),
            ok: ok_count,
            errors: err_count,
            elapsed_secs: total_elapsed.as_secs_f64(),
            remaining_frontier: frontier,
            visited,
            pages,
        }
    }

    /// Check if a discovered link should be added to the frontier.
    /// Returns `Some(normalized_url)` if it passes all filters, `None` otherwise.
    fn qualify_link(&self, href: &str, visited: &HashSet<String>) -> Option<String> {
        let parsed = Url::parse(href).ok()?;

        // Only http(s) schemes
        match parsed.scheme() {
            "http" | "https" => {}
            _ => return None,
        }

        // Same-origin check (scheme + host + port)
        if origin_key(&parsed) != self.seed_origin {
            return None;
        }

        // Path prefix filter
        if let Some(ref prefix) = self.config.path_prefix
            && !parsed.path().starts_with(prefix.as_str())
        {
            return None;
        }

        // Include patterns: if any are set, path must match at least one
        let path = parsed.path();
        if !self.config.include_patterns.is_empty()
            && !self
                .config
                .include_patterns
                .iter()
                .any(|pat| glob_match(pat, path))
        {
            return None;
        }

        // Exclude patterns: if path matches any, skip
        if self
            .config
            .exclude_patterns
            .iter()
            .any(|pat| glob_match(pat, path))
        {
            return None;
        }

        // Skip common non-page file extensions
        const SKIP_EXTENSIONS: &[&str] = &[
            ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".css", ".js",
            ".zip", ".tar", ".gz", ".xml", ".rss", ".mp3", ".mp4", ".avi", ".mov", ".woff",
            ".woff2", ".ttf", ".eot",
        ];
        if SKIP_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
            return None;
        }

        let normalized = normalize(&parsed);

        if visited.contains(&normalized) {
            return None;
        }

        Some(normalized)
    }
}

/// Canonical origin string for comparing same-origin: "scheme://host[:port]".
fn origin_key(url: &Url) -> String {
    let port_suffix = match url.port() {
        Some(p) => format!(":{p}"),
        None => String::new(),
    };
    let host = url.host_str().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    format!("{}://{}{}", url.scheme(), host, port_suffix)
}

/// Normalize a URL for dedup: strip fragment, remove trailing slash (except root "/"),
/// lowercase scheme + host. Preserves query params and path case.
fn normalize(url: &Url) -> String {
    let scheme = url.scheme();
    let host = url.host_str().unwrap_or("").to_ascii_lowercase();
    let port_suffix = match url.port() {
        Some(p) => format!(":{p}"),
        None => String::new(),
    };

    let mut path = url.path().to_string();
    if path.len() > 1 && path.ends_with('/') {
        path.pop();
    }

    let query = match url.query() {
        Some(q) => format!("?{q}"),
        None => String::new(),
    };

    // Fragment intentionally omitted
    format!("{scheme}://{host}{port_suffix}{path}{query}")
}

/// Simple glob matching for URL paths. Supports:
/// - `*` matches any characters within a single path segment (no `/`)
/// - `**` matches any characters including `/` (any number of segments)
/// - Literal characters match exactly
///
/// Examples:
/// - `/api/*` matches `/api/users` but not `/api/users/123`
/// - `/api/**` matches `/api/users`, `/api/users/123`, `/api/a/b/c`
/// - `/docs/*/intro` matches `/docs/v2/intro`
fn glob_match(pattern: &str, path: &str) -> bool {
    glob_match_inner(pattern.as_bytes(), path.as_bytes())
}

fn glob_match_inner(pat: &[u8], text: &[u8]) -> bool {
    let mut pi = 0;
    let mut ti = 0;
    let mut star_pi = usize::MAX;
    let mut star_ti = 0;

    while ti < text.len() {
        if pi < pat.len() && pat[pi] == b'*' && pi + 1 < pat.len() && pat[pi + 1] == b'*' {
            // `**` — match everything including slashes
            // Skip all consecutive `*`
            while pi < pat.len() && pat[pi] == b'*' {
                pi += 1;
            }
            // Skip trailing `/` after `**`
            if pi < pat.len() && pat[pi] == b'/' {
                pi += 1;
            }
            if pi >= pat.len() {
                return true; // `**` at end matches everything
            }
            // Try matching the rest of pattern against every suffix of text
            for start in ti..=text.len() {
                if glob_match_inner(&pat[pi..], &text[start..]) {
                    return true;
                }
            }
            return false;
        } else if pi < pat.len() && pat[pi] == b'*' {
            // `*` — match any chars except `/`
            star_pi = pi;
            star_ti = ti;
            pi += 1;
        } else if pi < pat.len() && (pat[pi] == text[ti] || pat[pi] == b'?') {
            pi += 1;
            ti += 1;
        } else if star_pi != usize::MAX {
            // Backtrack: `*` absorbs one more char (but not `/`)
            if text[star_ti] == b'/' {
                return false;
            }
            star_ti += 1;
            ti = star_ti;
            pi = star_pi + 1;
        } else {
            return false;
        }
    }

    // Consume trailing `*` or `**` in pattern
    while pi < pat.len() && pat[pi] == b'*' {
        pi += 1;
    }

    pi >= pat.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_strips_fragment() {
        let url = Url::parse("https://example.com/page#section").unwrap();
        assert_eq!(normalize(&url), "https://example.com/page");
    }

    #[test]
    fn normalize_strips_trailing_slash() {
        let url = Url::parse("https://example.com/docs/").unwrap();
        assert_eq!(normalize(&url), "https://example.com/docs");
    }

    #[test]
    fn normalize_keeps_root_slash() {
        let url = Url::parse("https://example.com/").unwrap();
        assert_eq!(normalize(&url), "https://example.com/");
    }

    #[test]
    fn normalize_preserves_query() {
        let url = Url::parse("https://example.com/search?q=rust&page=2").unwrap();
        assert_eq!(normalize(&url), "https://example.com/search?q=rust&page=2");
    }

    #[test]
    fn normalize_lowercases_host() {
        let url = Url::parse("https://Example.COM/Path").unwrap();
        assert_eq!(normalize(&url), "https://example.com/Path");
    }

    #[test]
    fn origin_includes_explicit_port() {
        let url = Url::parse("https://example.com:8443/foo").unwrap();
        assert_eq!(origin_key(&url), "https://example.com:8443");
    }

    #[test]
    fn origin_omits_default_port() {
        let url = Url::parse("https://example.com/foo").unwrap();
        assert_eq!(origin_key(&url), "https://example.com");
    }

    #[test]
    fn different_schemes_are_different_origins() {
        let http = Url::parse("http://example.com/").unwrap();
        let https = Url::parse("https://example.com/").unwrap();
        assert_ne!(origin_key(&http), origin_key(&https));
    }

    // -- glob_match tests --

    #[test]
    fn glob_star_matches_single_segment() {
        assert!(glob_match("/api/*", "/api/users"));
        assert!(glob_match("/api/*", "/api/products"));
        assert!(!glob_match("/api/*", "/api/users/123"));
    }

    #[test]
    fn glob_doublestar_matches_multiple_segments() {
        assert!(glob_match("/api/**", "/api/users"));
        assert!(glob_match("/api/**", "/api/users/123"));
        assert!(glob_match("/api/**", "/api/a/b/c/d"));
        assert!(!glob_match("/api/**", "/docs/intro"));
    }

    #[test]
    fn glob_exact_match() {
        assert!(glob_match("/about", "/about"));
        assert!(!glob_match("/about", "/about/team"));
    }

    #[test]
    fn glob_middle_wildcard() {
        assert!(glob_match("/docs/*/intro", "/docs/v2/intro"));
        assert!(!glob_match("/docs/*/intro", "/docs/v2/v3/intro"));
    }

    #[test]
    fn glob_no_pattern_matches_nothing() {
        // Empty pattern only matches empty string
        assert!(glob_match("", ""));
        assert!(!glob_match("", "/foo"));
    }

    #[test]
    fn glob_trailing_star() {
        assert!(glob_match("/blog*", "/blog"));
        assert!(glob_match("/blog*", "/blog-post"));
        assert!(!glob_match("/blog*", "/blog/post")); // * doesn't cross /
    }
}
