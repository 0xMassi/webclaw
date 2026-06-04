//! `webclaw bench <url>` — per-URL extraction micro-benchmark.
//!
//! Fetches a page, extracts it via the same pipeline that powers
//! `--format llm`, and reports how many tokens the LLM pipeline
//! removed vs. the raw HTML. Optional `--facts` reuses the
//! benchmark harness's curated fact lists to score fidelity.
//!
//! v1 uses an *approximate* tokenizer (chars/4 for Latin text,
//! chars/2 for CJK-heavy text). Output is clearly labeled
//! "≈ tokens" so nobody mistakes it for a real tiktoken run.
//! Swapping to tiktoken-rs later is a one-function change.

use std::path::{Path, PathBuf};
use std::time::Instant;

use webclaw_core::{extract, to_llm_text};
use webclaw_fetch::{BrowserProfile, FetchClient, FetchConfig};

/// Inputs collected from the clap subcommand.
pub struct BenchArgs {
    pub url: String,
    pub json: bool,
    pub facts: Option<PathBuf>,
}

/// What a single bench run measures.
struct BenchResult {
    url: String,
    raw_tokens: usize,
    raw_bytes: usize,
    llm_tokens: usize,
    llm_bytes: usize,
    reduction_pct: f64,
    elapsed_secs: f64,
    /// `Some((found, total))` when `--facts` is supplied and the URL has
    /// an entry in the facts file; `None` otherwise.
    facts: Option<(usize, usize)>,
}

