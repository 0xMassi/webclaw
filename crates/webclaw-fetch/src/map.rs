//! Layered URL discovery for the `map` command.
//!
//! `sitemap::discover` only finds URLs a site explicitly advertises in its
//! `sitemap.xml`. Plenty of sites have no sitemap (news.ycombinator.com), a
//! stale one, or a thin one that lists a handful of section roots. For those,
//! a sitemap-only map returns almost nothing.
//!
//! Two more layers fill the gap:
//! 1. Sitemaps via [`sitemap::discover`]: authoritative, carries metadata
//!    (lastmod / priority / changefreq).
//! 2. Optionally, the public Common Crawl URL index ([`common_crawl_urls`]):
//!    every page of the site that Common Crawl captured in its latest crawl,
//!    linked or not.
//! 3. If those are thin (`< min_sitemap_urls`) and the fallback is enabled,
//!    a *bounded* same-origin crawl harvests every URL it touches: fetched
//!    pages, the visited set, and the frontier it queued but never fetched.
//!
//! URLs keep that order, each once, compared without scheme, `www.`,
//! fragment, trailing slash or tracking parameters (`utm_*`, `ref`,
//! `fbclid`, `gclid`), which index URLs also lose in the output. A crawl ([`Crawler`]) keeps a page's `www.`
//! and bare-host URLs apart; a map lists one of them. Like the crawl, the
//! fallback follows `http` links to the seed's host on the seed's scheme.
//! Only the sitemap layer carries metadata.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use url::Url;

use crate::client::{FetchClient, FetchConfig};
use crate::crawler::{self, CrawlConfig, Crawler};
use crate::error::FetchError;
use crate::sitemap::{self, SitemapEntry};

const COLLECTIONS_URL: &str = "https://index.commoncrawl.org/collinfo.json";
const INDEX_PAUSE: Duration = Duration::from_secs(1);
const INDEX_RETRY_PAUSES: [Duration; 2] = [Duration::from_secs(2), Duration::from_secs(5)];
// ponytail: fixed cap of index pages (about 15,000 captures each); expose it when a site needs more.
const MAX_INDEX_PAGES: usize = 10;

/// Tuning knobs for [`map_site`] and [`discover_urls`].
#[derive(Debug, Clone)]
pub struct MapOptions {
    /// Hard cap on pages the fallback crawl will fetch. The crawl surfaces far
    /// more URLs than this via the unfetched frontier, so a small number still
    /// yields a large map while keeping the crawl fast and polite.
    pub max_crawl_pages: usize,
    /// How deep the fallback crawl follows links (1 = links off the seed only).
    pub crawl_depth: usize,
    /// URL count below which the crawl fallback kicks in. A site whose
    /// sitemap and index lookups found at least this many skips the crawl.
    pub min_sitemap_urls: usize,
    /// Master switch for the crawl fallback. When `false`, behaves exactly like
    /// the old sitemap-only `discover`.
    pub crawl_fallback: bool,
    /// Optional cap on URLs returned. `None` (default) = uncapped: return every
    /// URL discovered (the crawl is already bounded by `max_crawl_pages`, so the
    /// uncapped set is the links harvested from the fetched pages). Set `Some(n)`
    /// to truncate.
    pub max_urls: Option<usize>,
    /// Read the site's sitemaps. Default `true`.
    pub sitemap: bool,
    /// Also list the URLs the Common Crawl index holds for the site, see
    /// [`common_crawl_urls`]. Default `false`.
    pub common_crawl: bool,
    /// Keep only Common Crawl captures of HTML pages that answered 200.
    /// Default `true`.
    pub html_only: bool,
    /// Keep only URLs whose path starts with this.
    pub path_prefix: Option<String>,
    /// Keep only URLs whose path matches one of these globs, as in
    /// [`CrawlConfig::include_patterns`]. Empty keeps all.
    pub include_patterns: Vec<String>,
    /// Drop URLs whose path matches one of these globs.
    pub exclude_patterns: Vec<String>,
    /// List URLs best match first for this question, judged by the words of
    /// their paths. Words most of the site's paths share count for little.
    pub query: Option<String>,
    /// Time allowed for the whole Common Crawl lookup. When it runs out, the
    /// URLs read so far are kept. Default 120 seconds.
    pub index_budget: Duration,
}

impl Default for MapOptions {
    fn default() -> Self {
        Self {
            max_crawl_pages: 150,
            crawl_depth: 2,
            min_sitemap_urls: 200,
            crawl_fallback: true,
            max_urls: None,
            sitemap: true,
            common_crawl: false,
            html_only: true,
            path_prefix: None,
            include_patterns: Vec::new(),
            exclude_patterns: Vec::new(),
            query: None,
            index_budget: Duration::from_secs(120),
        }
    }
}

