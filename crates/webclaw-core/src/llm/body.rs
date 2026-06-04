/// Body processing pipeline for LLM output.
///
/// Orchestrates the multi-step pipeline that transforms raw markdown into
/// token-efficient LLM text. Each step is implemented in a sibling module
/// (cleanup, images, links) -- this module just wires them together in order.
use std::collections::{HashMap, HashSet};

use once_cell::sync::Lazy;
use regex::Regex;

use super::cleanup;
use super::images;
use super::links;

pub(crate) struct ProcessedBody {
    pub text: String,
    pub links: Vec<(String, String)>,
}

/// Run the full body processing pipeline on extracted markdown.
/// Step ordering matters: entity decode -> images -> links -> dedup -> cleanup.
pub(crate) fn process_body(markdown: &str) -> ProcessedBody {
    // 0a. Decode leaked HTML entities (&nbsp; &amp; &#123; etc.)
    let text = cleanup::decode_html_entities(markdown);

    // 0b. Strip invisible Unicode (zero-width spaces, joiners, soft hyphens)
    let text = cleanup::strip_invisible_unicode(&text);

    // 0c. Strip leaked JavaScript (framework hydration, self.__wrap_n, etc.)
    let text = cleanup::strip_leaked_js(&text);

    // 0c2. Strip a11y link chrome ("opens new tab", external link hints)
    let text = cleanup::strip_a11y_link_chrome(&text);

    // 0d. Collapse spaced-out text (CSS animation artifacts like "S t a r t")
    // Must run before any dedup -- spaced text confuses word-based dedup.
    let text = cleanup::collapse_spaced_text(&text);

    // 1. Convert linked images [![alt](img)](url) -> [alt](url)\n
    let text = images::convert_linked_images(&text);

    // a. Collapse consecutive image-only lines into a summary
    let text = images::collapse_logo_images(&text);

    // b. Strip remaining standalone image markdown
    let text = images::strip_remaining_images(&text);

    // b2. Strip bare image file references (e.g., "hero.webp", "https://cdn.example.com/logo.svg")
    let text = images::strip_bare_image_refs(&text);

    // c. Strip bold/italic markers
    let text = cleanup::strip_emphasis(&text);

    // c2. Strip alt text noise (descriptive image captions, broken image refs, social avatars)
    let text = cleanup::strip_alt_text_noise(&text);

    // c3. Strip UI control text (Material Icons ligatures, nav arrows)
    //     Runs AFTER emphasis stripping so *navigate_before* -> navigate_before is caught.
    let text = cleanup::strip_ui_control_text(&text);

    // c4. Strip long alt-text descriptions ("An illustration of...", 80+ chars)
    let text = cleanup::strip_long_alt_descriptions(&text);

    // c5. Strip CSS artifacts (@keyframes, inline CSS blocks)
    let text = cleanup::strip_css_artifacts(&text);

    // c6. Collapse long unstructured word/name lists (contributor names, API lists)
    let text = cleanup::collapse_word_lists(&text);

    // c7. Dedup adjacent duplicate descriptions (card layouts with repeated text)
    let text = cleanup::dedup_adjacent_descriptions(&text);

    // d. Extract links, replace inline `[text](url)` with just `text`
    let (text, extracted_links) = links::extract_and_strip_links(&text);

    // d1. Strip bare-integer paragraphs after link extraction, so
    // `[0](#comments)` collapses to `0` before the paragraph-aware check.
    let text = cleanup::strip_bare_number_lines(&text);

    // d2. Run UI-control stripping again after link extraction. Lines like
    // `[0](url) Next` become `0 Next`, which is pure pagination chrome.
    let text = cleanup::strip_ui_control_text(&text);

    // d3. Collapse repeated adjacent phrases on the same line
    // (responsive variants: "Read more Read more Read more" -> "Read more")
    let text = dedup_repeated_phrases(&text);

    // e. Deduplicate heading + following paragraph
    let text = dedup_heading_paragraph(&text);

    // e2. Remove plain text lines that duplicate a heading elsewhere
    let text = dedup_text_against_headings(&text);

    // e3. Remove non-adjacent duplicate headings (same heading text far apart)
    let text = dedup_duplicate_headings(&text);

    // f. Remove empty headings
    let text = strip_empty_headings(&text);

    // g. Strip CMS asset labels (e.g., "Homepage | Agents 26 | Bento | Desktop")
    let text = cleanup::strip_asset_labels(&text);

    // g2. Strip decorative CSS class text (e.g., "text-4xl font-bold tracking-tight")
    let text = cleanup::strip_css_class_lines(&text);

    // h. Collapse whitespace (max 1 blank line)
    let text = cleanup::collapse_whitespace(&text);

    // h2. Deduplicate repeated content blocks (carousels, animation dupes)
    let text = dedup_content_blocks(&text);

    // h3. Line-level dedup within blocks (catches carousel items on separate lines
    //     within a single block, e.g. repeated customer stories)
    let text = dedup_lines(&text);

    // h4. Dedup repeated comma-separated lists (logo carousels that repeat the
    //     full set for infinite scroll: "a, b, c, a, b, c" -> "a, b, c")
    let text = dedup_comma_lists(&text);

    // i. Strip trailing empty headings (heading followed only by headings/EOF)
    let text = strip_trailing_empty_headings(&text);

    // j. Strip empty code blocks (``` with nothing between fences)
    let text = strip_empty_code_blocks(&text);

    // k. Collapse whitespace again after heading/code-block removal
    let text = cleanup::collapse_whitespace(&text);

    // l. Merge orphaned stat lines with their descriptions
    let text = merge_stat_lines(&text);

    ProcessedBody {
        text,
        links: extracted_links,
    }
}