pub async fn run(args: &BenchArgs) -> Result<(), String> {
    // Dedicated client so bench doesn't care about global CLI flags
    // (proxies, custom headers, etc.). A reproducible microbench is
    // more useful than an over-configurable one; if someone wants to
    // bench behind a proxy they can set WEBCLAW_PROXY — respected
    // by FetchConfig via the regular channels if we extend later.
    let config = FetchConfig {
        browser: BrowserProfile::Chrome,
        ..FetchConfig::default()
    };
    let client = FetchClient::new(config).map_err(|e| format!("build client: {e}"))?;

    let start = Instant::now();
    let fetched = client
        .fetch(&args.url)
        .await
        .map_err(|e| format!("fetch: {e}"))?;

    let extraction =
        extract(&fetched.html, Some(&fetched.url)).map_err(|e| format!("extract: {e}"))?;
    let llm_text = to_llm_text(&extraction, Some(&fetched.url));
    let elapsed = start.elapsed();

    let raw_tokens = approx_tokens(&fetched.html);
    let llm_tokens = approx_tokens(&llm_text);
    let raw_bytes = fetched.html.len();
    let llm_bytes = llm_text.len();
    let reduction_pct = if raw_tokens == 0 {
        0.0
    } else {
        100.0 * (1.0 - llm_tokens as f64 / raw_tokens as f64)
    };

    let facts = match args.facts.as_deref() {
        Some(path) => check_facts(path, &args.url, &llm_text)?,
        None => None,
    };

    let result = BenchResult {
        url: args.url.clone(),
        raw_tokens,
        raw_bytes,
        llm_tokens,
        llm_bytes,
        reduction_pct,
        elapsed_secs: elapsed.as_secs_f64(),
        facts,
    };

    if args.json {
        print_json(&result);
    } else {
        print_box(&result);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Approximate tokenizer
// ---------------------------------------------------------------------------

/// Rough token count. `chars / 4` is the classic English rule of thumb
/// (close to cl100k_base for typical prose). CJK scripts pack ~2 chars
/// per token, so we switch to `chars / 2` when CJK dominates.
///
/// Off by ±10% vs. a real BPE tokenizer, which is fine for "is webclaw's
/// output 66% smaller or 66% bigger than raw HTML" — the signal is
/// order-of-magnitude, not precise accounting.
fn approx_tokens(s: &str) -> usize {
    let total: usize = s.chars().count();
    if total == 0 {
        return 0;
    }
    let cjk = s.chars().filter(|c| is_cjk(*c)).count();
    let cjk_ratio = cjk as f64 / total as f64;
    if cjk_ratio > 0.30 {
        total.div_ceil(2)
    } else {
        total.div_ceil(4)
    }
}

fn is_cjk(c: char) -> bool {
    let n = c as u32;
    (0x4E00..=0x9FFF).contains(&n)   // CJK Unified Ideographs
        || (0x3040..=0x309F).contains(&n) // Hiragana
        || (0x30A0..=0x30FF).contains(&n) // Katakana
        || (0xAC00..=0xD7AF).contains(&n) // Hangul Syllables
        || (0x3400..=0x4DBF).contains(&n) // CJK Extension A
}

// ---------------------------------------------------------------------------
// Output: ASCII / Unicode box
// ---------------------------------------------------------------------------

const BOX_WIDTH: usize = 62; // inner width between the two side borders

fn print_box(r: &BenchResult) {
    let host = display_host(&r.url);
    let version = env!("CARGO_PKG_VERSION");

    let top = "─".repeat(BOX_WIDTH);
    let sep = "─".repeat(BOX_WIDTH);

    // Header: host on the left, "webclaw X.Y.Z" on the right.
    let left = host;
    let right = format!("webclaw {version}");
    let pad = BOX_WIDTH.saturating_sub(left.chars().count() + right.chars().count() + 2);
    let header = format!(" {}{}{} ", left, " ".repeat(pad), right);

    println!("┌{top}┐");
    println!("│{header}│");
    println!("├{sep}┤");
    print_row(
        "raw HTML",
        &format!("{} ≈ tokens", fmt_int(r.raw_tokens)),
        &fmt_bytes(r.raw_bytes),
    );
    print_row(
        "--format llm",
        &format!("{} ≈ tokens", fmt_int(r.llm_tokens)),
        &fmt_bytes(r.llm_bytes),
    );
    print_row("token reduction", &format!("{:.1}%", r.reduction_pct), "");
    print_row("extraction time", &format!("{:.2} s", r.elapsed_secs), "");
    if let Some((found, total)) = r.facts {
        let pct = if total == 0 {
            0.0
        } else {
            100.0 * found as f64 / total as f64
        };
        print_row(
            "facts preserved",
            &format!("{found}/{total} ({pct:.1}%)"),
            "",
        );
    }
    println!("└{top}┘");
    println!();
    println!("note: token counts are approximate (chars/4 Latin, chars/2 CJK).");
}

fn print_row(label: &str, middle: &str, right: &str) {
    // Layout inside the box:
    //   " <label padded to 18>   <middle>   <right right-aligned to fit> "
    let left_col = format!(" {:<18}", label);
    let right_col = format!("{right} ");
    let budget = BOX_WIDTH
        .saturating_sub(left_col.chars().count())
        .saturating_sub(right_col.chars().count());
    let middle_col = format!("{:<width$}", middle, width = budget);
    println!("│{left_col}{middle_col}{right_col}│");
}

fn fmt_int(n: usize) -> String {
    // Comma-group thousands. Avoids pulling in num-format / thousands
    // for one call site.
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn fmt_bytes(n: usize) -> String {
    const KB: usize = 1024;
    const MB: usize = KB * 1024;
    if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{} KB", n / KB)
    } else {
        format!("{n} B")
    }
}

/// Best-effort host extraction — if the URL doesn't parse we fall back
/// to the raw string so the box still prints something recognizable.
fn display_host(url: &str) -> String {
    url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| url.to_string())
}

// ---------------------------------------------------------------------------
// JSON output — single line, stable key order for scripting / CI.
// ---------------------------------------------------------------------------

fn print_json(r: &BenchResult) {
    let mut obj = serde_json::Map::new();
    obj.insert("url".into(), r.url.clone().into());
    obj.insert("raw_tokens".into(), r.raw_tokens.into());
    obj.insert("raw_bytes".into(), r.raw_bytes.into());
    obj.insert("llm_tokens".into(), r.llm_tokens.into());
    obj.insert("llm_bytes".into(), r.llm_bytes.into());
    obj.insert("token_reduction_pct".into(), round1(r.reduction_pct).into());
    obj.insert("elapsed_secs".into(), round2(r.elapsed_secs).into());
    obj.insert("token_method".into(), "approx".into());
    obj.insert("webclaw_version".into(), env!("CARGO_PKG_VERSION").into());
    if let Some((found, total)) = r.facts {
        obj.insert("facts_found".into(), found.into());
        obj.insert("facts_total".into(), total.into());
    }
    // Single-line JSON — easy to append to ndjson for CI runs.
    println!("{}", serde_json::Value::Object(obj));
}

fn round1(f: f64) -> f64 {
    (f * 10.0).round() / 10.0
}
fn round2(f: f64) -> f64 {
    (f * 100.0).round() / 100.0
}

// ---------------------------------------------------------------------------
// Facts file support
// ---------------------------------------------------------------------------

/// Load `facts.json` (same schema as `benchmarks/facts.json`) and check how
/// many curated facts for this URL appear in the extracted LLM text.
/// Returns `None` when the URL has no entry in the file — don't penalize
/// a site that simply hasn't been curated yet.
fn check_facts(path: &Path, url: &str, llm_text: &str) -> Result<Option<(usize, usize)>, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("read facts file {}: {e}", path.display()))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("parse facts file: {e}"))?;

    let facts_obj = parsed
        .get("facts")
        .and_then(|v| v.as_object())
        .ok_or_else(|| "facts file missing `facts` object".to_string())?;

    let Some(entry) = facts_obj.get(url) else {
        // URL not curated in this facts file — don't print a fidelity
        // column rather than showing a misleading 0/0.
        return Ok(None);
    };
    let Some(list) = entry.as_array() else {
        return Err(format!("facts['{url}'] is not an array"));
    };

    let total = list.len();
    let text_low = llm_text.to_lowercase();
    let mut found = 0usize;
    for f in list {
        let Some(fact) = f.as_str() else { continue };
        if matches_fact(&text_low, fact) {
            found += 1;
        }
    }
    Ok(Some((found, total)))
}

