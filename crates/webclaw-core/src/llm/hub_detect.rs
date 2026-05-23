/// JS-hub page detector.
///
/// Some sites (notably ESPN /nba /nfl /mlb /nhl /soccer) render most of
/// their content via JavaScript and ship a thin "nav-card hub" in the
/// initial HTML: short body, a small set of nav-style links, and no real
/// article prose. Chrome retry does not help — the article body genuinely
/// isn't in the rendered DOM; it lives behind further JS API calls under
/// `/story/_/id/<id>/...` URLs.
///
/// This module classifies an `ExtractionResult` as "hub" / "not hub" so
/// callers can either emit a stderr hint or honor `--prefer-articles`
/// and return just the extracted link list.
///
/// Heuristic (iter-2 phase A measured baseline, see
/// `baselines/probe-run-r-iter2-baseline.json`):
///
///   is_hub = (word_count < `WORD_COUNT_THRESHOLD`)
///         AND (link_count >= `MIN_LINK_COUNT`)
///
/// Calibration against the iter-0 corpus + iter-2 hub probes:
///   - ESPN /nba (288 words, 7 links) -> HUB
///   - ESPN /nfl (304 words, 7 links) -> HUB
///   - ESPN root (330 words, 7 links) -> HUB (borderline accepted)
///   - BBC /news/world (1981 words, 28 links) -> NOT hub (word_count too high)
///   - n1info root (3015 words, 134 links) -> NOT hub (word_count too high)
///   - THR root (209 words, 1 link) -> NOT hub (link_count too low)
///   - Reuters ME broken-fetch (21 words, 0 links) -> NOT hub
///   - synthetic url-escape (85 words, 0 links) -> NOT hub
///
/// 8 / 8 correct with comfortable margins on both sides.
use crate::types::ExtractionResult;

use super::body;
use super::links;

/// A page with fewer words than this is a candidate hub (gated by
/// `MIN_LINK_COUNT`). Iter-2 phase A picked 500 to give a >3.9x gap above
/// the lowest aggregator word count seen in the corpus (BBC /news/world =
/// 1981 words).
pub const WORD_COUNT_THRESHOLD: usize = 500;

/// A candidate hub must also have at least this many links — excludes
/// broken / thin-body / synthetic cases that look short but aren't hubs.
/// Iter-2 phase A picked 5 with the lowest observed hub link_count of 7
/// for safety margin.
pub const MIN_LINK_COUNT: usize = 5;

/// Result of classifying an `ExtractionResult`. Includes the raw signals
/// used so callers can emit a useful stderr hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HubClassification {
    pub is_hub: bool,
    pub word_count: usize,
    pub link_count: usize,
}

impl HubClassification {
    /// Format the per-page numbers as a single line suitable for a
    /// stderr hint. Does not include the leading "# hint:" / newline so
    /// callers control the surrounding context.
    pub fn hint_line(&self) -> String {
        format!(
            "this page looks like a JS hub (word_count={}, link_count={}). \
             The article body is likely not in the rendered DOM — drill /story/_/id/<id>/ \
             or similar article URLs for content. \
             Use --prefer-articles to return the extracted link list directly.",
            self.word_count, self.link_count
        )
    }
}

/// Classify an extraction result as hub / not-hub.
///
/// Operates on the same processed-body pipeline used by the main LLM
/// formatter and `to_llm_summary` so the link count matches what the
/// caller will see if they switch to `--prefer-articles`.
pub fn classify(result: &ExtractionResult) -> HubClassification {
    let word_count = count_body_words(result);
    let link_count = count_clean_links(result);
    let is_hub = word_count < WORD_COUNT_THRESHOLD && link_count >= MIN_LINK_COUNT;
    HubClassification {
        is_hub,
        word_count,
        link_count,
    }
}

/// Count words in the *body* text after the body pipeline (which strips
/// chrome / nav / dedup'd repeats). We deliberately don't trust
/// `result.metadata.word_count` because that comes from the raw plain
/// text — chrome-inclusive — and would over-count hub pages.
fn count_body_words(result: &ExtractionResult) -> usize {
    let processed = body::process_body(&result.content.markdown);
    processed
        .text
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .count()
}

