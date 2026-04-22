//! ArXiv paper structured extractor.
//!
//! Uses the public ArXiv API at `export.arxiv.org/api/query?id_list={id}`
//! which returns Atom XML. We parse just enough to surface title, authors,
//! abstract, categories, and the canonical PDF link. No HTML scraping
//! required and no auth.

use quick_xml::Reader;
use quick_xml::events::Event;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "arxiv",
    label: "ArXiv paper",
    description: "Returns paper metadata: title, authors, abstract, categories, primary category, PDF URL.",
    url_patterns: &[
        "https://arxiv.org/abs/{id}",
        "https://arxiv.org/abs/{id}v{n}",
        "https://arxiv.org/pdf/{id}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "arxiv.org" && host != "www.arxiv.org" {
        return false;
    }
    url.contains("/abs/") || url.contains("/pdf/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let id = parse_id(url)
        .ok_or_else(|| FetchError::Build(format!("arxiv: cannot parse id from '{url}'")))?;

    let api_url = format!("https://export.arxiv.org/api/query?id_list={id}");
    let resp = client.fetch(&api_url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "arxiv api returned status {}",
            resp.status
        )));
    }

    let entry = parse_atom_entry(&resp.html)
        .ok_or_else(|| FetchError::BodyDecode("arxiv: no <entry> in response".into()))?;
    if entry.title.is_none() && entry.summary.is_none() {
        return Err(FetchError::BodyDecode(format!(
            "arxiv: paper '{id}' returned empty entry (likely withdrawn or invalid id)"
        )));
    }

    Ok(json!({
        "url":              url,
        "id":               id,
        "arxiv_id":         entry.id,
        "title":            entry.title,
        "authors":          entry.authors,
        "abstract":         entry.summary.map(|s| collapse_whitespace(&s)),
        "published":        entry.published,
        "updated":          entry.updated,
        "primary_category": entry.primary_category,
        "categories":       entry.categories,
        "doi":              entry.doi,
        "comment":          entry.comment,
        "pdf_url":          entry.pdf_url,
        "abs_url":          entry.abs_url,
    }))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Parse an arxiv id from a URL. Strips the version suffix (`v2`, `v3`)
/// and the `.pdf` extension when present.
fn parse_id(url: &str) -> Option<String> {
    let after = url
        .split("/abs/")
        .nth(1)
        .or_else(|| url.split("/pdf/").nth(1))?;
    let stripped = after
        .split(['?', '#'])
        .next()?
        .trim_end_matches('/')
        .trim_end_matches(".pdf");
    // Strip optional version suffix, e.g. "2401.12345v2" → "2401.12345"
    let no_version = match stripped.rfind('v') {
        Some(i) if stripped[i + 1..].chars().all(|c| c.is_ascii_digit()) => &stripped[..i],
        _ => stripped,
    };
    if no_version.is_empty() {
        None
    } else {
        Some(no_version.to_string())
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Default)]
struct AtomEntry {
    id: Option<String>,
    title: Option<String>,
    summary: Option<String>,
    published: Option<String>,
    updated: Option<String>,
    primary_category: Option<String>,
    categories: Vec<String>,
    authors: Vec<String>,
    doi: Option<String>,
    comment: Option<String>,
    pdf_url: Option<String>,
    abs_url: Option<String>,
}