impl MapOptions {
    /// Picks the URL sources from a list such as `"sitemap,cc"`, names split
    /// by `,` or `+`: `sitemap` reads the sitemaps, `cc` the Common Crawl
    /// index. A list without `sitemap` also turns the crawl fallback off.
    pub fn set_sources(&mut self, list: &str) -> Result<(), String> {
        let (mut sitemap, mut common_crawl) = (false, false);
        for name in list
            .split([',', '+'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            match name.to_ascii_lowercase().as_str() {
                "sitemap" => sitemap = true,
                "cc" => common_crawl = true,
                other => {
                    return Err(format!(
                        "unknown map source \"{other}\"; use sitemap, cc or both"
                    ));
                }
            }
        }
        if !sitemap && !common_crawl {
            return Err("name at least one map source: sitemap, cc".into());
        }
        self.sitemap = sitemap;
        self.common_crawl = common_crawl;
        self.crawl_fallback &= sitemap;
        Ok(())
    }
}

/// What [`map_site`] found.
#[derive(Debug)]
pub struct SiteMap {
    /// Sitemap URLs first, then Common Crawl URLs, then crawl-found URLs,
    /// each once, or best match first with [`MapOptions::query`].
    pub entries: Vec<SitemapEntry>,
    /// Why the Common Crawl lookup stopped early, when it was asked for and
    /// did. `entries` still holds the other sources' URLs and the index URLs
    /// read before.
    pub index_error: Option<String>,
}

/// Discover URLs for a site using the layered strategy described in the module
/// docs. Errors only on an invalid include or exclude glob; a source that
/// fails yields fewer URLs, and a failed Common Crawl lookup is reported in
/// [`SiteMap::index_error`].
pub async fn map_site(
    client: &FetchClient,
    base_url: &str,
    opts: &MapOptions,
) -> Result<SiteMap, FetchError> {
    for pattern in opts.include_patterns.iter().chain(&opts.exclude_patterns) {
        crawler::validate_glob(pattern)?;
    }
    let mut entries = if opts.sitemap {
        sitemap::discover(client, base_url)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let mut seen: HashSet<String> = entries.iter().filter_map(|e| site_key(&e.url)).collect();

    let listed = entries
        .iter()
        .filter(|e| Url::parse(&e.url).is_ok_and(|u| wanted(&u, opts)))
        .count();
    let cap = room(opts, listed);
    let mut index_error = None;
    if opts.common_crawl && cap != Some(0) {
        let found = index_urls(client, base_url, opts, &seen, cap).await;
        for url in found.urls {
            if site_key(&url).is_some_and(|key| seen.insert(key)) {
                entries.push(bare_entry(url));
            }
        }
        index_error = found.error;
    }

    if opts.crawl_fallback && entries.len() < opts.min_sitemap_urls {
        crawl_fallback(base_url, opts, &mut entries, &mut seen).await;
    }

    entries.retain(|e| Url::parse(&e.url).is_ok_and(|u| wanted(&u, opts)));
    if let Some(query) = &opts.query {
        rank(&mut entries, query);
    }
    if let Some(cap) = opts.max_urls {
        entries.truncate(cap);
    }
    Ok(SiteMap {
        entries,
        index_error,
    })
}

/// [`map_site`] without the error report: never errors, an invalid glob or a
/// failed source simply yields fewer URLs (an empty vec in the worst case),
/// matching `sitemap::discover`'s "absence is not an error" contract.
pub async fn discover_urls(
    client: &FetchClient,
    base_url: &str,
    opts: &MapOptions,
) -> Vec<SitemapEntry> {
    map_site(client, base_url, opts)
        .await
        .map(|site| site.entries)
        .unwrap_or_default()
}

async fn crawl_fallback(
    base_url: &str,
    opts: &MapOptions,
    entries: &mut Vec<SitemapEntry>,
    seen: &mut HashSet<String>,
) {
    let Some(base_origin) = Url::parse(base_url).ok().map(|u| crawler::origin_key(&u)) else {
        return;
    };
    let Ok(crawler) = Crawler::new(base_url, fallback_config(opts)) else {
        return;
    };
    let result = crawler.crawl(base_url, None).await;

    let mut discovered: Vec<String> = Vec::new();
    for p in &result.pages {
        discovered.push(p.url.clone());
        if let Some(ex) = p.extraction.as_ref() {
            let page_base = Url::parse(&p.url).ok();
            for link in &ex.content.links {
                let abs = match &page_base {
                    Some(b) => b.join(&link.href).ok(),
                    None => Url::parse(&link.href).ok(),
                };
                if let Some(u) = abs {
                    discovered.push(u.to_string());
                }
            }
        }
    }
    discovered.extend(result.visited);
    discovered.extend(result.remaining_frontier.into_iter().map(|(url, _)| url));

    append_crawled(entries, seen, discovered, &base_origin);
}

fn fallback_config(opts: &MapOptions) -> CrawlConfig {
    CrawlConfig {
        fetch: FetchConfig::default(),
        max_depth: opts.crawl_depth,
        max_pages: Some(opts.max_crawl_pages),
        delay: Duration::from_millis(50),
        path_prefix: opts.path_prefix.clone(),
        include_patterns: opts.include_patterns.clone(),
        exclude_patterns: opts.exclude_patterns.clone(),
        ..CrawlConfig::default()
    }
}

/// What [`common_crawl_urls`] read.
#[derive(Debug, Default)]
pub struct IndexLookup {
    /// The URLs read, in index order.
    pub urls: Vec<String>,
    /// Why the lookup stopped early, when it did. `urls` keeps what was read
    /// before.
    pub error: Option<String>,
}

/// Lists the URLs the latest Common Crawl index holds for `base_url`'s host,
/// with or without `www.` (other subdomains are other sites), each once.
///
/// Applies `opts`' path prefix (also narrowing the index query), include and
/// exclude globs, and [`MapOptions::html_only`]. Stops at
/// [`MapOptions::max_urls`] (unless a [`MapOptions::query`] will rank the
/// URLs first) or after ten index pages of about 15,000 captures each. Where
/// a page was captured over both schemes, the `https` URL is kept.
///
/// The index is a free public service and often slow: requests go one at a
/// time, a second apart, and bypass the client's response cache. An error
/// answer the busy index gives (400, 429 or 5xx), an answer cut short, or a
/// failed request is retried twice, after two and five seconds. One request
/// can take 20 seconds or more, so give `client` a generous
/// [`FetchConfig::total_timeout`]. The whole lookup stops after
/// [`MapOptions::index_budget`]. When the index cannot be read in time, the
/// result carries the URLs read until then and the reason; a site it holds
/// nothing for is an empty list.
pub async fn common_crawl_urls(
    client: &FetchClient,
    base_url: &str,
    opts: &MapOptions,
) -> IndexLookup {
    index_urls(client, base_url, opts, &HashSet::new(), room(opts, 0)).await
}

async fn index_urls(
    client: &FetchClient,
    base_url: &str,
    opts: &MapOptions,
    known: &HashSet<String>,
    cap: Option<usize>,
) -> IndexLookup {
    let Some(host) = Url::parse(base_url).ok().as_ref().and_then(bare_host) else {
        return IndexLookup {
            urls: Vec::new(),
            error: Some(format!("{base_url} is not a URL with a host")),
        };
    };
    let started = tokio::time::Instant::now();
    let budget = opts.index_budget;
    let (query, pages) = match tokio::time::timeout(budget, index_query(client, &host, opts)).await
    {
        Ok(Ok(found)) => found,
        Ok(Err(e)) => {
            return IndexLookup {
                urls: Vec::new(),
                error: Some(describe(e)),
            };
        }
        Err(_) => {
            return IndexLookup {
                urls: Vec::new(),
                error: Some(out_of_time(budget)),
            };
        }
    };
    gather(
        pages,
        |page| {
            let url = format!("{query}&page={page}");
            async move {
                tokio::time::sleep(INDEX_PAUSE).await;
                index_get(client, &url).await
            }
        },
        IndexUrls::new(host, opts, known, cap),
        started,
        budget,
    )
    .await
}

async fn index_query(
    client: &FetchClient,
    host: &str,
    opts: &MapOptions,
) -> Result<(Url, usize), FetchError> {
    let collections = index_get(client, COLLECTIONS_URL)
        .await?
        .unwrap_or_default();
    let api = latest_index(&collections).ok_or_else(|| {
        FetchError::BodyDecode("the Common Crawl collection list names no index".into())
    })?;
    let prefix = opts
        .path_prefix
        .as_deref()
        .filter(|p| p.starts_with('/'))
        .unwrap_or("/");
    let mut query = Url::parse(&api).map_err(|e| FetchError::InvalidUrl(format!("{api}: {e}")))?;
    query
        .query_pairs_mut()
        .append_pair("url", &format!("{host}{prefix}*"))
        .append_pair("output", "json")
        .append_pair("fl", "url,status,mime");
    if opts.html_only {
        query
            .query_pairs_mut()
            .append_pair("filter", "=status:200")
            .append_pair("filter", "=mime:text/html");
    }
    tokio::time::sleep(INDEX_PAUSE).await;
    let pages = index_get(client, &format!("{query}&showNumPages=true"))
        .await?
        .and_then(|body| serde_json::from_str::<serde_json::Value>(&body).ok())
        .and_then(|v| v["pages"].as_u64())
        .unwrap_or(1) as usize;
    Ok((query, pages))
}

async fn gather<F, Fut>(
    pages: usize,
    mut fetch: F,
    mut urls: IndexUrls<'_>,
    started: tokio::time::Instant,
    budget: Duration,
) -> IndexLookup
where
    F: FnMut(usize) -> Fut,
    Fut: std::future::Future<Output = Result<Option<String>, FetchError>>,
{
    let mut error = None;
    for page in 0..pages.min(MAX_INDEX_PAGES) {
        if urls.full() {
            break;
        }
        let left = budget.saturating_sub(started.elapsed());
        match tokio::time::timeout(left, fetch(page)).await {
            Ok(Ok(Some(body))) => urls.add_lines(&body),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => {
                error = Some(describe(e));
                break;
            }
            Err(_) => {
                error = Some(out_of_time(budget));
                break;
            }
        }
    }
    IndexLookup {
        urls: urls.list,
        error,
    }
}

fn room(opts: &MapOptions, listed: usize) -> Option<usize> {
    match opts.query {
        Some(_) => None,
        None => opts.max_urls.map(|cap| cap.saturating_sub(listed)),
    }
}

fn out_of_time(budget: Duration) -> String {
    format!("the index did not finish within {budget:?}")
}

fn describe(error: FetchError) -> String {
    match error {
        FetchError::UpstreamStatus(status) => format!("the index answered HTTP {status}"),
        FetchError::BodyDecode(reason) => reason,
        other => other.to_string(),
    }
}

struct IndexUrls<'a> {
    host: String,
    opts: &'a MapOptions,
    known: &'a HashSet<String>,
    cap: Option<usize>,
    list: Vec<String>,
    at: HashMap<String, usize>,
}

impl<'a> IndexUrls<'a> {
    fn new(
        host: String,
        opts: &'a MapOptions,
        known: &'a HashSet<String>,
        cap: Option<usize>,
    ) -> Self {
        Self {
            host,
            opts,
            known,
            cap,
            list: Vec::new(),
            at: HashMap::new(),
        }
    }

