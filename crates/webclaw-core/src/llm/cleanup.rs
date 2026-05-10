/// Whitespace cleanup, HTML entity decoding, invisible Unicode stripping,
/// leaked JS removal, CSS artifact filtering, and text-level noise removal.
use once_cell::sync::Lazy;
use regex::Regex;

use crate::noise;

// ---------------------------------------------------------------------------
// HTML entity decoding
// ---------------------------------------------------------------------------

/// Decode common HTML entities that survive into extracted text.
///
/// HTML parsers decode entities in text nodes, but double-encoded entities
/// (e.g., `&amp;nbsp;` -> `&nbsp;` after first parse) and entities in
/// attribute-derived text can leak through.
pub(crate) fn decode_html_entities(input: &str) -> String {
    // Fast path: no ampersands means nothing to decode
    if !input.contains('&') {
        return input.to_string();
    }

    static ENTITY_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"&(#[xX][0-9a-fA-F]+|#[0-9]+|[a-zA-Z]+);").unwrap());

    ENTITY_RE
        .replace_all(input, |caps: &regex::Captures| {
            let entity = caps.get(1).unwrap().as_str();
            match entity {
                "nbsp" => " ".to_string(),
                "amp" => "&".to_string(),
                "lt" => "<".to_string(),
                "gt" => ">".to_string(),
                "quot" => "\"".to_string(),
                "apos" => "'".to_string(),
                "mdash" => "\u{2014}".to_string(),
                "ndash" => "\u{2013}".to_string(),
                "laquo" => "\u{00AB}".to_string(),
                "raquo" => "\u{00BB}".to_string(),
                "copy" => "\u{00A9}".to_string(),
                "reg" => "\u{00AE}".to_string(),
                "trade" => "\u{2122}".to_string(),
                "hellip" => "\u{2026}".to_string(),
                "bull" => "\u{2022}".to_string(),
                s if s.starts_with("#x") || s.starts_with("#X") => u32::from_str_radix(&s[2..], 16)
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| caps[0].to_string()),
                s if s.starts_with('#') => s[1..]
                    .parse::<u32>()
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| caps[0].to_string()),
                _ => caps[0].to_string(), // unknown entity -- leave as-is
            }
        })
        .into_owned()
}

// ---------------------------------------------------------------------------
// Invisible Unicode stripping
// ---------------------------------------------------------------------------

