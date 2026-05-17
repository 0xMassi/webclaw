//! 1000-site benchmark using FetchClient (wreq backend).
//! Run: cargo test -p webclaw-fetch --test bench_1k --release -- --nocapture

use std::sync::Arc;
use std::time::Instant;
use webclaw_fetch::{BrowserProfile, FetchClient, FetchConfig};

fn load_targets() -> Vec<(String, String, Vec<String>)> {
    let candidates = [
        "targets_1000.txt",
        "../../targets_1000.txt",
        "../../../targets_1000.txt",
    ];
    let path = std::env::var("TARGETS_FILE")
        .ok()
        .or_else(|| {
            candidates
                .iter()
                .find(|p| std::path::Path::new(p).exists())
                .map(|s| s.to_string())
        })
        .expect("targets_1000.txt not found — set TARGETS_FILE env var");
    let content = std::fs::read_to_string(&path).expect("failed to read targets file");
    content
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| {
            let parts: Vec<&str> = l.splitn(3, '|').collect();
            let kw: Vec<String> = parts
                .get(2)
                .unwrap_or(&"")
                .split(',')
                .map(|s| s.trim().to_string())
                .collect();
            (parts[0].to_string(), parts[1].to_string(), kw)
        })
        .collect()
}

fn load_proxy() -> Option<String> {
    let candidates = ["proxies.txt", "../../proxies.txt", "../../../proxies.txt"];
    let path = std::env::var("PROXY_FILE").ok().or_else(|| {
        candidates
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .map(|s| s.to_string())
    })?;
    let content = std::fs::read_to_string(&path).ok()?;
    let line = content.lines().next()?;
    let p: Vec<&str> = line.split(':').collect();
    if p.len() == 4 {
        Some(format!("http://{}:{}@{}:{}", p[2], p[3], p[0], p[1]))
    } else {
        Some(line.to_string())
    }
}

fn classify(body: &str, len: usize, status: u16, kw: &[String]) -> &'static str {
    let lower = body.to_lowercase();
    let challenge = lower.contains("just a moment")
        || lower.contains("verify you are human")
        || lower.contains("cf-chl-bypass")
        || lower.contains("challenge page")
        || lower.contains("pardon our interruption")
        || lower.contains("are you a robot")
        || (lower.contains("captcha") && len < 50000);
    let hits = kw.iter().filter(|k| lower.contains(k.as_str())).count();
    if hits >= 2 && len > 5000 && !challenge {
        "OK"
    } else if challenge {
        "CHALLENGE"
    } else if status == 403 || status == 429 {
        "BLOCKED"
    } else if (300..400).contains(&status) {
        "REDIRECT"
    } else if len < 1000 {
        "EMPTY"
    } else {
        "UNCLEAR"
    }
}

#[tokio::test]
async fn bench_1k_sites() {
    let targets = load_targets();
    let proxy = load_proxy();

    let config = FetchConfig {
        browser: BrowserProfile::Chrome,
        proxy,
        timeout: std::time::Duration::from_secs(12),
        ..Default::default()
    };

    let client = Arc::new(FetchClient::new(config).expect("build client"));

    println!(
        "\n=== webclaw-fetch + wreq — {} targets ===\n",
        targets.len()
    );

    let start = Instant::now();
    let mut pass = 0usize;
    let mut errors = 0usize;
    let mut challenges = 0usize;
    let mut blocked = 0usize;
    let mut redirects = 0usize;
    let mut unclear = 0usize;
    let total = targets.len();

    // Process in batches of 20 concurrent
    for chunk in targets.chunks(20) {
        let mut handles = Vec::new();
        for (name, url, kw) in chunk {
            let c = Arc::clone(&client);
            let url = url.clone();
            let name = name.clone();
            let kw = kw.clone();
            handles.push(tokio::spawn(async move {
                match c.fetch(&url).await {
                    Ok(result) => {
                        let v = classify(&result.html, result.html.len(), result.status, &kw);
                        (name, result.status, result.html.len(), v, String::new())
                    }
                    Err(e) => (name, 0u16, 0usize, "ERROR", format!("{e}")),
                }
            }));
        }

        for h in handles {
            if let Ok((name, status, len, verdict, err)) = h.await {
                match verdict {
                    "OK" => pass += 1,
                    "CHALLENGE" => {
                        challenges += 1;
                        println!("  CHALLENGE {:<25} {:>4} {:>8}B", name, status, len);
                    }
                    "BLOCKED" => {
                        blocked += 1;
                        println!("  BLOCKED   {:<25} {:>4} {:>8}B", name, status, len);
                    }
                    "REDIRECT" => redirects += 1,
                    "ERROR" => {
                        errors += 1;
                        let short = if err.len() > 50 { &err[..50] } else { &err };
                        println!("  ERROR     {:<25} {}", name, short);
                    }
                    _ => unclear += 1,
                }
            }
        }
    }

    let elapsed = start.elapsed();

    println!("\n{}", "=".repeat(60));
    println!(
        "  PASS:      {pass}/{total} ({:.0}%)",
        (pass as f64 / total as f64) * 100.0
    );
    println!("  CHALLENGE: {challenges}");
    println!("  BLOCKED:   {blocked}");
    println!("  REDIRECT:  {redirects}");
    println!("  UNCLEAR:   {unclear}");
    println!("  ERROR:     {errors}");
    println!("  TIME:      {:.1}s", elapsed.as_secs_f64());
    println!("{}", "=".repeat(60));
}