/// Match a single fact against the lowercased text. Mirrors the
/// python harness in `benchmarks/scripts/bench.py`:
/// - Single alphanumeric token → word-boundary (so `API` doesn't hit
///   `apiece`).
/// - Multi-word or non-alpha facts (e.g. `99.999`) → substring.
fn matches_fact(text_low: &str, fact: &str) -> bool {
    let fact_low = fact.to_lowercase();
    if fact_low.is_empty() {
        return false;
    }
    let is_simple_token = fact_low.chars().all(|c| c.is_ascii_alphanumeric())
        && fact_low
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());

    if !is_simple_token {
        return text_low.contains(&fact_low);
    }
    // Word-boundary scan without pulling in the regex dependency just
    // for this: find each occurrence and check neighbouring chars.
    let bytes = text_low.as_bytes();
    let needle = fact_low.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle {
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let after_idx = i + needle.len();
            let after_ok = after_idx >= bytes.len() || !bytes[after_idx].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
        }
        i += 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approx_tokens_empty() {
        assert_eq!(approx_tokens(""), 0);
    }

    #[test]
    fn approx_tokens_latin_roughly_chars_over_4() {
        // 100 ASCII chars → ~25 tokens
        let s = "a".repeat(100);
        assert_eq!(approx_tokens(&s), 25);
    }

    #[test]
    fn approx_tokens_cjk_denser() {
        // 100 CJK chars → ~50 tokens (chars/2 branch)
        let s: String = "中".repeat(100);
        assert_eq!(approx_tokens(&s), 50);
    }

    #[test]
    fn approx_tokens_mixed_uses_latin_branch() {
        // 80 latin + 20 CJK → CJK ratio 20% < 30% → chars/4 branch
        let s = format!("{}{}", "a".repeat(80), "中".repeat(20));
        assert_eq!(approx_tokens(&s), 25);
    }

    #[test]
    fn fmt_int_commas() {
        assert_eq!(fmt_int(0), "0");
        assert_eq!(fmt_int(100), "100");
        assert_eq!(fmt_int(1_000), "1,000");
        assert_eq!(fmt_int(243_465), "243,465");
        assert_eq!(fmt_int(12_345_678), "12,345,678");
    }

    #[test]
    fn fmt_bytes_units() {
        assert_eq!(fmt_bytes(500), "500 B");
        assert_eq!(fmt_bytes(1024), "1 KB");
        assert_eq!(fmt_bytes(1024 * 1024), "1.0 MB");
        assert_eq!(fmt_bytes(1024 * 1024 * 3 + 1024 * 512), "3.5 MB");
    }

    #[test]
    fn matches_fact_word_boundary() {
        assert!(matches_fact("the api is ready", "API"));
        // single-token alphanumeric: API should not hit apiece
        assert!(!matches_fact("an apiece of land", "API"));
    }

    #[test]
    fn matches_fact_multiword_substring() {
        assert!(matches_fact("uptime is 99.999% this year", "99.999"));
        assert!(matches_fact("the app router routes requests", "App Router"));
    }

    #[test]
    fn matches_fact_case_insensitive() {
        assert!(matches_fact("the claude model is opus", "Claude"));
        assert!(matches_fact("the claude model is opus", "opus"));
    }

    #[test]
    fn matches_fact_missing() {
        assert!(!matches_fact("nothing to see here", "vercel"));
    }

    #[test]
    fn display_host_parses_url() {
        assert_eq!(display_host("https://stripe.com/"), "stripe.com");
        assert_eq!(
            display_host("https://docs.python.org/3/"),
            "docs.python.org"
        );
    }

    #[test]
    fn display_host_falls_back_on_garbage() {
        assert_eq!(display_host("not a url"), "not a url");
    }
}
