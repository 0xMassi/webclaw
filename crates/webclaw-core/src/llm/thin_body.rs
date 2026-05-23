/// Thin-body detector (M10, issue #3).
///
/// Some sites — most notably Penske publications (Variety, Hollywood
/// Reporter, Deadline) — serve a root HTML document whose article content
/// is not present in the initial DOM. The page is hydrated client-side
/// by JS that webclaw cannot execute (webclaw's `--browser chrome` is a
/// wreq TLS-fingerprint impersonation, NOT a headless JS engine). The
/// served HTML contains only ~200 words of chrome / navigation, so a
/// caller pointing webclaw at e.g. `https://www.hollywoodreporter.com/`
/// receives a thin body with no signal that JS rendering would have
/// produced the actual content.
///
/// This module classifies an `ExtractionResult` as "thin body" / "not
/// thin" / "exempt" so callers can emit a stderr hint nudging the user
/// toward a workaround (subsection URL, e.g. `/c/movies/movie-news/`,
/// or M11's pending `--paywall-bypass`).
///
/// Decision rule (iter-6 phase A measured baseline, see
/// `iter-06-…-phase-A-report.md`):
///
///   word_count < `WORD_COUNT_THRESHOLD`  AND host not in EXEMPT_HOSTS
///       -> Thin { word_count }
///   word_count < `WORD_COUNT_THRESHOLD`  AND host in EXEMPT_HOSTS
///       -> Exempt
///   word_count >= `WORD_COUNT_THRESHOLD`
///       -> NotThin
///
/// Calibration against the iter-6 phase A corpus:
///   - HR root (228 words, www.hollywoodreporter.com) -> Thin
///   - HR /c/movies/movie-news/ (979 words) -> NotThin
///   - BBC article (866 words, www.bbc.com) -> NotThin
///   - example.com (20 words) -> Exempt
///   - httpbin.org (synthetic, ~5 words) -> Exempt
///
/// Threshold and exemption choice rationale: see phase A report
/// section "M10 threshold + exemption logic". This module implements
/// **Option E1** (small hard-coded exempt list for utility/test domains).
use crate::types::ExtractionResult;

/// A page with fewer extracted body words than this triggers the
/// thin-body hint. Iter-6 phase A picked 500, matching the hub-detector's
/// `WORD_COUNT_THRESHOLD` so the two classifiers stay in lockstep — a
/// page that is "hub" is also "thin" (the hub hint takes precedence; see
/// CLI `apply_thin_body_detection`).
pub const WORD_COUNT_THRESHOLD: usize = 500;

/// Domains where a thin body is by-design (test fixtures, utility
/// endpoints). Hint is suppressed on these so CI/probe runs against
/// `example.com` / `httpbin.org` don't grow noisy stderr.
///
/// Matched against the URL host, lowercased, with no leading `www.`.
/// Phase A approved this hard-coded list (Option E1).
const EXEMPT_HOSTS: &[&str] = &[
    "example.com",
    "example.net",
    "example.org",
    "httpbin.org",
    "localhost",
    "127.0.0.1",
];

/// Classification produced by [`classify`]. Carries the measured word
/// count for `Thin` so the hint can quote it back to the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinBodyClassification {
    /// Body has >= `WORD_COUNT_THRESHOLD` words. No hint.
    NotThin,
    /// Body has < `WORD_COUNT_THRESHOLD` words. Emit hint.
    Thin { word_count: usize },
    /// Body has < `WORD_COUNT_THRESHOLD` words but host is in the
    /// exempt list (utility / test domain). Hint suppressed.
    Exempt,
}

impl ThinBodyClassification {
    /// Format the per-page numbers as a single line suitable for a
    /// stderr hint. Does not include the leading "# hint:" / newline so
    /// callers control the surrounding context.
    ///
    /// Returns `None` for `NotThin` and `Exempt` — callers should
    /// short-circuit before formatting in those cases.
    pub fn hint_line(&self) -> Option<String> {
        match self {
            ThinBodyClassification::Thin { word_count } => Some(format!(
                "extracted body is {word_count} words (<{threshold}); page may be JS-rendered or paywalled. \
                 Try a subsection URL (e.g. /<topic>/) for content-heavy pages, \
                 or see M11 (--paywall-bypass, pending) for paywalled sites.",
                threshold = WORD_COUNT_THRESHOLD,
            )),
            ThinBodyClassification::NotThin | ThinBodyClassification::Exempt => None,
        }
    }
}

