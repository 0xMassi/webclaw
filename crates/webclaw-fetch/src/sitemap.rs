/// Sitemap parsing and URL discovery.
///
/// Discovers URLs from a site's sitemaps using a 3-step process:
/// 1. Parse robots.txt for `Sitemap:` directives
/// 2. Try common sitemap paths as fallback
/// 3. Recursively resolve sitemap index files
///
/// All HTTP requests go through FetchClient to inherit TLS fingerprinting.
use std::collections::HashSet;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::Serialize;
use tracing::{debug, warn};

use crate::client::FetchClient;
use crate::error::FetchError;

/// Maximum depth when recursively fetching sitemap index files.
/// Prevents infinite loops from circular sitemap references.
const MAX_RECURSION_DEPTH: usize = 3;

/// Common sitemap paths to try when robots.txt doesn't list any.
const FALLBACK_SITEMAP_PATHS: &[&str] = &[
    "/sitemap.xml",
    "/sitemap_index.xml",
    "/wp-sitemap.xml",
    "/sitemap/sitemap-index.xml",
];

/// A single URL discovered from a sitemap.
#[derive(Debug, Clone, Serialize)]
pub struct SitemapEntry {
    pub url: String,
    pub last_modified: Option<String>,
    pub priority: Option<f64>,
    pub change_freq: Option<String>,
}

/// Discover all URLs from a site's sitemaps.
///
/// Discovery order:
/// 1. Fetch /robots.txt, parse `Sitemap:` directives
/// 2. Try common sitemap paths as fallback (skipping any already found)
/// 3. If sitemap index, recursively fetch child sitemaps
/// 4. Deduplicate by URL
///
/// Returns an empty vec (not an error) if no sitemaps are found.
pub async fn discover(
    client: &FetchClient,
    base_url: &str,
) -> Result<Vec<SitemapEntry>, FetchError> {
    let base = base_url.trim_end_matches('/');
    let mut sitemap_urls: Vec<String> = Vec::new();

    // Step 1: Try robots.txt
    let robots_url = format!("{base}/robots.txt");
    debug!(url = %robots_url, "fetching robots.txt");

    match client.fetch(&robots_url).await {
        Ok(result) if result.status == 200 => {
            let found = parse_robots_txt(&result.html);
            debug!(count = found.len(), "sitemap URLs from robots.txt");
            sitemap_urls.extend(found);
        }
        Ok(result) => {
            debug!(status = result.status, "robots.txt not found");
        }
        Err(e) => {
            debug!(error = %e, "failed to fetch robots.txt");
        }
    }

    // Step 2: Try common sitemap paths (skipping any already discovered via robots.txt)
    for path in FALLBACK_SITEMAP_PATHS {
        let candidate = format!("{base}{path}");
        if !sitemap_urls.iter().any(|u| u == &candidate) {
            sitemap_urls.push(candidate);
        }
    }

    // Step 3: Fetch and parse each sitemap, handling indexes recursively
    let mut seen_urls: HashSet<String> = HashSet::new();
    let mut entries: Vec<SitemapEntry> = Vec::new();

    fetch_sitemaps(client, &sitemap_urls, &mut entries, &mut seen_urls, 0).await;

    debug!(total = entries.len(), "sitemap discovery complete");
    Ok(entries)
}

