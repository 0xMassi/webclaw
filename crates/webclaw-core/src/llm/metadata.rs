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
        out.push_str(&format!("> Word count: {}\n", meta.word_count));
    }
}
