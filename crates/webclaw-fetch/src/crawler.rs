/// Recursive web crawler built on top of [`FetchClient`].
///
/// Starts from a seed URL, extracts content, discovers links, and follows
/// them breadth-first up to a configurable depth/page limit. Uses a semaphore
/// for bounded concurrency and per-request delays for politeness.
///
/// Scope control: by default only same-origin links are followed. Enable
/// `allow_subdomains` to include sibling/child subdomains of the seed host,
/// or `allow_external_links` to follow links to any domain.
///
/// When `use_sitemap` is enabled, the crawler first discovers URLs from the
/// site's sitemaps and seeds the BFS frontier before crawling.
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};
use url::Url;

use crate::client::{FetchClient, FetchConfig};
use crate::error::FetchError;
use crate::robots::{RobotsRules, USER_AGENT_TOKEN};
use crate::sitemap;

/// Controls crawl scope, depth, concurrency, and politeness.
#[derive(Debug, Clone)]
pub struct CrawlConfig {
    /// Fetch configuration (browser profile, proxy, timeout, etc.)
    pub fetch: FetchConfig,
    /// How deep to follow links. 1 = only immediate links from seed page.
    pub max_depth: usize,
    /// Cap on total pages fetched (including the seed). `None` = no cap:
    /// crawl until the frontier is exhausted, cancelled, or the caller stops
    /// consuming.
    ///
    /// A page cap is a *policy* choice — quota, budget, patience — and belongs
    /// to whoever is paying for the crawl, not to the engine. Callers that need
    /// one set it; callers that want the whole site pass `None`. Pair `None`
    /// with [`stream_only`](Self::stream_only) so memory stays flat.
    pub max_pages: Option<usize>,
    /// Max concurrent in-flight requests.
    pub concurrency: usize,
    /// Minimum delay before each request (politeness).
    pub delay: Duration,
    /// Only follow URLs whose path starts with this prefix (e.g. "/docs/").
    pub path_prefix: Option<String>,
    /// Seed BFS frontier from sitemap discovery before crawling.
    pub use_sitemap: bool,
    /// Glob patterns for paths to include. If non-empty, only matching URLs are crawled.
    /// E.g. `["/api/*", "/guides/*"]` -- matched against the URL path.
    pub include_patterns: Vec<String>,
    /// Glob patterns for paths to exclude. Checked after include_patterns.
    /// E.g. `["/changelog/*", "/blog/*"]` -- matching URLs are skipped.
    pub exclude_patterns: Vec<String>,
    /// Follow links on subdomains of the seed domain (e.g. blog.example.com
    /// when crawling example.com). Default: false (same-origin only).
    pub allow_subdomains: bool,
    /// Follow links to entirely different domains. Default: false.
    /// When true, the crawler becomes cross-origin. Use with caution.
    pub allow_external_links: bool,
    /// Optional channel sender for streaming per-page results as they complete.
    /// When set, each `PageResult` is sent on this channel immediately after extraction.
    pub progress_tx: Option<tokio::sync::broadcast::Sender<PageResult>>,
    /// When set to `true`, the crawler breaks out of the main loop early.
    /// Callers (e.g. a Ctrl+C handler) can flip this to request graceful cancellation.
    pub cancel_flag: Option<Arc<AtomicBool>>,
    /// Drop each `PageResult` after reporting it instead of collecting it into
    /// [`CrawlResult::pages`].
    ///
    /// This is what makes a large crawl possible at all. Retention is O(pages ×
    /// page size), and an extracted page runs to ~90 KB, so a 5 000-page crawl
    /// holds roughly 440 MB and a 50 000-page crawl roughly 4.4 GB — the crawl
    /// only returns once it is finished, so that is all live at once. The page
    /// ceiling exists to bound that, not because the crawl itself cannot go
    /// further.
    ///
    /// Set this with [`progress_tx`](Self::progress_tx) to stream results to
    /// the consumer as they complete and keep peak memory flat regardless of
    /// page count. `total`/`ok`/`errors` are still counted, so the summary is
    /// unaffected; only `pages` comes back empty.
    ///
    /// Defaults to `false`, so existing callers keep collecting.
    pub stream_only: bool,
    /// Skip URLs that the site's robots.txt disallows for webclaw, and honour
    /// its `Crawl-delay`. Defaults to `true`.
    pub respect_robots: bool,
    /// While crawling, write the crawl state to this file about once a second,
    /// so a crash or kill can resume from it via [`Crawler::load_state`].
    /// Pages still in flight are saved back into the frontier.
    pub checkpoint: Option<std::path::PathBuf>,
}