/// Recursively fetch and parse sitemap URLs, handling both urlsets and indexes.
async fn fetch_sitemaps(
    client: &FetchClient,
    urls: &[String],
    entries: &mut Vec<SitemapEntry>,
    seen_urls: &mut HashSet<String>,
    depth: usize,
) {
    if depth > MAX_RECURSION_DEPTH {
        warn!(depth, "sitemap recursion limit reached, stopping");
        return;
    }

    for sitemap_url in urls {
        debug!(url = %sitemap_url, depth, "fetching sitemap");

        let xml = match client.fetch(sitemap_url).await {
            Ok(result) if result.status == 200 => result.html,
            Ok(result) => {
                debug!(url = %sitemap_url, status = result.status, "sitemap not found");
                continue;
            }
            Err(e) => {
                debug!(url = %sitemap_url, error = %e, "failed to fetch sitemap");
                continue;
            }
        };

        match detect_sitemap_type(&xml) {
            SitemapType::UrlSet => {
                let parsed = parse_urlset(&xml);
                for entry in parsed {
                    if seen_urls.insert(entry.url.clone()) {
                        entries.push(entry);
                    }
                }
            }
            SitemapType::Index => {
                let child_urls = parse_sitemap_index(&xml);
                debug!(count = child_urls.len(), "found child sitemaps in index");

                // Box the recursive call to avoid large future sizes
                Box::pin(fetch_sitemaps(
                    client,
                    &child_urls,
                    entries,
                    seen_urls,
                    depth + 1,
                ))
                .await;
            }
            SitemapType::Unknown => {
                debug!(url = %sitemap_url, "unrecognized sitemap format, skipping");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Pure parsing functions (no I/O, fully testable)
// ---------------------------------------------------------------------------

/// Extract `Sitemap:` directive URLs from robots.txt content.
pub fn parse_robots_txt(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            // Case-insensitive match for "Sitemap:" prefix
            if trimmed.len() > 8 && trimmed[..8].eq_ignore_ascii_case("sitemap:") {
                let url = trimmed[8..].trim();
                if !url.is_empty() {
                    return Some(url.to_string());
                }
            }
            None
        })
        .collect()
}

/// Parse a sitemap XML string. Handles both `<urlset>` and `<sitemapindex>`.
/// Returns entries from urlsets and recursion targets from indexes.
pub fn parse_sitemap_xml(xml: &str) -> Vec<SitemapEntry> {
    match detect_sitemap_type(xml) {
        SitemapType::UrlSet => parse_urlset(xml),
        SitemapType::Index => {
            // For the public parsing API, convert index <loc> entries into
            // SitemapEntry with just the URL. The async `discover` function
            // handles actual recursive fetching.
            parse_sitemap_index(xml)
                .into_iter()
                .map(|url| SitemapEntry {
                    url,
                    last_modified: None,
                    priority: None,
                    change_freq: None,
                })
                .collect()
        }
        SitemapType::Unknown => Vec::new(),
    }
}

#[derive(Debug, PartialEq)]
enum SitemapType {
    UrlSet,
    Index,
    Unknown,
}

/// Peek at the first element to determine if this is a urlset or sitemapindex.
fn detect_sitemap_type(xml: &str) -> SitemapType {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name = e.local_name();
                return match name.as_ref() {
                    b"urlset" => SitemapType::UrlSet,
                    b"sitemapindex" => SitemapType::Index,
                    _ => continue, // skip processing instructions, comments
                };
            }
            Ok(Event::Eof) => return SitemapType::Unknown,
            Err(_) => return SitemapType::Unknown,
            _ => continue,
        }
    }
}