    fn full(&self) -> bool {
        self.cap.is_some_and(|cap| self.list.len() >= cap)
    }

    fn add_lines(&mut self, body: &str) {
        for line in body.lines() {
            if self.full() {
                return;
            }
            let Some(raw) = serde_json::from_str::<serde_json::Value>(line)
                .ok()
                .and_then(|v| v["url"].as_str().map(str::to_string))
            else {
                continue;
            };
            self.add(&raw);
        }
    }

    fn add(&mut self, raw: &str) {
        let Ok(mut url) = Url::parse(raw) else {
            return;
        };
        if !matches!(url.scheme(), "http" | "https")
            || bare_host(&url).as_deref() != Some(self.host.as_str())
            || !wanted(&url, self.opts)
        {
            return;
        }
        url.set_fragment(None);
        let url = without_tracking(&url);
        let Some(key) = site_key(url.as_str()) else {
            return;
        };
        if self.known.contains(&key) {
            return;
        }
        match self.at.get(&key) {
            Some(&i) => {
                if url.scheme() == "https" && self.list[i].starts_with("http:") {
                    self.list[i] = url.into();
                }
            }
            None => {
                self.at.insert(key, self.list.len());
                self.list.push(url.into());
            }
        }
    }
}

async fn index_get(client: &FetchClient, url: &str) -> Result<Option<String>, FetchError> {
    let mut outcome = client.fetch_raw(url).await;
    for pause in INDEX_RETRY_PAUSES {
        if matches!(&outcome, Ok((status, body)) if !busy(*status, body)) {
            break;
        }
        tokio::time::sleep(pause).await;
        outcome = client.fetch_raw(url).await;
    }
    let (status, body) = outcome?;
    let body = String::from_utf8_lossy(&body).into_owned();
    match status {
        404 => Ok(None),
        200..=299 if whole(&body) => Ok(Some(body)),
        200..=299 => Err(FetchError::BodyDecode(
            "the Common Crawl index cut its answer short".into(),
        )),
        status => Err(FetchError::UpstreamStatus(status)),
    }
}

fn busy(status: u16, body: &[u8]) -> bool {
    matches!(status, 400 | 429 | 500..)
        || ((200..300).contains(&status) && !whole(&String::from_utf8_lossy(body)))
}

fn whole(body: &str) -> bool {
    body.trim_end().ends_with(['}', ']'])
}

fn latest_index(collections: &str) -> Option<String> {
    let list: Vec<serde_json::Value> = serde_json::from_str(collections).ok()?;
    list.first()?["cdx-api"].as_str().map(str::to_string)
}

fn bare_host(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    Some(
        host.strip_prefix("www.")
            .map(str::to_string)
            .unwrap_or(host),
    )
}

fn wanted(url: &Url, opts: &MapOptions) -> bool {
    crawler::path_allowed(
        url.path(),
        opts.path_prefix.as_deref(),
        &opts.include_patterns,
        &opts.exclude_patterns,
    )
}

fn rank(entries: &mut Vec<SitemapEntry>, query: &str) {
    let Some(question) = webclaw_core::Question::new(query) else {
        return;
    };
    let anchors: Vec<webclaw_core::Anchor> = entries
        .iter()
        .map(|e| webclaw_core::Anchor {
            href: e.url.clone(),
            text: String::new(),
            context: String::new(),
        })
        .collect();
    let paths = entries
        .iter()
        .map(|e| {
            Url::parse(&e.url)
                .map(|u| u.path().to_string())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let (_, scores) =
        webclaw_core::Relevance::new(&question).rate(&question.read(&paths, &anchors));
    let mut scored: Vec<(f64, SitemapEntry)> = scores.into_iter().zip(entries.drain(..)).collect();
    scored.sort_by(|a, b| b.0.total_cmp(&a.0));
    entries.extend(scored.into_iter().map(|(_, e)| e));
}

fn bare_entry(url: String) -> SitemapEntry {
    SitemapEntry {
        url,
        last_modified: None,
        priority: None,
        change_freq: None,
    }
}

fn site_key(raw: &str) -> Option<String> {
    let norm = crawler::normalize(&without_tracking(&Url::parse(raw).ok()?));
    let rest = norm.split_once("://")?.1;
    Some(rest.strip_prefix("www.").unwrap_or(rest).to_string())
}

fn without_tracking(url: &Url) -> Url {
    let tracking = |key: &str| key.starts_with("utm_") || matches!(key, "ref" | "fbclid" | "gclid");
    let mut clean = url.clone();
    if !url.query_pairs().any(|(key, _)| tracking(&key)) {
        return clean;
    }
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(key, _)| !tracking(key))
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect();
    if kept.is_empty() {
        clean.set_query(None);
    } else {
        clean.query_pairs_mut().clear().extend_pairs(kept);
    }
    clean
}

fn append_crawled(
    entries: &mut Vec<SitemapEntry>,
    seen: &mut HashSet<String>,
    discovered: impl IntoIterator<Item = String>,
    base_origin: &str,
) {
    let Ok(seed) = Url::parse(base_origin) else {
        return;
    };
    let scope = crawler::origin_key(&crawler::as_https(&seed));
    for raw in discovered {
        let Ok(mut parsed) = Url::parse(&raw) else {
            continue;
        };
        if !matches!(parsed.scheme(), "http" | "https")
            || crawler::origin_key(&crawler::as_https(&parsed)) != scope
        {
            continue;
        }
        let _ = parsed.set_scheme(seed.scheme());
        let norm = crawler::normalize(&parsed);
        if site_key(&norm).is_some_and(|key| seen.insert(key)) {
            entries.push(bare_entry(norm));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str) -> SitemapEntry {
        SitemapEntry {
            url: url.to_string(),
            last_modified: None,
            priority: None,
            change_freq: None,
        }
    }

    fn origin_of(url: &str) -> String {
        crawler::origin_key(&Url::parse(url).unwrap())
    }

    #[test]
    fn append_adds_new_same_origin_urls() {
        let mut entries = vec![entry("https://example.com/")];
        let mut seen: HashSet<String> = entries.iter().filter_map(|e| site_key(&e.url)).collect();

        append_crawled(
            &mut entries,
            &mut seen,
            vec![
                "https://example.com/about".to_string(),
                "https://example.com/contact".to_string(),
            ],
            &origin_of("https://example.com"),
        );

        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://example.com/",
                "https://example.com/about",
                "https://example.com/contact",
            ]
        );
    }

    #[test]
    fn append_dedups_against_sitemap_and_self() {
        let mut entries = vec![entry("https://example.com/about")];
        let mut seen: HashSet<String> = entries.iter().filter_map(|e| site_key(&e.url)).collect();

        append_crawled(
            &mut entries,
            &mut seen,
            vec![
                // Same as sitemap entry (trailing slash normalizes away).
                "https://example.com/about/".to_string(),
                // Fragment + duplicate -> only one new entry survives.
                "https://example.com/new#frag".to_string(),
                "https://example.com/new".to_string(),
            ],
            &origin_of("https://example.com"),
        );

        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/about", "https://example.com/new"]
        );
    }