/// Parse the first `<entry>` block of an ArXiv Atom feed.
fn parse_atom_entry(xml: &str) -> Option<AtomEntry> {
    let mut reader = Reader::from_str(xml);
    let mut buf = Vec::new();

    // States
    let mut in_entry = false;
    let mut current: Option<&'static str> = None;
    let mut in_author = false;
    let mut in_author_name = false;
    let mut entry = AtomEntry::default();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => in_entry = true,
                    b"id" if in_entry && !in_author => current = Some("id"),
                    b"title" if in_entry => current = Some("title"),
                    b"summary" if in_entry => current = Some("summary"),
                    b"published" if in_entry => current = Some("published"),
                    b"updated" if in_entry => current = Some("updated"),
                    b"author" if in_entry => in_author = true,
                    b"name" if in_author => {
                        in_author_name = true;
                        current = Some("author_name");
                    }
                    b"category" if in_entry => {
                        // primary_category is namespaced (arxiv:primary_category)
                        // category is plain. quick-xml gives us local-name only,
                        // so we treat both as categories and take the first as
                        // primary.
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"term"
                                && let Ok(v) = attr.unescape_value()
                            {
                                let term = v.to_string();
                                if entry.primary_category.is_none() {
                                    entry.primary_category = Some(term.clone());
                                }
                                entry.categories.push(term);
                            }
                        }
                    }
                    b"link" if in_entry => {
                        let mut href = None;
                        let mut rel = None;
                        let mut typ = None;
                        for attr in e.attributes().flatten() {
                            match attr.key.as_ref() {
                                b"href" => href = attr.unescape_value().ok().map(|s| s.to_string()),
                                b"rel" => rel = attr.unescape_value().ok().map(|s| s.to_string()),
                                b"type" => typ = attr.unescape_value().ok().map(|s| s.to_string()),
                                _ => {}
                            }
                        }
                        if let Some(h) = href {
                            if typ.as_deref() == Some("application/pdf") {
                                entry.pdf_url = Some(h.clone());
                            }
                            if rel.as_deref() == Some("alternate") {
                                entry.abs_url = Some(h);
                            }
                        }
                    }
                    _ => current = None,
                }
            }
            Ok(Event::Empty(ref e)) => {
                // Self-closing tags (<link href="..." />). Same handling as Start.
                let local = e.local_name();
                if (local.as_ref() == b"link" || local.as_ref() == b"category") && in_entry {
                    let mut href = None;
                    let mut rel = None;
                    let mut typ = None;
                    let mut term = None;
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"href" => href = attr.unescape_value().ok().map(|s| s.to_string()),
                            b"rel" => rel = attr.unescape_value().ok().map(|s| s.to_string()),
                            b"type" => typ = attr.unescape_value().ok().map(|s| s.to_string()),
                            b"term" => term = attr.unescape_value().ok().map(|s| s.to_string()),
                            _ => {}
                        }
                    }
                    if let Some(t) = term {
                        if entry.primary_category.is_none() {
                            entry.primary_category = Some(t.clone());
                        }
                        entry.categories.push(t);
                    }
                    if let Some(h) = href {
                        if typ.as_deref() == Some("application/pdf") {
                            entry.pdf_url = Some(h.clone());
                        }
                        if rel.as_deref() == Some("alternate") {
                            entry.abs_url = Some(h);
                        }
                    }
                }
            }
            Ok(Event::Text(ref e)) => {
                if let (Some(field), Ok(text)) = (current, e.unescape()) {
                    let text = text.to_string();
                    match field {
                        "id" => entry.id = Some(text.trim().to_string()),
                        "title" => entry.title = append_text(entry.title.take(), &text),
                        "summary" => entry.summary = append_text(entry.summary.take(), &text),
                        "published" => entry.published = Some(text.trim().to_string()),
                        "updated" => entry.updated = Some(text.trim().to_string()),
                        "author_name" => entry.authors.push(text.trim().to_string()),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let local = e.local_name();
                match local.as_ref() {
                    b"entry" => break,
                    b"author" => in_author = false,
                    b"name" => in_author_name = false,
                    _ => {}
                }
                if !in_author_name {
                    current = None;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }

    if in_entry { Some(entry) } else { None }
}

/// Concatenate text fragments (long fields can be split across multiple
/// text events if they contain entities or CDATA).
fn append_text(prev: Option<String>, next: &str) -> Option<String> {
    match prev {
        Some(mut s) => {
            s.push_str(next);
            Some(s)
        }
        None => Some(next.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_arxiv_urls() {
        assert!(matches("https://arxiv.org/abs/2401.12345"));
        assert!(matches("https://arxiv.org/abs/2401.12345v2"));
        assert!(matches("https://arxiv.org/pdf/2401.12345.pdf"));
        assert!(!matches("https://arxiv.org/"));
        assert!(!matches("https://example.com/abs/foo"));
    }

    #[test]
    fn parse_id_strips_version_and_extension() {
        assert_eq!(
            parse_id("https://arxiv.org/abs/2401.12345"),
            Some("2401.12345".into())
        );
        assert_eq!(
            parse_id("https://arxiv.org/abs/2401.12345v3"),
            Some("2401.12345".into())
        );
        assert_eq!(
            parse_id("https://arxiv.org/pdf/2401.12345v2.pdf"),
            Some("2401.12345".into())
        );
    }

    #[test]
    fn collapse_whitespace_handles_newlines_and_tabs() {
        assert_eq!(collapse_whitespace("a   b\n\tc  "), "a b c");
    }
}
