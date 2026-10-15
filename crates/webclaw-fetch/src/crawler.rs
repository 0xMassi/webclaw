/// Recursive web crawler built on top of [`FetchClient`].
///
/// Starts from a seed URL, extracts content, discovers links, and follows
/// them breadth-first up to a configurable depth/page limit. Uses a semaphore
/// for bounded concurrency and spaces the requests to each host for politeness.
///
/// Scope control: by default only same-origin links are followed. Enable
/// `allow_subdomains` to include sibling/child subdomains of the seed host,
/// or `allow_external_links` to follow links to any domain.
///
/// When `use_sitemap` is enabled, the crawler first discovers URLs from the
/// site's sitemaps and seeds the BFS frontier before crawling.
///
/// With a `query`, the frontier is best-first instead: links that match the
/// question are fetched first.
use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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
    /// Minimum time between the starts of two requests to the same host
    /// (host and port, `http://` and `https://` together), shared by all
    /// workers. Each host is paced on its own, so a slow host never holds
    /// back another, and `concurrency` still caps the requests in flight
    /// across all hosts. A robots.txt `Crawl-delay` raises it for its host,
    /// up to 30 seconds. While robots.txt is respected, every redirect hop
    /// is a request of its own and is spaced too; without it the client
    /// follows redirects inside one fetch and those hops are not spaced.
    /// The first page waits one interval after the start host's robots.txt
    /// and sitemap requests, which are not spaced among themselves, and the
    /// one robots.txt request to each other host is not spaced.
    /// `Duration::ZERO` means no spacing.
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
    /// Pages still in flight are saved back into the frontier. When each host
    /// was last requested is not saved, so a resumed crawl starts each host's
    /// [`delay`](Self::delay) spacing afresh.
    pub checkpoint: Option<std::path::PathBuf>,
    /// Called for every finished page, in order, before the page counts as
    /// done in a checkpoint. Unlike `progress_tx` it never drops a page, so
    /// output written here survives a crash together with the checkpoint.
    pub on_page: Option<PageSink>,
    /// A question to crawl for. Links are fetched best match first, scored
    /// by the question's words in their text, URL path and surrounding block
    /// and by how well the page linking to them answers it; ties go to the
    /// shallower link. Each page gets a [`PageResult::relevance`] and keeps
    /// only the parts that answer the question, as with
    /// [`webclaw_core::focus_extraction`]. The first 32 distinct words of the
    /// question count, and a query without letters or digits counts as none.
    pub query: Option<String>,
    /// With a [`query`](Self::query), stop starting new pages once a page
    /// rated at least 0.8 has been fetched and the 8 pages fetched after it
    /// rated no higher. Pages already in flight still finish, and
    /// [`max_pages`](Self::max_pages) still caps the crawl. [`Crawler::new`]
    /// refuses it without a query that has a letter or digit.
    pub stop_when_answered: bool,
}

/// A callback that receives each finished page; see [`CrawlConfig::on_page`].
#[derive(Clone)]
pub struct PageSink(pub Arc<dyn Fn(&PageResult) + Send + Sync>);

impl std::fmt::Debug for PageSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PageSink")
    }
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
            on_page: None,
            query: None,
            stop_when_answered: false,
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
    /// How well the page answers [`CrawlConfig::query`], from 0 to 1. Set
    /// only when the crawl has a query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance: Option<f64>,
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
    /// Priority of each frontier URL in a query crawl, so a resumed crawl
    /// keeps its order.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub scores: HashMap<String, f64>,
    /// The query those scores answer. A resume with another query, or none,
    /// ignores them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// In a query crawl, the best page rating so far and the pages finished
    /// since, so a resume with [`CrawlConfig::stop_when_answered`] stops
    /// where the crawl would have stopped without the interruption.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_relevance: Option<(f64, usize)>,
}

/// Recursive crawler that wraps a shared [`FetchClient`].
pub struct Crawler {
    client: Arc<FetchClient>,
    page_client: Arc<FetchClient>,
    follow_redirects: bool,
    config: CrawlConfig,
    seed_origin: String,
    seed_scheme: String,
    /// Root domain of the seed URL for subdomain matching (e.g. "example.com").
    seed_root_domain: String,
    robots: std::sync::Mutex<HashMap<String, Arc<RobotsRules>>>,
    ask: Option<Arc<Ask>>,
}

struct Ask {
    question: webclaw_core::Question,
    query: String,
}

impl Ask {
    fn new(query: Option<&str>) -> Option<Arc<Self>> {
        let query = query?;
        Some(Arc::new(Self {
            question: webclaw_core::Question::new(query)?,
            query: query.to_string(),
        }))
    }
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
        let seed_origin = origin_key(&as_https(&seed));
        let seed_root_domain = root_domain(&seed);

        for pat in config
            .include_patterns
            .iter()
            .chain(config.exclude_patterns.iter())
        {
            validate_glob(pat)?;
        }

        let ask = Ask::new(config.query.as_deref());
        if config.stop_when_answered && ask.is_none() {
            return Err(FetchError::Build(
                "stopping when answered needs a question with a letter or digit".into(),
            ));
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
            ask,
            config,
            seed_origin,
            seed_scheme: seed.scheme().to_string(),
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
        Self::write_state(
            path,
            &CrawlState {
                seed_url: seed_url.to_string(),
                visited: visited.iter().cloned().collect(),
                frontier: frontier.to_vec(),
                completed_pages,
                max_pages,
                max_depth,
                scores: HashMap::new(),
                query: None,
                best_relevance: None,
            },
        )
    }

