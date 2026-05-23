/// Metadata header building for LLM-optimized output.
///
/// Produces `> ` prefixed lines with URL, title, author, etc.
/// Omits empty/zero fields to minimize token waste.
use crate::types::ExtractionResult;

pub(crate) fn build_metadata_header(
    out: &mut String,
    result: &ExtractionResult,
    url: Option<&str>,
) {
    build_metadata_header_with_opts(out, result, url, true);
}

/// Same as [`build_metadata_header`] but with an `include_status` toggle.
///
/// `--mode summary` / `--mode toc` callers pass `include_status=false` so
/// the link-list / outline output stays uncluttered (M7 / issue #19 — the
/// status line is most useful on full-extraction output where the caller
/// is reading the body and needs to know whether they're looking at a 404
/// error page vs a real article).
pub(crate) fn build_metadata_header_with_opts(
    out: &mut String,
    result: &ExtractionResult,
    url: Option<&str>,
    include_status: bool,
) {
    let meta = &result.metadata;

    // URL: prefer explicit arg, fall back to metadata
    let effective_url = url.or(meta.url.as_deref());
    if let Some(u) = effective_url {
        out.push_str(&format!("> URL: {u}\n"));
    }
    // M7 (issue #19): HTTP status immediately after URL so callers can
    // distinguish a real 404 from a thin-body 200 without parsing the page
    // body. Emitted only when populated (network path); local-file /
    // --stdin / extract_with_options direct calls leave http_status=None.
    // Summary / toc modes suppress this line via include_status=false.
    if include_status
        && let Some(code) = meta.http_status
    {
        out.push_str(&format!("> Status: {code}\n"));
    }
    if let Some(t) = &meta.title
        && !t.is_empty()
    {
        out.push_str(&format!("> Title: {t}\n"));
    }
    if let Some(d) = &meta.description
        && !d.is_empty()
    {
        out.push_str(&format!("> Description: {d}\n"));
    }
    if let Some(a) = &meta.author
        && !a.is_empty()
    {
        out.push_str(&format!("> Author: {a}\n"));
    }
    if let Some(d) = &meta.published_date
        && !d.is_empty()
    {
        out.push_str(&format!("> Published: {d}\n"));
    }
    if let Some(l) = &meta.language
        && !l.is_empty()
    {
        out.push_str(&format!("> Language: {l}\n"));
    }
    if meta.word_count > 0 {
        // M12 (issue #7): split the total into an article-body portion and
        // a chrome remainder so LLM callers can tell at a glance whether
        // there's real content under the chrome. When the breakdown is
        // available (article + chrome == total, set in
        // `extract_with_options_inner::compute_word_count_breakdown`), emit
        // the parenthetical; otherwise fall back to the legacy single-N
        // form (e.g. local-file / --stdin / direct
        // `extract_with_options` calls that leave the breakdown fields at
        // their `Default` zero — same shape as the http_status fallback).
        //
        // `--mode summary` / `--mode toc` (`include_status=false`)
        // intentionally fall back to the simple `Word count: N` form: the
        // link-list / outline modes don't surface article content, so the
        // breakdown's "did chrome eat the body?" question is irrelevant
        // there. This piggybacks on the existing `include_status` toggle
        // — same modes, same suppression intent (iter-7 next-prompt
        // explicitly authorized either omit-or-simple). `--mode sections`
        // builds its own header and doesn't reach this code at all.
        let n = meta.word_count;
        let m = meta.word_count_article;
        let k = meta.word_count_chrome;
        if include_status && m + k == n && (m > 0 || k > 0) {
            out.push_str(&format!(
                "> Word count: {n} (article: {m}, chrome: {k})\n"
            ));
        } else {
            out.push_str(&format!("> Word count: {n}\n"));
        }
    }
}

