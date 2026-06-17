//! Layered URL discovery for the `map` command.
//!
//! `sitemap::discover` only finds URLs a site explicitly advertises in its
//! `sitemap.xml`. Plenty of sites have no sitemap (news.ycombinator.com), a
//! stale one, or a thin one that lists a handful of section roots. For those,
//! a sitemap-only map returns almost nothing.
//!
//! This module adds a second layer: when the sitemap yields fewer than a
//! threshold of URLs, run a *bounded* same-origin crawl and harvest every URL
//! it touches — fetched pages, the visited set, **and** the remaining frontier
//! (links queued but never fetched because the page cap was hit). That last
//! bucket is the gold: a 150-page crawl of a link-dense site surfaces several
//! thousand frontier URLs, turning a useless map into a real one.
//!
//! Strategy (layered, sitemap-first):
//! 1. Sitemaps via [`sitemap::discover`] — authoritative, carries metadata
//!    (lastmod / priority / changefreq).
//! 2. If sitemaps are thin (`< min_sitemap_urls`) and the fallback is enabled,
//!    a bounded crawl fills in the rest. Crawl-discovered URLs carry no
//!    metadata (`None` everywhere) since they come from link harvesting, not a
//!    sitemap.
//!
//! Sitemap entries always come first in the returned vec; crawl-discovered
//! URLs are appended, deduplicated against the sitemap set using the *same*
//! normalization the crawler uses ([`crawler::normalize`]) so map output stays
//! internally consistent.

use std::collections::HashSet;
use std::time::Duration;

use url::Url;

use crate::client::{FetchClient, FetchConfig};
use crate::crawler::{self, CrawlConfig, Crawler};
use crate::sitemap::{self, SitemapEntry};

/// Tuning knobs for [`discover_urls`].
#[derive(Debug, Clone)]
pub struct MapOptions {
    /// Hard cap on pages the fallback crawl will fetch. The crawl surfaces far
    /// more URLs than this via the unfetched frontier, so a small number still
    /// yields a large map while keeping the crawl fast and polite.
    pub max_crawl_pages: usize,
    /// How deep the fallback crawl follows links (1 = links off the seed only).
    pub crawl_depth: usize,
    /// Sitemap-URL count below which the crawl fallback kicks in. A site with a
    /// rich sitemap (≥ this many URLs) skips the crawl entirely.
    pub min_sitemap_urls: usize,
    /// Master switch for the crawl fallback. When `false`, behaves exactly like
    /// the old sitemap-only `discover`.
    pub crawl_fallback: bool,
    /// Optional cap on URLs returned. `None` (default) = uncapped: return every
    /// URL discovered (the crawl is already bounded by `max_crawl_pages`, so the
    /// uncapped set is the links harvested from the fetched pages). Set `Some(n)`
    /// to truncate.
    pub max_urls: Option<usize>,
}

impl Default for MapOptions {
    fn default() -> Self {
        Self {
            max_crawl_pages: 150,
            crawl_depth: 2,
            min_sitemap_urls: 200,
            crawl_fallback: true,
            max_urls: None,
        }
    }
}

/// Discover URLs for a site using the layered strategy described in the module
/// docs: sitemaps first, then a bounded crawl fallback when the sitemap is
/// thin.
///
/// Never errors — sitemap and crawl failures are swallowed and simply yield
/// fewer URLs (an empty vec in the worst case), matching `sitemap::discover`'s
/// "absence is not an error" contract.
pub async fn discover_urls(
    client: &FetchClient,
    base_url: &str,
    opts: &MapOptions,
) -> Vec<SitemapEntry> {
    // Layer 1: sitemaps.
    let mut entries = sitemap::discover(client, base_url)
        .await
        .unwrap_or_default();

    // Track normalized URLs we've already emitted, for cross-layer dedup.
    let mut seen: HashSet<String> = entries.iter().filter_map(normalize_str).collect();

    // Layer 2: bounded crawl fallback, only when the sitemap is thin.
    if !opts.crawl_fallback || entries.len() >= opts.min_sitemap_urls {
        return entries;
    }

    let Some(base_origin) = Url::parse(base_url).ok().map(|u| crawler::origin_key(&u)) else {
        // Unparseable base URL — nothing sensible to crawl against.
        return entries;
    };

    let config = CrawlConfig {
        fetch: FetchConfig::default(),
        max_depth: opts.crawl_depth,
        max_pages: opts.max_crawl_pages,
        // Politeness + scope: same-origin only (crawler default), modest delay.
        delay: Duration::from_millis(50),
        ..CrawlConfig::default()
    };

    let crawler = match Crawler::new(base_url, config) {
        Ok(c) => c,
        Err(_) => return entries,
    };

    let result = crawler.crawl(base_url, None).await;

    // Richest source first: every link harvested from each fetched page. A
    // directory/index page holds hundreds of same-origin links, and this set is
    // NOT bound by the crawler's internal frontier cap. Then the URLs the crawl
    // itself touched (fetched, visited, queued-but-unfetched frontier).
    let mut discovered: Vec<String> = Vec::new();
    for p in &result.pages {
        discovered.push(p.url.clone());
        if let Some(ex) = p.extraction.as_ref() {
            let page_base = Url::parse(&p.url).ok();
            for link in &ex.content.links {
                // Resolve relative/protocol-relative hrefs against the page URL
                // so the same-origin filter and dedup see absolute URLs.
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

    append_crawled(&mut entries, &mut seen, discovered, &base_origin);

    // Uncapped by default; only truncate if the caller set an explicit limit
    // (sitemap entries added first keep priority).
    if let Some(cap) = opts.max_urls {
        entries.truncate(cap);
    }
    entries
}

/// Normalize a raw URL string to the crawler's canonical form, returning `None`
/// if it doesn't parse.
fn normalize_url(raw: &str) -> Option<String> {
    Url::parse(raw).ok().map(|u| crawler::normalize(&u))
}

/// Normalize a [`SitemapEntry`]'s URL for the dedup set.
fn normalize_str(entry: &SitemapEntry) -> Option<String> {
    normalize_url(&entry.url)
}

/// Append crawl-discovered URLs to `entries`, skipping any that are off-origin,
/// unparseable, or already present (by normalized form).
///
/// Split out from [`discover_urls`] so the union/dedup/same-origin logic is
/// unit-testable without touching the network. Mutates `entries` and `seen` in
/// place; crawl URLs get empty metadata.
fn append_crawled(
    entries: &mut Vec<SitemapEntry>,
    seen: &mut HashSet<String>,
    discovered: impl IntoIterator<Item = String>,
    base_origin: &str,
) {
    for raw in discovered {
        let Ok(parsed) = Url::parse(&raw) else {
            continue;
        };
        // Same-origin filter: drop anything whose origin differs from the seed.
        if crawler::origin_key(&parsed) != base_origin {
            continue;
        }
        let norm = crawler::normalize(&parsed);
        if seen.insert(norm.clone()) {
            entries.push(SitemapEntry {
                url: norm,
                last_modified: None,
                priority: None,
                change_freq: None,
            });
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
        let mut seen: HashSet<String> = entries.iter().filter_map(normalize_str).collect();

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
        let mut seen: HashSet<String> = entries.iter().filter_map(normalize_str).collect();

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
    }
}
