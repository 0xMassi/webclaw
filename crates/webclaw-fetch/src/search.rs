//! Web search via Serper.dev (Google results) with optional content scraping.
//!
//! This is the self-hosted search path: the caller supplies their own
//! Serper.dev API key (free tier at serper.dev). The CLI, MCP server, and
//! OSS REST server all route through [`search`] so search works without the
//! hosted webclaw API.
//!
//! Serper returns a plain JSON API, so we hit it with a vanilla wreq client
//! (10s timeout) — no browser TLS fingerprinting needed. When `scrape` is
//! set, the top results are fetched through the caller's [`FetchClient`]
//! (which *does* carry the fingerprinting) and extracted to markdown.
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use tracing::warn;

use crate::client::FetchClient;
use crate::error::FetchError;

/// Serper.dev search endpoint.
const SERPER_URL: &str = "https://google.serper.dev/search";

/// Bound on the number of result pages scraped concurrently when
/// `scrape` is enabled. Keeps the fan-out from overwhelming the proxy
/// pool / remote hosts on a large result set.
const SCRAPE_CONCURRENCY: usize = 5;

/// Options controlling a search request.
#[derive(Debug, Clone)]
pub struct SearchOptions {
    /// Number of organic results to request (clamped to `1..=10`).
    pub num_results: usize,
    /// Country code for localization (Serper `gl`, e.g. `"us"`, `"gb"`).
    pub country: Option<String>,
    /// Language code for localization (Serper `hl`, e.g. `"en"`, `"it"`).
    pub lang: Option<String>,
    /// When true, fetch + extract the result pages and fill in `content`.
    pub scrape: bool,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            num_results: 5,
            country: None,
            lang: None,
            scrape: false,
        }
    }
}

/// A single organic search result. When `scrape` was requested and the
/// fetch succeeded, `content` holds the extracted markdown; otherwise it
/// is `None` (a per-result fetch failure never fails the whole search).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub link: String,
    pub snippet: String,
    pub position: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
}

/// Run a web search through Serper.dev.
///
/// `client`     — the caller's [`FetchClient`], used only when `opts.scrape`
///                is set (to fetch + extract the result pages).
/// `serper_key` — the caller's Serper.dev API key.
/// `query`      — the search query.
/// `opts`       — result count, localization, and whether to scrape.
///
/// Returns the organic results in Serper's order. With `scrape` enabled,
/// the top results are fetched concurrently (bounded) and their extracted
/// markdown is attached to `content`.
pub async fn search(
    client: &FetchClient,
    serper_key: &str,
    query: &str,
    opts: &SearchOptions,
) -> Result<Vec<SearchResult>, FetchError> {
    let num = opts.num_results.clamp(1, 10);

    let response = call_serper(
        serper_key,
        query,
        num,
        opts.country.as_deref(),
        opts.lang.as_deref(),
    )
    .await?;

    let mut results = parse_serper_organic(&response);

    if opts.scrape && !results.is_empty() {
        scrape_results(client, &mut results).await;
    }

    Ok(results)
}

/// POST the query to Serper.dev and return the raw JSON response.
///
/// Builds a plain wreq client (no browser emulation — Serper is a JSON
/// API, not a bot-protected page). Non-2xx responses are surfaced as a
/// [`FetchError::Build`] carrying the status and body so the caller can
/// show Serper's own error (bad key, quota exceeded, etc.).
async fn call_serper(
    api_key: &str,
    query: &str,
    num: usize,
    country: Option<&str>,
    lang: Option<&str>,
) -> Result<Value, FetchError> {
    let http = wreq::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| FetchError::Build(format!("failed to build serper client: {e}")))?;

    let mut body = json!({ "q": query, "num": num });
    if let Some(gl) = country {
        body["gl"] = json!(gl);
    }
    if let Some(hl) = lang {
        body["hl"] = json!(hl);
    }
    // Serialize ourselves rather than `.json()` — the wreq `json` feature
    // is not enabled in this crate and isn't worth pulling in for one call.
    let payload = serde_json::to_vec(&body)
        .map_err(|e| FetchError::Build(format!("serper request encode error: {e}")))?;

    let resp = http
        .post(SERPER_URL)
        .header("X-API-KEY", api_key)
        .header("Content-Type", "application/json")
        .body(payload)
        .send()
        .await?;

    let status = resp.status();
    if !status.is_success() {
        let code = status.as_u16();
        let text = resp.text().await.unwrap_or_default();
        return Err(FetchError::Build(format!("serper returned {code}: {text}")));
    }

    let text = resp
        .text()
        .await
        .map_err(|e| FetchError::BodyDecode(format!("serper response read error: {e}")))?;
    serde_json::from_str::<Value>(&text)
        .map_err(|e| FetchError::BodyDecode(format!("serper response parse error: {e}")))
}

