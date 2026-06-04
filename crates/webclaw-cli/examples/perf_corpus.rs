//! Deterministic extraction micro-benchmark over a fixed HTML corpus.
//!
//!   cargo run --release -p webclaw-cli --example perf_corpus -- capture
//!   cargo run --release -p webclaw-cli --example perf_corpus -- bench [iters]
//!
//! `capture` fetches a fixed URL list via the real FetchClient and saves the
//! RAW html to /tmp/webclaw-bench/corpus (shared across baseline/fixed runs).
//! `bench` reads that corpus and times extract() and to_llm_text() in-process,
//! so the only variable between runs is the extraction code itself.

use std::fs;
use std::path::Path;
use std::time::Instant;

use webclaw_core::{extract, to_llm_text};
use webclaw_fetch::{BrowserProfile, FetchClient, FetchConfig};

const CORPUS: &str = "/tmp/webclaw-bench/corpus";

const URLS: &[&str] = &[
    "https://example.com",
    "https://en.wikipedia.org/wiki/Rust_(programming_language)",
    "https://en.wikipedia.org/wiki/Web_scraping",
    "https://en.wikipedia.org/wiki/PostgreSQL",
    "https://news.ycombinator.com",
    "https://developer.mozilla.org/en-US/docs/Web/HTTP",
    "https://www.rust-lang.org",
    "https://blog.rust-lang.org/2024/09/05/Rust-1.81.0/",
    "https://docs.python.org/3/library/asyncio.html",
    "https://github.com/tokio-rs/tokio",
    "https://doc.rust-lang.org/book/ch01-00-getting-started.html",
    "https://www.gnu.org/licenses/agpl-3.0.en.html",
    "https://old.reddit.com/r/rust/",
    "https://arstechnica.com/",
    "https://www.theverge.com/",
    "https://crates.io/",
    "https://www.cloudflare.com/",
    "https://stackoverflow.com/questions/tagged/rust",
    "https://www.postgresql.org/docs/16/index.html",
    "https://go.dev/",
    "https://nodejs.org/en",
    "https://www.djangoproject.com/",
];

#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "bench".into());
    match mode.as_str() {
        "capture" => capture().await,
        "bench" => {
            let iters: usize = std::env::args()
                .nth(2)
                .and_then(|s| s.parse().ok())
                .unwrap_or(60);
            bench(iters);
        }
        "snapshot" => {
            let label = std::env::args().nth(2).unwrap_or_else(|| "baseline".into());
            snapshot(&label);
        }
        other => {
            eprintln!("unknown mode '{other}' (use capture|bench)");
            std::process::exit(2);
        }
    }
}

async fn capture() {
    fs::create_dir_all(CORPUS).unwrap();
    let config = FetchConfig {
        browser: BrowserProfile::Chrome,
        ..FetchConfig::default()
    };
    let client = FetchClient::new(config).expect("build client");
    let mut ok = 0;
    for (i, u) in URLS.iter().enumerate() {
        let name = format!(
            "{:02}_{}.html",
            i + 1,
            u.replace("https://", "")
                .chars()
                .map(|c| if c.is_alphanumeric() { c } else { '_' })
                .take(40)
                .collect::<String>()
        );
        match client.fetch(u).await {
            Ok(f) if f.html.len() > 1000 => {
                fs::write(Path::new(CORPUS).join(&name), &f.html).unwrap();
                println!("OK   {name}  ({} bytes)", f.html.len());
                ok += 1;
            }
            Ok(f) => println!("SKIP {name}  (thin {} bytes)", f.html.len()),
            Err(e) => println!("FAIL {name}  ({e})"),
        }
    }
    println!("--- captured {ok} docs into {CORPUS}");
}

