/// Image handling for LLM output: linked image conversion, logo detection,
/// standalone image stripping, and bare image reference removal.
use std::sync::LazyLock as Lazy;

use regex::Regex;

use super::cleanup::is_asset_label;

// ---------------------------------------------------------------------------
// Linked image conversion: [![alt](img)](url) -> [alt](url)
// ---------------------------------------------------------------------------

/// Matches `[![alt](img-url)](link-url)` -- an image wrapped in a link.
static LINKED_IMAGE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[!\[([^\]]*)\]\([^)]+\)\]\(([^)]+)\)").unwrap());

/// Matches empty markdown links `[](url)` left after image stripping.
pub(crate) static EMPTY_LINK_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\[\s*\]\([^)]+\)").unwrap());

/// Convert linked images to plain links, preserving the alt text and link target.
/// Adds a newline after each to prevent text mashing when multiple are adjacent.
pub(crate) fn convert_linked_images(input: &str) -> String {
    LINKED_IMAGE_RE
        .replace_all(input, |caps: &regex::Captures| {
            let alt = caps.get(1).map_or("", |m| m.as_str());
            let href = caps.get(2).map_or("", |m| m.as_str());
            format!("[{alt}]({href})\n")
        })
        .into_owned()
}

// ---------------------------------------------------------------------------
// Logo image collapsing
// ---------------------------------------------------------------------------

/// Regex matching a line that is *only* a markdown image (with optional whitespace).
static IMAGE_LINE_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^!\[([^\]]*)\]\([^)]+\)\s*$").unwrap());

/// Collapse consecutive image-only lines into a comma-separated summary
/// of their alt texts (for logo bars, partner grids, etc.).
pub(crate) fn collapse_logo_images(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < lines.len() {
        // Check if this starts a run of consecutive image-only lines
        if IMAGE_LINE_RE.is_match(lines[i].trim()) {
            let mut alts: Vec<String> = Vec::new();
            let start = i;
            while i < lines.len() {
                let trimmed = lines[i].trim();
                // Allow blank lines between images in the same run
                if trimmed.is_empty() {
                    i += 1;
                    continue;
                }
                if let Some(caps) = IMAGE_LINE_RE.captures(trimmed) {
                    let alt = caps.get(1).map_or("", |m| m.as_str()).trim().to_string();
                    if !alt.is_empty() {
                        alts.push(alt);
                    }
                    i += 1;
                } else {
                    break;
                }
            }

            let image_count = if alts.is_empty() {
                i - start
            } else {
                alts.len()
            };

            if image_count >= 2 && !alts.is_empty() {
                out.push_str(&alts.join(", "));
                out.push('\n');
            } else if image_count == 1 && !alts.is_empty() && alts[0].len() > 30 {
                out.push_str(&alts[0]);
                out.push('\n');
            }
            // else: single image with short/empty alt -- drop entirely
        } else {
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Remaining inline image stripping
// ---------------------------------------------------------------------------

/// Matches `![alt](url)` anywhere in a line, including multiple on the same line.
static INLINE_IMAGE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]+\)").unwrap());

/// Strip inline images. For multi-image lines, separate short alts (logos)
/// from long alts (descriptive) so they don't get mixed together.
pub(crate) fn strip_remaining_images(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for line in input.lines() {
        let image_matches: Vec<_> = INLINE_IMAGE_RE.find_iter(line).collect();

        if image_matches.len() >= 2 {
            // Separate short alts (brand names/logos) from long alts (descriptions)
            let mut short_alts: Vec<&str> = Vec::new();
            let mut long_alts: Vec<&str> = Vec::new();

            for caps in INLINE_IMAGE_RE.captures_iter(line) {
                let alt = caps.get(1).map_or("", |m| m.as_str()).trim();
                // Skip empty alts and quoted-empty alts like `""`
                if alt.is_empty() || alt == "\"\"" {
                    continue;
                }
                if alt.len() <= 30 {
                    short_alts.push(alt);
                } else {
                    long_alts.push(alt);
                }
            }

            // Filter out CMS asset labels from alt texts before output
            short_alts.retain(|alt| !is_asset_label(alt));
            long_alts.retain(|alt| !is_asset_label(alt));

            // Remove images, then strip empty link remnants [](url)
            let remaining = INLINE_IMAGE_RE.replace_all(line, "");
            let remaining = EMPTY_LINK_RE.replace_all(&remaining, "");
            let remaining = remaining.trim();

            if !short_alts.is_empty() {
                if !remaining.is_empty() {
                    out.push_str(remaining);
                    out.push('\n');
                }
                out.push_str(&short_alts.join(", "));
                out.push('\n');
            } else if !remaining.is_empty() {
                out.push_str(remaining);
                out.push('\n');
            }

            // Long alts on their own lines (descriptions, not logos)
            for alt in &long_alts {
                out.push_str(alt);
                out.push('\n');
            }
        } else {
            // 0 or 1 image -- keep long alt text, drop short/empty/CMS labels
            let replaced = INLINE_IMAGE_RE.replace_all(line, |caps: &regex::Captures| {
                let alt = caps.get(1).map_or("", |m| m.as_str()).trim();
                if alt.len() > 30 && !is_asset_label(alt) {
                    alt.to_string()
                } else {
                    String::new()
                }
            });
            out.push_str(&replaced);
            out.push('\n');
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Bare image file reference stripping
// ---------------------------------------------------------------------------

const IMAGE_EXTENSIONS: &[&str] = &[
    ".webp", ".svg", ".png", ".jpg", ".jpeg", ".gif", ".avif", ".ico", ".bmp",
];

/// Strip lines that are just bare image filenames or image URLs.
/// Keeps lines where an image filename appears within a larger sentence.
pub(crate) fn strip_bare_image_refs(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for line in input.lines() {
        let trimmed = line.trim();

        if !trimmed.is_empty() && is_bare_image_ref(trimmed) {
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

/// A line is a bare image reference if it's a single token ending with an image extension.
/// Catches filenames ("hero.webp") and URLs ("https://cdn.example.com/logo.svg").
fn is_bare_image_ref(line: &str) -> bool {
    if line.starts_with('#')
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("```")
        || line.starts_with("> ")
    {
        return false;
    }

    if line.contains(' ') {
        return false;
    }

    let lower = line.to_lowercase();
    IMAGE_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_image_conversion() {
        let input = "[![docs](https://img/d.png)](https://docs.example.com)";
        let result = convert_linked_images(input);
        assert!(result.contains("[docs](https://docs.example.com)"));
        assert!(!result.contains("!["));
    }

    #[test]
    fn bare_image_ref_detected() {
        assert!(is_bare_image_ref("hero.webp"));
        assert!(is_bare_image_ref("https://cdn.example.com/logo.svg"));
        assert!(!is_bare_image_ref("The file output.png is saved to disk."));
        assert!(!is_bare_image_ref("# heading.png"));
    }
}