    fn write_state(path: &Path, state: &CrawlState) -> Result<(), String> {
        let json = serde_json::to_string(state).map_err(|e| format!("serialize state: {e}"))?;
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
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
    /// starts the next URL from a FIFO frontier as soon as one finishes and its
    /// host's [`CrawlConfig::delay`] has passed, so the crawl stays roughly
    /// breadth-first without waiting for the slowest page of each depth. A URL
    /// whose host must still wait gives way to the next URL on a host that is
    /// free. With `config.query` the next URL is the best scored one instead. Links come from the whole fetched page. With
    /// `respect_robots`, redirects are not followed blindly: each `Location`
    /// goes back through the scope and robots.txt checks like any link.
    ///
    /// When `config.use_sitemap` is true, sitemap URLs are discovered first and
    /// added to the frontier at depth 0 alongside the seed URL.
    ///
    /// If `resume_state` is provided, the crawl resumes from the saved state
    /// (pre-populated visited set and frontier) instead of starting fresh.
    pub async fn crawl(&self, start_url: &str, resume_state: Option<CrawlState>) -> CrawlResult {
        #[cfg(feature = "browser")]
        if let Some(render) = &self.config.fetch.render
            && render.pool.persistent()
            && let Err(e) = render.pool.open().await
        {
            return failed_crawl(start_url, e.to_string());
        }
        let client = Arc::clone(&self.page_client);
        let follow_redirects = self.follow_redirects;
        let with_context = self.ask.is_some();
        self.run(start_url, resume_state, move |url| {
            let client = Arc::clone(&client);
            async move { fetch_page(&client, &url, follow_redirects, with_context).await }
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
                    .map(|e| {
                        e.content
                            .links
                            .iter()
                            .map(|l| webclaw_core::Anchor {
                                href: l.href.clone(),
                                text: l.text.clone(),
                                context: String::new(),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Page {
                    extraction,
                    links,
                    redirect: None,
                    words: None,
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
            Err(_) => return failed_crawl(start_url, format!("invalid URL: {start_url}")),
        };

        let seed_rules = if self.config.respect_robots {
            self.robots_for(&seed).await
        } else {
            Arc::new(RobotsRules::allow_all())
        };

        let resumed_pages = resume_state.as_ref().map_or(0, |s| s.completed_pages);
        let mut quiet = Quiet::default();
        let mut visited: HashSet<String>;
        let mut frontier = Frontier {
            evict: self.ask.is_some(),
            ..Default::default()
        };
        if let Some(state) = resume_state {
            visited = state.visited.iter().map(|k| dedup_key(k)).collect();
            let query = self.ask.as_ref().map(|a| a.query.as_str());
            let scores = if query.is_some() && state.query.as_deref() == query {
                if let Some((best, since)) = state.best_relevance {
                    quiet = Quiet { best, since };
                }
                state.scores
            } else {
                if state.query.is_some() {
                    warn!(
                        saved = ?state.query,
                        "resuming with another query, the saved link order is dropped"
                    );
                }
                HashMap::new()
            };
            for (url, depth) in state.frontier {
                let score = scores.get(&url).copied().unwrap_or(0.0);
                frontier.push_back(url, depth, score);
            }
            info!(
                visited = visited.len(),
                frontier = frontier.len(),
                "resuming crawl from saved state"
            );
        } else {
            visited = HashSet::new();
            frontier.push_back(without_fragment(&seed), 0, 0.0);
            if self.config.use_sitemap && seed_rules.is_allowed("/sitemap.xml") {
                let base_url = format!("{}://{}", seed.scheme(), seed.host_str().unwrap_or(""));
                match sitemap::discover(&self.client, &base_url).await {
                    Ok(entries) => {
                        let before = frontier.len();
                        for entry in entries {
                            if let Some(url) =
                                self.qualify_link(&self.on_seed_scheme(&entry.url), &visited)
                            {
                                frontier.push_back(url, 0, 0.0);
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
        if (self.config.respect_robots || self.config.use_sitemap)
            && let Some(host) = pace_key(start_url)
        {
            next_start.insert(host, Instant::now() + self.interval(start_url).await);
        }
        let mut started: usize = resumed_pages;
        let mut in_flight = FuturesUnordered::new();
        let mut in_flight_depths: BTreeMap<usize, usize> = BTreeMap::new();
        let mut in_flight_urls: HashMap<String, (usize, f64)> = HashMap::new();
        let mut rating = self
            .ask
            .as_ref()
            .map(|ask| webclaw_core::Relevance::new(&ask.question));
        let mut last_checkpoint = Instant::now();
        let mut dirty = false;
        let mut redirect_hops: HashMap<String, usize> = HashMap::new();
        let mut same_page_redirects: HashSet<String> = HashSet::new();
        let mut answered = false;

        loop {
            let mut wake: Option<Instant> = None;
            while in_flight.len() < concurrency
                && self.under_page_cap(started)
                && !answered
                && !self.is_cancelled()
            {
                let deepest_allowed = match rating {
                    Some(_) => usize::MAX,
                    None => in_flight_depths
                        .keys()
                        .next()
                        .map_or(usize::MAX, |shallowest| shallowest + 1),
                };
                let now = Instant::now();
                let Some((url, depth, score)) =
                    frontier.pop(deepest_allowed, |host| match next_start.get(host) {
                        Some(&free) if free > now => {
                            wake = Some(wake.map_or(free, |w| w.min(free)));
                            false
                        }
                        _ => true,
                    })
                else {
                    break;
                };
                let key = dedup_key(&url);
                let same_page_target = same_page_redirects.remove(&url);
                if visited.contains(&key) || denied.contains(&url) {
                    if same_page_target {
                        visited.insert(key);
                    }
                    continue;
                }
                if self.config.respect_robots && !self.robots_allow(&url).await {
                    debug!(url = %url, "skipped: disallowed by robots.txt");
                    skipped_by_robots += 1;
                    if same_page_target {
                        visited.insert(key);
                    }
                    denied.insert(url);
                    continue;
                }
                visited.insert(key);
                started += 1;
                *in_flight_depths.entry(depth).or_default() += 1;
                in_flight_urls.insert(url.clone(), (depth, score));
                let interval = self.interval(&url).await;
                if let Some(host) = pace_key(&url) {
                    next_start.insert(host, Instant::now() + interval);
                }
                let fetch = Arc::clone(&fetch);
                let task_url = url.clone();
                let ask = self.ask.clone();
                let task = crate::transfer::spawn(async move {
                    let page_start = Instant::now();
                    let page = fetch(task_url).await;
                    let page = match ask {
                        Some(ask) => tokio::task::spawn_blocking(move || page.read(&ask))
                            .await
                            .unwrap_or_else(|e| Page::failed(format!("reading failed: {e}"))),
                        None => page,
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

            if in_flight.is_empty() && wake.is_none() {
                break;
            }
            let checkpoint = self.config.checkpoint.as_ref().filter(|_| dirty);
            let checkpoint_at = last_checkpoint + CHECKPOINT_INTERVAL;
            let mut wake_at = wake.unwrap_or(checkpoint_at);
            if self.config.cancel_flag.is_some() {
                wake_at = wake_at.min(Instant::now() + CANCEL_POLL);
            }
            let next = tokio::select! {
                next = in_flight.next(), if !in_flight.is_empty() => next,
                () = tokio::time::sleep_until(wake_at.into()), if wake.is_some() => continue,
                () = tokio::time::sleep_until(checkpoint_at.into()), if checkpoint.is_some() => {
                    if let Some(path) = checkpoint {
                        self.write_checkpoint(
                            path,
                            start_url,
                            &visited,
                            &frontier,
                            &in_flight_urls,
                            resumed_pages + completed,
                            &quiet,
                        );
                    }
                    last_checkpoint = Instant::now();
                    dirty = false;
                    continue;
                }
            };
            let Some((url, depth, page, elapsed)) = next else {
                break;
            };
            dirty = true;
            if let Some(n) = in_flight_depths.get_mut(&depth) {
                *n -= 1;
                if *n == 0 {
                    in_flight_depths.remove(&depth);
                }
            }
            let score = in_flight_urls.remove(&url).map_or(0.0, |(_, s)| s);

            let mut extraction = page.extraction;
            if let Some(location) = page.redirect {
                let hop = redirect_hops.remove(&url).unwrap_or(0) + 1;
                if hop <= self.config.fetch.max_redirects as usize {
                    started -= 1;
                    if let Some(target) = Url::parse(&url)
                        .ok()
                        .and_then(|base| base.join(&location).ok())
                    {
                        let source = dedup_key(&url);
                        let same_page = page_key(&target) == source;
                        if same_page {
                            visited.remove(&source);
                        }
                        if let Some(target) = self.qualify_link(target.as_str(), &visited) {
                            debug!(from = %url, to = %target, "following redirect");
                            if same_page {
                                same_page_redirects.insert(target.clone());
                            }
                            redirect_hops.insert(target.clone(), hop);
                            frontier.push_front(target, depth, score);
                        } else if same_page {
                            visited.insert(source);
                        }
                    }
                    continue;
                }
                extraction = Err(format!(
                    "stopped after {} redirects",
                    self.config.fetch.max_redirects
                ));
            }

            let mut relevance = None;
            let mut link_scores = Vec::new();
            if let (Some(rater), Some(words), true) = (&mut rating, &page.words, extraction.is_ok())
            {
                let (rated, scores) = rater.rate(words);
                relevance = Some(rated);
                link_scores = scores;
            }
            if self.config.stop_when_answered
                && rating.is_some()
                && !answered
                && quiet.answered(relevance.unwrap_or(0.0))
            {
                info!(
                    best = quiet.best,
                    "stopping: no better page in the last {ANSWERED_AFTER}"
                );
                answered = true;
            }

            if depth < self.config.max_depth && extraction.is_ok() {
                let mut best: Vec<(&str, f64)> = Vec::new();
                let mut at: HashMap<&str, usize> = HashMap::new();
                for (i, link) in page.links.iter().enumerate() {
                    let score = link_scores.get(i).copied().unwrap_or(0.0);
                    match at.get(link.href.as_str()) {
                        Some(&j) => best[j].1 = best[j].1.max(score),
                        None => {
                            at.insert(&link.href, best.len());
                            best.push((&link.href, score));
                        }
                    }
                }
                let mut dropped = false;
                for (href, score) in best {
                    if let Some(candidate) = self.qualify_link(&self.on_seed_scheme(href), &visited)
                    {
                        dropped |= !frontier.push_capped(candidate, depth + 1, score, frontier_cap);
                    }
                }
                if dropped {
                    warn!(cap = frontier_cap, "frontier full, dropping links");
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
                        relevance,
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
                        relevance: None,
                    }
                }
            };
            if let Some(sink) = &self.config.on_page {
                (sink.0)(&page);
            }
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
                    &quiet,
                );
                last_checkpoint = Instant::now();
                dirty = false;
            }
        }

        if self.is_cancelled()
            && let Some(path) = &self.config.checkpoint
        {
            self.write_checkpoint(
                path,
                start_url,
                &visited,
                &frontier,
                &in_flight_urls,
                resumed_pages + completed,
                &quiet,
            );
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

    #[allow(clippy::too_many_arguments)]
    fn write_checkpoint(
        &self,
        path: &Path,
        seed_url: &str,
        visited: &HashSet<String>,
        frontier: &Frontier,
        in_flight: &HashMap<String, (usize, f64)>,
        completed_pages: usize,
        quiet: &Quiet,
    ) {
        let in_flight_keys: HashSet<String> = in_flight.keys().map(|u| dedup_key(u)).collect();
        let mut scores: HashMap<String, f64> = HashMap::new();
        let mut pending = Vec::new();
        for (url, (depth, score)) in in_flight
            .iter()
            .map(|(url, entry)| (url.clone(), *entry))
            .chain(frontier.snapshot())
        {
            if score > 0.0 {
                let best = scores.entry(url.clone()).or_default();
                *best = best.max(score);
            }
            pending.push((url, depth));
        }
        let state = CrawlState {
            seed_url: seed_url.to_string(),
            visited: visited.difference(&in_flight_keys).cloned().collect(),
            frontier: pending,
            completed_pages,
            max_pages: self.config.max_pages.unwrap_or(0),
            max_depth: self.config.max_depth,
            scores,
            query: self.ask.as_ref().map(|a| a.query.clone()),
            best_relevance: self.ask.as_ref().map(|_| (quiet.best, quiet.since)),
        };
        if let Err(e) = Self::write_state(path, &state) {
            warn!(error = %e, "checkpoint failed");
        }
    }

    async fn interval(&self, url: &str) -> Duration {
        let base = self.config.delay;
        let Ok(parsed) = Url::parse(url) else {
            return base;
        };
        if !self.config.respect_robots {
            return base;
        }
        self.robots_for(&parsed)
            .await
            .crawl_delay
            .map_or(base, |d| d.min(MAX_CRAWL_DELAY).max(base))
    }

    fn on_seed_scheme(&self, href: &str) -> String {
        match Url::parse(href) {
            Ok(mut url)
                if matches!(url.scheme(), "http" | "https")
                    && url.scheme() != self.seed_scheme
                    && origin_key(&as_https(&url)) == self.seed_origin =>
            {
                let _ = url.set_scheme(&self.seed_scheme);
                url.to_string()
            }
            _ => href.to_string(),
        }
    }

    /// Check if a discovered link should be added to the frontier.
    /// Returns `Some(normalized_url)` if it passes all filters, `None` otherwise.
    fn qualify_link(&self, href: &str, visited: &HashSet<String>) -> Option<String> {
        let parsed = Url::parse(href).ok()?;

        match parsed.scheme() {
            "http" | "https" => {}
            _ => return None,
        }

        if !self.config.allow_external_links {
            let link_origin = origin_key(&as_https(&parsed));
            if link_origin != self.seed_origin {
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

        if let Some(ref prefix) = self.config.path_prefix
            && !parsed.path().starts_with(prefix.as_str())
        {
            return None;
        }

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

        if self
            .config
            .exclude_patterns
            .iter()
            .any(|pat| glob_match(pat, path))
        {
            return None;
        }

        const SKIP_EXTENSIONS: &[&str] = &[
            ".pdf", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".css", ".js",
            ".zip", ".tar", ".gz", ".xml", ".rss", ".mp3", ".mp4", ".avi", ".mov", ".woff",
            ".woff2", ".ttf", ".eot",
        ];
        if SKIP_EXTENSIONS.iter().any(|ext| path.ends_with(ext)) {
            return None;
        }

        if visited.contains(&page_key(&parsed)) {
            return None;
        }

        Some(without_fragment(&parsed))
    }
}

const MAX_CRAWL_DELAY: Duration = Duration::from_secs(30);
// ponytail: a crawl waiting for a host checks its cancel flag this often; wake it through a Notify if 100 ms is too slow.
const CANCEL_POLL: Duration = Duration::from_millis(100);
// ponytail: a fixed 1s interval rewrites the whole visited set each time; space it out by size if crawls past ~100k pages make it costly.
const CHECKPOINT_INTERVAL: Duration = Duration::from_secs(1);

const ANSWERED_RELEVANCE: f64 = 0.8;
const ANSWERED_AFTER: usize = 8;

#[derive(Default)]
struct Quiet {
    best: f64,
    since: usize,
}

impl Quiet {
    fn answered(&mut self, relevance: f64) -> bool {
        if relevance > self.best {
            self.best = relevance;
            self.since = 0;
        } else {
            self.since += 1;
        }
        self.best >= ANSWERED_RELEVANCE && self.since >= ANSWERED_AFTER
    }
}

type FrontierKey = (Reverse<u64>, usize, i64);

#[derive(Default)]
struct Frontier {
    queue: BTreeMap<FrontierKey, String>,
    hosts: HashMap<String, BTreeSet<FrontierKey>>,
    back: i64,
    front: i64,
    evict: bool,
}

impl Frontier {
    fn key(score: f64, depth: usize, seq: i64) -> FrontierKey {
        let score = if score.is_finite() && score > 0.0 {
            score
        } else {
            0.0
        };
        (Reverse(score.to_bits()), depth, seq)
    }

    fn insert(&mut self, key: FrontierKey, url: String) {
        self.hosts
            .entry(pace_key(&url).unwrap_or_default())
            .or_default()
            .insert(key);
        self.queue.insert(key, url);
    }

    fn remove(&mut self, key: &FrontierKey) -> Option<String> {
        let url = self.queue.remove(key)?;
        let host = pace_key(&url).unwrap_or_default();
        if let Some(keys) = self.hosts.get_mut(&host) {
            keys.remove(key);
            if keys.is_empty() {
                self.hosts.remove(&host);
            }
        }
        Some(url)
    }

    fn push_back(&mut self, url: String, depth: usize, score: f64) {
        self.insert(Self::key(score, depth, self.back), url);
        self.back += 1;
    }

    fn push_front(&mut self, url: String, depth: usize, score: f64) {
        self.front -= 1;
        self.insert(Self::key(score, depth, self.front), url);
    }

    fn push_capped(&mut self, url: String, depth: usize, score: f64, cap: usize) -> bool {
        if self.queue.len() >= cap {
            let key = Self::key(score, depth, self.back);
            match self.queue.last_key_value() {
                Some((&last, _)) if self.evict && key < last => {
                    self.remove(&last);
                }
                _ => return false,
            }
        }
        self.push_back(url, depth, score);
        true
    }

    // ponytail: each pop looks at the head of every queued host; keep the heads in a heap if a crawl queues thousands of hosts at once.
    fn pop(
        &mut self,
        deepest_allowed: usize,
        mut ready: impl FnMut(&str) -> bool,
    ) -> Option<(String, usize, f64)> {
        let key = self
            .hosts
            .iter()
            .filter_map(|(host, keys)| {
                keys.first()
                    .filter(|(_, depth, _)| *depth <= deepest_allowed)
                    .filter(|_| ready(host))
            })
            .min()
            .copied()?;
        let url = self.remove(&key)?;
        let (Reverse(score), depth, _) = key;
        Some((url, depth, f64::from_bits(score)))
    }

    fn len(&self) -> usize {
        self.queue.len()
    }

    fn snapshot(&self) -> Vec<(String, (usize, f64))> {
        self.queue
            .iter()
            .map(|((Reverse(score), depth, _), url)| {
                (url.clone(), (*depth, f64::from_bits(*score)))
            })
            .collect()
    }

    fn into_vec(self) -> Vec<(String, usize)> {
        self.queue
            .into_iter()
            .map(|((_, depth, _), url)| (url, depth))
            .collect()
    }
}

struct Page {
    extraction: Result<webclaw_core::ExtractionResult, String>,
    links: Vec<webclaw_core::Anchor>,
    redirect: Option<String>,
    words: Option<webclaw_core::PageWords>,
}

impl Page {
    fn failed(error: String) -> Self {
        Self {
            extraction: Err(error),
            links: Vec::new(),
            redirect: None,
            words: None,
        }
    }

    fn read(mut self, ask: &Ask) -> Self {
        if let Ok(e) = &mut self.extraction {
            self.words = Some(ask.question.read(&e.content.markdown, &self.links));
            webclaw_core::focus_extraction(e, &ask.query);
        }
        self
    }
}

fn failed_crawl(start_url: &str, error: String) -> CrawlResult {
    CrawlResult {
        pages: vec![PageResult {
            url: start_url.to_string(),
            depth: 0,
            extraction: None,
            error: Some(error),
            elapsed: Duration::ZERO,
            relevance: None,
        }],
        total: 1,
        ok: 0,
        errors: 1,
        skipped_by_robots: 0,
        elapsed_secs: 0.0,
        visited: HashSet::new(),
        remaining_frontier: Vec::new(),
    }
}

async fn fetch_page(
    client: &FetchClient,
    url: &str,
    follow_redirects: bool,
    with_context: bool,
) -> Page {
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
            words: None,
        };
    }
    let (page, extraction) = client.extract_page(fetched, &Default::default()).await;
    let links = match (page.document_bytes.is_none(), with_context) {
        (false, _) => Vec::new(),
        (true, true) => webclaw_core::page_anchors(&page.html, &page.url),
        (true, false) => webclaw_core::page_links(&page.html, &page.url)
            .into_iter()
            .map(|href| webclaw_core::Anchor {
                href,
                ..Default::default()
            })
            .collect(),
    };
    Page {
        extraction: extraction.map_err(|e| e.to_string()),
        links,
        redirect: None,
        words: None,
    }
}

/// Canonical origin string for comparing same-origin: "scheme://host[:port]".
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
    Url::parse(url).map_or_else(|_| url.to_string(), |u| page_key(&u))
}

fn page_key(url: &Url) -> String {
    normalize(&as_https(url))
}

fn as_https(url: &Url) -> Url {
    let mut url = url.clone();
    if url.scheme() == "http" {
        let _ = url.set_scheme("https");
    }
    url
}

fn pace_key(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .map(|u| as_https(&u).origin().ascii_serialization())
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

    fn html_page(url: &str, html: &str) -> Page {
        let extraction = webclaw_core::extract_with_options(html, Some(url), &Default::default())
            .map_err(|e| e.to_string());
        Page {
            extraction,
            links: webclaw_core::page_anchors(html, url),
            redirect: None,
            words: None,
        }
    }

    fn redirect(to: &str) -> Page {
        Page {
            extraction: Err("redirected (301)".into()),
            links: Vec::new(),
            redirect: Some(to.into()),
            words: None,
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

    #[tokio::test]
    async fn on_page_sees_every_page_when_streaming() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink_seen = seen.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 1,
                delay: Duration::ZERO,
                respect_robots: false,
                stream_only: true,
                on_page: Some(PageSink(Arc::new(move |page: &PageResult| {
                    sink_seen.lock().unwrap().push(page.url.clone());
                }))),
                ..Default::default()
            },
        )
        .unwrap();
        let result = crawler
            .run("https://example.com/", None, |url| async move {
                html_page(&url, "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/a'>A</a><a href='/b'>B</a></main>")
            })
            .await;
        assert!(result.pages.is_empty());
        assert_eq!(seen.lock().unwrap().len(), 3);
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
        frontier.push_back("https://example.com/b".into(), 1, 0.0);
        let in_flight = HashMap::from([("https://example.com/a/".to_string(), (1, 0.0))]);
        crawler.write_checkpoint(
            &path,
            "https://example.com/",
            &visited,
            &frontier,
            &in_flight,
            7,
            &Quiet::default(),
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

    fn food_site(url: &str) -> Page {
        let path = Url::parse(url).unwrap().path().to_string();
        let html = match path.as_str() {
            "/" => {
                "<main><h1>Food</h1><p>Recipes from our kitchen.</p><ul><li><a href='/apples'>Apples</a></li><li><a href='/bread'>Bread</a></li></ul></main>"
            }
            "/bread" => {
                "<main><h1>Bread</h1><p>All our bread.</p><ul><li><a href='/bread/rye'>Rye</a></li><li><a href='/bread/banana'>Banana bread recipe</a></li></ul></main>"
            }
            "/bread/banana" => {
                "<main><h1>Banana bread</h1><p>This banana bread recipe needs ripe bananas.</p><h2>Storage</h2><p>Keep it in a tin for three days.</p></main>"
            }
            _ => "<main><h1>Leaf</h1><p>Nothing to see on this page.</p></main>",
        };
        html_page(url, html)
    }

    async fn crawl_food(query: Option<&str>) -> (CrawlResult, Vec<String>) {
        let done = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = done.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 2,
                concurrency: 1,
                delay: Duration::ZERO,
                respect_robots: false,
                query: query.map(String::from),
                ..Default::default()
            },
        )
        .unwrap();
        let result = crawler
            .run("https://example.com/", None, move |url| {
                recorded
                    .lock()
                    .unwrap()
                    .push(Url::parse(&url).unwrap().path().to_string());
                async move { food_site(&url) }
            })
            .await;
        let order = done.lock().unwrap().clone();
        (result, order)
    }

    #[tokio::test]
    async fn a_query_crawl_follows_the_best_link_first() {
        let (_, bfs) = crawl_food(None).await;
        assert_eq!(bfs[..3], ["/", "/apples", "/bread"]);
        let (result, guided) = crawl_food(Some("banana bread recipe")).await;
        assert_eq!(guided[..3], ["/", "/bread", "/bread/banana"]);
        let banana = result
            .pages
            .iter()
            .find(|p| p.url.ends_with("/bread/banana"))
            .unwrap();
        let best = result
            .pages
            .iter()
            .filter_map(|p| p.relevance)
            .fold(0.0, f64::max);
        assert_eq!(banana.relevance, Some(best));
        let markdown = &banana.extraction.as_ref().unwrap().content.markdown;
        assert!(markdown.contains("ripe bananas"), "{markdown}");
        assert!(!markdown.contains("tin"), "{markdown}");
        assert!(result.pages.iter().all(|p| p.relevance.is_some()));
    }

    #[test]
    fn answered_once_a_good_page_has_no_better_successor_for_eight_pages() {
        let mut quiet = Quiet::default();
        assert!(!quiet.answered(0.7));
        assert!((0..20).all(|_| !quiet.answered(0.2)));
        assert!(!quiet.answered(0.8));
        assert!((0..7).all(|_| !quiet.answered(0.8)));
        assert!(!quiet.answered(0.9));
        assert!((0..7).all(|_| !quiet.answered(0.1)));
        assert!(quiet.answered(0.9));
    }

    fn library(url: &str) -> Page {
        let path = Url::parse(url).unwrap().path().to_string();
        let html = if path == "/" {
            let mut links = String::from("<a href='/banana'>Fruit loaf</a>");
            for i in 0..30 {
                links.push_str(&format!("<a href='/shelf{i}'>Shelf {i}</a>"));
            }
            format!("<main><h1>Library</h1><p>Our kitchen notes.</p>{links}</main>")
        } else if path == "/banana" {
            "<main><h1>Banana bread recipe</h1><p>Mash ripe bananas into the batter.</p><p>Bake it for an hour.</p></main>".to_string()
        } else {
            format!("<main><h1>{path}</h1><p>Notes on bread, without fruit.</p></main>")
        };
        html_page(url, &html)
    }

    async fn crawl_library(query: Option<&str>, stop: bool) -> CrawlResult {
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 1,
                concurrency: 1,
                delay: Duration::ZERO,
                respect_robots: false,
                query: query.map(String::from),
                stop_when_answered: stop,
                ..Default::default()
            },
        )
        .unwrap();
        crawler
            .run(
                "https://example.com/",
                None,
                |url| async move { library(&url) },
            )
            .await
    }

    #[tokio::test]
    async fn a_resumed_crawl_stops_where_the_uninterrupted_one_would() {
        let dir = std::env::temp_dir().join(format!("webclaw-answered-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = dir.join("state.json");
        let config = |cancel: Option<Arc<AtomicBool>>| CrawlConfig {
            max_depth: 1,
            concurrency: 1,
            delay: Duration::ZERO,
            respect_robots: false,
            query: Some("banana bread recipe".into()),
            stop_when_answered: true,
            checkpoint: Some(state.clone()),
            cancel_flag: cancel,
            ..Default::default()
        };
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let fetched = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let first = Crawler::new("https://example.com/", config(Some(stop)))
            .unwrap()
            .run("https://example.com/", None, move |url| {
                if fetched.fetch_add(1, Ordering::SeqCst) + 1 == 5 {
                    flag.store(true, Ordering::Release);
                }
                async move { library(&url) }
            })
            .await;
        let saved = Crawler::load_state(&state).unwrap();
        let second = Crawler::new("https://example.com/", config(None))
            .unwrap()
            .run("https://example.com/", Some(saved), |url| async move {
                library(&url)
            })
            .await;
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(first.total, 5);
        assert_eq!(first.total + second.total, 10);
    }

    #[test]
    fn stop_when_answered_is_refused_without_a_question() {
        let config = |query: Option<&str>| CrawlConfig {
            query: query.map(String::from),
            stop_when_answered: true,
            ..Default::default()
        };
        for query in [None, Some(""), Some("  ?! ")] {
            let err = Crawler::new("https://example.com/", config(query)).err();
            assert!(
                matches!(err, Some(FetchError::Build(ref m)) if m.contains("question")),
                "{query:?}"
            );
        }
        assert!(Crawler::new("https://example.com/", config(Some("banana"))).is_ok());
    }

    #[tokio::test]
    async fn stop_when_answered_ends_a_query_crawl_eight_pages_after_its_best_page() {
        let full = crawl_library(Some("banana bread recipe"), false).await;
        assert_eq!(full.total, 32);
        let stopped = crawl_library(Some("banana bread recipe"), true).await;
        let urls: Vec<&str> = stopped.pages.iter().map(|p| p.url.as_str()).collect();
        assert_eq!(urls[1], "https://example.com/banana", "{urls:?}");
        assert_eq!(stopped.total, 10, "{urls:?}");
    }

    #[tokio::test]
    async fn a_query_without_words_crawls_breadth_first() {
        for query in ["", "   ", " ?! "] {
            let (result, order) = crawl_food(Some(query)).await;
            assert_eq!(order[..3], ["/", "/apples", "/bread"], "{query:?}");
            assert!(result.pages.iter().all(|p| p.relevance.is_none()));
        }
    }

    #[test]
    fn a_full_breadth_first_frontier_drops_new_links() {
        let mut frontier = Frontier::default();
        assert!(frontier.push_capped("deep-a".into(), 3, 0.0, 2));
        assert!(frontier.push_capped("deep-b".into(), 3, 0.0, 2));
        assert!(!frontier.push_capped("shallow".into(), 2, 0.0, 2));
        assert_eq!(
            frontier.into_vec(),
            vec![("deep-a".to_string(), 3), ("deep-b".to_string(), 3)]
        );
    }

    #[test]
    fn a_full_frontier_drops_its_lowest_priority_link() {
        let mut frontier = Frontier {
            evict: true,
            ..Default::default()
        };
        assert!(frontier.push_capped("a".into(), 1, 0.5, 2));
        assert!(frontier.push_capped("b".into(), 1, 0.1, 2));
        assert!(frontier.push_capped("c".into(), 1, 0.3, 2));
        assert!(!frontier.push_capped("d".into(), 1, 0.2, 2));
        assert!(!frontier.push_capped("e".into(), 1, 0.3, 2));
        assert_eq!(
            frontier.into_vec(),
            vec![("a".to_string(), 1), ("c".to_string(), 1)]
        );
    }

    async fn resume(saved: Option<&str>, resumed: Option<&str>) -> (CrawlState, Vec<String>) {
        let config = |query: Option<&str>| CrawlConfig {
            max_depth: 1,
            concurrency: 1,
            delay: Duration::ZERO,
            respect_robots: false,
            query: query.map(String::from),
            ..Default::default()
        };
        let dir = std::env::temp_dir().join(format!(
            "webclaw-resume-{}-{saved:?}-{resumed:?}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let writer = Crawler::new("https://example.com/", config(saved)).unwrap();
        let mut frontier = Frontier::default();
        frontier.push_back("https://example.com/low".into(), 1, 0.1);
        frontier.push_back("https://example.com/high".into(), 1, 0.9);
        let visited = HashSet::from(["https://example.com".to_string()]);
        writer.write_checkpoint(
            &path,
            "https://example.com/",
            &visited,
            &frontier,
            &HashMap::new(),
            1,
            &Quiet::default(),
        );
        let state = Crawler::load_state(&path).unwrap();
        std::fs::remove_dir_all(&dir).ok();
        let mut resumed_state = state.clone();
        resumed_state.frontier.reverse();
        let done = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = done.clone();
        Crawler::new("https://example.com/", config(resumed))
            .unwrap()
            .run("https://example.com/", Some(resumed_state), move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move { food_site(&url) }
            })
            .await;
        let order = done.lock().unwrap().clone();
        (state, order)
    }

    #[tokio::test]
    async fn a_resumed_query_crawl_keeps_its_priorities() {
        let (state, order) = resume(Some("banana"), Some("banana")).await;
        assert_eq!(state.scores.get("https://example.com/high"), Some(&0.9));
        assert_eq!(state.scores.get("https://example.com/low"), Some(&0.1));
        assert_eq!(
            order,
            ["https://example.com/high", "https://example.com/low"]
        );
    }

    #[tokio::test]
    async fn a_resume_without_the_saved_query_ignores_its_scores() {
        for resumed in [None, Some("apple")] {
            let (_, order) = resume(Some("banana"), resumed).await;
            assert_eq!(
                order,
                ["https://example.com/low", "https://example.com/high"],
                "{resumed:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_cancelled_crawl_leaves_its_checkpoint_behind() {
        let dir = std::env::temp_dir().join(format!("webclaw-cancel-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.json");
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                respect_robots: false,
                delay: Duration::ZERO,
                cancel_flag: Some(Arc::new(AtomicBool::new(true))),
                checkpoint: Some(path.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        crawler
            .run("https://example.com/", None, |url| async move {
                food_site(&url)
            })
            .await;
        let state = Crawler::load_state(&path);
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            state.unwrap().frontier,
            vec![("https://example.com/".to_string(), 0)]
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

    #[cfg(feature = "browser")]
    #[tokio::test]
    async fn a_crawl_on_a_profile_that_cannot_open_fails_once_at_the_start() {
        let render = crate::render::RenderConfig {
            pool: Arc::new(crate::render::RenderPool::new(
                crate::render::BrowserSource::Profile {
                    proxy: None,
                    dir: "/nonexistent/webclaw-profile".into(),
                },
                Duration::from_secs(1),
            )),
            always: false,
            wait_for: None,
            block: crate::Block::default(),
            solve: false,
        };
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::ZERO,
                fetch: FetchConfig {
                    render: Some(render),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let result = crawler.crawl("https://example.com/", None).await;
        assert_eq!((result.total, result.errors), (1, 1));
        let error = result.pages[0].error.as_deref().unwrap_or_default();
        assert!(error.contains("/nonexistent/webclaw-profile"), "{error}");
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
    async fn a_link_in_the_other_scheme_is_fetched_on_the_seed_scheme() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "http://example.com/",
            CrawlConfig {
                max_depth: 1,
                delay: Duration::ZERO,
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap();
        let result = crawler
            .run("http://example.com/", None, move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move {
                    if url.starts_with("https:") {
                        return Page::failed("connection refused".into());
                    }
                    match Url::parse(&url).unwrap().path() {
                        "/" => html_page(&url, "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='https://example.com/page'>Page</a><a href='http://example.com/page'>Page</a></main>"),
                        _ => html_page(&url, "<main><h1>Leaf</h1><p>Leaf content for the crawl.</p></main>"),
                    }
                }
            })
            .await;
        let calls = calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["http://example.com/", "http://example.com/page"],
            "{calls:?}"
        );
        assert_eq!((result.ok, result.errors), (2, 0));
    }

    #[tokio::test]
    async fn a_resume_from_an_older_state_skips_visited_http_pages() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let state = CrawlState {
            seed_url: "http://example.com/".into(),
            visited: vec!["http://example.com/".into(), "http://example.com/a".into()],
            frontier: vec![("http://example.com/b".into(), 1)],
            completed_pages: 2,
            max_pages: 0,
            max_depth: 2,
            scores: HashMap::new(),
            query: None,
            best_relevance: None,
        };
        Crawler::new(
            "http://example.com/",
            CrawlConfig {
                max_depth: 2,
                delay: Duration::ZERO,
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap()
        .run("http://example.com/", Some(state), move |url| {
            recorded.lock().unwrap().push(url.clone());
            async move {
                html_page(&url, "<main><h1>Page</h1><p>Page content for the crawl.</p><a href='/'>Home</a><a href='/a'>A</a></main>")
            }
        })
        .await;
        assert_eq!(*calls.lock().unwrap(), vec!["http://example.com/b"]);
    }

    #[tokio::test]
    async fn a_redirect_to_the_same_page_is_followed_once() {
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                max_depth: 2,
                delay: Duration::ZERO,
                fetch: FetchConfig {
                    max_redirects: 3,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let robots = "User-agent: *\nDisallow: /tag/x/\nDisallow: /private\n";
        for origin in ["https://example.com", "http://example.com"] {
            crawler.preload_robots(origin, RobotsRules::parse(robots, USER_AGENT_TOKEN));
        }
        crawler.preload_robots("https://www.example.com", RobotsRules::allow_all());
        let result = crawler
            .run("https://example.com/", None, move |url| {
                recorded.lock().unwrap().push(url.clone());
                async move {
                    let path = Url::parse(&url).unwrap().path().to_string();
                    match path.as_str() {
                        "/author/a" | "/tag/x" | "/plain" | "/loop" => redirect(&format!("{path}/")),
                        "/loop/" => redirect("/loop"),
                        "/plain/" if url.starts_with("https:") => redirect("http://example.com/plain/"),
                        "/Upper" => redirect("/upper"),
                        "/index.html" => redirect("/"),
                        "/w" if !url.contains("www.") => redirect("https://www.example.com/w"),
                        "/w" => html_page(&url, "<main><h1>W</h1><p>Leaf content for the crawl.</p></main>"),
                        "/start" => redirect("/private/report"),
                        "/" => html_page(&url, "<main><h1>Seed</h1><p>Seed content for the crawl.</p><a href='/author/a'>A</a><a href='/tag/x'>X</a><a href='/plain'>P</a><a href='/loop'>L</a><a href='/Upper'>U</a><a href='/index.html'>I</a><a href='/w'>W</a><a href='/start'>S</a></main>"),
                        _ => html_page(&url, "<main><h1>Leaf</h1><p>Leaf content for the crawl.</p><a href='/author/a'>A</a><a href='/author/a/'>A</a><a href='/tag/x'>X</a></main>"),
                    }
                }
            })
            .await;
        let calls = calls.lock().unwrap().clone();
        let count = |u: &str| calls.iter().filter(|c| *c == u).count();
        for url in [
            "https://example.com/",
            "https://example.com/author/a",
            "https://example.com/author/a/",
            "https://example.com/tag/x",
            "https://example.com/plain",
            "https://example.com/plain/",
            "http://example.com/plain/",
            "https://example.com/upper",
            "https://www.example.com/w",
        ] {
            assert_eq!(count(url), 1, "{url} in {calls:?}");
        }
        for url in [
            "https://example.com/tag/x/",
            "https://example.com/private/report",
        ] {
            assert_eq!(count(url), 0, "{url} in {calls:?}");
        }
        assert_eq!(
            count("https://example.com/loop") + count("https://example.com/loop/"),
            4,
            "{calls:?}"
        );
        assert_eq!(result.skipped_by_robots, 2);
        let pages: Vec<&str> = result.pages.iter().map(|p| p.url.as_str()).collect();
        assert!(
            pages.contains(&"https://example.com/author/a/"),
            "{pages:?}"
        );
        assert!(pages.contains(&"http://example.com/plain/"), "{pages:?}");
        assert_eq!(result.errors, 1, "{pages:?}");
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
            scores: HashMap::new(),
            query: None,
            best_relevance: None,
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

    #[tokio::test]
    async fn each_host_gets_the_larger_of_delay_and_crawl_delay() {
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::from_millis(1500),
                ..Default::default()
            },
        )
        .unwrap();
        for (origin, robots) in [
            ("https://example.com", "User-agent: *\nCrawl-delay: 2\n"),
            ("https://short.test", "User-agent: *\nCrawl-delay: 1\n"),
            ("https://slow.test", "User-agent: *\nCrawl-delay: 3600\n"),
            ("https://other.test", ""),
        ] {
            crawler.preload_robots(origin, RobotsRules::parse(robots, USER_AGENT_TOKEN));
        }
        let interval = |url: &'static str| crawler.interval(url);
        assert_eq!(
            interval("https://example.com/a").await,
            Duration::from_secs(2)
        );
        assert_eq!(
            interval("https://short.test/a").await,
            Duration::from_millis(1500)
        );
        assert_eq!(interval("https://slow.test/a").await, MAX_CRAWL_DELAY);
        assert_eq!(
            interval("https://other.test/a").await,
            Duration::from_millis(1500)
        );
    }

    #[tokio::test]
    async fn ignoring_robots_keeps_the_delay_and_drops_crawl_delay() {
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                delay: Duration::from_millis(250),
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nCrawl-delay: 5\n", USER_AGENT_TOKEN),
        );
        assert_eq!(
            crawler.interval("https://example.com/a").await,
            Duration::from_millis(250)
        );
    }

    #[test]
    fn http_and_https_share_a_host_and_ports_do_not() {
        assert_eq!(
            pace_key("http://example.com/a"),
            pace_key("https://example.com/b")
        );
        assert_ne!(
            pace_key("https://example.com/a"),
            pace_key("https://example.com:8443/a")
        );
        assert_ne!(
            pace_key("https://example.com/a"),
            pace_key("https://www.example.com/a")
        );
    }

    #[test]
    fn pop_skips_urls_whose_host_must_wait() {
        let mut frontier = Frontier::default();
        for url in ["https://a.test/1", "https://a.test/2", "https://b.test/1"] {
            frontier.push_back(url.into(), 1, 0.0);
        }
        let first = frontier.pop(usize::MAX, |host| host == "https://b.test");
        assert_eq!(
            first.map(|(url, ..)| url).as_deref(),
            Some("https://b.test/1")
        );
        assert!(frontier.pop(usize::MAX, |_| false).is_none());
        let next = frontier.pop(usize::MAX, |_| true);
        assert_eq!(
            next.map(|(url, ..)| url).as_deref(),
            Some("https://a.test/1")
        );
        assert!(frontier.pop(0, |_| true).is_none());
    }

    #[tokio::test]
    async fn cancelling_while_a_host_waits_stops_at_once_and_saves_state() {
        let stopped = Arc::new(AtomicBool::new(false));
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state.json");
        let crawler = Crawler::new(
            "https://example.com/",
            CrawlConfig {
                cancel_flag: Some(stopped.clone()),
                checkpoint: Some(state.clone()),
                max_depth: 2,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://example.com",
            RobotsRules::parse("User-agent: *\nCrawl-delay: 30\n", USER_AGENT_TOKEN),
        );
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            stopped.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let crawl = crawler.crawl_with_fetcher("https://example.com/", None, |url| async move {
            webclaw_core::extract_with_options(
                "<main><h1>Seed</h1><p>Links to wait for.</p><a href='/a'>A</a><a href='/b'>B</a></main>",
                Some(&url),
                &Default::default(),
            )
            .map_err(|e| e.to_string())
        });
        tokio::time::timeout(Duration::from_secs(5), crawl)
            .await
            .expect("the crawl kept waiting after it was cancelled");
        assert!(started.elapsed() < Duration::from_secs(3));
        let saved = Crawler::load_state(&state).expect("no state saved");
        assert!(!saved.frontier.is_empty(), "{saved:?}");
    }

    #[tokio::test]
    async fn the_first_page_waits_one_interval_after_the_setup_requests() {
        for respect_robots in [true, false] {
            let crawler = Crawler::new(
                "https://example.com/",
                CrawlConfig {
                    delay: Duration::from_millis(200),
                    respect_robots,
                    ..Default::default()
                },
            )
            .unwrap();
            crawler.preload_robots("https://example.com", RobotsRules::allow_all());
            let first = Arc::new(std::sync::Mutex::new(None));
            let seen = first.clone();
            let started = Instant::now();
            crawler
                .crawl_with_fetcher("https://example.com/", None, move |url| {
                    seen.lock().unwrap().get_or_insert(started.elapsed());
                    async move {
                        webclaw_core::extract_with_options(
                            "<main><h1>Seed</h1><p>Only page.</p></main>",
                            Some(&url),
                            &Default::default(),
                        )
                        .map_err(|e| e.to_string())
                    }
                })
                .await;
            let first = first.lock().unwrap().unwrap();
            if respect_robots {
                assert!(first >= Duration::from_millis(190), "{first:?}");
            } else {
                assert!(first < Duration::from_millis(100), "{first:?}");
            }
        }
    }

    #[test]
    fn pop_takes_the_best_url_among_free_hosts_in_order_within_each_host() {
        let mut frontier = Frontier::default();
        frontier.push_back("https://a.test/good".into(), 1, 0.9);
        frontier.push_back("https://a.test/fair".into(), 1, 0.5);
        frontier.push_back("https://b.test/fine".into(), 1, 0.7);
        let order = |frontier: &mut Frontier, free: &dyn Fn(&str) -> bool| {
            std::iter::from_fn(|| frontier.pop(usize::MAX, |host| free(host)).map(|(u, ..)| u))
                .collect::<Vec<_>>()
        };
        let mut both = Frontier::default();
        for (url, (depth, score)) in frontier.snapshot() {
            both.push_back(url, depth, score);
        }
        assert_eq!(
            order(&mut both, &|_| true),
            [
                "https://a.test/good",
                "https://b.test/fine",
                "https://a.test/fair"
            ]
        );
        assert_eq!(
            order(&mut frontier, &|host| host != "https://a.test"),
            ["https://b.test/fine"]
        );
        assert_eq!(frontier.len(), 2);
        assert_eq!(frontier.hosts.len(), 1);
    }

    #[tokio::test]
    async fn a_host_on_a_long_crawl_delay_never_holds_back_another() {
        let crawler = Crawler::new(
            "https://a.test/",
            CrawlConfig {
                delay: Duration::from_millis(100),
                concurrency: 4,
                max_pages: Some(4),
                allow_external_links: true,
                ..Default::default()
            },
        )
        .unwrap();
        crawler.preload_robots(
            "https://a.test",
            RobotsRules::parse("User-agent: *\nCrawl-delay: 2\n", USER_AGENT_TOKEN),
        );
        crawler.preload_robots("https://b.test", RobotsRules::allow_all());
        let frontier = (0..300)
            .map(|i| (format!("https://a.test/{i}"), 1))
            .chain((0..3).map(|i| (format!("https://b.test/{i}"), 1)))
            .collect();
        let resume = CrawlState {
            seed_url: "https://a.test/".into(),
            visited: vec![],
            frontier,
            completed_pages: 0,
            max_pages: 4,
            max_depth: 1,
            scores: HashMap::new(),
            query: None,
            best_relevance: None,
        };
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = calls.clone();
        let started = Instant::now();
        let crawl = crawler.crawl_with_fetcher("https://a.test/", Some(resume), move |url| {
            recorded
                .lock()
                .unwrap()
                .push((url.clone(), started.elapsed()));
            async move {
                webclaw_core::extract_with_options(
                    "<main><h1>Page</h1><p>Some content.</p></main>",
                    Some(&url),
                    &Default::default(),
                )
                .map_err(|e| e.to_string())
            }
        });
        let result = tokio::time::timeout(Duration::from_secs(6), crawl)
            .await
            .expect("host b waited behind host a");
        let calls = calls.lock().unwrap().clone();
        assert_eq!(result.total, 4, "{calls:?}");
        let b: Vec<Duration> = calls
            .iter()
            .filter(|(url, _)| url.starts_with("https://b.test/"))
            .map(|(_, at)| *at)
            .collect();
        assert_eq!(b.len(), 3, "{calls:?}");
        assert!(b[0] < Duration::from_millis(150), "{calls:?}");
    }

    type Hits = Arc<std::sync::Mutex<Vec<(String, String, Instant)>>>;

    const SEED_HOST: &str = "example.com";
    const OTHER_HOST: &str = "example.org";

    fn two_host_html(host: &str, path: &str) -> Option<String> {
        match (host, path) {
            (_, "/robots.txt") => Some("User-agent: *\nAllow: /\n".into()),
            (_, "/moved") => None,
            (SEED_HOST, "/") => {
                let mut links = String::new();
                for i in 1..=4 {
                    links.push_str(&format!(
                        "<a href='/a{i}'>Page a{i}</a> <a href='http://{OTHER_HOST}/b{i}'>Page b{i}</a> "
                    ));
                }
                Some(format!(
                    "<html><body><main><h1>Seed</h1><p>Seed page with links to two hosts.</p>{links}<a href='/moved'>Moved</a></main></body></html>"
                ))
            }
            _ => Some(format!(
                "<html><body><main><h1>Leaf {path}</h1><p>Leaf page served for {host}.</p></main></body></html>"
            )),
        }
    }

    fn assert_paced(hits: &[(String, String, Instant)], delay: Duration, elapsed: Duration) {
        let starts = |host: &str| -> Vec<Instant> {
            hits.iter()
                .filter(|(h, path, _)| h == host && path != "/robots.txt")
                .map(|(_, _, at)| *at)
                .collect()
        };
        let (seed, other) = (starts(SEED_HOST), starts(OTHER_HOST));
        assert!(seed.len() >= 6 && other.len() == 4, "{hits:?}");
        let tolerance = Duration::from_millis(30);
        for host in [&seed, &other] {
            for pair in host.windows(2) {
                let gap = pair[1] - pair[0];
                assert!(
                    gap + tolerance >= delay,
                    "two requests to one host {gap:?} apart"
                );
            }
        }
        assert!(other[0] < seed[1], "{hits:?}");
        assert!(elapsed < delay * 9, "took {elapsed:?}");
    }

    #[tokio::test]
    async fn delay_spaces_each_host_while_hosts_run_side_by_side() {
        let hits: Hits = Default::default();
        let recorded = hits.clone();
        let delay = Duration::from_millis(300);
        let seed = format!("https://{SEED_HOST}/");
        let crawler = Crawler::new(
            &seed,
            CrawlConfig {
                delay,
                concurrency: 4,
                allow_external_links: true,
                respect_robots: false,
                ..Default::default()
            },
        )
        .unwrap();
        let started = Instant::now();
        let result = crawler
            .crawl_with_fetcher(&seed, None, move |url| {
                let parsed = Url::parse(&url).unwrap();
                let host = parsed.host_str().unwrap_or_default().to_string();
                let path = parsed.path().to_string();
                recorded
                    .lock()
                    .unwrap()
                    .push((host.clone(), path.clone(), Instant::now()));
                async move {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let html = two_host_html(&host, &path).unwrap_or_default();
                    webclaw_core::extract_with_options(&html, Some(&url), &Default::default())
                        .map_err(|e| e.to_string())
                }
            })
            .await;
        let elapsed = started.elapsed();
        assert_eq!(result.total, 10, "{:?}", result.pages);
        assert_paced(&hits.lock().unwrap(), delay, elapsed);
    }

    #[tokio::test]
    #[ignore = "requires local TCP sockets and public DNS"]
    async fn delay_spaces_each_host_on_the_wire_including_redirect_hops() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let hits: Hits = Default::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let recorded = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let recorded = recorded.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut chunk = [0; 1024];
                    while !head.ends_with(b"\r\n\r\n") && head.len() < 16384 {
                        match socket.read(&mut chunk).await {
                            Ok(n) if n > 0 => head.extend_from_slice(&chunk[..n]),
                            _ => return,
                        }
                    }
                    let head = String::from_utf8_lossy(&head);
                    let Some(url) = head.split(' ').nth(1).and_then(|u| Url::parse(u).ok()) else {
                        return;
                    };
                    let host = url.host_str().unwrap_or_default().to_string();
                    let path = url.path().to_string();
                    recorded
                        .lock()
                        .unwrap()
                        .push((host.clone(), path.clone(), Instant::now()));
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let reply = match two_host_html(&host, &path) {
                        Some(body) => format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        ),
                        None => "HTTP/1.1 301 Moved Permanently\r\nLocation: /a5\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".into(),
                    };
                    let _ = socket.write_all(reply.as_bytes()).await;
                });
            }
        });
        let delay = Duration::from_millis(300);
        let seed = format!("http://{SEED_HOST}/");
        let crawler = Crawler::new(
            &seed,
            CrawlConfig {
                fetch: FetchConfig {
                    proxy: Some(proxy),
                    ..Default::default()
                },
                delay,
                concurrency: 4,
                allow_external_links: true,
                ..Default::default()
            },
        )
        .unwrap();
        let started = Instant::now();
        let result = crawler.crawl(&seed, None).await;
        let elapsed = started.elapsed();
        assert_eq!(result.ok, 10, "{:?}", result.pages);
        let hits = hits.lock().unwrap().clone();
        let seed_hit = |path: &str| {
            hits.iter()
                .find(|(h, p, _)| h == SEED_HOST && p == path)
                .map(|(_, _, at)| *at)
                .unwrap()
        };
        let setup_gap = seed_hit("/") - seed_hit("/robots.txt");
        assert!(
            setup_gap + Duration::from_millis(30) >= delay,
            "the first page came {setup_gap:?} after robots.txt"
        );
        assert!(
            hits.iter()
                .any(|(h, path, _)| h == SEED_HOST && path == "/moved")
                && hits
                    .iter()
                    .any(|(h, path, _)| h == SEED_HOST && path == "/a5"),
            "{hits:?}"
        );
        assert_paced(&hits, delay, elapsed);
    }
}
