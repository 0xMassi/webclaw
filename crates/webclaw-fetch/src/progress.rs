//! Periodic stderr progress line emitter for slow fetches (M13).
//!
//! Wraps any async fetch future with a `tokio::select!` against a
//! `tokio::time::interval`. Every `PROGRESS_INTERVAL` (default 10s) of
//! elapsed time, emits one line to STDERR of the form:
//!
//! ```text
//! # webclaw: still fetching <URL> (Ns)
//! ```
//!
//! Fetches completing in under `PROGRESS_INTERVAL` emit zero lines (the
//! timer never fires). Stdout is untouched.
//!
//! The URL is truncated to at most 80 chars (head + `...` + tail) so
//! pathological query strings don't blow up the stderr line. Truncation
//! is char-boundary safe (operates on `chars`, not bytes).

use std::future::Future;
use std::time::Duration;

use tokio::time::{interval, Instant, MissedTickBehavior};

/// Default progress emission interval. The first tick fires at +10s
/// elapsed; subsequent ticks at +20s, +30s, etc.
pub const PROGRESS_INTERVAL: Duration = Duration::from_secs(10);

/// Maximum URL length in the progress line. Longer URLs are truncated
/// `head...tail` style.
const MAX_URL_LEN: usize = 80;

/// Wrap a fetch future with the default 10s progress emitter. Writes
/// progress lines to STDERR via `eprintln!`. Returns the inner future's
/// result unchanged.
pub async fn with_progress<F, T>(url: &str, future: F) -> T
where
    F: Future<Output = T>,
{
    with_progress_writer(url, future, PROGRESS_INTERVAL, |s| eprintln!("{s}")).await
}

/// Test-friendly variant of [`with_progress`]: caller supplies the tick
/// interval (so tests can use a 50ms period instead of 10s) and a
/// writer closure (so tests can capture emitted lines without touching
/// real stderr).
///
/// Production code uses [`with_progress`] which delegates here with
/// [`PROGRESS_INTERVAL`] and an `eprintln!` writer.
pub async fn with_progress_writer<F, T, W>(
    url: &str,
    future: F,
    period: Duration,
    mut writer: W,
) -> T
where
    F: Future<Output = T>,
    W: FnMut(String),
{
    let start = Instant::now();
    let mut ticker = interval(period);
    // First tick of `tokio::time::interval(period)` fires *immediately*
    // (at construction time). We don't want a t=0 emit — consume that
    // first tick before entering the select loop. Subsequent ticks fire
    // at `start + period`, `start + 2*period`, ...
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
    ticker.tick().await;

    tokio::pin!(future);

    loop {
        tokio::select! {
            // Bias toward the future — if both are ready (rare), prefer
            // returning the result over emitting a final tick.
            biased;
            result = &mut future => {
                return result;
            }
            _ = ticker.tick() => {
                let elapsed = start.elapsed();
                writer(format_progress_line(url, elapsed));
            }
        }
    }
}

/// Build the progress line: `# webclaw: still fetching <URL> (Ns)`.
/// URL is truncated via [`truncate_url`] to [`MAX_URL_LEN`] chars.
/// Elapsed is rounded to whole seconds (10, 20, 30, ...).
pub(crate) fn format_progress_line(url: &str, elapsed: Duration) -> String {
    let truncated = truncate_url(url, MAX_URL_LEN);
    let secs = elapsed.as_secs();
    format!("# webclaw: still fetching {truncated} ({secs}s)")
}

