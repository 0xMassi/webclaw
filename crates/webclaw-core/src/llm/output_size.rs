/// Output-size control: alternate output modes (summary, toc) plus
/// post-format byte-cap truncation with a clear footer.
///
/// Three orthogonal axes:
///   - `OutputMode` (full | summary | toc) selects what to emit
///   - `OutputFormat` (text/markdown vs json) is owned by the caller
///   - `max_output_bytes` caps the FINAL byte count after format emission
///
/// `summary` returns a navigation/link list extracted from the page.
/// `toc` returns the H1/H2 outline plus the first paragraph after each H2.
/// `truncate_with_footer` walks UTF-8 codepoint boundaries so it never
/// produces an invalid UTF-8 split.
use crate::types::ExtractionResult;

use super::body;
use super::links;
use super::metadata::build_metadata_header_with_opts;

// ---------------------------------------------------------------------------
// Summary mode — link/title list, no body
// ---------------------------------------------------------------------------

/// Build a markdown link list (`- [Title](URL)`) of all non-noise links on
/// the page. Includes the metadata header so callers can still see what
/// page the summary came from.
pub fn to_llm_summary(result: &ExtractionResult, url: Option<&str>) -> String {
    let links = collect_summary_links(result);
    let mut out = String::new();
    // M7: suppress the `> Status:` line in summary mode — the link list
    // is conceptually navigation, not protocol-level outcome.
    build_metadata_header_with_opts(&mut out, result, url, false);
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str("## Links\n");
    for (label, href) in &links {
        out.push_str(&format!("- [{label}]({href})\n"));
    }
    out.trim_end().to_string()
}