impl Default for CrawlConfig {
    fn default() -> Self {
        Self {
            fetch: FetchConfig::default(),
            max_depth: 1,
            max_pages: Some(50),
            concurrency: 5,
            delay: Duration::from_millis(100),
            path_prefix: None,
            use_sitemap: false,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            allow_subdomains: false,
            allow_external_links: false,
            progress_tx: None,
            cancel_flag: None,
            stream_only: false,
            respect_robots: true,
            checkpoint: None,
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
    /// URLs left unfetched because robots.txt disallows them.
    #[serde(default)]
    pub skipped_by_robots: usize,
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
    page_client: Arc<FetchClient>,
    follow_redirects: bool,
    config: CrawlConfig,
    seed_origin: String,
    /// Root domain of the seed URL for subdomain matching (e.g. "example.com").
    seed_root_domain: String,
    robots: std::sync::Mutex<HashMap<String, Arc<RobotsRules>>>,
}

/// Frontier ceiling used when [`CrawlConfig::max_pages`] is `None`.
///
/// Not a page limit — see the frontier-trim comment in `crawl`. At roughly
/// 100 bytes per queued URL this is on the order of 50 MB of pending
/// frontier, which is the point at which holding more queued work costs more
/// than it is worth.
const UNCAPPED_FRONTIER_LIMIT: usize = 500_000;

impl Crawler {
    /// Whether the crawl may fetch another page.
    ///
    /// `None` means no cap, so this is always true and the crawl ends when the
    /// frontier empties or the caller cancels.
    fn under_page_cap(&self, completed: usize) -> bool {
        self.config.max_pages.is_none_or(|max| completed < max)
    }

    /// Build a new crawler from a seed URL and config.
    /// Constructs the underlying `FetchClient` from `config.fetch`.
    pub fn new(seed_url: &str, config: CrawlConfig) -> Result<Self, FetchError> {
        let seed = Url::parse(seed_url).map_err(|_| FetchError::InvalidUrl(seed_url.into()))?;
        let seed_origin = origin_key(&seed);
        let seed_root_domain = root_domain(&seed);

        // Reject pathological user-supplied glob patterns before they can
        // exercise the recursive `**` handler in glob_match_inner. The
        // matcher is a straight backtracking implementation; a deeply
        // nested `**/**/**/...` pattern against a long path can degrade
        // to exponential time per link checked, per page crawled.
        for pat in config
            .include_patterns
            .iter()
            .chain(config.exclude_patterns.iter())
        {
            validate_glob(pat)?;
        }

        let client = Arc::new(FetchClient::new(config.fetch.clone())?);
        let page_client = if config.respect_robots && config.fetch.follow_redirects {
            Arc::new(FetchClient::new(FetchConfig {
                follow_redirects: false,
                ..config.fetch.clone()
            })?)
        } else {
            Arc::clone(&client)
        };

        Ok(Self {
            client,
            page_client,
            follow_redirects: config.fetch.follow_redirects,
            config,
            seed_origin,
            seed_root_domain,
            robots: Default::default(),
        })
    }

    /// Use `rules` for `origin` (e.g. `https://example.com`) instead of
    /// fetching its robots.txt.
    pub fn preload_robots(&self, origin: &str, rules: RobotsRules) {
        self.robots
            .lock()
            .unwrap()
            .insert(origin.to_string(), Arc::new(rules));
    }

    async fn robots_for(&self, url: &Url) -> Arc<RobotsRules> {
        let origin = url.origin().ascii_serialization();
        if let Some(rules) = self.robots.lock().unwrap().get(&origin) {
            return Arc::clone(rules);
        }
        let rules = match self.client.fetch_raw(&format!("{origin}/robots.txt")).await {
            Ok((status, body)) if (200..300).contains(&status) => {
                RobotsRules::parse(&String::from_utf8_lossy(&body), USER_AGENT_TOKEN)
            }
            Ok((status, _)) if (400..500).contains(&status) => RobotsRules::allow_all(),
            Ok((status, _)) => {
                warn!(%origin, status, "robots.txt unavailable, treating the site as disallowed");
                RobotsRules::disallow_all()
            }
            Err(e) => {
                debug!(%origin, error = %e, "robots.txt fetch failed, page fetches will report the error");
                RobotsRules::allow_all()
            }
        };
        let rules = Arc::new(rules);
        self.robots
            .lock()
            .unwrap()
            .insert(origin, Arc::clone(&rules));
        rules
    }

    async fn robots_allow(&self, url: &str) -> bool {
        let Ok(parsed) = Url::parse(url) else {
            return true;
        };
        let path = match parsed.query() {
            Some(q) => format!("{}?{q}", parsed.path()),
            None => parsed.path().to_string(),
        };
        self.robots_for(&parsed).await.is_allowed(&path)
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
        let json = serde_json::to_string(&state).map_err(|e| format!("serialize state: {e}"))?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, json)
            .and_then(|()| std::fs::rename(&tmp, path))
            .map_err(|e| format!("write state to {}: {e}", path.display()))
    }

    /// Load crawl state from a JSON file. Returns `None` if file doesn't exist.
    pub fn load_state(path: &Path) -> Option<CrawlState> {
        let content = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Returns true if the cancel flag has been set.
    ///
    /// Uses `Acquire` load to pair with a `Release` store on the cancel
    /// path. `Relaxed` was technically fine in practice (x86/arm64 give
    /// release semantics for free on single-word stores) but `Acquire`
    /// makes the ordering explicit so the compiler and future readers
    /// don't need to reason about the memory model.
    fn is_cancelled(&self) -> bool {
        self.config
            .cancel_flag
            .as_ref()
            .is_some_and(|f| f.load(Ordering::Acquire))
    }

    /// Crawl starting from `start_url`, returning results for every page visited.
    ///
    /// A worker pool keeps up to `config.concurrency` fetches in flight and
    /// starts the next URL from a FIFO frontier as soon as one finishes, so the
    /// crawl stays roughly breadth-first without waiting for the slowest page
    /// of each depth. Links come from the whole fetched page. With
    /// `respect_robots`, redirects are not followed blindly: each `Location`
    /// goes back through the scope and robots.txt checks like any link.
    ///
    /// When `config.use_sitemap` is true, sitemap URLs are discovered first and
    /// added to the frontier at depth 0 alongside the seed URL.
    ///
    /// If `resume_state` is provided, the crawl resumes from the saved state
    /// (pre-populated visited set and frontier) instead of starting fresh.
    pub async fn crawl(&self, start_url: &str, resume_state: Option<CrawlState>) -> CrawlResult {
        let client = Arc::clone(&self.page_client);
        let follow_redirects = self.follow_redirects;
        self.run(start_url, resume_state, move |url| {
            let client = Arc::clone(&client);
            async move { fetch_page(&client, &url, follow_redirects).await }
        })
        .await
    }

    /// Crawl using one caller-supplied extraction path for every content page,
    /// including the seed. Links in each returned extraction drive the frontier,
    /// and the supplied fetcher follows redirects itself. Scope, sitemap,
    /// robots.txt, cancellation and concurrency rules still apply; sitemap and
    /// robots.txt discovery use the configured fetch client.
    pub async fn crawl_with_fetcher<F, Fut>(
        &self,
        start_url: &str,
        resume_state: Option<CrawlState>,
        fetcher: F,
    ) -> CrawlResult
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Result<webclaw_core::ExtractionResult, String>>
            + Send
            + 'static,
    {
        let fetcher = Arc::new(fetcher);
        self.run(start_url, resume_state, move |url| {
            let fetcher = Arc::clone(&fetcher);
            async move {
                let extraction = fetcher(url).await;
                let links = extraction
                    .as_ref()
                    .map(|e| e.content.links.iter().map(|l| l.href.clone()).collect())
                    .unwrap_or_default();
                Page {
                    extraction,
                    links,
                    redirect: None,
                }
            }
        })
        .await
    }

    async fn run<F, Fut>(
        &self,
        start_url: &str,
        resume_state: Option<CrawlState>,
        fetch: F,
    ) -> CrawlResult
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Page> + Send + 'static,
    {
        let fetch = Arc::new(fetch);
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
                    skipped_by_robots: 0,
                    elapsed_secs: 0.0,
                    visited: HashSet::new(),
                    remaining_frontier: Vec::new(),
                };
            }
        };