    #[test]
    fn append_filters_off_origin() {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        append_crawled(
            &mut entries,
            &mut seen,
            vec![
                "https://example.com/keep".to_string(),
                "https://evil.com/drop".to_string(),
                "https://sub.example.com/drop".to_string(), // different origin
                "ftp://example.com/drop".to_string(),       // unparseable as http origin match
            ],
            &origin_of("https://example.com"),
        );

        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(urls, vec!["https://example.com/keep"]);
    }

    #[test]
    fn append_follows_http_links_on_the_seed_scheme_like_the_crawl() {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        append_crawled(
            &mut entries,
            &mut seen,
            vec![
                "http://example.com/x".to_string(),
                "http://www.example.com/y".to_string(),
                "https://example.com/x/".to_string(),
            ],
            &origin_of("https://example.com"),
        );

        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(
            urls,
            vec!["https://example.com/x", "https://www.example.com/y"]
        );
    }

    #[test]
    fn append_treats_www_as_same_origin() {
        // origin_key strips a leading `www.`, so www and apex collapse.
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        append_crawled(
            &mut entries,
            &mut seen,
            vec!["https://www.example.com/page".to_string()],
            &origin_of("https://example.com"),
        );

        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn crawl_urls_carry_no_metadata() {
        let mut entries = Vec::new();
        let mut seen = HashSet::new();

        append_crawled(
            &mut entries,
            &mut seen,
            vec!["https://example.com/x".to_string()],
            &origin_of("https://example.com"),
        );

        assert_eq!(entries.len(), 1);
        assert!(entries[0].last_modified.is_none());
        assert!(entries[0].priority.is_none());
        assert!(entries[0].change_freq.is_none());
    }

    #[test]
    fn map_options_defaults() {
        let o = MapOptions::default();
        assert_eq!(o.max_crawl_pages, 150);
        assert_eq!(o.crawl_depth, 2);
        assert_eq!(o.min_sitemap_urls, 200);
        assert!(o.crawl_fallback);
        assert!(o.sitemap);
        assert!(!o.common_crawl);
        assert!(o.html_only);
    }

    #[test]
    fn sources_parse_from_a_list() {
        let mut o = MapOptions::default();
        o.set_sources("sitemap,cc").unwrap();
        assert!(o.sitemap && o.common_crawl && o.crawl_fallback);

        let mut o = MapOptions::default();
        o.set_sources(" CC ").unwrap();
        assert!(!o.sitemap && o.common_crawl && !o.crawl_fallback);

        let mut o = MapOptions {
            crawl_fallback: false,
            ..MapOptions::default()
        };
        o.set_sources("sitemap+cc").unwrap();
        assert!(!o.crawl_fallback);

        assert!(MapOptions::default().set_sources("sitemap,web").is_err());
        assert!(MapOptions::default().set_sources(" , ").is_err());
    }

    static NO_URLS: std::sync::LazyLock<HashSet<String>> = std::sync::LazyLock::new(HashSet::new);

    fn fresh(opts: &MapOptions) -> IndexUrls<'_> {
        IndexUrls::new("example.com".into(), opts, &NO_URLS, room(opts, 0))
    }