/// Write canonical extraction output per corpus doc so baseline/fixed runs can be diffed.
fn snapshot(label: &str) {
    let outdir = format!("/tmp/webclaw-bench/snapshots/{label}");
    fs::create_dir_all(&outdir).unwrap();
    let mut files: Vec<_> = fs::read_dir(CORPUS)
        .expect("corpus dir missing — run `capture` first")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "html").unwrap_or(false))
        .collect();
    files.sort();
    let mut n = 0;
    for path in &files {
        let html = fs::read_to_string(path).unwrap_or_default();
        if html.is_empty() {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let url = format!("https://corpus/{stem}");
        match extract(&html, Some(&url)) {
            Ok(ex) => {
                let json = serde_json::to_string_pretty(&ex).unwrap_or_default();
                let llm = to_llm_text(&ex, Some(&url));
                fs::write(format!("{outdir}/{stem}.json"), json).unwrap();
                fs::write(format!("{outdir}/{stem}.llm"), llm).unwrap();
                n += 1;
            }
            Err(e) => fs::write(format!("{outdir}/{stem}.ERROR"), format!("{e}")).unwrap(),
        }
    }
    println!("snapshot '{label}': wrote {n} docs to {outdir}");
}

fn percentile(sorted: &[u128], p: f64) -> u128 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn bench(iters: usize) {
    let mut files: Vec<_> = fs::read_dir(CORPUS)
        .expect("corpus dir missing — run `capture` first")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map(|x| x == "html").unwrap_or(false))
        .collect();
    files.sort();
    if files.is_empty() {
        eprintln!("no corpus files in {CORPUS}");
        std::process::exit(1);
    }

    println!("# perf_corpus bench  docs={}  iters={}", files.len(), iters);
    println!(
        "{:<42} {:>10} {:>10} {:>10} {:>10}",
        "doc(KB)", "extract_us", "llm_us", "p50_us", "p90_us"
    );

    let mut grand_extract = 0u128;
    let mut grand_llm = 0u128;
    let mut grand_total_p50 = 0u128;

    for path in &files {
        let html = fs::read_to_string(path).unwrap_or_default();
        if html.is_empty() {
            continue;
        }
        let url = format!(
            "https://corpus/{}",
            path.file_name().unwrap().to_string_lossy()
        );

        // warmup
        for _ in 0..5 {
            if let Ok(ex) = extract(&html, Some(&url)) {
                std::hint::black_box(to_llm_text(&ex, Some(&url)));
            }
        }

        let mut ex_times = Vec::with_capacity(iters);
        let mut llm_times = Vec::with_capacity(iters);
        let mut total_times = Vec::with_capacity(iters);
        for _ in 0..iters {
            let t0 = Instant::now();
            let ex = match extract(&html, Some(&url)) {
                Ok(e) => e,
                Err(_) => continue,
            };
            let t1 = Instant::now();
            let txt = to_llm_text(&ex, Some(&url));
            let t2 = Instant::now();
            std::hint::black_box(&txt);
            ex_times.push((t1 - t0).as_micros());
            llm_times.push((t2 - t1).as_micros());
            total_times.push((t2 - t0).as_micros());
        }
        ex_times.sort();
        llm_times.sort();
        total_times.sort();
        let ex_p50 = percentile(&ex_times, 0.50);
        let llm_p50 = percentile(&llm_times, 0.50);
        let tot_p50 = percentile(&total_times, 0.50);
        let tot_p90 = percentile(&total_times, 0.90);
        grand_extract += ex_p50;
        grand_llm += llm_p50;
        grand_total_p50 += tot_p50;

        let label = format!(
            "{} ({}KB)",
            path.file_stem().unwrap().to_string_lossy(),
            html.len() / 1024
        );
        println!(
            "{:<42} {:>10} {:>10} {:>10} {:>10}",
            label.chars().take(42).collect::<String>(),
            ex_p50,
            llm_p50,
            tot_p50,
            tot_p90
        );
    }

    println!("---");
    println!(
        "CORPUS_PASS_P50_SUM_US extract={grand_extract} llm={grand_llm} total={grand_total_p50}"
    );
    println!("(lower is better; total = one full extract+llm pass over the whole corpus at p50)");
}