/// Parse `<url>` entries from a `<urlset>` sitemap.
fn parse_urlset(xml: &str) -> Vec<SitemapEntry> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut entries = Vec::new();

    // State for current <url> element being parsed
    let mut in_url = false;
    let mut current_tag: Option<UrlTag> = None;
    let mut loc: Option<String> = None;
    let mut lastmod: Option<String> = None;
    let mut priority: Option<f64> = None;
    let mut changefreq: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"url" => {
                        in_url = true;
                        loc = None;
                        lastmod = None;
                        priority = None;
                        changefreq = None;
                    }
                    b"loc" if in_url => current_tag = Some(UrlTag::Loc),
                    b"lastmod" if in_url => current_tag = Some(UrlTag::LastMod),
                    b"priority" if in_url => current_tag = Some(UrlTag::Priority),
                    b"changefreq" if in_url => current_tag = Some(UrlTag::ChangeFreq),
                    _ => current_tag = None,
                }
            }
            Ok(Event::Text(ref e)) => {
                if let Some(ref tag) = current_tag
                    && let Ok(text) = e.unescape()
                {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        match tag {
                            UrlTag::Loc => loc = Some(text),
                            UrlTag::LastMod => lastmod = Some(text),
                            UrlTag::Priority => priority = text.parse().ok(),
                            UrlTag::ChangeFreq => changefreq = Some(text),
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.local_name();
                if name.as_ref() == b"url" && in_url {
                    if let Some(url) = loc.take() {
                        entries.push(SitemapEntry {
                            url,
                            last_modified: lastmod.take(),
                            priority: priority.take(),
                            change_freq: changefreq.take(),
                        });
                    }
                    in_url = false;
                }
                current_tag = None;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!(error = %e, "XML parse error in sitemap, returning partial results");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    entries
}

#[derive(Debug)]
enum UrlTag {
    Loc,
    LastMod,
    Priority,
    ChangeFreq,
}

/// Parse `<sitemap>` entries from a `<sitemapindex>`, returning child sitemap URLs.
fn parse_sitemap_index(xml: &str) -> Vec<String> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();
    let mut urls = Vec::new();

    let mut in_sitemap = false;
    let mut in_loc = false;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"sitemap" => in_sitemap = true,
                    b"loc" if in_sitemap => in_loc = true,
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) => {
                if in_loc && let Ok(text) = e.unescape() {
                    let text = text.trim().to_string();
                    if !text.is_empty() {
                        urls.push(text);
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.local_name();
                match name.as_ref() {
                    b"sitemap" => {
                        in_sitemap = false;
                        in_loc = false;
                    }
                    b"loc" => in_loc = false,
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                warn!(error = %e, "XML parse error in sitemap index, returning partial results");
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    urls
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_urlset() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://example.com/</loc>
    <lastmod>2026-01-15</lastmod>
    <changefreq>daily</changefreq>
    <priority>1.0</priority>
  </url>
  <url>
    <loc>https://example.com/about</loc>
    <lastmod>2026-01-10</lastmod>
    <changefreq>monthly</changefreq>
    <priority>0.8</priority>
  </url>
  <url>
    <loc>https://example.com/blog/post-1</loc>
  </url>
</urlset>"#;

        let entries = parse_urlset(xml);
        assert_eq!(entries.len(), 3);

        assert_eq!(entries[0].url, "https://example.com/");
        assert_eq!(entries[0].last_modified.as_deref(), Some("2026-01-15"));
        assert_eq!(entries[0].change_freq.as_deref(), Some("daily"));
        assert_eq!(entries[0].priority, Some(1.0));

        assert_eq!(entries[1].url, "https://example.com/about");
        assert_eq!(entries[1].priority, Some(0.8));

        assert_eq!(entries[2].url, "https://example.com/blog/post-1");
        assert_eq!(entries[2].last_modified, None);
        assert_eq!(entries[2].priority, None);
        assert_eq!(entries[2].change_freq, None);
    }

    #[test]
    fn test_parse_sitemap_index() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap>
    <loc>https://example.com/sitemap-posts.xml</loc>
    <lastmod>2026-03-01</lastmod>
  </sitemap>
  <sitemap>
    <loc>https://example.com/sitemap-pages.xml</loc>
  </sitemap>
</sitemapindex>"#;

        let urls = parse_sitemap_index(xml);
        assert_eq!(urls.len(), 2);
        assert_eq!(urls[0], "https://example.com/sitemap-posts.xml");
        assert_eq!(urls[1], "https://example.com/sitemap-pages.xml");
    }

    #[test]
    fn test_parse_sitemap_xml_dispatches_urlset() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/page</loc></url>
</urlset>"#;

        let entries = parse_sitemap_xml(xml);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://example.com/page");
    }

    #[test]
    fn test_parse_sitemap_xml_dispatches_index() {
        let xml = r#"<?xml version="1.0"?>
<sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <sitemap><loc>https://example.com/sitemap-1.xml</loc></sitemap>
</sitemapindex>"#;

        let entries = parse_sitemap_xml(xml);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].url, "https://example.com/sitemap-1.xml");
        // Index entries have no metadata when parsed through the public API
        assert_eq!(entries[0].priority, None);
    }

    #[test]
    fn test_parse_robots_txt() {
        let robots = "User-agent: *\n\
                       Disallow: /admin/\n\
                       \n\
                       Sitemap: https://example.com/sitemap.xml\n\
                       sitemap: https://example.com/sitemap-news.xml\n\
                       SITEMAP: https://example.com/sitemap-images.xml\n\
                       \n\
                       User-agent: Googlebot\n\
                       Allow: /\n";

        let urls = parse_robots_txt(robots);
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://example.com/sitemap.xml");
        assert_eq!(urls[1], "https://example.com/sitemap-news.xml");
        assert_eq!(urls[2], "https://example.com/sitemap-images.xml");
    }

    #[test]
    fn test_parse_robots_txt_empty_value() {
        // "Sitemap:" with no URL should be skipped
        let robots = "Sitemap:\nSitemap:   \nSitemap: https://example.com/s.xml\n";
        let urls = parse_robots_txt(robots);
        assert_eq!(urls.len(), 1);
        assert_eq!(urls[0], "https://example.com/s.xml");
    }

    #[test]
    fn test_deduplicate() {
        // parse_sitemap_xml deduplicates via the discover() path, but
        // we can verify that parsing the same URL twice produces entries
        // that the HashSet in discover() would collapse.
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/page</loc></url>
  <url><loc>https://example.com/page</loc></url>
  <url><loc>https://example.com/other</loc></url>
</urlset>"#;

        let entries = parse_urlset(xml);
        assert_eq!(entries.len(), 3, "parser returns all entries");

        // Simulate the dedup that discover() does
        let mut seen = HashSet::new();
        let deduped: Vec<_> = entries
            .into_iter()
            .filter(|e| seen.insert(e.url.clone()))
            .collect();
        assert_eq!(deduped.len(), 2, "dedup collapses duplicates");
    }

    #[test]
    fn test_empty_sitemap() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