/// Truncate `url` to at most `max` chars, using `head...tail` shape
/// when truncation is needed. Char-boundary safe (operates on `chars`).
pub(crate) fn truncate_url(url: &str, max: usize) -> String {
    let total_chars = url.chars().count();
    if total_chars <= max {
        return url.to_string();
    }
    // Reserve 3 chars for "..." and split the remainder ~70/30 between
    // head (path-side) and tail (query-side).
    let avail = max.saturating_sub(3);
    let head_chars = avail.saturating_sub(17);
    let tail_chars = 17;
    let head: String = url.chars().take(head_chars).collect();
    let tail: String = url
        .chars()
        .rev()
        .take(tail_chars)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Collect emitted lines into a `Vec<String>` via a captured writer.
    fn capture() -> (Arc<Mutex<Vec<String>>>, impl FnMut(String)) {
        let sink: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink_clone = Arc::clone(&sink);
        let writer = move |s: String| {
            sink_clone.lock().unwrap().push(s);
        };
        (sink, writer)
    }

    #[tokio::test]
    async fn test_progress_emits_after_interval_elapsed() {
        let (sink, writer) = capture();
        // 250ms future, 50ms interval — expect ~4-5 ticks before resolution.
        let fut = tokio::time::sleep(Duration::from_millis(250));
        with_progress_writer(
            "https://example.com/slow",
            async {
                fut.await;
                42_i32
            },
            Duration::from_millis(50),
            writer,
        )
        .await;
        let lines = sink.lock().unwrap();
        assert!(
            !lines.is_empty(),
            "expected >=1 progress line; got {} ({:?})",
            lines.len(),
            *lines
        );
        for line in lines.iter() {
            assert!(
                line.starts_with("# webclaw: still fetching"),
                "line shape wrong: {line:?}"
            );
            assert!(
                line.contains("https://example.com/slow"),
                "url missing from line: {line:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_progress_silent_on_fast_future() {
        let (sink, writer) = capture();
        // 10ms future, 1s interval — zero ticks expected.
        let result = with_progress_writer(
            "https://example.com/fast",
            async {
                tokio::time::sleep(Duration::from_millis(10)).await;
                "done"
            },
            Duration::from_secs(1),
            writer,
        )
        .await;
        assert_eq!(result, "done");
        let lines = sink.lock().unwrap();
        assert_eq!(
            lines.len(),
            0,
            "expected 0 progress lines on fast future; got {:?}",
            *lines
        );
    }

    #[tokio::test]
    async fn test_progress_line_includes_url() {
        let (sink, writer) = capture();
        let target_url = "https://news.ycombinator.com/item?id=12345";
        with_progress_writer(
            target_url,
            async {
                tokio::time::sleep(Duration::from_millis(150)).await;
            },
            Duration::from_millis(50),
            writer,
        )
        .await;
        let lines = sink.lock().unwrap();
        assert!(!lines.is_empty(), "expected progress lines");
        assert!(
            lines.iter().all(|l| l.contains(target_url)),
            "every line should contain the URL: {:?}",
            *lines
        );
    }

    #[tokio::test]
    async fn test_progress_returns_inner_result_ok() {
        let (_sink, writer) = capture();
        let r: Result<i32, String> = with_progress_writer(
            "https://example.com/",
            async { Ok::<i32, String>(7) },
            Duration::from_secs(1),
            writer,
        )
        .await;
        assert_eq!(r, Ok(7));
    }

    #[tokio::test]
    async fn test_progress_propagates_error() {
        let (_sink, writer) = capture();
        let r: Result<i32, String> = with_progress_writer(
            "https://example.com/",
            async { Err::<i32, String>("boom".to_string()) },
            Duration::from_secs(1),
            writer,
        )
        .await;
        assert_eq!(r, Err("boom".to_string()));
    }

    #[test]
    fn test_truncate_url_short_passthrough() {
        let url = "https://example.com/";
        assert_eq!(truncate_url(url, 80), url);
    }

    #[test]
    fn test_truncate_url_long_head_dots_tail() {
        let url = "https://www.example.com/very/long/path/segments/with/lots/of/text/and/then?q=some_long_query_string_value_here&other=more&another=thing";
        let truncated = truncate_url(url, 80);
        assert!(
            truncated.chars().count() <= 80,
            "truncated length {} > 80: {truncated:?}",
            truncated.chars().count()
        );
        assert!(
            truncated.contains("..."),
            "expected '...' marker in truncated url: {truncated:?}"
        );
        assert!(
            truncated.starts_with("https://www.example.com/"),
            "truncated should start with the URL head: {truncated:?}"
        );
    }

    #[test]
    fn test_truncate_url_unicode_safe() {
        // Cyrillic URL longer than 80 chars — must not panic on a
        // mid-codepoint split.
        let url = "https://example.com/путь/к/очень/длинной/странице/с/большим/количеством/кириллицы/тут";
        let truncated = truncate_url(url, 80);
        assert!(truncated.is_char_boundary(truncated.len()));
        // Roundtrip through chars to confirm valid UTF-8 throughout.
        let _: String = truncated.chars().collect();
    }

    #[test]
    fn test_format_progress_line_shape() {
        let line = format_progress_line("https://example.com/", Duration::from_secs(10));
        assert_eq!(
            line,
            "# webclaw: still fetching https://example.com/ (10s)"
        );
    }

    #[test]
    fn test_format_progress_line_seconds_only() {
        // Sub-second elapsed rounds to 0s, not fractions. (In practice
        // the first tick fires at +PROGRESS_INTERVAL so this is mostly
        // a defensive shape assertion.)
        let line = format_progress_line("https://x/", Duration::from_millis(9_500));
        assert!(line.ends_with("(9s)"), "line should end with `(9s)`: {line:?}");
    }
}