        let seed_rules = if self.config.respect_robots {
            self.robots_for(&seed).await
        } else {
            Arc::new(RobotsRules::allow_all())
        };

        let resumed_pages = resume_state.as_ref().map_or(0, |s| s.completed_pages);
        let mut visited: HashSet<String>;
        let mut frontier = Frontier::default();
        if let Some(state) = resume_state {
            visited = state.visited.into_iter().collect();
            for (url, depth) in state.frontier {
                frontier.push_back(url, depth);
            }
            info!(
                visited = visited.len(),
                frontier = frontier.len(),
                "resuming crawl from saved state"
            );
        } else {
            visited = HashSet::new();
            frontier.push_back(without_fragment(&seed), 0);
            if self.config.use_sitemap && seed_rules.is_allowed("/sitemap.xml") {
                let base_url = format!("{}://{}", seed.scheme(), seed.host_str().unwrap_or(""));
                match sitemap::discover(&self.client, &base_url).await {
                    Ok(entries) => {
                        let before = frontier.len();
                        for entry in entries {
                            if let Some(url) = self.qualify_link(&entry.url, &visited) {
                                frontier.push_back(url, 0);
                            }
                        }
                        info!(
                            sitemap_urls = frontier.len() - before,
                            "seeded frontier from sitemap discovery"
                        );
                    }
                    Err(e) => {
                        warn!(error = %e, "sitemap discovery failed, continuing with seed URL only");
                    }
                }
            }
        }

        let frontier_cap = match self.config.max_pages {
            Some(max) => max.saturating_mul(10).max(100),
            None => UNCAPPED_FRONTIER_LIMIT,
        };
        let concurrency = self.config.concurrency.max(1);
        let mut pages: Vec<PageResult> = Vec::new();
        let mut completed: usize = 0;
        let mut ok_count: usize = 0;
        let mut err_count: usize = 0;
        let mut skipped_by_robots: usize = 0;
        let mut denied: HashSet<String> = HashSet::new();
        let mut next_start: HashMap<String, Instant> = HashMap::new();
        let mut started: usize = resumed_pages;
        let mut in_flight = FuturesUnordered::new();
        let mut in_flight_depths: BTreeMap<usize, usize> = BTreeMap::new();
        let mut in_flight_urls: HashMap<String, usize> = HashMap::new();
        let mut last_checkpoint = Instant::now();
        let mut redirect_hops: HashMap<String, usize> = HashMap::new();