/// Count emitted links after the same noise filter the main LLM
/// formatter uses. Mirrors `to_llm_summary`'s collection so detector
/// output matches what `--prefer-articles` will print.
fn count_clean_links(result: &ExtractionResult) -> usize {
    let processed = body::process_body(&result.content.markdown);
    let mut n = 0usize;
    for (text, _href) in processed.links {
        let label = links::clean_link_label(&text);
        if label.is_empty() {
            continue;
        }
        n += 1;
    }
    n
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Content, ExtractionResult, Metadata};

    fn make_result(markdown: &str) -> ExtractionResult {
        ExtractionResult {
            metadata: Metadata {
                title: Some("Test Page".to_string()),
                description: None,
                author: None,
                published_date: None,
                language: None,
                url: Some("https://example.com/".to_string()),
                site_name: None,
                image: None,
                favicon: None,
                word_count: 0,
                http_status: None,
            },
            content: Content {
                markdown: markdown.to_string(),
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

    /// Build a markdown body with `n_links` link lines and approximately
    /// `n_body_words` body words. Each "sentence" is given a unique
    /// numeric stamp so the body-processing pipeline's dedup steps don't
    /// collapse repeating sentences. Mirrors how webclaw emits a real
    /// page: prose body + a link list.
    fn synth_hub(n_links: usize, n_body_words: usize) -> String {
        // Each base sentence is ~14 words. We tag each sentence with a
        // unique counter so dedup_lines / dedup_content_blocks /
        // dedup_repeated_phrases never see two identical lines.
        let base_sentences = [
            "The proposed amendment would require ratification by at least three quarters of the member legislatures",
            "Investigators say the malfunction was traced to a faulty heat exchanger in the secondary loop",
            "Critics argue that the policy would disproportionately burden small businesses already operating on thin margins",
            "Researchers documented behavioral changes in juvenile salmon exposed to elevated water temperatures over time",
            "The committee voted unanimously to defer the matter pending further independent technical review next quarter",
            "Lawyers for the defendant filed a motion seeking dismissal on procedural grounds before trial began",
            "Survey respondents reported declining confidence in the long term solvency of the pension fund balance",
            "Officials confirmed that the planned shutdown would last approximately seventy two hours barring complications",
            "Analysts noted that quarterly revenue exceeded internal projections despite weakness in two regional markets",
            "Volunteers spent the weekend clearing debris and restoring access along the lower river trail",
        ];
        let words_per_sentence = 15; // 14 base + 1 unique stamp
        let n_sentences = n_body_words.div_ceil(words_per_sentence);

        let mut md = String::from("# Synthetic Hub Page\n\n");
        for i in 0..n_sentences {
            // Stamp goes BEFORE the base sentence so the first
            // DEDUP_PREFIX_WORDS (10) leading words differ across cycles
            // and the body pipeline's near-duplicate prefix detector
            // doesn't collapse our cycling base sentences.
            md.push_str(&format!("Item {i}: "));
            md.push_str(base_sentences[i % base_sentences.len()]);
            md.push_str(".\n\n");
        }
        md.push_str("## Links\n\n");
        for i in 0..n_links {
            md.push_str(&format!(
                "- [Story headline {i}](https://example.com/story/{i})\n"
            ));
        }
        md
    }

    // ----- detector recognizes hub-shaped pages -----

    /// p35-equivalent: ESPN /nba shape. Phase A measured 288 words, 7 links.
    /// Use 30 links + 200 body words per phase B brief (closer to the
    /// synthetic fixture spec than the live measurement).
    #[test]
    fn test_hub_detector_recognizes_espn_nba() {
        let md = synth_hub(30, 200);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(c.is_hub, "expected hub; got {c:?}");
        assert!(c.word_count < WORD_COUNT_THRESHOLD, "words {} >= threshold", c.word_count);
        assert!(c.link_count >= MIN_LINK_COUNT, "links {} < min", c.link_count);
    }

    /// p36-equivalent: ESPN /nfl shape. Slightly different but still
    /// hub-like ratios — fewer links, slightly more body.
    #[test]
    fn test_hub_detector_recognizes_espn_nfl() {
        let md = synth_hub(7, 304);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(c.is_hub, "expected hub; got {c:?}");
    }

    /// p38-equivalent: aggregator with real body — many links but
    /// thousands of words of prose. Phase A: BBC /news/world = 1981 words
    /// 28 links. Detector must NOT classify as hub.
    #[test]
    fn test_hub_detector_passes_aggregator_with_real_body() {
        let md = synth_hub(100, 1500);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(
            !c.is_hub,
            "false positive on link-heavy but content-rich page; got {c:?}"
        );
        assert!(c.word_count >= WORD_COUNT_THRESHOLD);
    }

    /// Normal long article — few links, lots of prose. Common case;
    /// must NOT classify as hub. We use a much larger body target so
    /// the body pipeline's dedup steps still leave us well above the
    /// 500-word threshold.
    #[test]
    fn test_hub_detector_passes_normal_article() {
        // Aim for ~2400 raw words so post-dedup body stays >500.
        let md = synth_hub(5, 2400);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(
            !c.is_hub,
            "false positive on normal article; got {c:?} (threshold {})",
            WORD_COUNT_THRESHOLD
        );
        assert!(c.word_count >= WORD_COUNT_THRESHOLD);
    }

    /// Cross-reference iter-0 corpus: THR-style thin-body page (low words,
    /// 1 link). Must NOT classify as hub — chrome retry is the right fix
    /// for THR per issue #3 / M10, not hub detection.
    #[test]
    fn test_hub_detector_excludes_thin_body_thr_shape() {
        let md = synth_hub(1, 209);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(!c.is_hub, "thin-body misclassified as hub; got {c:?}");
        assert!(c.link_count < MIN_LINK_COUNT);
    }

    /// Cross-reference iter-0 corpus: broken / nearly-empty fetch
    /// (21 words, 0 links — Reuters ME baseline). Must NOT be a hub.
    #[test]
    fn test_hub_detector_excludes_broken_low_link() {
        let md = synth_hub(0, 21);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(!c.is_hub, "broken-fetch misclassified as hub; got {c:?}");
    }

    /// Models the CLI `--prefer-articles` decision point: on a
    /// hub-classified page, the CLI replaces `mode=Full` with
    /// `mode=Summary` so the summary emitter returns the link list
    /// instead of the full body. Verify the two pieces compose correctly
    /// (classifier says hub -> summary path produces a link section).
    #[test]
    fn test_prefer_articles_emits_link_list_on_hub() {
        let md = synth_hub(30, 200);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(c.is_hub, "fixture must be hub-shaped; got {c:?}");
        // When --prefer-articles is set and we're a hub, the CLI calls
        // to_llm_summary instead of to_llm_text. The summary output must
        // contain the link list, not body prose.
        let summary = crate::llm::to_llm_summary(&r, Some("https://example.com/"));
        assert!(summary.contains("## Links"), "summary missing Links header: {summary}");
        assert!(
            summary.contains("Story headline 0"),
            "summary missing first link label: {summary}"
        );
        assert!(
            summary.contains("https://example.com/story/0"),
            "summary missing first link href: {summary}"
        );
    }

    /// Negative-flag sentinel: when --prefer-articles is passed but the
    /// page is NOT a hub (BBC-like rich aggregator), the classifier
    /// returns is_hub=false and the CLI keeps the requested mode (Full).
    /// This is the false-positive-resistance guarantee for p42_bbc_world.
    #[test]
    fn test_prefer_articles_falls_through_on_non_hub() {
        let md = synth_hub(100, 2400);
        let r = make_result(&md);
        let c = classify(&r);
        assert!(
            !c.is_hub,
            "non-hub aggregator must not flip with --prefer-articles; got {c:?}"
        );
        // CLI code path: if !is_hub, requested_mode is returned unchanged.
        // Nothing extra to assert beyond is_hub=false — that's the contract
        // the CLI's apply_hub_detection() honors.
    }

    /// Hint string mentions both signals + the suggested flag, so the
    /// user-visible stderr message is actionable.
    #[test]
    fn test_hub_classification_hint_line_mentions_signals() {
        let c = HubClassification {
            is_hub: true,
            word_count: 288,
            link_count: 7,
        };
        let hint = c.hint_line();
        assert!(hint.contains("288"), "missing word count: {hint}");
        assert!(hint.contains('7'), "missing link count: {hint}");
        assert!(
            hint.contains("--prefer-articles"),
            "missing flag suggestion: {hint}"
        );
    }
}