/// Classify an extraction result as thin / not-thin / exempt.
///
/// Reads `result.metadata.word_count` directly (the field is already
/// computed during extraction; no additional CPU). Host extraction is a
/// single `url::Url::parse` + `.host_str()`.
///
/// Zero I/O, zero allocation on the NotThin fast path (the common case
/// for the bulk of the probe corpus).
pub fn classify(result: &ExtractionResult) -> ThinBodyClassification {
    let word_count = result.metadata.word_count;
    if word_count >= WORD_COUNT_THRESHOLD {
        return ThinBodyClassification::NotThin;
    }
    // Below threshold: check exempt list.
    if let Some(url_str) = result.metadata.url.as_deref() {
        if host_is_exempt(url_str) {
            return ThinBodyClassification::Exempt;
        }
    }
    ThinBodyClassification::Thin { word_count }
}

/// Return true when the URL's host (lower-cased, leading `www.` stripped)
/// matches one of the exempt domains. Falls through to `false` on any
/// parse error — better to emit a hint than to silently swallow.
fn host_is_exempt(url_str: &str) -> bool {
    let parsed = match url::Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => return false,
    };
    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return false,
    };
    let host = host.strip_prefix("www.").unwrap_or(&host);
    EXEMPT_HOSTS.iter().any(|exempt| *exempt == host)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Content, ExtractionResult, Metadata};

    fn make_result(word_count: usize, url: Option<&str>) -> ExtractionResult {
        ExtractionResult {
            metadata: Metadata {
                title: Some("Test Page".to_string()),
                description: None,
                author: None,
                published_date: None,
                language: None,
                url: url.map(|s| s.to_string()),
                site_name: None,
                image: None,
                favicon: None,
                word_count,
                http_status: Some(200),
            },
            content: Content {
                markdown: String::new(),
                plain_text: String::new(),
                links: Vec::new(),
                images: Vec::new(),
                code_blocks: Vec::new(),
                raw_html: None,
            },
            domain_data: None,
            structured_data: Vec::new(),
        }
    }

    #[test]
    fn test_thin_body_detected_at_under_500_words() {
        // 200-word HR root simulation.
        let result = make_result(200, Some("https://www.hollywoodreporter.com/"));
        assert_eq!(
            classify(&result),
            ThinBodyClassification::Thin { word_count: 200 }
        );
    }

    #[test]
    fn test_thin_body_not_detected_at_over_500_words() {
        // 1000-word substantive article.
        let result = make_result(1000, Some("https://www.hollywoodreporter.com/c/movies/"));
        assert_eq!(classify(&result), ThinBodyClassification::NotThin);
    }

    #[test]
    fn test_thin_body_not_detected_at_exact_threshold() {
        // Boundary: 500 words exactly is NOT thin (strict <).
        let result = make_result(500, Some("https://www.hollywoodreporter.com/"));
        assert_eq!(classify(&result), ThinBodyClassification::NotThin);
    }

    #[test]
    fn test_thin_body_exempt_on_example_com() {
        let result = make_result(20, Some("https://example.com/"));
        assert_eq!(classify(&result), ThinBodyClassification::Exempt);
    }

    #[test]
    fn test_thin_body_exempt_on_example_com_with_www() {
        // www. prefix is stripped before matching.
        let result = make_result(20, Some("https://www.example.com/"));
        assert_eq!(classify(&result), ThinBodyClassification::Exempt);
    }

    #[test]
    fn test_thin_body_exempt_on_httpbin() {
        let result = make_result(5, Some("https://httpbin.org/html"));
        assert_eq!(classify(&result), ThinBodyClassification::Exempt);
    }

    #[test]
    fn test_thin_body_exempt_on_localhost() {
        let result = make_result(10, Some("http://localhost:8080/"));
        assert_eq!(classify(&result), ThinBodyClassification::Exempt);
    }

    #[test]
    fn test_thin_body_thin_with_no_url() {
        // Local-file / --stdin paths have no URL. Exemption check
        // short-circuits and the page is classified as Thin (the CLI
        // layer suppresses the hint on local-file paths separately).
        let result = make_result(50, None);
        assert_eq!(
            classify(&result),
            ThinBodyClassification::Thin { word_count: 50 }
        );
    }

    #[test]
    fn test_thin_body_hint_line_shape() {
        let cls = ThinBodyClassification::Thin { word_count: 228 };
        let hint = cls.hint_line().expect("Thin should produce a hint");
        assert!(hint.contains("228 words"));
        assert!(hint.contains("<500"));
        assert!(hint.contains("subsection URL"));
    }

    #[test]
    fn test_thin_body_hint_line_none_for_not_thin() {
        assert_eq!(ThinBodyClassification::NotThin.hint_line(), None);
        assert_eq!(ThinBodyClassification::Exempt.hint_line(), None);
    }
}