        loop {
            while in_flight.len() < concurrency
                && self.under_page_cap(started)
                && !self.is_cancelled()
            {
                let deepest_allowed = in_flight_depths
                    .keys()
                    .next()
                    .map_or(usize::MAX, |shallowest| shallowest + 1);
                let Some((url, depth)) = frontier.pop(deepest_allowed) else {
                    break;
                };
                if visited.contains(&dedup_key(&url)) || denied.contains(&url) {
                    continue;
                }
                if self.config.respect_robots && !self.robots_allow(&url).await {
                    debug!(url = %url, "skipped: disallowed by robots.txt");
                    skipped_by_robots += 1;
                    denied.insert(url);
                    continue;
                }
                visited.insert(dedup_key(&url));
                started += 1;
                *in_flight_depths.entry(depth).or_default() += 1;
                in_flight_urls.insert(url.clone(), depth);
                let wait = self.start_delay(&url, &mut next_start).await;
                let fetch = Arc::clone(&fetch);
                let cancel_flag = self.config.cancel_flag.clone();
                let task_url = url.clone();
                let task = crate::transfer::spawn(async move {
                    let page_start = Instant::now();
                    tokio::time::sleep(wait).await;
                    let page = if cancel_flag
                        .as_ref()
                        .is_some_and(|flag| flag.load(Ordering::Relaxed))
                    {
                        Page::failed("crawl cancelled before fetch".into())
                    } else {
                        fetch(task_url).await
                    };
                    (page, page_start.elapsed())
                });
                in_flight.push(async move {
                    let (page, elapsed) = task.await.unwrap_or_else(|e| {
                        warn!(url = %url, error = %e, "crawl task panicked");
                        (
                            Page::failed(format!("crawl task failed: {e}")),
                            Duration::ZERO,
                        )
                    });
                    (url, depth, page, elapsed)
                });
            }

            let Some((url, depth, page, elapsed)) = in_flight.next().await else {
                break;
            };
            if let Some(n) = in_flight_depths.get_mut(&depth) {
                *n -= 1;
                if *n == 0 {
                    in_flight_depths.remove(&depth);
                }
            }
            in_flight_urls.remove(&url);

            let mut extraction = page.extraction;
            if let Some(location) = page.redirect {
                let hop = redirect_hops.remove(&url).unwrap_or(0) + 1;
                if hop <= self.config.fetch.max_redirects as usize {
                    started -= 1;
                    if let Some(target) = Url::parse(&url)
                        .ok()
                        .and_then(|base| base.join(&location).ok())
                        .and_then(|target| self.qualify_link(target.as_str(), &visited))
                    {
                        debug!(from = %url, to = %target, "following redirect");
                        redirect_hops.insert(target.clone(), hop);
                        frontier.push_front(target, depth);
                    }
                    continue;
                }
                extraction = Err(format!(
                    "stopped after {} redirects",
                    self.config.fetch.max_redirects
                ));
            }

            if depth < self.config.max_depth && extraction.is_ok() {
                for link in &page.links {
                    if frontier.len() >= frontier_cap {
                        warn!(
                            cap = frontier_cap,
                            "frontier full, dropping newly found links"
                        );
                        break;
                    }
                    if let Some(candidate) = self.qualify_link(link, &visited) {
                        frontier.push_back(candidate, depth + 1);
                    }
                }
            }

            completed += 1;
            let page = match extraction {
                Ok(extraction) => {
                    ok_count += 1;
                    debug!(url = %url, depth, elapsed_ms = %elapsed.as_millis(), "page extracted");
                    PageResult {
                        url,
                        depth,
                        extraction: Some(extraction),
                        error: None,
                        elapsed,
                    }
                }
                Err(e) => {
                    err_count += 1;
                    warn!(url = %url, depth, error = %e, "page failed");
                    PageResult {
                        url,
                        depth,
                        extraction: None,
                        error: Some(e),
                        elapsed,
                    }
                }
            };
            match (&self.config.progress_tx, self.config.stream_only) {
                (Some(tx), true) => {
                    let _ = tx.send(page);
                }
                (Some(tx), false) => {
                    let _ = tx.send(page.clone());
                    pages.push(page);
                }
                (None, true) => {}
                (None, false) => pages.push(page),
            }

            if let Some(path) = &self.config.checkpoint
                && last_checkpoint.elapsed() >= CHECKPOINT_INTERVAL
            {
                self.write_checkpoint(
                    path,
                    start_url,
                    &visited,
                    &frontier,
                    &in_flight_urls,
                    resumed_pages + completed,
                );
                last_checkpoint = Instant::now();
            }
        }

        let total_elapsed = start.elapsed();
        info!(
            total = completed,
            ok = ok_count,
            errors = err_count,
            elapsed_ms = %total_elapsed.as_millis(),
            "crawl complete"
        );

        CrawlResult {
            total: completed,
            ok: ok_count,
            errors: err_count,
            skipped_by_robots,
            elapsed_secs: total_elapsed.as_secs_f64(),
            remaining_frontier: frontier.into_vec(),
            visited,
            pages,
        }
    }

    fn write_checkpoint(
        &self,
        path: &Path,
        seed_url: &str,
        visited: &HashSet<String>,
        frontier: &Frontier,
        in_flight: &HashMap<String, usize>,
        completed_pages: usize,
    ) {
        let in_flight_keys: HashSet<String> = in_flight.keys().map(|u| dedup_key(u)).collect();
        let visited: HashSet<String> = visited.difference(&in_flight_keys).cloned().collect();
        let pending: Vec<(String, usize)> = in_flight
            .iter()
            .map(|(url, depth)| (url.clone(), *depth))
            .chain(frontier.snapshot())
            .collect();
        if let Err(e) = Self::save_state(
            path,
            seed_url,
            &visited,
            &pending,
            completed_pages,
            self.config.max_pages.unwrap_or(0),
            self.config.max_depth,
        ) {
            warn!(error = %e, "checkpoint failed");
        }
    }

    async fn start_delay(&self, url: &str, next_start: &mut HashMap<String, Instant>) -> Duration {
        let base = self.config.delay;
        let Ok(parsed) = Url::parse(url) else {
            return base;
        };
        if !self.config.respect_robots {
            return base;
        }
        let Some(gap) = self
            .robots_for(&parsed)
            .await
            .crawl_delay
            .map(|d| d.min(MAX_CRAWL_DELAY))
        else {
            return base;
        };
        let now = Instant::now();
        let earliest = now + base;
        let origin = parsed.origin().ascii_serialization();
        let start = next_start
            .get(&origin)
            .map_or(earliest, |slot| (*slot).max(earliest));
        next_start.insert(origin, start + gap);
        start - now
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

        // Scope check: same-origin, subdomain, or external
        if !self.config.allow_external_links {
            let link_origin = origin_key(&parsed);
            if link_origin != self.seed_origin {
                // Not same-origin. Check if subdomain crawling is allowed.
                if self.config.allow_subdomains {
                    let link_root = root_domain(&parsed);
                    if link_root != self.seed_root_domain {
                        return None;
                    }
                } else {
                    return None;
                }
            }
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

        if visited.contains(&normalize(&parsed)) {
            return None;
        }

        Some(without_fragment(&parsed))
    }
}

/// Canonical origin string for comparing same-origin: "scheme://host[:port]".
const MAX_CRAWL_DELAY: Duration = Duration::from_secs(30);
// ponytail: a fixed 1s interval rewrites the whole visited set each time; space it out by size if crawls past ~100k pages make it costly.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Frontier {
    levels: BTreeMap<usize, VecDeque<String>>,
    len: usize,
}

impl Frontier {
    fn push_back(&mut self, url: String, depth: usize) {
        self.levels.entry(depth).or_default().push_back(url);
        self.len += 1;
    }

    fn push_front(&mut self, url: String, depth: usize) {
        self.levels.entry(depth).or_default().push_front(url);
        self.len += 1;
    }