// ---------------------------------------------------------------------------
// Repeated phrase dedup
// ---------------------------------------------------------------------------

static HEADING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(#{1,6})\s+(.+)$").unwrap());

/// Responsive HTML often produces "Read more Read more Read more" after link
/// stripping. Collapse N consecutive identical phrases into one.
fn dedup_repeated_phrases(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_code_block = false;

    for line in input.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_code_block || trimmed.is_empty() || trimmed.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let deduped = collapse_repeated_in_line(trimmed);
        out.push_str(&deduped);
        out.push('\n');
    }

    out
}

/// Detect repeating cycles in long word sequences. Tries multiple starting
/// offsets to handle lines with a short prefix before the carousel begins.
/// Returns the deduplicated line if a cycle (2+ repeats) is found.
fn detect_long_line_cycle(words: &[&str]) -> Option<String> {
    // Try starting offsets: 0, then 1..=15 to skip short prefixes
    for start in 0..=15.min(words.len().saturating_sub(100)) {
        let slice = &words[start..];
        if slice.len() < 100 {
            break;
        }

        // Try exact N-copy cycles first
        for n_copies in (2..=5).rev() {
            if slice.len() % n_copies != 0 {
                continue;
            }
            let cycle_len = slice.len() / n_copies;
            if cycle_len < 20 {
                continue;
            }
            let pattern = &slice[..cycle_len];
            if slice.chunks(cycle_len).all(|chunk| chunk == pattern) {
                let mut result: Vec<&str> = words[..start].to_vec();
                result.extend_from_slice(pattern);
                return Some(result.join(" "));
            }
        }

        // Try cycle with trailing remainder
        for cycle_len in (30..=slice.len() / 2).rev().step_by(1) {
            let pattern = &slice[..cycle_len];
            let mut pos = cycle_len;
            let mut copies = 1;
            while pos + cycle_len <= slice.len() && &slice[pos..pos + cycle_len] == pattern {
                pos += cycle_len;
                copies += 1;
            }
            if copies >= 2 {
                let mut result: Vec<&str> = words[..start].to_vec();
                result.extend_from_slice(pattern);
                // Append any trailing remainder (partial repeat or suffix)
                let remaining_start = start + pos;
                if remaining_start < words.len() {
                    result.extend_from_slice(&words[remaining_start..]);
                }
                return Some(result.join(" "));
            }
            // Only try a few cycle lengths per offset to avoid O(n^2)
            if cycle_len < slice.len() / 2 - 50 {
                break;
            }
        }
    }

    None
}