/// Strip invisible Unicode characters (zero-width spaces, joiners, soft hyphens,
/// directional marks) that are used for text-wrap balancing or copy-protection.
pub(crate) fn strip_invisible_unicode(input: &str) -> String {
    if !input.contains('\u{200B}')
        && !input.contains('\u{200C}')
        && !input.contains('\u{200D}')
        && !input.contains('\u{200E}')
        && !input.contains('\u{200F}')
        && !input.contains('\u{FEFF}')
        && !input.contains('\u{00AD}')
        && !input.contains('\u{2060}')
        && !input.contains('\u{2062}')
        && !input.contains('\u{2063}')
        && !input.contains('\u{2064}')
        && !input.contains('\u{034F}')
    {
        return input.to_string();
    }

    input
        .chars()
        .filter(|c| {
            !matches!(
                c,
                '\u{200B}' // zero-width space
            | '\u{200C}' // zero-width non-joiner
            | '\u{200D}' // zero-width joiner
            | '\u{200E}' // left-to-right mark
            | '\u{200F}' // right-to-left mark
            | '\u{FEFF}' // byte order mark / zero-width no-break space
            | '\u{00AD}' // soft hyphen
            | '\u{2060}' // word joiner
            | '\u{2062}' // invisible times
            | '\u{2063}' // invisible separator
            | '\u{2064}' // invisible plus
            | '\u{034F}' // combining grapheme joiner
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Leaked JavaScript stripping
// ---------------------------------------------------------------------------

/// Strip lines containing raw JavaScript that leaked from inline <script> or
/// framework hydration code (e.g., Next.js `self.__wrap_n=...`).
pub(crate) fn strip_leaked_js(input: &str) -> String {
    static JS_PATTERN: Lazy<Regex> = Lazy::new(|| Regex::new(r"self\.__\w+").unwrap());

    let mut out = String::with_capacity(input.len());
    let mut in_code_fence = false;
    for line in input.lines() {
        if !out.is_empty() {
            out.push('\n');
        }

        if line.trim().starts_with("```") {
            in_code_fence = !in_code_fence;
            out.push_str(line);
            continue;
        }
        if in_code_fence {
            out.push_str(line);
            continue;
        }

        // Only handle the most reliable pattern: `self.__` which is framework hydration
        if JS_PATTERN.is_match(line) {
            if let Some(idx) = line.find("self.__") {
                let cleaned = line[..idx].trim_end();
                if !cleaned.is_empty() {
                    out.push_str(cleaned);
                }
            }
            // If entire line is JS, skip it (cleaned is empty)
        } else {
            out.push_str(line);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Accessibility link chrome ("opens new tab", "external link")
// ---------------------------------------------------------------------------

/// Strip screen-reader-only link chrome that bleeds into rendered text.
///
/// Sites like Reuters wrap external/new-window links with hidden spans
/// like `<span class="visually-hidden">, opens new tab</span>`. The noise
/// filter can't reliably catch these (no consistent class hook across
/// sites), so they end up duplicated all over the body text. This is a
/// targeted text-level scrub of the most common phrasings.
pub(crate) fn strip_a11y_link_chrome(input: &str) -> String {
    static A11Y_PATTERN: Lazy<Regex> = Lazy::new(|| {
        Regex::new(
            r"(?i)(?:\s*,\s*(?:opens (?:in )?(?:a )?new (?:tab|window)|opens external (?:link|website)|external link)\b\.?|\s+\((?:opens (?:in )?(?:a )?new (?:tab|window)|opens external (?:link|website)|external link)\)\.?|\s+external link\b\.?$)",
        )
        .unwrap()
    });

    let mut out = String::with_capacity(input.len());
    let mut in_code_fence = false;
    for (i, line) in input.lines().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.trim().starts_with("```") {
            in_code_fence = !in_code_fence;
            out.push_str(line);
            continue;
        }
        if in_code_fence {
            out.push_str(line);
            continue;
        }
        out.push_str(&A11Y_PATTERN.replace_all(line, ""));
    }
    out
}

// ---------------------------------------------------------------------------
// Spaced-out text collapsing (CSS animation artifacts)
// ---------------------------------------------------------------------------

/// Detect text where each character is separated by a space, a common artifact
/// from CSS letter-spacing or animation effects. "S t a r t" becomes "Start".
/// Only triggers when 4+ actual characters alternate with spaces.
pub(crate) fn collapse_spaced_text(input: &str) -> String {
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

        if in_code_block || trimmed.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        let collapsed = collapse_spaced_segments(trimmed);
        out.push_str(&collapsed);
        out.push('\n');
    }

    if !input.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    out
}

/// Collapse spaced-out segments within a line. A "spaced segment" is a run of
/// single non-space chars each followed by a space: "S t a r t" (4+ real chars).
/// Restores word boundaries using uppercase transitions (lowercase->uppercase = space).
fn collapse_spaced_segments(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() < 7 {
        return line.to_string();
    }

    let mut result = String::with_capacity(line.len());
    let mut i = 0;

    while i < chars.len() {
        if !chars[i].is_whitespace() {
            let seg_start = i;
            let mut real_chars: Vec<char> = vec![chars[i]];
            let mut j = i + 1;

            while j + 1 < chars.len() && chars[j] == ' ' && !chars[j + 1].is_whitespace() {
                real_chars.push(chars[j + 1]);
                j += 2;
            }

            if real_chars.len() >= 4 {
                let starts_ok = seg_start == 0 || chars[seg_start - 1].is_whitespace();
                let ends_ok = j >= chars.len() || chars[j].is_whitespace();

                if starts_ok && ends_ok {
                    let mut collapsed = String::with_capacity(real_chars.len() + 4);
                    for (idx, &ch) in real_chars.iter().enumerate() {
                        if idx > 0 && ch.is_uppercase() && real_chars[idx - 1].is_lowercase() {
                            collapsed.push(' ');
                        }
                        collapsed.push(ch);
                    }
                    result.push_str(&collapsed);
                    i = j;
                    continue;
                }
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

// ---------------------------------------------------------------------------
// Whitespace collapsing
// ---------------------------------------------------------------------------

/// Collapse whitespace (max 1 blank line between sections).
pub(crate) fn collapse_whitespace(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut consecutive_blanks = 0;

    for line in input.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            consecutive_blanks += 1;
            // Allow at most 1 blank line (which means 2 consecutive \n in the output)
            if consecutive_blanks <= 1 {
                out.push('\n');
            }
        } else {
            consecutive_blanks = 0;
            out.push_str(trimmed);
            out.push('\n');
        }
    }

    out.trim().to_string()
}

// ---------------------------------------------------------------------------
// Emphasis stripping
// ---------------------------------------------------------------------------

/// `**text**` or `__text__`
static BOLD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*([^*]+)\*\*|__([^_]+)__").unwrap());

/// `*text*` -- safe to use after bold `**` is already stripped.
static ITALIC_STAR_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*([^*]+)\*").unwrap());

/// `_text_` -- match underscores at word boundaries.
static ITALIC_UNDER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b_([^_]+)_\b").unwrap());

/// Strip bold/italic emphasis markers, preserving code blocks.
pub(crate) fn strip_emphasis(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_code_block = false;

    for line in input.lines() {
        if line.trim_start().starts_with("```") {
            in_code_block = !in_code_block;
            out.push_str(line);
            out.push('\n');
            continue;
        }

        if in_code_block {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Bold first (** before *)
        let s = BOLD_RE.replace_all(line, |caps: &regex::Captures| {
            caps.get(1)
                .or_else(|| caps.get(2))
                .map_or("", |m| m.as_str())
                .to_string()
        });
        let s = ITALIC_STAR_RE.replace_all(&s, "$1");
        let s = ITALIC_UNDER_RE.replace_all(&s, "$1");
        out.push_str(&s);
        out.push('\n');
    }

    // Don't add an extra trailing newline beyond what the input had
    if !input.ends_with('\n') && out.ends_with('\n') {
        out.pop();
    }

    out
}

// ---------------------------------------------------------------------------
// UI control text filtering
// ---------------------------------------------------------------------------

/// Strip lines that consist entirely of UI control text (Material Icons
/// ligatures, navigation arrows, etc.) that get captured as visible text.
pub(crate) fn strip_ui_control_text(input: &str) -> String {
    input
        .lines()
        .filter(|line| !is_ui_control_line(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Check if a line consists only of UI control tokens.
pub(crate) fn is_ui_control_line(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    // Browser fallback messages from <video>/<audio> tags
    let lower = trimmed.to_lowercase();
    if lower.contains("your browser does not support") {
        return true;
    }

    // Must be relatively short -- real content mixed with a control word is fine
    if trimmed.len() > 120 {
        return false;
    }

    // Split by whitespace: every token must be a known UI control
    let tokens: Vec<&str> = trimmed.split_whitespace().collect();
    if tokens.is_empty() {
        return false;
    }
    tokens.iter().all(|t| is_ui_control_token(t))
}

/// Known UI control tokens from Material Icons ligatures, icon fonts, and
/// common navigation elements that leak into text extraction.
fn is_ui_control_token(token: &str) -> bool {
    const UI_CONTROLS: &[&str] = &[
        // Material Icons ligatures
        "navigate_before",
        "navigate_next",
        "chevron_left",
        "chevron_right",
        "arrow_back",
        "arrow_forward",
        "arrow_upward",
        "arrow_downward",
        "arrow_drop_down",
        "arrow_drop_up",
        "arrow_left",
        "arrow_right",
        "expand_more",
        "expand_less",
        "unfold_more",
        "unfold_less",
        "first_page",
        "last_page",
        "more_horiz",
        "more_vert",
        "open_in_new",
        "open_in_full",
        "close_fullscreen",
        "fullscreen",
        "fullscreen_exit",
        // Common single-word UI tokens
        "close",
        "search",
        "menu",
        "share",
        // Arrow/nav characters
        "\u{2190}",
        "\u{2192}",
        "\u{2191}",
        "\u{2193}",
        "\u{25B8}",
        "\u{25BE}",
        "\u{25C0}",
        "\u{25B6}",
        "\u{2913}",
        "\u{23F5}",
        "\u{203A}",
        "\u{2039}",
        "\u{00BB}",
        "\u{00AB}",
    ];
    UI_CONTROLS.contains(&token)
}

// ---------------------------------------------------------------------------
// Alt-text noise filtering
// ---------------------------------------------------------------------------

/// Remove lines that are descriptive image alt text, broken image fragments,
/// social avatar labels, or repeated brand icon lists.
pub(crate) fn strip_alt_text_noise(input: &str) -> String {
    let mut in_code = false;
    input
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                in_code = !in_code;
            }
            if in_code {
                return true;
            }
            // Skip structural lines
            if trimmed.starts_with('#') || trimmed.starts_with('-') || trimmed.starts_with('>') {
                return true;
            }
            !is_alt_text_noise(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_alt_text_noise(line: &str) -> bool {
    if line.is_empty() {
        return false;
    }
    is_descriptive_alt_prefix(line)
        || is_broken_image_fragment(line)
        || is_social_avatar_label(line)
        || is_repeated_brand_list(line)
}

fn is_descriptive_alt_prefix(line: &str) -> bool {
    let lower = line.to_lowercase();
    let prefixes = [
        "image of ",
        "photo of ",
        "animation of ",
        "interactive animation of ",
        "screenshot of ",
        "illustration of ",
        "illustration ",
        "picture of ",
        "a image of ",
        "a photo of ",
        "an image of ",
        "an illustration ",
        "an animation ",
        "a screenshot ",
        "a rendering ",
        "a graphic ",
        "a diagram ",
    ];
    let has_prefix = prefixes.iter().any(|p| lower.starts_with(p));
    // Require 4+ words to avoid stripping short headings
    has_prefix && line.split_whitespace().count() >= 4
}

fn is_broken_image_fragment(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    // All whitespace-separated tokens must be broken image refs like ".webp)" or ".svg)"
    trimmed.split_whitespace().all(|token| {
        let t = token.trim();
        if t.is_empty() {
            return true;
        }
        let exts = [
            ".webp)", ".svg)", ".png)", ".jpg)", ".jpeg)", ".gif)", ".avif)",
        ];
        exts.iter()
            .any(|ext| t.ends_with(ext) && t.len() <= ext.len() + 5)
    })
}

fn is_social_avatar_label(line: &str) -> bool {
    let lower = line.to_lowercase();
    let twitter_count = lower.matches("twitter image").count();
    if twitter_count >= 3 {
        return true;
    }
    let handle_count = line
        .split_whitespace()
        .filter(|w| w.starts_with('@'))
        .count();
    let avatar_count = lower.matches("avatar").count();
    handle_count >= 3 && avatar_count >= 2
}

fn is_repeated_brand_list(line: &str) -> bool {
    use std::collections::HashMap;

    let items: Vec<&str> = line
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if items.len() < 4 {
        return false;
    }
    // Check if >50% of multi-word items share the same first word
    let multi_word: Vec<&str> = items
        .iter()
        .filter(|i| i.split_whitespace().count() >= 2)
        .copied()
        .collect();
    if multi_word.len() < 4 {
        return false;
    }
    let mut first_words: HashMap<&str, usize> = HashMap::new();
    for item in &multi_word {
        if let Some(fw) = item.split_whitespace().next() {
            *first_words.entry(fw).or_insert(0) += 1;
        }
    }
    first_words
        .values()
        .any(|&count| count * 2 > multi_word.len())
}

// ---------------------------------------------------------------------------
// Long alt-text description filtering
// ---------------------------------------------------------------------------

/// Strip lines that are clearly long image alt-text descriptions that leaked
/// through. These are multi-sentence scene descriptions typically starting with
/// "An illustration", "A screenshot", "A photo", etc.
pub(crate) fn strip_long_alt_descriptions(input: &str) -> String {
    // Inline regex for "This element contains..." descriptions embedded mid-line
    // (common on sites like cursor.com where ARIA descriptions leak into text)
    static ELEMENT_DESC_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"This element contains [^.]*\.[^.]*\.(?:\s*[^.]*\.)*").unwrap());

    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        if is_long_alt_description(line) {
            continue;
        }
        let cleaned = ELEMENT_DESC_RE.replace_all(line, "");
        let cleaned = cleaned.trim_end();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(cleaned);
    }
    out
}

pub(crate) fn is_long_alt_description(line: &str) -> bool {
    let trimmed = line.trim();

    // Must be long enough to be a description (not a real heading/paragraph)
    if trimmed.len() < 80 {
        return false;
    }

    // Must not start with markdown structure
    if trimmed.starts_with('#') || trimmed.starts_with('-') || trimmed.starts_with('>') {
        return false;
    }

    let lower = trimmed.to_lowercase();
    const ALT_PREFIXES: &[&str] = &[
        "an illustration ",
        "an image ",
        "a screenshot ",
        "a photo ",
        "a picture ",
        "a diagram ",
        "a graphic ",
        "a rendering ",
        "an animation ",
        "an icon ",
        "this element contains ",
        "this image shows ",
        "this image depicts ",
    ];

    ALT_PREFIXES.iter().any(|p| lower.starts_with(p))
}

// ---------------------------------------------------------------------------
// CSS artifact filtering
// ---------------------------------------------------------------------------

/// Strip CSS artifacts leaking into text, both standalone lines and inline.
/// E.g., `@keyframes copy{from{background:var(--runtime)}`
pub(crate) fn strip_css_artifacts(input: &str) -> String {
    static CSS_INLINE_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"@(?:keyframes|font-face|media|supports|layer)\s*[^{]*\{[^}]*\}?").unwrap()
    });

    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        let trimmed = line.trim();
        if is_css_artifact_line(trimmed) {
            continue;
        }
        // Also strip inline CSS artifacts within a line
        let cleaned = CSS_INLINE_RE.replace_all(line, "");
        let cleaned = cleaned.trim_end();
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(cleaned);
    }
    out
}

pub(crate) fn is_css_artifact_line(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }

    // Standalone CSS block: `selector{property:value}` with no spaces
    if trimmed.len() > 10
        && trimmed.contains('{')
        && trimmed.contains('}')
        && trimmed.contains(':')
        && !trimmed.contains(' ')
        && !trimmed.starts_with('#')
    {
        return true;
    }

    false
}

// ---------------------------------------------------------------------------
// CMS asset label stripping
// ---------------------------------------------------------------------------

/// Lines like "Homepage | Agents 26 | Bento | Desktop" are CMS asset labels
/// from image alt attributes, not actual content. Filter them out.
pub(crate) fn strip_asset_labels(input: &str) -> String {
    let mut in_code_block = false;
    input
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            if trimmed.starts_with("```") {
                in_code_block = !in_code_block;
                return true;
            }
            if in_code_block {
                return true;
            }
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('>') {
                return true;
            }
            !is_asset_label(trimmed)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Heuristic: a line with 3+ pipe-separated short segments is an asset label.
/// Also catches "oai Blog Codex Security Art Card 1x1" style labels.
pub(crate) fn is_asset_label(line: &str) -> bool {
    // Markdown table rows start with | -- never treat as asset labels
    if line.starts_with('|') {
        return false;
    }
    // Pipe-separated asset label: "Homepage | Fall 25 | Hero | Desktop"
    // But NOT stat lines: "100M+ users | #1 rated | 99.99% uptime"
    if line.contains(" | ") {
        let segments: Vec<&str> = line.split(" | ").collect();
        let all_short = segments.iter().all(|s| s.len() < 40);
        // Stat-like numbers have units/symbols (%, M, K, B, +, #, x)
        // CMS version numbers (e.g., "Agents 26") are plain digits -- not stats
        let has_stat_numbers = segments.iter().any(|s| is_stat_text(s));
        if segments.len() >= 3 && all_short && !has_stat_numbers {
            return true;
        }
    }
    // CMS asset reference using ">" separator: "Scaling AI > Cover Image"
    if line.contains(" > ") {
        let parts: Vec<&str> = line.split(" > ").collect();
        let has_asset_word = parts.iter().any(|p| {
            let lower = p.trim().to_lowercase();
            ["cover", "card", "image", "poster", "logo", "thumbnail"]
                .iter()
                .any(|kw| lower.contains(kw))
        });
        if has_asset_word && line.len() < 80 {
            return true;
        }
    }
    // CMS asset naming patterns: short lines with Art Card, SEO, 1x1, Cover, etc.
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() >= 3 && words.len() <= 12 {
        let label_keywords = [
            "Art Card",
            "ArtCard",
            "Card Image",
            "Cover Image",
            "1x1",
            "SEO",
        ];
        if label_keywords.iter().any(|kw| line.contains(kw)) {
            return true;
        }
    }
    // URL-slug lines (CMS slugs that leak into content)
    // "our-agreement-with-the-department-of-war-1-1"
    if !line.contains(' ') && line.contains('-') && line.len() > 10 && line.len() < 80 {
        let is_slug = line
            .chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_');
        if is_slug {
            return true;
        }
    }
    false
}

/// Does a string contain stat-like numbers (with units/symbols like %, M+, #1)?
/// Plain digits like "26" or "2026" are NOT stats -- they're version/year numbers.
fn is_stat_text(s: &str) -> bool {
    let s = s.trim();
    // Look for digits adjacent to stat indicators: %, M, K, B, +, #, x
    s.contains('%')
        || s.contains('#')
        || s.contains("M+")
        || s.contains("K+")
        || s.contains("B+")
        || s.contains("M ")
        || s.contains("K ")
        || s.contains("B ")
        || (s.ends_with('x') && s.chars().any(|c| c.is_ascii_digit()))
}

// ---------------------------------------------------------------------------
// CSS class line stripping
// ---------------------------------------------------------------------------

/// Drop lines whose content is predominantly CSS utility class names.
/// Preserves headings, code fences, blockquotes, and list items unconditionally.
pub(crate) fn strip_css_class_lines(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_code_block = false;

    for line in input.lines() {
        let trimmed = line.trim();

        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
        }

        // Never strip inside code blocks
        if in_code_block || trimmed.is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }

        // Strip empty list items (leftover from image-only bullets)
        if trimmed == "-" || trimmed == "*" || trimmed == "- " || trimmed == "* " {
            continue;
        }

        // Skip structural lines from full-line stripping
        let is_structural = trimmed.starts_with('#')
            || trimmed.starts_with('>')
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ");

        // Full line is CSS class text -> drop entirely
        if !is_structural && noise::is_css_class_text(trimmed) {
            continue;
        }

        // Mixed line: strip trailing CSS class words from non-structural lines.
        // e.g., "Docs Blog text-4xl text-gray-950 tracking-tighter" -> "Docs Blog"
        if !is_structural {
            let cleaned = strip_trailing_css_classes(trimmed);
            if !cleaned.is_empty() {
                out.push_str(&cleaned);
                out.push('\n');
                continue;
            }
        }

        out.push_str(line);
        out.push('\n');
    }

    out
}

/// Strip trailing CSS utility class words from the end of a line.
/// Returns the line with trailing CSS classes removed.
fn strip_trailing_css_classes(line: &str) -> String {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 3 {
        return line.to_string();
    }

    // Find the last non-CSS-class word
    let mut last_content = words.len();
    for i in (0..words.len()).rev() {
        if noise::is_css_class_word_pub(words[i]) {
            last_content = i;
        } else {
            break;
        }
    }

    // Only strip if we found trailing CSS classes (at least 2)
    if last_content < words.len() && words.len() - last_content >= 2 {
        words[..last_content].join(" ")
    } else {
        line.to_string()
    }
}

// ---------------------------------------------------------------------------
// Long unstructured word/name list collapsing
// ---------------------------------------------------------------------------

/// Detect and collapse lines that are long unstructured lists of single words
/// (contributor names, API names, etc.) -- not prose sentences.
pub(crate) fn collapse_word_lists(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for line in input.lines() {
        if !out.is_empty() {
            out.push('\n');
        }

        let trimmed = line.trim();
        // Only process long non-markdown lines
        if trimmed.len() < 200
            || trimmed.starts_with('#')
            || trimmed.starts_with('-')
            || trimmed.starts_with('>')
            || trimmed.starts_with('|')
            || trimmed.starts_with("```")
        {
            out.push_str(line);
            continue;
        }

        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.len() < 20 {
            out.push_str(line);
            continue;
        }

        // Detect name/identifier dumps by looking at prose structure.
        // Real prose has common English function words (the, a, of, to, etc.).
        // Name dumps and API lists have almost none.
        //
        // Handle two cases:
        // 1. Pure dump: entire line is names/identifiers
        // 2. Mixed: prose prefix followed by a name dump tail

        // Find where prose ends and dump begins by scanning with a sliding window
        let dump_start = find_dump_start(&words);

        if let Some(start_idx) = dump_start {
            let dump_len = words.len() - start_idx;
            if dump_len > 20 {
                // Keep the prose prefix, collapse the dump tail
                let prose_part: Vec<&str> = words[..start_idx].to_vec();
                let dump_preview: Vec<&str> =
                    words[start_idx..start_idx + 3.min(dump_len)].to_vec();
                if prose_part.is_empty() {
                    out.push_str(&format!(
                        "{} ... and {} more",
                        dump_preview.join(" "),
                        dump_len - dump_preview.len()
                    ));
                } else {
                    out.push_str(&prose_part.join(" "));
                }
            } else {
                out.push_str(line);
            }
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Scan a word list to find where a name/identifier dump begins.
/// Uses a sliding window: when 15+ consecutive words have no function words, that's a dump.
fn find_dump_start(words: &[&str]) -> Option<usize> {
    if words.len() < 25 {
        return None;
    }
    let window = 15;
    let mut consecutive_non_prose = 0;
    for (i, word) in words.iter().enumerate() {
        if is_prose_function_word(&word.to_lowercase()) {
            consecutive_non_prose = 0;
        } else {
            consecutive_non_prose += 1;
            if consecutive_non_prose >= window {
                let start = i + 1 - window;
                // Verify the rest of the line is also a dump
                let remaining = &words[start..];
                let prose_in_remaining = remaining
                    .iter()
                    .filter(|w| is_prose_function_word(&w.to_lowercase()))
                    .count();
                let ratio = prose_in_remaining as f64 / remaining.len() as f64;
                if ratio < 0.05 {
                    return Some(start);
                }
            }
        }
    }
    None
}

/// Common English function words that indicate prose structure.
fn is_prose_function_word(word: &str) -> bool {
    matches!(
        word,
        "the"
            | "a"
            | "an"
            | "of"
            | "to"
            | "for"
            | "with"
            | "in"
            | "on"
            | "is"
            | "are"
            | "was"
            | "were"
            | "be"
            | "been"
            | "being"
            | "and"
            | "but"
            | "or"
            | "not"
            | "that"
            | "this"
            | "these"
            | "it"
            | "its"
            | "you"
            | "your"
            | "we"
            | "our"
            | "they"
            | "from"
            | "by"
            | "at"
            | "as"
            | "if"
            | "so"
            | "no"
            | "can"
            | "will"
            | "has"
            | "have"
            | "had"
            | "do"
            | "does"
            | "did"
            | "about"
            | "into"
            | "than"
            | "then"
            | "also"
            | "more"
    )
}

// ---------------------------------------------------------------------------
// Adjacent description dedup
// ---------------------------------------------------------------------------

/// Collapse patterns where a product name + description appears twice in a row,
/// common in card-based layouts:
/// "Infrastructure From overview to deep details, fast"
/// "Learn more ** From overview to deep details, fast"
/// -> Keep only the first occurrence of each description.
pub(crate) fn dedup_adjacent_descriptions(input: &str) -> String {
    let lines: Vec<&str> = input.lines().collect();
    if lines.len() < 3 {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut skip_next = false;

    for i in 0..lines.len() {
        if skip_next {
            skip_next = false;
            continue;
        }

        let current = lines[i].trim();

        // Check if next line starts with "Learn more" and repeats text from current line
        if i + 1 < lines.len() {
            let next = lines[i + 1].trim();
            // Pattern: "Learn more ** <description>" or "LEARN MORE ** <description>"
            if let Some(rest) = next
                .strip_prefix("Learn more")
                .or_else(|| next.strip_prefix("LEARN MORE"))
                .or_else(|| next.strip_prefix("learn more"))
            {
                let rest = rest.trim().trim_start_matches('*').trim();
                // If the rest repeats text from current line, skip the next line
                if !rest.is_empty() && rest.len() > 15 && current.contains(rest) {
                    skip_next = true;
                }
            }
        }

        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(lines[i]);
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- HTML entity decoding --

    #[test]
    fn decode_nbsp() {
        assert_eq!(
            decode_html_entities("From overview to deep details,&nbsp;fast"),
            "From overview to deep details, fast"
        );
    }

    #[test]
    fn decode_amp_lt_gt() {
        assert_eq!(decode_html_entities("A &amp; B"), "A & B");
        assert_eq!(decode_html_entities("&lt;div&gt;"), "<div>");
    }

    #[test]
    fn decode_numeric_entities() {
        assert_eq!(decode_html_entities("&#169;"), "\u{00A9}"); // (C)
        assert_eq!(decode_html_entities("&#x2019;"), "\u{2019}"); // '
    }

    #[test]
    fn decode_no_entity_passthrough() {
        let input = "Normal text without entities";
        assert_eq!(decode_html_entities(input), input);
    }

    #[test]
    fn decode_named_entities() {
        assert_eq!(decode_html_entities("&mdash;"), "\u{2014}");
        assert_eq!(decode_html_entities("&copy;"), "\u{00A9}");
        assert_eq!(decode_html_entities("&hellip;"), "\u{2026}");
    }

    #[test]
    fn decode_unknown_entity_preserved() {
        assert_eq!(decode_html_entities("&foobar;"), "&foobar;");
    }

    // -- UI control text filtering --

    #[test]
    fn ui_control_material_icons() {
        assert!(is_ui_control_line("navigate_before navigate_next"));
        assert!(is_ui_control_line("chevron_left"));
        assert!(is_ui_control_line("arrow_back arrow_forward"));
        assert!(is_ui_control_line("expand_more"));
    }

    #[test]
    fn ui_control_arrow_chars() {
        assert!(is_ui_control_line("\u{2190} \u{2192}"));
        assert!(is_ui_control_line("\u{203A}"));
    }

    #[test]
    fn ui_control_not_real_content() {
        assert!(!is_ui_control_line("Navigate to the next page"));
        assert!(!is_ui_control_line("Click the menu button to expand"));
        assert!(!is_ui_control_line("Search for products"));
        assert!(!is_ui_control_line(""));
    }

    #[test]
    fn ui_control_strip_from_text() {
        let input = "Hello\nnavigate_before navigate_next\nWorld";
        assert_eq!(strip_ui_control_text(input), "Hello\nWorld");
    }

    // -- Long alt-text descriptions --

    #[test]
    fn long_alt_description_detected() {
        assert!(is_long_alt_description(
            "An illustration in the style of lo-fi anime showing a cute dinosaur coding on a laptop in a cozy room with lots of details."
        ));
        assert!(is_long_alt_description(
            "A screenshot showing the dashboard interface with multiple panels and configuration options for the user."
        ));
        assert!(is_long_alt_description(
            "This element contains an interactive demo for sighted users. It's a demonstration of Cursor's IDE showing AI-powered features."
        ));
    }

    #[test]
    fn long_alt_description_real_content_kept() {
        assert!(!is_long_alt_description("An illustration")); // too short
        assert!(!is_long_alt_description(
            "The quick brown fox jumps over the lazy dog and keeps running for a very long time across the field."
        ));
        assert!(!is_long_alt_description(
            "# An illustration of the main heading which is quite long and spans multiple words for testing purposes."
        ));
    }

    // -- CSS artifact filtering --

    #[test]
    fn css_artifact_keyframes_stripped() {
        let input = "curl -fsSL https://deno.land/install.sh | sh@keyframes copy{from{background:var(--runtime)}";
        let out = strip_css_artifacts(input);
        assert_eq!(out, "curl -fsSL https://deno.land/install.sh | sh");
    }

    #[test]
    fn css_artifact_standalone_line() {
        assert!(is_css_artifact_line("selector{property:value}"));
    }

    #[test]
    fn css_artifact_not_real_code() {
        assert!(!is_css_artifact_line("let x = { key: value };"));
        assert!(!is_css_artifact_line("# Heading with {braces}"));
        assert!(!is_css_artifact_line("@username mentioned you"));
    }

    // -- Leaked JavaScript stripping --

    #[test]
    fn leaked_js_self_wrap() {
        let input = "## Accelerate speed, reduce riskself.__wrap_n=self.__wrap_n||(self.CSS&&CSS.supports(\"text-wrap\",\"balance\")?1:2);";
        let result = strip_leaked_js(input);
        assert_eq!(result, "## Accelerate speed, reduce risk");
    }

    #[test]
    fn leaked_js_normal_text_preserved() {
        let input = "Normal text without any JavaScript";
        assert_eq!(strip_leaked_js(input), input);
    }

    #[test]
    fn leaked_js_code_block_preserved() {
        let input = "```\nself.__wrap_n = 42;\n```";
        assert_eq!(strip_leaked_js(input), input);
    }

    // -- Invisible Unicode stripping --

    #[test]
    fn invisible_unicode_stripped() {
        let input = "Hello\u{200B}World\u{200D}Test\u{FEFF}End";
        assert_eq!(strip_invisible_unicode(input), "HelloWorldTestEnd");
    }

    #[test]
    fn invisible_unicode_no_change() {
        let input = "Normal visible text";
        assert_eq!(strip_invisible_unicode(input), input);
    }

    // -- Collapse spaced text --

    #[test]
    fn collapse_spaced_text_basic() {
        assert_eq!(
            collapse_spaced_text("S t a r t D e p l o y i n g"),
            "Start Deploying"
        );
    }

    #[test]
    fn collapse_spaced_text_single_word() {
        assert_eq!(collapse_spaced_text("H e l l o"), "Hello");
    }

    #[test]
    fn collapse_spaced_text_skips_code_blocks() {
        let input = "```\nS t a r t\n```";
        assert_eq!(collapse_spaced_text(input), input);
    }

    #[test]
    fn collapse_spaced_text_short_ignored() {
        // Only 3 real chars -- below threshold of 4
        assert_eq!(collapse_spaced_text("a b c"), "a b c");
    }

    #[test]
    fn collapse_spaced_text_mixed_line() {
        assert_eq!(
            collapse_spaced_text("Welcome to S t a r t"),
            "Welcome to Start"
        );
    }

    // -- Word list collapsing --

    #[test]
    fn long_api_list_collapsed() {
        let words: Vec<&str> = vec![
            "Worker",
            "MessageEvent",
            "WritableStreamDefaultController",
            "DecompressionStream",
            "CompressionStream",
            "Blob",
            "Response",
            "EventTarget",
            "WebSocket",
            "CryptoKey",
            "ErrorEvent",
            "PerformanceMark",
            "WorkerNavigator",
            "TextDecoder",
            "TextEncoder",
            "TransformStream",
            "File",
            "CustomEvent",
            "Event",
            "DOMException",
            "ReadableStream",
            "Storage",
            "WebAssembly",
            "URLSearchParams",
            "ProgressEvent",
            "FileReader",
        ];
        let line = words.join(" ");
        let input = format!("Some prefix text {line}");
        let result = collapse_word_lists(&input);
        assert!(result.contains("... and"), "should collapse: {result}");
    }

    #[test]
    fn normal_prose_not_collapsed() {
        let input = "This is a perfectly normal paragraph with lots of words but they are all lowercase prose that should not be collapsed because it's actual content.";
        assert_eq!(collapse_word_lists(input), input);
    }

    // -- Adjacent description dedup --

    #[test]
    fn adjacent_description_deduped() {
        let input = "Infrastructure From overview to deep details, fast\nLearn more ** From overview to deep details, fast\nAPM Monitor performance";
        let result = dedup_adjacent_descriptions(input);
        assert!(result.contains("Infrastructure From overview"));
        assert!(!result.contains("Learn more ** From overview"));
        assert!(result.contains("APM Monitor"));
    }

    #[test]
    fn non_duplicate_learn_more_preserved() {
        let input = "Product A does something\nLearn more about different things\nProduct B";
        assert_eq!(dedup_adjacent_descriptions(input), input);
    }

    // -- Alt text noise --

    #[test]
    fn alt_text_descriptive_prefix_stripped() {
        let input = "Hello\nImage of Glossier website selling beauty products\nWorld";
        assert_eq!(strip_alt_text_noise(input), "Hello\nWorld");
    }

    #[test]
    fn alt_text_photo_prefix_stripped() {
        let input = "Text\nPhoto of customer at plant retailer The Sill\nMore";
        assert_eq!(strip_alt_text_noise(input), "Text\nMore");
    }

    #[test]
    fn alt_text_animation_stripped() {
        let input = "Above\nAnimation of example abandoned cart email with graph\nBelow";
        assert_eq!(strip_alt_text_noise(input), "Above\nBelow");
    }

    #[test]
    fn alt_text_normal_prose_kept() {
        let input = "Image quality is important for this use case";
        assert_eq!(strip_alt_text_noise(input), input);
    }

    #[test]
    fn alt_text_short_prefix_kept() {
        let input = "Image of X";
        assert_eq!(strip_alt_text_noise(input), input);
    }

    #[test]
    fn broken_image_fragment_stripped() {
        assert_eq!(strip_alt_text_noise("Hello\n.webp)\nWorld"), "Hello\nWorld");
        assert_eq!(
            strip_alt_text_noise("Text\n.svg)                .webp)\nMore"),
            "Text\nMore"
        );
    }

    #[test]
    fn social_avatar_labels_stripped() {
        let input = "@a twitter image, @b twitter image, @c twitter image";
        assert_eq!(strip_alt_text_noise(input), "");
    }

    #[test]
    fn repeated_brand_list_stripped() {
        let input =
            "Supabase DB, Supabase Auth, Supabase Functions, Supabase Storage, Supabase Vector";
        assert_eq!(strip_alt_text_noise(input), "");
    }

    #[test]
    fn alt_text_code_block_preserved() {
        let input = "```\nImage of something in code\n```";
        assert_eq!(strip_alt_text_noise(input), input);
    }

    #[test]
    fn a11y_strips_opens_new_tab() {
        let input = "Download the App, opens new tab and Subscribe, opens new tab.";
        let out = strip_a11y_link_chrome(input);
        assert!(!out.to_lowercase().contains("opens new tab"), "leak: {out}");
        assert!(out.contains("Download the App"));
        assert!(out.contains("Subscribe"));
    }

    #[test]
    fn a11y_strips_external_link_variants() {
        let cases = [
            ("Visit our docs, opens external link", "Visit our docs"),
            ("Click here, opens in a new window.", "Click here"),
            ("More info external link", "More info"),
        ];
        for (input, expected_prefix) in cases {
            let out = strip_a11y_link_chrome(input);
            assert!(
                out.starts_with(expected_prefix),
                "input={input:?} got={out:?}"
            );
            assert!(!out.to_lowercase().contains("opens"), "leak: {out}");
        }
    }

    #[test]
    fn a11y_preserves_code_blocks() {
        let input = "```\nopens new tab is a function\n```\nDownload, opens new tab";
        let out = strip_a11y_link_chrome(input);
        assert!(
            out.contains("opens new tab is a function"),
            "code stripped: {out}"
        );
        // Outside the fence, the chrome is removed.
        assert!(!out.to_lowercase().contains("download, opens new tab"));
    }

    #[test]
    fn a11y_preserves_external_link_prose() {
        let input = "Researchers found an external link between the two incidents.";
        assert_eq!(strip_a11y_link_chrome(input), input);
    }
}
