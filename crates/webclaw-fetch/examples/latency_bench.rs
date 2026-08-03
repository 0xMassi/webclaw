//! Fetch-path latency benchmark over a real URL list.
//!
//! Separates network time from CPU time so it is clear which one to attack:
//! `FetchClient::fetch` (DNS + TCP + TLS + TTFB + body) is reported apart from
//! `webclaw_core::extract_with_options` (pure parse + extraction).
//!
//! The URL list is supplied at runtime and never committed — production URLs
//! are customer data and this repo is public.
//!
//! ```text
//! BENCH_URLS=/path/to/urls.txt \
//! BENCH_CONCURRENCY=8 \
//! BENCH_LIMIT=200 \
//!   cargo run --release -p webclaw-fetch --example latency_bench
//! ```
//!
//! Input: one URL per line. Lines may carry extra `|`-separated columns (the
//! first field is used), so a DB export can be fed in unchanged. `#` comments
//! and blank lines are skipped.
//!
//! Writes JSONL to stdout (one object per URL) and a percentile summary to
//! stderr, so `> runs/today.jsonl` keeps the raw data while the summary stays
//! on the terminal.

use std::sync::Arc;
use std::time::{Duration, Instant};

use webclaw_fetch::{BrowserProfile, FetchClient, FetchConfig};

/// One measured URL.
struct Row {
    url: String,
    status: Option<u16>,
    fetch_ms: u128,
    extract_ms: u128,
    bytes: usize,
    words: usize,
    error: Option<String>,
}

impl Row {
    fn to_json(&self) -> String {
        // Hand-rolled so the example pulls in no serde_json dev-dependency.
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            r#"{{"url":"{}","status":{},"fetch_ms":{},"extract_ms":{},"bytes":{},"words":{},"error":{}}}"#,
            esc(&self.url),
            self.status.map_or("null".into(), |s| s.to_string()),
            self.fetch_ms,
            self.extract_ms,
            self.bytes,
            self.words,
            self.error
                .as_ref()
                .map_or("null".to_string(), |e| format!("\"{}\"", esc(e))),
        )
    }
}

fn env_usize(key: &str, default: usize) -> usize {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    // Nearest-rank; index is clamped so p=1.0 lands on the last element.
    let idx = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn summarize(label: &str, mut v: Vec<u128>) {
    if v.is_empty() {
        eprintln!("{label:<12} (no samples)");
        return;
    }
    v.sort_unstable();
    let sum: u128 = v.iter().sum();
    eprintln!(
        "{label:<12} n={:<5} mean={:<7} p50={:<7} p90={:<7} p99={:<7} max={}",
        v.len(),
        sum / v.len() as u128,
        percentile(&v, 0.50),
        percentile(&v, 0.90),
        percentile(&v, 0.99),
        v[v.len() - 1],
    );
}

#[tokio::main]
async fn main() {
    let path = std::env::var("BENCH_URLS")
        .expect("set BENCH_URLS to a file of URLs (one per line, extra |-columns ignored)");
    let concurrency = env_usize("BENCH_CONCURRENCY", 8);
    let limit = env_usize("BENCH_LIMIT", usize::MAX);
    let timeout_secs = env_usize("BENCH_TIMEOUT_SECS", 12) as u64;

    let raw = std::fs::read_to_string(&path).expect("read BENCH_URLS file");
    let urls: Vec<String> = raw
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split('|').next())
        .filter(|u| u.starts_with("http"))
        .map(str::to_string)
        .take(limit)
        .collect();

    eprintln!(
        "urls={} concurrency={} timeout={}s",
        urls.len(),
        concurrency,
        timeout_secs
    );

    let client = Arc::new(
        FetchClient::new(FetchConfig {
            browser: BrowserProfile::Chrome,
            proxy: std::env::var("WEBCLAW_PROXY").ok(),
            timeout: Duration::from_secs(timeout_secs),
            ..Default::default()
        })
        .expect("build client"),
    );

    let started = Instant::now();
    let sem = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let mut tasks = Vec::with_capacity(urls.len());

    for url in urls {
        let client = Arc::clone(&client);
        let sem = Arc::clone(&sem);
        tasks.push(tokio::spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore open");

            let t0 = Instant::now();
            let fetched = client.fetch(&url).await;
            let fetch_ms = t0.elapsed().as_millis();

            match fetched {
                Ok(r) => {
                    let bytes = r.html.len();
                    let t1 = Instant::now();
                    let extraction = webclaw_core::extract_with_options(
                        &r.html,
                        Some(&r.url),
                        &webclaw_core::ExtractionOptions::default(),
                    );
                    let extract_ms = t1.elapsed().as_millis();
                    let words = extraction
                        .as_ref()
                        .map(|e| e.content.plain_text.split_whitespace().count())
                        .unwrap_or(0);
                    Row {
                        url,
                        status: Some(r.status),
                        fetch_ms,
                        extract_ms,
                        bytes,
                        words,
                        error: extraction.err().map(|e| e.to_string()),
                    }
                }
                Err(e) => Row {
                    url,
                    status: None,
                    fetch_ms,
                    extract_ms: 0,
                    bytes: 0,
                    words: 0,
                    error: Some(e.to_string()),
                },
            }
        }));
    }

    let mut rows = Vec::with_capacity(tasks.len());
    for t in tasks {
        match t.await {
            Ok(r) => rows.push(r),
            Err(e) => eprintln!("task panicked: {e}"),
        }
    }
    let wall = started.elapsed();

    for r in &rows {
        println!("{}", r.to_json());
    }

    let ok: Vec<&Row> = rows.iter().filter(|r| r.error.is_none()).collect();
    let failed = rows.len() - ok.len();

    eprintln!("\n=== latency (ms) ===");
    summarize("fetch", ok.iter().map(|r| r.fetch_ms).collect());
    summarize("extract", ok.iter().map(|r| r.extract_ms).collect());
    summarize(
        "total",
        ok.iter().map(|r| r.fetch_ms + r.extract_ms).collect(),
    );

    let fetch_sum: u128 = ok.iter().map(|r| r.fetch_ms).sum();
    let extract_sum: u128 = ok.iter().map(|r| r.extract_ms).sum();
    let cpu_share = if fetch_sum + extract_sum > 0 {
        100.0 * extract_sum as f64 / (fetch_sum + extract_sum) as f64
    } else {
        0.0
    };

    eprintln!("\n=== shape ===");
    eprintln!("ok={} failed={}", ok.len(), failed);
    eprintln!("extract share of measured time: {cpu_share:.1}%");
    eprintln!(
        "wall={:.1}s  throughput={:.1} url/s",
        wall.as_secs_f64(),
        rows.len() as f64 / wall.as_secs_f64().max(0.001)
    );

    // A page that returns 200 but almost no text is what makes the production
    // pipeline escalate to a browser; count them so the benchmark shows how
    // much of the corpus is at risk of that before any escalation runs.
    let thin = ok
        .iter()
        .filter(|r| r.status == Some(200) && (r.bytes < 10_000 || r.words < 50))
        .count();
    eprintln!(
        "200s that would trip the thin-body escalation rule: {thin}/{} ({:.1}%)",
        ok.len(),
        100.0 * thin as f64 / ok.len().max(1) as f64
    );
}