</urlset>"#;

        let entries = parse_urlset(xml);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_malformed_xml() {
        let xml = "this is not xml at all <><><";
        let entries = parse_sitemap_xml(xml);
        assert!(entries.is_empty(), "malformed XML returns empty vec");
    }

    #[test]
    fn test_malformed_xml_partial() {
        // Partial XML that starts valid but breaks mid-stream
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url><loc>https://example.com/good</loc></url>
  <url><loc>broken
"#;
        let entries = parse_sitemap_xml(xml);
        // Should return at least the successfully parsed entry
        assert!(entries.len() >= 1);
        assert_eq!(entries[0].url, "https://example.com/good");
    }

    #[test]
    fn test_missing_loc() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <lastmod>2026-01-01</lastmod>
    <priority>0.5</priority>
  </url>
  <url>
    <loc>https://example.com/valid</loc>
  </url>
</urlset>"#;

        let entries = parse_urlset(xml);
        assert_eq!(entries.len(), 1, "entry without <loc> is skipped");
        assert_eq!(entries[0].url, "https://example.com/valid");
    }

    #[test]
    fn test_priority_parsing() {
        let xml = r#"<?xml version="1.0"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
  <url>
    <loc>https://example.com/high</loc>
    <priority>1.0</priority>
  </url>
  <url>
    <loc>https://example.com/mid</loc>
    <priority>0.5</priority>
  </url>
  <url>
    <loc>https://example.com/low</loc>
    <priority>0.1</priority>
  </url>
  <url>
    <loc>https://example.com/invalid</loc>
    <priority>not-a-number</priority>
  </url>
</urlset>"#;

        let entries = parse_urlset(xml);
        assert_eq!(entries.len(), 4);

        assert_eq!(entries[0].priority, Some(1.0));
        assert_eq!(entries[1].priority, Some(0.5));
        assert_eq!(entries[2].priority, Some(0.1));
        assert_eq!(entries[3].priority, None, "invalid priority parses as None");
    }

    #[test]
    fn test_detect_sitemap_type() {
        let urlset = r#"<?xml version="1.0"?><urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"></urlset>"#;
        assert_eq!(detect_sitemap_type(urlset), SitemapType::UrlSet);

        let index = r#"<?xml version="1.0"?><sitemapindex xmlns="http://www.sitemaps.org/schemas/sitemap/0.9"></sitemapindex>"#;
        assert_eq!(detect_sitemap_type(index), SitemapType::Index);

        assert_eq!(detect_sitemap_type("garbage"), SitemapType::Unknown);
        assert_eq!(detect_sitemap_type(""), SitemapType::Unknown);
    }

    #[test]
    fn test_fallback_paths_constant() {
        // Verify the constant has the expected paths
        assert!(FALLBACK_SITEMAP_PATHS.contains(&"/sitemap.xml"));
        assert!(FALLBACK_SITEMAP_PATHS.contains(&"/sitemap_index.xml"));
        assert!(FALLBACK_SITEMAP_PATHS.contains(&"/wp-sitemap.xml"));
        assert!(FALLBACK_SITEMAP_PATHS.contains(&"/sitemap/sitemap-index.xml"));
    }
}