    fn pop(&mut self, deepest_allowed: usize) -> Option<(String, usize)> {
        let (&depth, queue) = self.levels.iter_mut().find(|(_, q)| !q.is_empty())?;
        if depth > deepest_allowed {
            return None;
        }
        let url = queue.pop_front()?;
        self.len -= 1;
        Some((url, depth))
    }

    fn len(&self) -> usize {
        self.len
    }

    fn snapshot(&self) -> Vec<(String, usize)> {
        self.levels
            .iter()
            .flat_map(|(depth, urls)| urls.iter().map(move |url| (url.clone(), *depth)))
            .collect()
    }

    fn into_vec(self) -> Vec<(String, usize)> {
        self.levels
            .into_iter()
            .flat_map(|(depth, urls)| urls.into_iter().map(move |url| (url, depth)))
            .collect()
    }
}

struct Page {
    extraction: Result<webclaw_core::ExtractionResult, String>,
    links: Vec<String>,
    redirect: Option<String>,
}

impl Page {
    fn failed(error: String) -> Self {
        Self {
            extraction: Err(error),
            links: Vec::new(),
            redirect: None,
        }
    }
}

async fn fetch_page(client: &FetchClient, url: &str, follow_redirects: bool) -> Page {
    let fetched = match client.fetch_smart(url).await {
        Ok(fetched) => fetched,
        Err(e) => return Page::failed(e.to_string()),
    };
    if follow_redirects
        && (300..400).contains(&fetched.status)
        && let Some(location) = fetched
            .headers
            .get(http::header::LOCATION)
            .and_then(|v| v.to_str().ok())
    {
        return Page {
            extraction: Err(format!("redirected ({})", fetched.status)),
            links: Vec::new(),
            redirect: Some(location.to_string()),
        };
    }
    let links = if fetched.document_bytes.is_none() {
        webclaw_core::page_links(&fetched.html, &fetched.url)
    } else {
        Vec::new()
    };
    Page {
        extraction: client
            .extract(&fetched, &Default::default())
            .map_err(|e| e.to_string()),
        links,
        redirect: None,
    }
}

pub(crate) fn origin_key(url: &Url) -> String {
    let port_suffix = match url.port() {
        Some(p) => format!(":{p}"),
        None => String::new(),
    };
    let host = url.host_str().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    format!("{}://{}{}", url.scheme(), host, port_suffix)
}

/// Extract the root domain from a URL for subdomain comparison.
/// "blog.docs.example.com" -> "example.com", "example.co.uk" -> "example.co.uk" (best-effort).
///
/// Uses a simple heuristic: take the last two labels, or three if the second-to-last
/// is short (<=3 chars, likely a country SLD like "co.uk", "com.au").
fn root_domain(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    let host = host.strip_prefix("www.").unwrap_or(host);
    let labels: Vec<&str> = host.split('.').collect();

    if labels.len() <= 2 {
        return host.to_ascii_lowercase();
    }

    // Heuristic for two-part TLDs (co.uk, com.au, org.br, etc.)
    let sld = labels[labels.len() - 2];
    if labels.len() >= 3 && sld.len() <= 3 {
        labels[labels.len() - 3..].join(".").to_ascii_lowercase()
    } else {
        labels[labels.len() - 2..].join(".").to_ascii_lowercase()
    }
}

/// Normalize a URL for dedup: strip fragment, remove trailing slash (except root "/"),
/// lowercase scheme + host. Preserves query params and path case.
fn without_fragment(url: &Url) -> String {
    let mut url = url.clone();
    url.set_fragment(None);
    url.to_string()
}

fn dedup_key(url: &str) -> String {
    Url::parse(url).map_or_else(|_| url.to_string(), |u| normalize(&u))
}