/// Parse the `organic` array of a Serper response into [`SearchResult`]s.
///
/// Pure (no network), so it is unit-tested against a fixture. Entries
/// missing `title` or `link` are skipped; `snippet` defaults to empty.
/// `position` is 1-based over the kept entries.
pub fn parse_serper_organic(response: &Value) -> Vec<SearchResult> {
    let Some(organic) = response.get("organic").and_then(|v| v.as_array()) else {
        return Vec::new();
    };

    organic
        .iter()
        .filter_map(|item| {
            let title = item.get("title")?.as_str()?.to_string();
            let link = item.get("link")?.as_str()?.to_string();
            let snippet = item
                .get("snippet")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(SearchResult {
                title,
                link,
                snippet,
                // Filled in after collection so it tracks kept entries,
                // not the raw array index (which may include skips).
                position: 0,
                content: None,
            })
        })
        .enumerate()
        .map(|(i, mut r)| {
            r.position = i + 1;
            r
        })
        .collect()
}

/// Fetch + extract the result pages and attach markdown to `content`.
///
/// Bounded by [`SCRAPE_CONCURRENCY`]. A per-result fetch or extraction
/// failure leaves that result's `content` as `None` rather than failing
/// the whole search.
async fn scrape_results(client: &FetchClient, results: &mut [SearchResult]) {
    let sem = Arc::new(Semaphore::new(SCRAPE_CONCURRENCY));

    // Collect owned links first so the per-result futures don't borrow
    // `results`. That keeps the future captures free of the slice's
    // lifetime, which is what lets this compile inside the MCP `#[tool]`
    // macro's stricter `Send`/lifetime bounds.
    let links: Vec<String> = results.iter().map(|r| r.link.clone()).collect();

    let scrapes = links.into_iter().map(|link| {
        let sem = sem.clone();
        async move {
            // If the semaphore is closed (shutdown race), skip rather than panic.
            let _permit = match sem.acquire().await {
                Ok(p) => p,
                Err(_) => return None,
            };
            match client.fetch(&link).await {
                Ok(fetched) => match webclaw_core::extract(&fetched.html, Some(&fetched.url)) {
                    Ok(extraction) => Some(extraction.content.markdown),
                    Err(e) => {
                        warn!(url = %link, error = %e, "search: extraction failed");
                        None
                    }
                },
                Err(e) => {
                    warn!(url = %link, error = %e, "search: fetch failed");
                    None
                }
            }
        }
    });

    // `join_all` drives every scrape future concurrently and returns
    // results in input order; the semaphore caps how many fetches run at
    // once. Result set is tiny (≤10), so the all-at-once poll is fine.
    let contents = futures_util::future::join_all(scrapes).await;
    for (r, content) in results.iter_mut().zip(contents) {
        r.content = content;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        json!({
            "searchParameters": { "q": "rust async", "type": "search" },
            "organic": [
                {
                    "title": "Async Rust",
                    "link": "https://example.com/async",
                    "snippet": "Learn async in Rust.",
                    "position": 1
                },
                {
                    // snippet missing on purpose -> defaults to ""
                    "title": "Tokio",
                    "link": "https://tokio.rs"
                },
                {
                    // no link -> skipped, must not shift positions of the rest
                    "title": "No Link Here"
                }
            ]
        })
    }

    #[test]
    fn parses_organic_results() {
        let results = parse_serper_organic(&fixture());
        assert_eq!(results.len(), 2);

        assert_eq!(results[0].title, "Async Rust");
        assert_eq!(results[0].link, "https://example.com/async");
        assert_eq!(results[0].snippet, "Learn async in Rust.");
        assert_eq!(results[0].position, 1);
        assert!(results[0].content.is_none());

        // Missing snippet -> empty string, and position is 1-based over
        // kept entries (the link-less entry is dropped, not counted).
        assert_eq!(results[1].title, "Tokio");
        assert_eq!(results[1].snippet, "");
        assert_eq!(results[1].position, 2);
    }

    #[test]
    fn missing_organic_key_yields_empty() {
        assert!(parse_serper_organic(&json!({})).is_empty());
        assert!(parse_serper_organic(&json!({ "organic": "not-an-array" })).is_empty());
    }

    #[test]
    fn search_result_serializes_without_null_content() {
        let r = SearchResult {
            title: "T".into(),
            link: "https://e.com".into(),
            snippet: "s".into(),
            position: 1,
            content: None,
        };
        let v = serde_json::to_value(&r).unwrap();
        assert!(v.get("content").is_none(), "None content should be skipped");

        let r2 = SearchResult {
            content: Some("# md".into()),
            ..r
        };
        let v2 = serde_json::to_value(&r2).unwrap();
        assert_eq!(v2.get("content").and_then(|c| c.as_str()), Some("# md"));
    }

    #[test]
    fn default_options() {
        let o = SearchOptions::default();
        assert_eq!(o.num_results, 5);
        assert!(!o.scrape);
        assert!(o.country.is_none());
        assert!(o.lang.is_none());
    }
}