/// Given "A B C Read more Read more Read more D" -> "A B C Read more D"
/// For very long lines (>100 words), first tries full-line cycle detection
/// to catch carousel-style repeats where the entire content block repeats N times.
pub(crate) fn collapse_repeated_in_line(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 4 {
        return line.to_string();
    }

    // For long lines, try cycle detection. The carousel may start after a short
    // prefix ("Join us on Discord ..."), so try multiple starting offsets.
    if words.len() > 100
        && let Some(deduped) = detect_long_line_cycle(&words)
    {
        return deduped;
    }

    // Standard sliding window for shorter repeated phrases (2-20 words)
    let mut result: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    let max_phrase = (words.len() / 2).min(20);

    while i < words.len() {
        let mut found_repeat = false;
        for phrase_len in (2..=max_phrase).rev() {
            if i + phrase_len * 2 > words.len() {
                continue;
            }
            let phrase = &words[i..i + phrase_len];
            let next = &words[i + phrase_len..i + phrase_len * 2];
            if phrase == next {
                result.extend_from_slice(phrase);
                let mut j = i + phrase_len;
                while j + phrase_len <= words.len() && &words[j..j + phrase_len] == phrase {
                    j += phrase_len;
                }
                i = j;
                found_repeat = true;
                break;
            }
        }
        if !found_repeat {
            result.push(words[i]);
            i += 1;
        }
    }

    result.join(" ")
}

// ---------------------------------------------------------------------------
// Heading + paragraph dedup
// ---------------------------------------------------------------------------

fn dedup_heading_paragraph(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < lines.len() {
        if let Some(h_caps) = HEADING_RE.captures(lines[i].trim()) {
            let heading_text = h_caps.get(2).unwrap().as_str().trim();
            let heading_prefix = h_caps.get(1).unwrap().as_str();

            // Look ahead past blank lines for the next non-blank line
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }

            if j < lines.len() {
                let next_text = lines[j].trim();
                if !HEADING_RE.is_match(next_text) && text_is_duplicate(heading_text, next_text) {
                    let merged = if next_text.len() > heading_text.len() {
                        next_text
                    } else {
                        heading_text
                    };
                    out.push_str(&format!("{heading_prefix} {merged}\n"));
                    i = j + 1;
                    continue;
                }
            }
        }

        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }

    out
}

/// Check if a paragraph is a duplicate of a heading.
fn text_is_duplicate(heading: &str, paragraph: &str) -> bool {
    let h = heading.to_lowercase();
    let p = paragraph.to_lowercase();
    h == p || p.starts_with(&h) || h.starts_with(&p)
}

// ---------------------------------------------------------------------------
// Text-against-headings dedup
// ---------------------------------------------------------------------------