    fn index_lines(urls: &[&str]) -> String {
        urls.iter()
            .map(|u| {
                format!("{{\"url\": \"{u}\", \"status\": \"200\", \"mime\": \"text/html\"}}\n")
            })
            .collect()
    }

    #[test]
    fn index_urls_stay_on_the_host_and_appear_once() {
        let opts = MapOptions::default();
        let mut urls = fresh(&opts);
        urls.add_lines(&index_lines(&[
            "http://example.com/a/",
            "https://www.example.com/a",
            "https://example.com/a#top",
            "https://example.com/b?x=1",
            "https://docs.example.com/c",
            "https://other.org/d",
            "ftp://example.com/e",
        ]));
        urls.add_lines("{\"message\": \"Page 1 invalid\"}\nnot json\n");
        assert_eq!(
            urls.list,
            vec!["https://www.example.com/a", "https://example.com/b?x=1"]
        );
    }

    #[test]
    fn index_urls_follow_the_path_rules_and_the_cap() {
        let opts = MapOptions {
            path_prefix: Some("/docs".into()),
            include_patterns: vec!["/docs/**".into()],
            exclude_patterns: vec!["/docs/old/*".into()],
            max_urls: Some(2),
            ..MapOptions::default()
        };
        let mut urls = fresh(&opts);
        urls.add_lines(&index_lines(&[
            "https://example.com/blog/x",
            "https://example.com/docs/old/y",
            "https://example.com/docs/a",
            "https://example.com/docs/b",
            "https://example.com/docs/c",
        ]));
        assert!(urls.full());
        assert_eq!(
            urls.list,
            vec!["https://example.com/docs/a", "https://example.com/docs/b"]
        );
    }