// ---------------------------------------------------------------------------
// M12 tests for the header word-count breakdown emission. The breakdown POPULATION
// logic (jsonld articleBody → fallback heuristic) is tested in lib.rs::tests.
// These tests pin the FORMATTER behavior given pre-populated Metadata fields.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod m12_tests {
    use super::*;
    use crate::types::{Content, ExtractionResult, Metadata};

    fn make_result_with_wc(word_count: usize, article: usize, chrome: usize) -> ExtractionResult {
        ExtractionResult {
            metadata: Metadata {
                title: Some("Test Page".into()),
                description: None,
                author: None,
                published_date: None,
                language: None,
                url: Some("https://example.com/".into()),
                site_name: None,
                image: None,
                favicon: None,
                word_count,
                word_count_article: article,
                word_count_chrome: chrome,
                http_status: None,
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

    /// Phase B test 1: when M+K==N and at least one is non-zero, the header
    /// emits the parenthetical breakdown form.
    #[test]
    fn header_emits_breakdown_when_article_plus_chrome_equals_total() {
        let result = make_result_with_wc(1000, 600, 400);
        let mut out = String::new();
        build_metadata_header(&mut out, &result, None);
        assert!(
            out.contains("> Word count: 1000 (article: 600, chrome: 400)"),
            "expected breakdown form, got: {out}"
        );
        assert!(
            !out.contains("> Word count: 1000\n"),
            "must not contain legacy form when breakdown is present, got: {out}"
        );
    }

    /// Phase B test 2: when the breakdown fields are zero (default —
    /// `extract_with_options` direct path, local-file, --stdin), fall back
    /// to the legacy single-N form. This protects all the test fixtures
    /// that don't pre-populate the breakdown.
    #[test]
    fn header_falls_back_to_legacy_form_when_breakdown_unpopulated() {
        let result = make_result_with_wc(1000, 0, 0);
        let mut out = String::new();
        build_metadata_header(&mut out, &result, None);
        assert!(
            out.contains("> Word count: 1000\n"),
            "expected legacy single-N form, got: {out}"
        );
        assert!(
            !out.contains("(article:"),
            "must not contain parenthetical when fields are zero, got: {out}"
        );
    }

    /// Phase B test 3: chrome=0 (all-article page, e.g. YouTube fast path
    /// or document extractor) still emits the breakdown form, so the JSON
    /// shape and the header shape stay consistent.
    #[test]
    fn header_emits_breakdown_with_chrome_zero_when_article_equals_total() {
        let result = make_result_with_wc(500, 500, 0);
        let mut out = String::new();
        build_metadata_header(&mut out, &result, None);
        assert!(
            out.contains("> Word count: 500 (article: 500, chrome: 0)"),
            "expected breakdown with chrome=0, got: {out}"
        );
    }

    /// Phase B test 4: when total is zero, no Word count line is emitted at
    /// all (preserves existing behavior — see `metadata_header_includes_populated_fields`
    /// sentinel).
    #[test]
    fn header_omits_word_count_line_entirely_when_total_zero() {
        let result = make_result_with_wc(0, 0, 0);
        let mut out = String::new();
        build_metadata_header(&mut out, &result, None);
        assert!(
            !out.contains("Word count"),
            "expected no Word count line when total is 0, got: {out}"
        );
    }

    /// Phase B test 5: if article+chrome != total (shouldn't happen via the
    /// canonical `compute_word_count_breakdown` path — invariant), the
    /// formatter falls back to the legacy single-N form rather than
    /// surfacing inconsistent arithmetic. Defensive guard.
    #[test]
    fn header_falls_back_when_article_plus_chrome_mismatches_total() {
        let result = make_result_with_wc(1000, 600, 300); // 600 + 300 != 1000
        let mut out = String::new();
        build_metadata_header(&mut out, &result, None);
        assert!(
            out.contains("> Word count: 1000\n"),
            "expected legacy form when breakdown invariant violated, got: {out}"
        );
        assert!(
            !out.contains("(article:"),
            "must not surface inconsistent breakdown, got: {out}"
        );
    }
}