pub(crate) fn normalize(url: &Url) -> String {
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

/// Maximum number of `**` wildcards allowed in a single user glob. Each
/// additional `**` multiplies the backtracking fan-out of `glob_match_inner`
/// against adversarial paths; 4 is a practical ceiling for legitimate
/// nested include/exclude patterns and still keeps the matcher linear-ish.
const MAX_GLOB_DOUBLESTAR: usize = 4;

/// Maximum glob pattern length. Keeps a single pattern from taking
/// megabytes of RAM if someone copy-pastes garbage into --include.
const MAX_GLOB_LEN: usize = 1024;

/// Validate a user-supplied glob pattern before it hits the matcher.
/// Rejects patterns that would drive `glob_match_inner` into pathological
/// backtracking (too many `**`, excessive length).
fn validate_glob(pat: &str) -> Result<(), FetchError> {
    if pat.len() > MAX_GLOB_LEN {
        return Err(FetchError::Build(format!(
            "glob pattern exceeds {MAX_GLOB_LEN} chars ({} given)",
            pat.len()
        )));
    }
    // Count non-overlapping occurrences of `**`.
    let bytes = pat.as_bytes();
    let mut count = 0usize;
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'*' && bytes[i + 1] == b'*' {
            count += 1;
            // Skip run of consecutive `*` so `***` counts as one.
            while i < bytes.len() && bytes[i] == b'*' {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
    if count > MAX_GLOB_DOUBLESTAR {
        return Err(FetchError::Build(format!(
            "glob pattern has {count} `**` wildcards (max {MAX_GLOB_DOUBLESTAR})"
        )));
    }
    Ok(())
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
            // `**` -- match everything including slashes
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
            // `*` -- match any chars except `/`
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
    use crate::robots::{RobotsRules, USER_AGENT_TOKEN};

    async fn crawl_site(respect_robots: bool, robots: &str) -> (CrawlResult, Vec<String>) {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                use_sitemap: false,
                max_depth: 2,
                delay: Duration::ZERO,
                respect_robots,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse(robots, USER_AGENT_TOKEN),
        );
        let result = crawler
            .crawl_with_fetcher("https://example.com/", None, move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move {
                    let html = match url.as_str() {
                        "https://example.com/" => "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/private/report'>Report</a><a href='/public'>Public</a></main>",
                        _ => "<main><h1>Leaf</h1><p>Leaf content for the crawl.</p></main>",
                    };
                    webclaw_core::extract_with_options(html, Some(&url), &Default::default())
                        .map_err(|e| e.to_string())
                }
            })
            .await;
        let calls = calls.lock().unwrap().clone();
        (result, calls)
    }

    #[tokio::test]
    async fn never_fetches_what_robots_txt_disallows() {
        let (result, calls) = crawl_site(true, "User-agent: *\nDisallow: /private\n").await;
        assert_eq!(
            calls,
            vec!["https://example.com/", "https://example.com/public"]
        );
        assert_eq!(result.skipped_by_robots, 1);
    }

    #[tokio::test]
    async fn checks_robots_and_fetches_the_url_as_linked() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                use_sitemap: false,
                max_depth: 1,
                delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nDisallow: /lessons/\n", USER_AGENT_TOKEN),
        );
        crawler
            .crawl_with_fetcher("https://example.com/", None, move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move {
                    let html = "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/lessons/'>Lessons</a><a href='/public/#top'>Public</a></main>";
                    webclaw_core::extract_with_options(html, Some(&url), &Default::default())
                        .map_err(|e| e.to_string())
                }
            })
            .await;
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["https://example.com/", "https://example.com/public/"]
        );
    }

    #[tokio::test]
    async fn a_disallowed_variant_does_not_hide_an_allowed_url() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                use_sitemap: false,
                max_depth: 1,
                delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nDisallow: /lessons/\n", USER_AGENT_TOKEN),
        );
        crawler
            .crawl_with_fetcher("https://example.com/", None, move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move {
                    let html = "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/lessons/'>Denied</a><a href='/lessons'>Allowed</a></main>";
                    webclaw_core::extract_with_options(html, Some(&url), &Default::default())
                        .map_err(|e| e.to_string())
                }
            })
            .await;
        assert_eq!(
            *calls.lock().unwrap(),
            vec!["https://example.com/", "https://example.com/lessons"]
        );
    }

    #[tokio::test]
    async fn fetches_disallowed_urls_when_robots_is_ignored() {
        let (result, calls) = crawl_site(false, "User-agent: *\nDisallow: /\n").await;
        assert_eq!(calls.len(), 3);
        assert_eq!(result.skipped_by_robots, 0);
    }

    #[tokio::test]
    async fn a_disallowed_seed_fetches_nothing() {
        let (result, calls) = crawl_site(true, "User-agent: *\nDisallow: /\n").await;
        assert!(calls.is_empty());
        assert_eq!(result.skipped_by_robots, 1);
        assert_eq!(result.total, 0);
    }

    #[tokio::test]
    async fn crawl_delay_spaces_request_starts_per_origin() {
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::from_millis(100),
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nCrawl-delay: 2\n", USER_AGENT_TOKEN),
        );
        crawler.preload_robots(
            "https://slow.test",
            RobotsRules::parse("User-agent: *\nCrawl-delay: 3600\n", USER_AGENT_TOKEN),
        );
        crawler.preload_robots("https://other.test", RobotsRules::allow_all());
        let mut slots = HashMap::new();
        let first = crawler
            .start_delay("https://example.com/a", &mut slots)
            .await;
        let second = crawler
            .start_delay("https://example.com/b", &mut slots)
            .await;
        let third = crawler
            .start_delay("https://example.com/c", &mut slots)
            .await;
        let other = crawler
            .start_delay("https://other.test/a", &mut slots)
            .await;
        crawler.start_delay("https://slow.test/a", &mut slots).await;
        let capped = crawler.start_delay("https://slow.test/b", &mut slots).await;
        assert_eq!(first, Duration::from_millis(100));
        assert!(second > Duration::from_millis(2000) && second <= Duration::from_millis(2100));
        assert!(third > Duration::from_millis(4000) && third <= Duration::from_millis(4100));
        assert_eq!(other, Duration::from_millis(100));
        assert!(capped <= MAX_CRAWL_DELAY + Duration::from_millis(100));
    }

    fn html_page(url: &str, html: &str) -> Page {
        let extraction = webclaw_core::extract_with_options(html, Some(url), &Default::default())
            .map_err(|e| e.to_string());
        Page {
            extraction,
            links: webclaw_core::page_links(html, url),
            redirect: None,
        }
    }

    fn redirect(to: &str) -> Page {
        Page {
            extraction: Err("redirected (301)".into()),
            links: Vec::new(),
            redirect: Some(to.into()),
        }
    }

    #[tokio::test]
    async fn a_slow_page_does_not_hold_back_the_next_depth() {
        let done = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = done.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 2,
                concurrency: 4,
                delay: Duration::ZERO,
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap();
        crawler
            .run("https://example.com/", None, move |url| {
                let recorded = recorded.clone();
                async move {
                    let path = Url::parse(&url).unwrap().path().to_string();
                    if path == "/slow" {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    let html = match path.as_str() {
                        "/" => "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/slow'>Slow</a><a href='/fast'>Fast</a></main>",
                        "/fast" => "<main><h1>Fast</h1><p>Fast page content here.</p><a href='/child'>Child</a></main>",
                        _ => "<main><h1>Leaf</h1><p>Leaf content for the crawl.</p></main>",
                    };
                    recorded.lock().unwrap().push(path);
                    html_page(&url, html)
                }
            })
            .await;
        assert_eq!(*done.lock().unwrap(), vec!["/", "/fast", "/child", "/slow"]);
    }

    #[test]
    fn a_checkpoint_puts_in_flight_pages_back_in_the_frontier() {
        let crawler = Crawler::new("https://example.com/", CrawlConfig::default()).unwrap();
        let dir = std::env::temp_dir().join(format!("webclaw-checkpoint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let visited: HashSet<String> = ["https://example.com/", "https://example.com/a"]
            .into_iter()
            .map(String::from)
            .collect();
        let mut frontier = Frontier::default();
        frontier.push_back("https://example.com/b".into(), 1);
        let in_flight = HashMap::from([("https://example.com/a/".to_string(), 1)]);
        crawler.write_checkpoint(
            &path,
            "https://example.com/",
            &visited,
            &frontier,
            &in_flight,
            7,
        );
        let state = Crawler::load_state(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(state.completed_pages, 7);
        assert_eq!(state.visited, vec!["https://example.com/".to_string()]);
        assert_eq!(
            state.frontier,
            vec![
                ("https://example.com/a/".to_string(), 1),
                ("https://example.com/b".to_string(), 1)
            ]
        );
    }

    #[tokio::test]
    async fn redirect_chains_stop_at_max_redirects() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::ZERO,
                respect_robots: false,
                fetch: FetchConfig {
                    max_redirects: 3,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let result = crawler
            .run("https://example.com/", None, move |url| {
                let n = counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                async move {
                    let _ = url;
                    redirect(&format!("/r/{n}"))
                }
            })
            .await;
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 4);
        assert_eq!(result.errors, 1);
    }

    #[tokio::test]
    async fn a_page_keeps_its_shortest_depth() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 3,
                concurrency: 8,
                delay: Duration::ZERO,
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap();
        crawler
            .run("https://example.com/", None, move |url| {
                let recorded = recorded.clone();
                async move {
                    let path = Url::parse(&url).unwrap().path().to_string();
                    if path == "/slow" {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                    }
                    let html = match path.as_str() {
                        "/" => "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/slow'>Slow</a><a href='/fast'>Fast</a></main>",
                        "/fast" => "<main><h1>Fast</h1><p>Fast page content here.</p><a href='/mid'>Mid</a></main>",
                        "/mid" => "<main><h1>Mid</h1><p>Middle page content here.</p><a href='/shared'>Shared</a></main>",
                        "/slow" => "<main><h1>Slow</h1><p>Slow page content here.</p><a href='/shared'>Shared</a></main>",
                        "/shared" => "<main><h1>Shared</h1><p>Shared page content here.</p><a href='/leaf'>Leaf</a></main>",
                        _ => "<main><h1>Leaf</h1><p>Leaf content for the crawl.</p></main>",
                    };
                    recorded.lock().unwrap().push(path);
                    html_page(&url, html)
                }
            })
            .await;
        assert!(
            calls.lock().unwrap().contains(&"/leaf".to_string()),
            "{:?}",
            calls.lock().unwrap()
        );
    }

    #[tokio::test]
    async fn caller_disabled_redirects_are_not_followed() {
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::ZERO,
                fetch: FetchConfig {
                    follow_redirects: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!crawler.follow_redirects);
    }

    #[tokio::test]
    async fn base_delay_and_crawl_delay_both_hold_between_starts() {
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::from_secs(1),
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nCrawl-delay: 1\n", USER_AGENT_TOKEN),
        );
        let mut slots = HashMap::new();
        let first = crawler
            .start_delay("https://example.com/a", &mut slots)
            .await;
        let second = crawler
            .start_delay("https://example.com/b", &mut slots)
            .await;
        assert!(
            second >= first + Duration::from_millis(990),
            "{first:?} then {second:?}"
        );
    }

    #[tokio::test]
    async fn redirects_go_through_scope_and_robots_checks() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 1,
                delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nDisallow: /private\n", USER_AGENT_TOKEN),
        );
        let result = crawler
            .run("https://example.com/", None, move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move {
                    match Url::parse(&url).unwrap().path() {
                        "/start" => redirect("/private/report"),
                        "/moved" => redirect("https://example.com/public"),
                        "/away" => redirect("https://elsewhere.test/"),
                        "/" => html_page(&url, "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/start'>Start</a><a href='/moved'>Moved</a><a href='/away'>Away</a></main>"),
                        _ => html_page(&url, "<main><h1>Leaf</h1><p>Leaf content for the crawl.</p></main>"),
                    }
                }
            })
            .await;
        let calls = calls.lock().unwrap().clone();
        assert!(
            !calls
                .iter()
                .any(|u| u.contains("/private") || u.contains("elsewhere")),
            "{calls:?}"
        );
        assert!(
            calls.contains(&"https://example.com/public".to_string()),
            "{calls:?}"
        );
        assert_eq!(result.total, 2);
        assert_eq!(result.skipped_by_robots, 1);
    }

    #[tokio::test]
    async fn cancellation_retains_all_already_started_successful_results() {
        let stopped = Arc::new(AtomicBool::new(false));
        let flag = stopped.clone();
        let barrier = Arc::new(tokio::sync::Barrier::new(3));
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                cancel_flag: Some(stopped),
                use_sitemap: false,
                respect_robots: false,
                concurrency: 3,
                delay: Duration::ZERO,
                ..Default::default()
            },
        )
        .unwrap();
        let resume = CrawlState {
            seed_url: "https://example.com/".into(),
            visited: vec![],
            frontier: ["slow", "success", "stop"]
                .map(|p| (format!("https://example.com/{p}"), 0))
                .to_vec(),
            completed_pages: 0,
            max_pages: 50,
            max_depth: 2,
        };
        let result = crawler
            .crawl_with_fetcher("https://example.com/", Some(resume), move |url| {
                let barrier = barrier.clone();
                let flag = flag.clone();
                async move {
                    barrier.wait().await;
                    if url.ends_with("stop") {
                        flag.store(true, Ordering::Relaxed);
                        return Err("budget exhausted".into());
                    }
                    if url.ends_with("slow") {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    webclaw_core::extract_with_options(
                        "<main><h1>Completed</h1><p>Useful completed content.</p></main>",
                        Some(&url),
                        &Default::default(),
                    )
                    .map_err(|e| e.to_string())
                }
            })
            .await;
        assert_eq!(
            result.ok, 2,
            "completed siblings must survive mid-batch cancellation"
        );
        assert_eq!(result.pages.len(), 3);
    }

    #[tokio::test]
    async fn supplied_fetcher_handles_seed_and_discovered_children_once() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                use_sitemap: false,
                max_depth: 2,
                delay: Duration::ZERO,
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap();
        let result=crawler.crawl_with_fetcher("https://example.com/",None,move |url| {
            recorded.lock().unwrap().push(url.clone());
            async move {
                let html=match url.as_str() {
                    "https://example.com/" => "<main><h1>Seed</h1><p>Rendered seed content.</p><a href='/child'>Child</a><a href='https://other.test/'>Outside</a></main>",
                    "https://example.com/child" => "<main><h1>Child</h1><p>Child content.</p><a href='/grandchild'>Grandchild</a><a href='/'>Seed</a></main>",
                    "https://example.com/grandchild" => "<main><h1>Grandchild</h1><p>Grandchild content.</p></main>",
                    _ => return Err("unexpected URL".into()),
                };
                webclaw_core::extract_with_options(html,Some(&url),&Default::default()).map_err(|e|e.to_string())
            }
        }).await;
        assert_eq!(result.ok, 3);
        assert_eq!(
            *calls.lock().unwrap(),
            vec![
                "https://example.com/",
                "https://example.com/child",
                "https://example.com/grandchild"
            ]
        );
    }

    #[test]
    fn no_page_cap_is_expressible_and_never_halts_the_crawl() {
        // The whole point of `Option`: `None` must mean "keep going", not
        // "stop at zero". A regression here would silently turn an uncapped
        // crawl into a no-op.
        let uncapped = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_pages: None,
                ..Default::default()
            },
        )
        .expect("build crawler");
        assert!(uncapped.under_page_cap(0));
        assert!(uncapped.under_page_cap(1_000_000));

        let capped = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_pages: Some(3),
                ..Default::default()
            },
        )
        .expect("build crawler");
        assert!(capped.under_page_cap(2));
        assert!(!capped.under_page_cap(3));
        assert!(!capped.under_page_cap(4));
    }

    #[test]
    fn default_config_keeps_a_page_cap() {
        // Uncapped has to be opt-in: a caller who says nothing should not get
        // an unbounded crawl by accident.
        assert_eq!(CrawlConfig::default().max_pages, Some(50));
    }

    #[test]
    fn stream_only_defaults_off_so_existing_callers_keep_collecting() {
        assert!(
            !CrawlConfig::default().stream_only,
            "flipping this default would silently empty CrawlResult::pages for every caller"
        );
    }

    /// The seed-fetch failure path is the one place a `CrawlResult` is built
    /// without running the main loop, so it is reachable without a network.
    /// It must report the failure in the counters either way — those are what
    /// a streaming consumer has left once `pages` is empty.
    #[tokio::test]
    async fn failed_seed_is_counted_not_just_collected() {
        let cfg = CrawlConfig {
            max_pages: Some(5),
            ..Default::default()
        };
        let crawler = Crawler::new("https://invalid.invalid/", cfg).expect("build crawler");
        let res = crawler.crawl("https://invalid.invalid/", None).await;

        assert_eq!(res.total, 1);
        assert_eq!(res.ok, 0);
        assert_eq!(res.errors, 1);
        assert_eq!(
            res.total,
            res.ok + res.errors,
            "counters must reconcile independently of what `pages` retained"
        );
    }

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

    // -- root_domain tests --

    #[test]
    fn root_domain_simple() {
        let url = Url::parse("https://example.com/page").unwrap();
        assert_eq!(root_domain(&url), "example.com");
    }

    #[test]
    fn root_domain_subdomain() {
        let url = Url::parse("https://blog.example.com/page").unwrap();
        assert_eq!(root_domain(&url), "example.com");
    }

    #[test]
    fn root_domain_deep_subdomain() {
        let url = Url::parse("https://a.b.c.example.com/").unwrap();
        assert_eq!(root_domain(&url), "example.com");
    }

    #[test]
    fn root_domain_country_tld() {
        let url = Url::parse("https://blog.example.co.uk/").unwrap();
        assert_eq!(root_domain(&url), "example.co.uk");
    }

    #[test]
    fn root_domain_strips_www() {
        let url = Url::parse("https://www.example.com/").unwrap();
        assert_eq!(root_domain(&url), "example.com");
    }

    // -- validate_glob tests --

    #[test]
    fn validate_glob_accepts_reasonable_patterns() {
        assert!(validate_glob("/api/*").is_ok());
        assert!(validate_glob("/api/**").is_ok());
        assert!(validate_glob("/docs/**/page-*.html").is_ok());
        assert!(validate_glob("/a/**/b/**/c/**/d/**").is_ok());
    }

    #[test]
    fn validate_glob_rejects_too_many_doublestars() {
        // 5 `**` exceeds MAX_GLOB_DOUBLESTAR = 4.
        let pat = "/a/**/b/**/c/**/d/**/e/**";
        let err = validate_glob(pat).unwrap_err();
        assert!(matches!(err, FetchError::Build(ref m) if m.contains("`**` wildcards")));
    }

    #[test]
    fn validate_glob_treats_triple_star_as_one() {
        // `***` is still one run, should not count as 2.
        assert!(validate_glob("/a/***/b/***/c/***/d/***").is_ok());
    }

    #[test]
    fn validate_glob_rejects_oversized_pattern() {
        let giant = "x".repeat(2048);
        let err = validate_glob(&giant).unwrap_err();
        assert!(matches!(err, FetchError::Build(ref m) if m.contains("exceeds")));
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
