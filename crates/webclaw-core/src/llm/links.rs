/// Link extraction, deduplication, noise filtering, and label formatting
/// for the LLM output's deduplicated links section.
use std::collections::HashSet;

use once_cell::sync::Lazy;
use regex::Regex;

// ---------------------------------------------------------------------------
// Link extraction
// ---------------------------------------------------------------------------

/// Matches `[text](url)`. Images are already stripped, so no `!` prefix concern.
static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]*)\]\(([^)]+)\)").unwrap());

/// Extract all links from markdown, replacing inline `[text](url)` with just `text`.
/// Returns the cleaned text and a deduplicated list of (label, href) pairs.
pub(crate) fn extract_and_strip_links(input: &str) -> (String, Vec<(String, String)>) {
    let mut links: Vec<(String, String)> = Vec::new();
    let mut seen_hrefs: HashSet<String> = HashSet::new();

    let replaced = LINK_RE.replace_all(input, |caps: &regex::Captures| {
        let text = caps.get(1).map_or("", |m| m.as_str()).trim().to_string();
        let href = caps.get(2).map_or("", |m| m.as_str()).trim().to_string();

        let skip = href.starts_with('#')
            || href.starts_with("javascript:")
            || href.is_empty()
            || is_noise_link(&text, &href);

        if !skip && !text.is_empty() && seen_hrefs.insert(href.clone()) {
            links.push((text.clone(), href));
        }

        text
    });

    (replaced.into_owned(), links)
}

/// Links that are noise for LLM consumption: internal actions, timestamps,
/// user profiles, generic short text.
fn is_noise_link(text: &str, href: &str) -> bool {
    let t = text.to_lowercase();

    // Generic action links
    if matches!(
        t.as_str(),
        "hide"
            | "flag"
            | "reply"
            | "favorite"
            | "unflag"
            | "vouch"
            | "next"
            | "prev"
            | "previous"
            | "more"
    ) {
        return true;
    }

    // Timestamp text ("1 hour ago", "5 minutes ago", "yesterday")
    if t.ends_with(" ago") || t == "yesterday" || t == "just now" {
        return true;
    }

    // Single-char text that's not meaningful (but keep letters -- "X", "Go", etc.)
    if text.len() == 1 && !text.chars().next().unwrap_or(' ').is_alphanumeric() {
        return true;
    }

    // Bare integer labels are usually comment counts, vote counts, or page
    // numbers. The label alone carries no useful link context for an LLM.
    if !text.is_empty() && text.len() <= 4 && text.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }

    // In-page comment/discussion fragments that survived the bare-fragment
    // check because the href is a full URL with a comment fragment.
    if href.contains("#comment-stream") || href.contains("#comments") || href.contains("#disqus") {
        return true;
    }

    // Internal user profile / action URLs (HN-style)
    if href.contains("/user?id=")
        || href.contains("/hide?id=")
        || href.contains("/from?site=")
        || href.contains("/flag?id=")
    {
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// Link label cleaning
// ---------------------------------------------------------------------------

static MD_MARKERS_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"#{1,6}\s+|\*{1,2}|_{1,2}|`").unwrap());

static A11Y_LABEL_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(?:\s*,?\s*(?:opens (?:in )?(?:a )?new (?:tab|window)|opens external (?:link|website))\b\.?|\s*,\s*external link\b\.?|\s+external link\b\.?$)",
    )
    .unwrap()
});

/// Clean a link label: strip markdown, dedup repeated phrases, truncate.
pub(crate) fn clean_link_label(raw: &str) -> String {
    // Strip markdown markers
    let label = MD_MARKERS_RE.replace_all(raw, "").to_string();
    // Strip a11y link chrome ("opens new tab", etc.)
    let label = A11Y_LABEL_RE.replace_all(&label, "").to_string();
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");

    // Dedup repeated phrases in label
    let label = dedup_label_phrase(&label);

    // Truncate to ~80 chars (UTF-8 safe)
    if label.len() > 80 {
        // Find last whitespace boundary at or before 80 bytes
        let mut end = None;
        for (i, _) in label.char_indices() {
            if i > 80 {
                break;
            }
            if i > 0 && label.as_bytes()[i - 1].is_ascii_whitespace() {
                end = Some(i);
            }
        }
        let end = end.unwrap_or_else(|| {
            // No whitespace found -- find char boundary near 80
            label
                .char_indices()
                .map(|(i, _)| i)
                .find(|&i| i >= 80)
                .unwrap_or(label.len())
        });
        format!("{}...", label[..end].trim_end())
    } else {
        label
    }
}

/// If a label contains the same phrase twice (e.g., "X Y Z X Y Z"), return just one copy.
fn dedup_label_phrase(label: &str) -> String {
    let len = label.len();
    if len < 8 {
        return label.to_string();
    }
    // Try split at each whitespace boundary
    for (i, _) in label.match_indices(' ') {
        let left = label[..i].trim();
        let right = label[i + 1..].trim();
        if left.len() >= 4 && left.eq_ignore_ascii_case(right) {
            return left.to_string();
        }
    }
    label.to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_label_truncated() {
        let long = "The quick brown fox jumps over the lazy dog and then runs across the field to find more interesting things to do on a sunny afternoon";
        let result = clean_link_label(long);
        assert!(result.len() <= 84, "got len {}: {result}", result.len());
        assert!(result.ends_with("..."), "got: {result}");
    }

    #[test]
    fn link_label_markdown_stripped() {
        assert_eq!(clean_link_label("## Hello **world**"), "Hello world");
    }

    #[test]
    fn link_label_duplicate_deduped() {
        assert_eq!(
            clean_link_label("Express Delivery Express Delivery"),
            "Express Delivery"
        );
    }

    #[test]
    fn link_label_short_unchanged() {
        assert_eq!(clean_link_label("Click here"), "Click here");
    }

    #[test]
    fn noise_link_detected() {
        assert!(is_noise_link("hide", "https://example.com"));
        assert!(is_noise_link("5 minutes ago", "https://example.com"));
        assert!(is_noise_link("user", "https://hn.com/user?id=foo"));
        assert!(!is_noise_link("Rust docs", "https://rust-lang.org"));
    }

    #[test]
    fn link_label_preserves_external_link_prose() {
        assert_eq!(
            clean_link_label("Research found an external link between incidents"),
            "Research found an external link between incidents"
        );
    }

    #[test]
    fn link_label_strips_terminal_external_link_chrome() {
        assert_eq!(
            clean_link_label("Reuters story external link"),
            "Reuters story"
        );
    }
}