    #[test]
    fn a_query_reads_past_the_cap_so_ranking_sees_every_url() {
        let opts = MapOptions {
            max_urls: Some(1),
            query: Some("install".into()),
            ..MapOptions::default()
        };
        let mut urls = fresh(&opts);
        urls.add_lines(&index_lines(&[
            "https://example.com/a",
            "https://example.com/install",
        ]));
        assert!(!urls.full());
        assert_eq!(urls.list.len(), 2);
    }

    #[test]
    fn answers_cut_short_count_as_busy() {
        assert!(!busy(200, b"{\"url\": \"https://a.com/\"}\n"));
        assert!(!busy(200, b"[{\"id\": \"x\"}]"));
        assert!(busy(
            200,
            b"{\"url\": \"https://a.com/\"}\n{\"url\": \"https://a.c"
        ));
        assert!(busy(200, b""));
        assert!(busy(503, b"<html>busy</html>"));
        assert!(busy(429, b""));
        assert!(!busy(404, b"{\"message\": \"No Captures found\"}"));
        assert!(busy(400, b"{\"message\": \"Page 1 invalid\"}"));
        assert!(!busy(403, b"forbidden"));
    }

    #[tokio::test]
    async fn a_page_without_matching_captures_does_not_end_the_paging() {
        let opts = MapOptions::default();
        let found = gather(
            3,
            |page| async move {
                Ok(match page {
                    0 => Some(index_lines(&["https://example.com/a"])),
                    1 => None,
                    _ => Some(index_lines(&["https://example.com/c"])),
                })
            },
            fresh(&opts),
            tokio::time::Instant::now(),
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(
            found.urls,
            vec!["https://example.com/a", "https://example.com/c"]
        );
        assert!(found.error.is_none());
    }

    #[tokio::test]
    async fn urls_read_before_a_failed_page_are_kept() {
        let opts = MapOptions::default();
        let found = gather(
            3,
            |page| async move {
                match page {
                    0 => Ok(Some(index_lines(&["https://example.com/a"]))),
                    _ => Err(FetchError::UpstreamStatus(504)),
                }
            },
            fresh(&opts),
            tokio::time::Instant::now(),
            Duration::from_secs(60),
        )
        .await;
        assert_eq!(found.urls, vec!["https://example.com/a"]);
        assert_eq!(found.error.as_deref(), Some("the index answered HTTP 504"));
    }

    #[tokio::test]
    async fn the_lookup_stops_at_its_deadline_with_what_it_read() {
        let opts = MapOptions::default();
        let found = gather(
            3,
            |page| async move {
                if page > 0 {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
                Ok(Some(index_lines(&["https://example.com/a"])))
            },
            fresh(&opts),
            tokio::time::Instant::now(),
            Duration::from_millis(200),
        )
        .await;
        assert_eq!(found.urls, vec!["https://example.com/a"]);
        assert_eq!(
            found.error.as_deref(),
            Some("the index did not finish within 200ms")
        );
    }

    #[test]
    fn sitemap_urls_do_not_use_up_the_index_cap() {
        let opts = MapOptions {
            max_urls: Some(3),
            ..MapOptions::default()
        };
        let known: HashSet<String> = site_key("https://example.com/a").into_iter().collect();
        let mut urls = IndexUrls::new("example.com".into(), &opts, &known, room(&opts, 2));
        urls.add_lines(&index_lines(&[
            "https://example.com/a",
            "https://example.com/b",
        ]));
        assert_eq!(urls.list, vec!["https://example.com/b"]);
        assert!(urls.full());
    }

    #[test]
    fn a_full_sitemap_leaves_no_room_for_the_index_unless_a_query_ranks() {
        let capped = MapOptions {
            max_urls: Some(2),
            ..MapOptions::default()
        };
        assert_eq!(room(&capped, 2), Some(0));
        assert_eq!(room(&capped, 1), Some(1));
        let ranked = MapOptions {
            query: Some("install".into()),
            ..capped
        };
        assert_eq!(room(&ranked, 5), None);
        assert_eq!(room(&MapOptions::default(), 5), None);
    }

    #[test]
    fn the_fallback_crawl_follows_the_map_path_rules() {
        let opts = MapOptions {
            path_prefix: Some("/docs".into()),
            include_patterns: vec!["/docs/**".into()],
            exclude_patterns: vec!["/docs/old/*".into()],
            ..MapOptions::default()
        };
        let config = fallback_config(&opts);
        assert_eq!(config.path_prefix.as_deref(), Some("/docs"));
        assert_eq!(config.include_patterns, vec!["/docs/**"]);
        assert_eq!(config.exclude_patterns, vec!["/docs/old/*"]);
        assert_eq!(config.max_pages, Some(150));
    }

    #[test]
    fn tracking_parameters_do_not_make_a_new_url() {
        assert_eq!(
            site_key("https://example.com/p?utm_source=x&id=2&ref=feed"),
            site_key("https://example.com/p?id=2")
        );
        assert_eq!(
            site_key("https://example.com/p?fbclid=1&gclid=2"),
            site_key("https://example.com/p")
        );
        assert_ne!(
            site_key("https://example.com/p?page=2"),
            site_key("https://example.com/p")
        );

        let opts = MapOptions::default();
        let mut urls = fresh(&opts);
        urls.add_lines(&index_lines(&[
            "https://example.com/a?utm_source=news",
            "https://example.com/a",
            "https://example.com/b?gclid=1&id=3",
        ]));
        assert_eq!(
            urls.list,
            vec!["https://example.com/a", "https://example.com/b?id=3"]
        );
    }

    #[test]
    fn the_latest_index_is_the_first_collection() {
        let list = r#"[{"id": "CC-MAIN-2026-39", "cdx-api": "https://index.commoncrawl.org/CC-MAIN-2026-39-index"},
                       {"id": "CC-MAIN-2026-34", "cdx-api": "https://index.commoncrawl.org/CC-MAIN-2026-34-index"}]"#;
        assert_eq!(
            latest_index(list).as_deref(),
            Some("https://index.commoncrawl.org/CC-MAIN-2026-39-index")
        );
        assert_eq!(latest_index("[]"), None);
        assert_eq!(latest_index("<html>busy</html>"), None);
    }

    #[test]
    fn a_query_ranks_urls_by_their_path_words() {
        let mut entries: Vec<SitemapEntry> = [
            "https://docs.python.org/3/library/os.html",
            "https://docs.python.org/3/library/asyncio-task.html",
            "https://docs.python.org/3/tutorial/index.html",
            "https://docs.python.org/3/library/asyncio.html",
        ]
        .into_iter()
        .map(entry)
        .collect();
        rank(&mut entries, "how do I cancel an asyncio task");
        let urls: Vec<&str> = entries.iter().map(|e| e.url.as_str()).collect();
        assert_eq!(
            urls,
            vec![
                "https://docs.python.org/3/library/asyncio-task.html",
                "https://docs.python.org/3/library/asyncio.html",
                "https://docs.python.org/3/library/os.html",
                "https://docs.python.org/3/tutorial/index.html",
            ]
        );
    }

    #[test]
    fn a_query_without_words_keeps_the_order() {
        let mut entries = vec![
            entry("https://example.com/b"),
            entry("https://example.com/a"),
        ];
        rank(&mut entries, "?!");
        assert_eq!(entries[0].url, "https://example.com/b");
    }

    #[test]
    fn site_keys_ignore_scheme_www_slash_and_fragment() {
        assert_eq!(
            site_key("http://www.Example.com/a/#x"),
            site_key("https://example.com/a")
        );
        assert_ne!(
            site_key("https://example.com/a?x=1"),
            site_key("https://example.com/a")
        );
    }

    #[tokio::test]
    #[ignore = "reads the live Common Crawl index"]
    async fn common_crawl_lists_a_real_site() {
        let client = FetchClient::new(FetchConfig {
            total_timeout: Some(Duration::from_secs(90)),
            timeout: Duration::from_secs(45),
            ..FetchConfig::default()
        })
        .unwrap();
        let found = common_crawl_urls(
            &client,
            "https://quotes.toscrape.com/",
            &MapOptions::default(),
        )
        .await;
        assert_eq!(found.error, None);
        let urls = found.urls;
        assert!(urls.len() >= 5, "{urls:?}");
        assert!(urls.iter().all(|u| u.contains("quotes.toscrape.com/")));
    }
}