/// If a non-heading text line exactly matches a heading's text anywhere in the
/// document, remove the plain line (the heading already conveys the information).
fn dedup_text_against_headings(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();

    let heading_texts: HashSet<String> = lines
        .iter()
        .filter_map(|line| {
            HEADING_RE
                .captures(line.trim())
                .map(|caps| caps.get(2).unwrap().as_str().trim().to_lowercase())
        })
        .collect();

    if heading_texts.is_empty() {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());

    for line in &lines {
        let trimmed = line.trim();

        // Keep blank lines and headings unconditionally
        if trimmed.is_empty() || HEADING_RE.is_match(trimmed) {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Drop non-heading lines whose text matches a heading
        if heading_texts.contains(&trimmed.to_lowercase()) {
            continue;
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

// ---------------------------------------------------------------------------
// Duplicate heading dedup
// ---------------------------------------------------------------------------

/// Remove duplicate headings that appear far apart in the document. Keep the
/// first occurrence and remove subsequent duplicates along with matching content.
fn dedup_duplicate_headings(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();

    let mut heading_positions: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(caps) = HEADING_RE.captures(line.trim()) {
            let level = caps.get(1).unwrap().as_str();
            let text = caps.get(2).unwrap().as_str().trim();
            let key = format!("{} {}", level, normalize_heading_key(text));
            if !key.is_empty() {
                heading_positions.entry(key).or_default().push(i);
            }
        }
    }

    let mut skip: HashSet<usize> = HashSet::new();

    for positions in heading_positions.values() {
        if positions.len() < 2 {
            continue;
        }

        let first_idx = positions[0];
        let first_following = collect_following_content(&lines, first_idx);

        for &dup_idx in &positions[1..] {
            skip.insert(dup_idx);

            let dup_following = collect_following_content(&lines, dup_idx);
            for (offset, dup_line) in dup_following.iter().enumerate() {
                if offset < first_following.len()
                    && normalize_heading_key(dup_line)
                        == normalize_heading_key(&first_following[offset])
                {
                    let actual_idx = find_content_line_index(&lines, dup_idx, offset);
                    skip.insert(actual_idx);
                } else {
                    break;
                }
            }
        }
    }

    if skip.is_empty() {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    for (i, line) in lines.iter().enumerate() {
        if skip.contains(&i) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }

    out
}

/// Normalize heading text for dedup comparison: lowercase, strip punctuation.
fn normalize_heading_key(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace())
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Collect non-blank, non-heading content lines immediately following a heading.
fn collect_following_content(lines: &[&str], heading_idx: usize) -> Vec<String> {
    let mut content = Vec::new();
    let mut i = heading_idx + 1;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() || HEADING_RE.is_match(trimmed) {
            break;
        }
        content.push(trimmed.to_string());
        i += 1;
    }
    content
}

/// Find the actual line index for the Nth content line after a heading.
fn find_content_line_index(lines: &[&str], heading_idx: usize, content_offset: usize) -> usize {
    let mut i = heading_idx + 1;
    while i < lines.len() && lines[i].trim().is_empty() {
        i += 1;
    }
    i + content_offset
}

// ---------------------------------------------------------------------------
// Empty heading stripping
// ---------------------------------------------------------------------------

fn strip_empty_headings(input: &str) -> String {
    let mut out = String::with_capacity(input.len());

    for line in input.lines() {
        if let Some(h_caps) = HEADING_RE.captures(line.trim()) {
            let heading_text = h_caps.get(2).unwrap().as_str().trim();
            // Strip empty headings, headings with only invisible chars (ZWJ, NBSP),
            // and noise headings like "Footer", "Header", "Navigation"
            if heading_text.is_empty()
                || heading_text.chars().all(|c| !c.is_alphanumeric())
                || is_noise_heading(heading_text)
            {
                continue;
            }
        }
        out.push_str(line);
        out.push('\n');
    }

    out
}

/// Headings that are structural noise, not content.
fn is_noise_heading(text: &str) -> bool {
    const NOISE: &[&str] = &["footer", "header", "navigation", "sidebar", "menu"];
    let lower = text.to_lowercase();
    NOISE.iter().any(|n| lower == *n)
}

// ---------------------------------------------------------------------------
// Stat line merging
// ---------------------------------------------------------------------------

/// A short line (<=25 chars) like "100M+" or "99.99% uptime" followed by blank
/// lines then a descriptive line is a single stat -- merge them into one line.
fn merge_stat_lines(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    let mut in_code_block = false;

    while i < lines.len() {
        let trimmed = lines[i].trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
            continue;
        }

        if in_code_block {
            out.push_str(lines[i]);
            out.push('\n');
            i += 1;
            continue;
        }

        let len = trimmed.len();

        // Candidate: non-blank, non-structural, short line
        if len > 0 && len <= 25 && !is_structural_line(trimmed) {
            // Look ahead past blank lines for the next content line
            let mut j = i + 1;
            while j < lines.len() && lines[j].trim().is_empty() {
                j += 1;
            }

            // If we skipped at least one blank and the next line is a non-structural
            // content line, and the merged result fits in ~120 chars, merge them.
            if j > i + 1 && j < lines.len() {
                let next = lines[j].trim();
                if !next.is_empty() && !is_structural_line(next) && len + 1 + next.len() <= 120 {
                    out.push_str(trimmed);
                    out.push(' ');
                    out.push_str(next);
                    out.push('\n');
                    i = j + 1;
                    continue;
                }
            }
        }

        out.push_str(lines[i]);
        out.push('\n');
        i += 1;
    }

    out.trim().to_string()
}

/// Lines that should never be merged: headings, list items, code fences.
fn is_structural_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("```")
        || line.starts_with("> ")
}

// ---------------------------------------------------------------------------
// Content block dedup (carousels, animation dupes)
// ---------------------------------------------------------------------------

/// Minimum paragraph length to be eligible for deduplication.
/// Short text like "Learn more" or "Read more" can legitimately repeat.
const DEDUP_MIN_CHARS: usize = 20;

/// Number of leading words used as a prefix fingerprint for near-duplicate detection.
const DEDUP_PREFIX_WORDS: usize = 10;

/// Normalize text for fingerprinting: lowercase, collapse whitespace, strip punctuation.
fn normalize_fingerprint(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| c.is_alphanumeric() || *c == ' ')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract the first N words as a prefix fingerprint for near-duplicate matching.
fn prefix_fingerprint(normalized: &str) -> Option<String> {
    let words: Vec<&str> = normalized.split_whitespace().collect();
    if words.len() >= DEDUP_PREFIX_WORDS {
        Some(words[..DEDUP_PREFIX_WORDS].join(" "))
    } else {
        None
    }
}

/// Remove duplicate content blocks that appear when sites duplicate DOM subtrees
/// for carousels, sliders, or animation effects. Splits on blank-line boundaries,
/// fingerprints each block, and drops exact or near-duplicate repeats.
/// Short blocks (< 20 chars) are exempt -- headings and CTAs legitimately repeat.
fn dedup_content_blocks(input: &str) -> String {
    let blocks: Vec<&str> = input
        .split("\n\n")
        .filter(|b| !b.trim().is_empty())
        .collect();

    let mut seen_exact: HashSet<String> = HashSet::new();
    let mut seen_prefix: HashSet<String> = HashSet::new();
    let mut kept: Vec<String> = Vec::with_capacity(blocks.len());
    let mut in_code_block = false;

    for block in &blocks {
        let has_fence = block.lines().any(|l| l.trim_start().starts_with("```"));

        // Inside a code block or block contains a fence: preserve as-is (no trim)
        if in_code_block || has_fence {
            kept.push(block.to_string());
            for line in block.lines() {
                if line.trim_start().starts_with("```") {
                    in_code_block = !in_code_block;
                }
            }
            continue;
        }

        let trimmed = block.trim();

        // Short blocks are exempt -- headings, CTAs, etc. can repeat legitimately
        if trimmed.len() < DEDUP_MIN_CHARS {
            kept.push(trimmed.to_string());
            continue;
        }

        // Structural lines (headings, code fences, lists) are exempt individually,
        // but multi-line blocks containing them are still checked
        if trimmed.lines().count() == 1 && is_structural_line(trimmed) {
            kept.push(trimmed.to_string());
            continue;
        }

        let fp = normalize_fingerprint(trimmed);

        // Exact duplicate check
        if !seen_exact.insert(fp.clone()) {
            continue; // Already seen this exact block
        }

        // Near-duplicate: same first N words
        if let Some(pfp) = prefix_fingerprint(&fp)
            && !seen_prefix.insert(pfp)
        {
            continue; // Near-duplicate of a previously seen block
        }

        kept.push(trimmed.to_string());
    }

    kept.join("\n\n")
}

// ---------------------------------------------------------------------------
// Line-level dedup
// ---------------------------------------------------------------------------

/// Remove duplicate lines within each `\n\n` block. Catches carousel items
/// that appear on separate lines inside a single block. Uses both exact
/// and prefix fingerprinting (first N words) to catch near-duplicates where
/// company names or CTAs are appended. Lines shorter than [`DEDUP_MIN_CHARS`]
/// or structural lines are exempt.
pub(crate) fn dedup_lines(input: &str) -> String {
    let blocks: Vec<&str> = input.split("\n\n").collect();
    let mut out = Vec::with_capacity(blocks.len());
    let mut in_code_block = false;

    for block in blocks {
        let has_fence = block.lines().any(|l| l.trim_start().starts_with("```"));

        // Inside a code block or block contains a fence: preserve as-is
        if in_code_block || has_fence {
            out.push(block.to_string());
            for line in block.lines() {
                if line.trim_start().starts_with("```") {
                    in_code_block = !in_code_block;
                }
            }
            continue;
        }

        let lines: Vec<&str> = block.lines().collect();
        if lines.len() <= 2 {
            out.push(block.to_string());
            continue;
        }

        let mut seen_exact: HashSet<String> = HashSet::new();
        let mut seen_prefix: HashSet<String> = HashSet::new();
        let mut kept: Vec<&str> = Vec::new();
        for line in &lines {
            let trimmed = line.trim();
            if trimmed.len() < DEDUP_MIN_CHARS || is_structural_line(trimmed) {
                kept.push(line);
                continue;
            }
            let fp = normalize_fingerprint(trimmed);
            if !seen_exact.insert(fp.clone()) {
                continue;
            }
            if let Some(pfp) = prefix_fingerprint(&fp)
                && !seen_prefix.insert(pfp)
            {
                continue;
            }
            kept.push(line);
        }
        out.push(kept.join("\n"));
    }

    out.join("\n\n")
}

// ---------------------------------------------------------------------------
// Comma-separated list dedup
// ---------------------------------------------------------------------------

/// Detect comma-separated lists where the same sequence of items repeats
/// (e.g., "a, b, c, a, b, c, a, b, c" -> "a, b, c"). Also collapses
/// consecutive identical items ("A, A, B, B" -> "A, B"). Common in logo
/// carousels that duplicate DOM nodes for infinite scroll animation.
pub(crate) fn dedup_comma_lists(input: &str) -> String {
    input
        .lines()
        .map(|line| {
            let items: Vec<&str> = line.split(", ").map(|s| s.trim()).collect();
            if items.len() < 2 {
                return line.to_string();
            }

            // First: try full cycle dedup (a,b,c,a,b,c -> a,b,c)
            if items.len() >= 6 {
                for cycle_len in 1..=items.len() / 2 {
                    if items.len() % cycle_len != 0 {
                        continue;
                    }
                    let pattern = &items[..cycle_len];
                    let all_match = items.chunks(cycle_len).all(|chunk| chunk == pattern);
                    if all_match && items.len() / cycle_len >= 2 {
                        return pattern.join(", ");
                    }
                }
            }

            // Second: collapse consecutive identical items (A, A, B, B -> A, B)
            let mut deduped: Vec<&str> = Vec::with_capacity(items.len());
            for item in &items {
                if deduped
                    .last()
                    .is_none_or(|prev: &&str| !prev.eq_ignore_ascii_case(item))
                {
                    deduped.push(item);
                }
            }
            if deduped.len() < items.len() {
                return deduped.join(", ");
            }

            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Trailing empty headings
// ---------------------------------------------------------------------------

/// Remove headings that are followed only by another heading of same/higher
/// level (or EOF) with no content between them.
fn strip_trailing_empty_headings(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut remove = vec![false; lines.len()];

    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim();
        if !trimmed.starts_with('#') {
            continue;
        }
        let level = trimmed.chars().take_while(|&c| c == '#').count();

        // Find next non-blank line
        let mut next_content = None;
        for (j, line_j) in lines.iter().enumerate().skip(i + 1) {
            if !line_j.trim().is_empty() {
                next_content = Some(j);
                break;
            }
        }

        match next_content {
            None => {
                // Heading at EOF -- empty
                remove[i] = true;
            }
            Some(j) => {
                let next = lines[j].trim();
                if next.starts_with('#') {
                    let next_level = next.chars().take_while(|&c| c == '#').count();
                    // Empty if next heading is same or higher level (not a child)
                    if next_level <= level {
                        remove[i] = true;
                    }
                }
            }
        }
    }

    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !remove[*i])
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Empty code block stripping
// ---------------------------------------------------------------------------

/// Remove code blocks that contain only whitespace between fences.
fn strip_empty_code_blocks(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    let mut remove = vec![false; lines.len()];
    let mut i = 0;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.starts_with("```") {
            // Find closing fence
            let mut j = i + 1;
            let mut all_blank = true;
            while j < lines.len() {
                if lines[j].trim().starts_with("```") {
                    break;
                }
                if !lines[j].trim().is_empty() {
                    all_blank = false;
                }
                j += 1;
            }
            if j < lines.len() && all_blank {
                // Mark opening fence, content, and closing fence for removal
                for flag in &mut remove[i..=j] {
                    *flag = true;
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }

    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| !remove[*i])
        .map(|(_, line)| *line)
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapse_repeated_phrase_in_line() {
        let input = "talk play chat hang out talk play chat hang out";
        let result = collapse_repeated_in_line(input);
        assert_eq!(result, "talk play chat hang out");
    }

    #[test]
    fn collapse_repeated_phrase_triple() {
        let input = "go home go home go home";
        let result = collapse_repeated_in_line(input);
        assert_eq!(result, "go home");
    }

    // -- heading dedup --

    #[test]
    fn dedup_duplicate_headings_removes() {
        let input =
            "## Features\n\nGreat stuff\n\n## Other\n\nMore\n\n## Features\n\nGreat stuff\n";
        let result = dedup_duplicate_headings(input);
        assert_eq!(result.matches("## Features").count(), 1);
        assert!(result.starts_with("## Features"));
    }

    #[test]
    fn dedup_duplicate_headings_different_levels() {
        let input = "## Foo\n\nContent\n\n### Foo\n\nOther\n";
        let result = dedup_duplicate_headings(input);
        assert!(result.contains("## Foo"));
        assert!(result.contains("### Foo"));
    }

    #[test]
    fn dedup_duplicate_headings_no_dupes() {
        let input = "## A\n\nText\n\n## B\n\nMore\n";
        assert_eq!(dedup_duplicate_headings(input), input);
    }

    #[test]
    fn dedup_duplicate_headings_removes_following_content() {
        let input =
            "## Setup\n\nStep 1\nStep 2\n\n## Other\n\nStuff\n\n## Setup\n\nStep 1\nStep 2\n";
        let result = dedup_duplicate_headings(input);
        assert_eq!(result.matches("## Setup").count(), 1);
        assert_eq!(result.matches("Step 1").count(), 1);
        assert_eq!(result.matches("Step 2").count(), 1);
    }

    // -- comma list dedup --

    #[test]
    fn dedup_comma_list_catches_repeated_logos() {
        let input = "mozilla, github, 1password, pwc, mozilla, github, 1password, pwc, mozilla, github, 1password, pwc";
        let out = dedup_comma_lists(input);
        assert_eq!(out, "mozilla, github, 1password, pwc");
    }

    #[test]
    fn dedup_comma_list_preserves_unique_list() {
        let input = "apple, banana, cherry, date, elderberry, fig";
        let out = dedup_comma_lists(input);
        assert_eq!(out, input);
    }

    #[test]
    fn dedup_comma_list_consecutive() {
        assert_eq!(
            dedup_comma_lists("Runway, Runway, LeonardoAi, LeonardoAi"),
            "Runway, LeonardoAi"
        );
    }

    #[test]
    fn dedup_comma_list_case_insensitive() {
        assert_eq!(
            dedup_comma_lists("Apple, apple, Banana, banana"),
            "Apple, Banana"
        );
    }

    #[test]
    fn dedup_comma_list_no_dupes() {
        assert_eq!(dedup_comma_lists("A, B, C"), "A, B, C");
    }

    #[test]
    fn dedup_comma_list_cycle_still_works() {
        assert_eq!(dedup_comma_lists("a, b, c, a, b, c, a, b, c"), "a, b, c");
    }

    // -- line-level dedup --

    #[test]
    fn dedup_lines_removes_repeated_lines_in_block() {
        let input = "Story A about product launch\nStory B about scaling\nStory A about product launch\nStory C about funding\nStory B about scaling";
        let out = dedup_lines(input);
        assert_eq!(
            out.matches("Story A about product launch").count(),
            1,
            "Duplicate line not removed: {out}"
        );
        assert_eq!(
            out.matches("Story B about scaling").count(),
            1,
            "Duplicate line not removed: {out}"
        );
        assert!(out.contains("Story C about funding"));
    }

    // -- trailing empty headings --

    #[test]
    fn empty_heading_at_eof_stripped() {
        let input = "Content\n\n## Support\n\n## Developers";
        let result = strip_trailing_empty_headings(input);
        assert!(!result.contains("## Support"));
        assert!(!result.contains("## Developers"));
    }

    #[test]
    fn empty_heading_before_same_level_stripped() {
        let input = "## A\n\n## B\n\nContent here";
        let result = strip_trailing_empty_headings(input);
        assert!(!result.contains("## A"));
        assert!(result.contains("## B"));
        assert!(result.contains("Content here"));
    }

    #[test]
    fn heading_with_subsection_preserved() {
        let input = "## Section\n\n### Subsection\n\nContent";
        assert_eq!(strip_trailing_empty_headings(input), input);
    }

    #[test]
    fn heading_with_content_preserved() {
        let input = "## Features\n\nGreat stuff\n\n## More\n\nAlso great";
        assert_eq!(strip_trailing_empty_headings(input), input);
    }

    // -- empty code blocks --

    #[test]
    fn empty_code_block_stripped() {
        let input = "Before\n\n```\n\n```\n\nAfter";
        let result = strip_empty_code_blocks(input);
        assert!(!result.contains("```"));
        assert!(result.contains("Before"));
        assert!(result.contains("After"));
    }

    #[test]
    fn empty_code_block_with_lang_stripped() {
        let input = "Text\n\n```js\n\n```\n\nMore";
        let result = strip_empty_code_blocks(input);
        assert!(!result.contains("```"));
    }

    #[test]
    fn nonempty_code_block_preserved() {
        let input = "```\nconst x = 1;\n```";
        assert_eq!(strip_empty_code_blocks(input), input);
    }
}