/// JSON form of the summary: an array of `{"title": ..., "url": ...}`.
pub fn to_json_summary(result: &ExtractionResult) -> String {
    let links = collect_summary_links(result);
    let arr: Vec<serde_json::Value> = links
        .into_iter()
        .map(|(title, url)| {
            serde_json::json!({
                "title": title,
                "url": url,
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Collect a deduplicated (label, href) list from the page, reusing the
/// same noise-filter the main LLM output uses so summary stays consistent
/// with the existing extraction.
fn collect_summary_links(result: &ExtractionResult) -> Vec<(String, String)> {
    // Run the existing body pipeline; it already produces a clean, deduped
    // (label, href) list with noise links filtered out.
    let processed = body::process_body(&result.content.markdown);
    let mut out: Vec<(String, String)> = Vec::with_capacity(processed.links.len());
    for (text, href) in processed.links {
        let label = links::clean_link_label(&text);
        if label.is_empty() {
            continue;
        }
        out.push((label, href));
    }
    out
}

// ---------------------------------------------------------------------------
// TOC mode — H1/H2 outline + first paragraph after each H2
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TocEntry {
    pub level: u8,
    pub heading: String,
    pub intro: String,
}

/// Build a markdown outline from the processed body. Each H1 / H2 is
/// emitted as a heading line; the first non-empty, non-heading paragraph
/// immediately after an H2 is emitted as its `intro`.
pub fn to_llm_toc(result: &ExtractionResult, url: Option<&str>) -> String {
    let entries = collect_toc_entries(result);

    let mut out = String::new();
    // M7: suppress the `> Status:` line in toc mode — the outline is
    // structural, not protocol-level outcome.
    build_metadata_header_with_opts(&mut out, result, url, false);
    if !out.is_empty() {
        out.push('\n');
    }

    for entry in &entries {
        let hashes = "#".repeat(entry.level as usize);
        out.push_str(&format!("{hashes} {}\n", entry.heading));
        if !entry.intro.is_empty() {
            out.push('\n');
            out.push_str(&entry.intro);
            out.push_str("\n\n");
        } else {
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

/// JSON form of the TOC: an array of `{"level": N, "heading": ..., "intro": ...}`.
pub fn to_json_toc(result: &ExtractionResult) -> String {
    let entries = collect_toc_entries(result);
    let arr: Vec<serde_json::Value> = entries
        .into_iter()
        .map(|e| {
            serde_json::json!({
                "level": e.level,
                "heading": e.heading,
                "intro": e.intro,
            })
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".to_string())
}

/// Walk the processed body text, pulling out H1/H2 headings and the first
/// paragraph that follows each H2.
pub(crate) fn collect_toc_entries(result: &ExtractionResult) -> Vec<TocEntry> {
    let processed = body::process_body(&result.content.markdown);
    let text = &processed.text;

    let mut entries: Vec<TocEntry> = Vec::new();
    let mut current_h2_idx: Option<usize> = None;
    let mut paragraph: String = String::new();
    let mut in_paragraph = false;

    let flush_paragraph =
        |paragraph: &mut String, in_paragraph: &mut bool, current_h2_idx: &mut Option<usize>, entries: &mut Vec<TocEntry>| {
            if *in_paragraph {
                let trimmed = paragraph.trim().to_string();
                if !trimmed.is_empty()
                    && let Some(idx) = *current_h2_idx
                    && entries[idx].intro.is_empty()
                {
                    entries[idx].intro = trimmed;
                    *current_h2_idx = None;
                }
                paragraph.clear();
                *in_paragraph = false;
            }
        };

    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("# ") {
            flush_paragraph(&mut paragraph, &mut in_paragraph, &mut current_h2_idx, &mut entries);
            entries.push(TocEntry {
                level: 1,
                heading: rest.trim().to_string(),
                intro: String::new(),
            });
            current_h2_idx = None;
        } else if let Some(rest) = trimmed.strip_prefix("## ") {
            flush_paragraph(&mut paragraph, &mut in_paragraph, &mut current_h2_idx, &mut entries);
            entries.push(TocEntry {
                level: 2,
                heading: rest.trim().to_string(),
                intro: String::new(),
            });
            current_h2_idx = Some(entries.len() - 1);
        } else if trimmed.starts_with("#") {
            // H3+ — ignore for the outline, but ends any in-progress intro paragraph.
            flush_paragraph(&mut paragraph, &mut in_paragraph, &mut current_h2_idx, &mut entries);
        } else if trimmed.is_empty() {
            flush_paragraph(&mut paragraph, &mut in_paragraph, &mut current_h2_idx, &mut entries);
        } else {
            // Body text. Only collect intros for the most-recent H2 with no intro yet.
            if let Some(idx) = current_h2_idx
                && entries[idx].intro.is_empty()
            {
                if in_paragraph {
                    paragraph.push(' ');
                }
                paragraph.push_str(trimmed);
                in_paragraph = true;
            }
        }
    }
    // End-of-text flush
    flush_paragraph(&mut paragraph, &mut in_paragraph, &mut current_h2_idx, &mut entries);

    entries
}

// ---------------------------------------------------------------------------
// Byte-cap truncation
// ---------------------------------------------------------------------------

/// Truncate `s` so the returned string is at most `cap` bytes long,
/// honoring UTF-8 codepoint boundaries and appending a footer that names
/// how many bytes were dropped.
///
/// - `cap == 0` is treated as "no cap" — returns `s` unchanged.
/// - If `s.len() <= cap`, no footer is appended.
/// - When truncation happens, the FOOTER is included inside the cap:
///   the kept-body bytes + footer bytes never exceed `cap` (best-effort —
///   if `cap` is smaller than the footer itself, the body is empty and
///   the footer alone is returned, possibly slightly over cap; this only
///   happens for absurdly small caps like `--max-output-bytes 50`).
pub fn truncate_with_footer(s: &str, cap: usize) -> String {
    if cap == 0 {
        return s.to_string();
    }
    let original_bytes = s.len();
    if original_bytes <= cap {
        return s.to_string();
    }

    // First pass: build a placeholder footer to learn its byte length.
    // We don't yet know `kept` (depends on cap minus footer), so we use
    // a worst-case estimate for the byte counts and rebuild once. Two
    // passes is fine and avoids fixed-point loops.
    let placeholder_footer = build_footer(original_bytes, original_bytes, original_bytes);
    let footer_max_len = placeholder_footer.len();
    // Reserve room for the footer + a separator newline. Without the
    // explicit '+1', the body can end mid-text and the inserted '\n'
    // before the footer pushes us 1 byte over the cap.
    let body_budget = cap.saturating_sub(footer_max_len).saturating_sub(1);

    // Walk to the largest codepoint boundary <= body_budget.
    let mut kept_bytes = 0usize;
    for (i, _) in s.char_indices() {
        if i > body_budget {
            break;
        }
        kept_bytes = i;
    }
    // If body_budget falls past end-of-string somehow, clamp.
    if kept_bytes > original_bytes {
        kept_bytes = original_bytes;
    }

    let dropped_bytes = original_bytes - kept_bytes;
    let footer = build_footer(original_bytes, dropped_bytes, kept_bytes);

    let mut out = String::with_capacity(kept_bytes + footer.len() + 1);
    out.push_str(&s[..kept_bytes]);
    // Make sure the footer starts on its own line if the body didn't end with one.
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&footer);
    out
}

fn build_footer(original_bytes: usize, dropped_bytes: usize, _kept_bytes: usize) -> String {
    format!(
        "[truncated: {dropped_bytes} more bytes — original output was {original_bytes} bytes; pass --max-output-bytes 0 to disable, or increase the cap]\n"
    )
}

/// JSON-aware truncation: when a JSON document is too large, we don't
/// truncate the JSON itself (that would produce invalid syntax). Instead
/// we emit a wrapper object that names the truncation and embeds a
/// best-effort string prefix of the original JSON.
///
/// This is what `--max-output-bytes N -f json` returns when the rendered
/// JSON would exceed N bytes.
pub fn truncate_json_with_wrapper(s: &str, cap: usize) -> String {
    if cap == 0 {
        return s.to_string();
    }
    let original_bytes = s.len();
    if original_bytes <= cap {
        return s.to_string();
    }

    // Build the wrapper skeleton first to learn its overhead, then size
    // the embedded `data` slice to fit under the cap. We escape it as a
    // JSON string so the document stays valid.
    let wrapper = |kept_bytes: usize, data_escaped: &str| -> String {
        serde_json::json!({
            "_truncated": true,
            "_original_bytes": original_bytes,
            "_truncated_bytes": original_bytes - kept_bytes,
            "_note": "pass --max-output-bytes 0 to disable, or increase the cap",
            "data": data_escaped,
        })
        .to_string()
    };

    // Estimate overhead with an empty data string.
    let overhead = wrapper(0, "").len();
    // Each character of data may take up to 6 bytes when escaped (\uXXXX),
    // but ASCII typically takes 1 — we conservatively budget for 2× growth
    // and iterate down if we overshoot.
    let mut body_budget = cap.saturating_sub(overhead).saturating_sub(8) / 2;
    if body_budget == 0 {
        body_budget = 1;
    }

    loop {
        // Walk to the largest codepoint boundary <= body_budget.
        let mut kept_bytes = 0usize;
        for (i, _) in s.char_indices() {
            if i > body_budget {
                break;
            }
            kept_bytes = i;
        }
        if kept_bytes > original_bytes {
            kept_bytes = original_bytes;
        }
        let escaped = serde_json::to_string(&s[..kept_bytes]).unwrap_or_else(|_| "\"\"".to_string());
        // Strip outer quotes from the escaped string for embedding.
        let inner = if escaped.len() >= 2 {
            &escaped[1..escaped.len() - 1]
        } else {
            ""
        };
        let candidate = wrapper(kept_bytes, inner);
        if candidate.len() <= cap || body_budget <= 1 {
            return candidate;
        }
        // Overshoot — shrink body_budget and retry.
        let shrink = (candidate.len() - cap).max(64);
        if body_budget <= shrink {
            body_budget = 1;
        } else {
            body_budget -= shrink;
        }
    }
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
                word_count_article: 0,
                word_count_chrome: 0,
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

    // -- truncation tests --

    #[test]
    fn test_max_output_bytes_truncates_correctly() {
        // Build a ~100KB ASCII input.
        let input = "a".repeat(100_000);
        let out = truncate_with_footer(&input, 4096);
        assert!(out.len() <= 4096, "got {} bytes, cap 4096", out.len());
        assert!(out.contains("[truncated:"), "footer missing: {out}");
        assert!(out.contains("100000 bytes"), "original byte count missing: {out}");
        // The dropped-byte count in the footer must equal original - kept.
        // Body kept = out.len() - footer_len. Footer ends with \n.
        let footer_start = out.find("[truncated:").expect("footer present");
        let body_kept = footer_start.saturating_sub(1); // minus the newline before the footer
        let dropped = 100_000usize.saturating_sub(body_kept);
        let needle = format!("[truncated: {dropped} more bytes");
        assert!(
            out.contains(&needle),
            "expected dropped={dropped} in footer; got: {}",
            &out[footer_start..]
        );
    }

    #[test]
    fn test_max_output_bytes_zero_means_unlimited() {
        let input = "a".repeat(100_000);
        let out = truncate_with_footer(&input, 0);
        assert_eq!(out, input);
        assert!(!out.contains("[truncated:"));
    }

    #[test]
    fn test_max_output_bytes_utf8_boundary() {
        // Mix multibyte and ASCII so the boundary lands mid-codepoint if naive.
        // 'é' is 2 bytes in UTF-8. Build a string where byte 4095 is in the
        // middle of an 'é'.
        let mut s = String::new();
        // 4094 ASCII bytes
        for _ in 0..4094 {
            s.push('a');
        }
        // Then an 'é' that straddles byte 4094..4096
        s.push('é');
        // Pad to make it big enough to need truncation.
        for _ in 0..1000 {
            s.push('b');
        }
        let cap = 4096;
        let out = truncate_with_footer(&s, cap);
        // The truncated form must be valid UTF-8 (String guarantees this,
        // but also assert no mid-codepoint by re-decoding).
        assert!(std::str::from_utf8(out.as_bytes()).is_ok());
        // It must contain the footer (we truncated).
        assert!(out.contains("[truncated:"), "footer missing");
        // Must not contain "ab" right at the cap (boundary should fall before 'é').
        // Verify the body part (before the footer line) ends at a valid char boundary.
        let footer_start = out.find("\n[truncated:").unwrap_or(out.len());
        let body = &out[..footer_start];
        // The last char must NOT be the first byte of a multibyte sequence alone.
        let _ = std::str::from_utf8(body.as_bytes()).expect("body is valid UTF-8");
    }

    // -- mode tests --

    #[test]
    fn test_mode_summary_returns_links_only() {
        let md = r"# Some Headline

This is body text that summary mode should NOT include.

Read more articles:

- [Story One](https://example.com/story1)
- [Story Two](https://example.com/story2)
- [Story Three](https://example.com/story3)
- [Story Four](https://example.com/story4)
- [Story Five](https://example.com/story5)
";
        let r = make_result(md);
        let out = to_llm_summary(&r, Some("https://example.com/"));
        // Should contain all 5 links.
        for n in ["Story One", "Story Two", "Story Three", "Story Four", "Story Five"] {
            assert!(out.contains(n), "summary missing link {n}: {out}");
        }
        // Should NOT contain the body sentence.
        assert!(
            !out.contains("This is body text"),
            "summary leaked body text: {out}"
        );
        // Should have a Links section header.
        assert!(out.contains("## Links"), "missing Links header: {out}");
    }

    #[test]
    fn test_mode_toc_returns_outline() {
        let md = r"# Top Level Title

Intro paragraph that should not be associated with H1.

## Section A

First paragraph of section A goes here.

More body text we don't want as intro.

## Section B

First paragraph of section B.

## Section C

First paragraph of section C.
";
        let r = make_result(md);
        let out = to_llm_toc(&r, Some("https://example.com/"));
        // Should have one H1 and three H2s.
        assert!(out.contains("# Top Level Title"), "missing H1: {out}");
        assert!(out.contains("## Section A"), "missing H2-A: {out}");
        assert!(out.contains("## Section B"), "missing H2-B: {out}");
        assert!(out.contains("## Section C"), "missing H2-C: {out}");
        // Should have the first paragraph for each H2.
        assert!(
            out.contains("First paragraph of section A"),
            "missing intro A: {out}"
        );
        assert!(
            out.contains("First paragraph of section B"),
            "missing intro B: {out}"
        );
        assert!(
            out.contains("First paragraph of section C"),
            "missing intro C: {out}"
        );
        // Should NOT contain the second-paragraph-after-A body line.
        assert!(
            !out.contains("More body text"),
            "toc leaked second paragraph: {out}"
        );

        // Structured entries: 1 H1 + 3 H2s.
        let entries = collect_toc_entries(&r);
        assert_eq!(entries.len(), 4, "expected 4 entries, got {entries:?}");
        assert_eq!(entries[0].level, 1);
        assert_eq!(entries[1].level, 2);
    }

    #[test]
    fn test_mode_summary_with_byte_cap() {
        // Generate a summary that's bigger than the cap, then verify cap applies.
        let mut md = String::from("# Lots of links\n\n");
        for i in 0..200 {
            md.push_str(&format!(
                "- [Story number {i} with a fairly long title]({})\n",
                format!("https://example.com/story-{i}")
            ));
        }
        let r = make_result(&md);
        let summary = to_llm_summary(&r, Some("https://example.com/"));
        assert!(summary.len() > 4096, "expected summary > cap; got {}", summary.len());
        let capped = truncate_with_footer(&summary, 4096);
        assert!(capped.len() <= 4096, "got {} bytes", capped.len());
        assert!(capped.contains("[truncated:"));
    }

    #[test]
    fn test_json_summary_shape() {
        let md = "# T\n\n- [A](https://example.com/a)\n- [B](https://example.com/b)\n";
        let r = make_result(md);
        let s = to_json_summary(&r);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["title"].as_str().unwrap(), "A");
        assert_eq!(arr[0]["url"].as_str().unwrap(), "https://example.com/a");
    }

    #[test]
    fn test_json_toc_shape() {
        let md = "# H1\n\n## A\n\nIntro A.\n\n## B\n\nIntro B.\n";
        let r = make_result(md);
        let s = to_json_toc(&r);
        let v: serde_json::Value = serde_json::from_str(&s).expect("valid JSON");
        let arr = v.as_array().expect("array");
        assert_eq!(arr.len(), 3);
        assert_eq!(arr[0]["level"].as_u64().unwrap(), 1);
        assert_eq!(arr[0]["heading"].as_str().unwrap(), "H1");
        assert_eq!(arr[1]["level"].as_u64().unwrap(), 2);
        assert_eq!(arr[1]["intro"].as_str().unwrap(), "Intro A.");
    }

    #[test]
    fn test_json_truncation_remains_valid_json() {
        // Build a big serialized JSON.
        let huge = serde_json::json!({
            "data": "x".repeat(100_000),
        });
        let s = serde_json::to_string_pretty(&huge).unwrap();
        let out = truncate_json_with_wrapper(&s, 4096);
        // Resulting string must parse as JSON.
        let parsed: serde_json::Value =
            serde_json::from_str(&out).expect("truncated JSON should still parse");
        assert_eq!(parsed["_truncated"].as_bool(), Some(true));
        assert!(parsed["_original_bytes"].as_u64().is_some());
        assert!(out.len() <= 4096);
    }
}
