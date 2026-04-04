/// HTML-to-markdown converter.
/// Walks the DOM tree and emits clean markdown, resolving relative URLs
/// against the provided base URL when available.
use std::collections::HashSet;

use ego_tree::NodeId;
use once_cell::sync::Lazy;
use scraper::node::Node;
use scraper::{ElementRef, Selector};
use url::Url;

use crate::noise;
use crate::types::{CodeBlock, Image, Link};

static CODE_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("code").unwrap());

/// Maximum recursion depth for DOM traversal.
/// Express.co.uk live blogs and similar pages can nest 1000+ levels deep,
/// overflowing the default ~1 MB stack on Windows.  When we hit this limit
/// we fall back to plain-text collection (which uses an iterator, not recursion).
const MAX_DOM_DEPTH: usize = 200;

/// Collected assets found during conversion.
pub struct ConvertedAssets {
    pub links: Vec<Link>,
    pub images: Vec<Image>,
    pub code_blocks: Vec<CodeBlock>,
}

/// Convert an element subtree to markdown + plain text.
/// Elements whose NodeId is in `exclude` (and their descendants) are skipped.
pub fn convert(
    element: ElementRef<'_>,
    base_url: Option<&Url>,
    exclude: &HashSet<NodeId>,
) -> (String, String, ConvertedAssets) {
    let mut assets = ConvertedAssets {
        links: Vec::new(),
        images: Vec::new(),
        code_blocks: Vec::new(),
    };

    let md = node_to_md(element, base_url, &mut assets, 0, exclude, 0);
    let plain = strip_markdown(&md);
    let md = collapse_whitespace(&md);
    let plain = collapse_whitespace(&plain);

    (md, plain, assets)
}

/// Recursive descent through the DOM, emitting markdown for each node.
fn node_to_md(
    element: ElementRef<'_>,
    base_url: Option<&Url>,
    assets: &mut ConvertedAssets,
    list_depth: usize,
    exclude: &HashSet<NodeId>,
    depth: usize,
) -> String {
    if exclude.contains(&element.id()) {
        return String::new();
    }

    // Guard against deeply nested DOM trees (e.g., Express.co.uk live blogs).
    if depth > MAX_DOM_DEPTH {
        return collect_text(element);
    }

    if noise::is_noise(element) || noise::is_noise_descendant(element) {
        // Still collect images and links from noise elements — they're useful
        // metadata even though we don't include the noise text in markdown.
        // We strip noise text but preserve link/image references as metadata.
        collect_assets_from_noise(element, base_url, assets);
        return String::new();
    }

    let tag = element.value().name();
    match tag {
        // Headings
        "h1" => format!(
            "\n\n# {}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),
        "h2" => format!(
            "\n\n## {}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),
        "h3" => format!(
            "\n\n### {}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),
        "h4" => format!(
            "\n\n#### {}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),
        "h5" => format!(
            "\n\n##### {}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),
        "h6" => format!(
            "\n\n###### {}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),

        // Paragraph
        "p" => format!(
            "\n\n{}\n\n",
            inline_text(element, base_url, assets, exclude, depth)
        ),

        // Links
        "a" => {
            let text = inline_text(element, base_url, assets, exclude, depth);
            let href = element
                .value()
                .attr("href")
                .map(|h| resolve_url(h, base_url))
                .unwrap_or_default();

            if !text.is_empty() && !href.is_empty() {
                assets.links.push(Link {
                    text: text.clone(),
                    href: href.clone(),
                });
                format!("[{text}]({href})")
            } else if !text.is_empty() {
                text
            } else {
                String::new()
            }
        }

        // Images — handle lazy loading (data-src), srcset, and skip base64/blob
        "img" => {
            let alt = element.value().attr("alt").unwrap_or("").to_string();

            // Resolve src: prefer src, fall back to data-src (lazy loading),
            // then data-lazy-src, data-original (common lazy load patterns)
            let raw_src = element
                .value()
                .attr("src")
                .or_else(|| element.value().attr("data-src"))
                .or_else(|| element.value().attr("data-lazy-src"))
                .or_else(|| element.value().attr("data-original"))
                .unwrap_or("");

            // Skip base64 data URIs and blob URLs (they bloat markdown)
            let src = if raw_src.starts_with("data:") || raw_src.starts_with("blob:") {
                String::new()
            } else {
                resolve_url(raw_src, base_url)
            };

            // Try srcset for better resolution image
            let src = if src.is_empty() {
                // No src found, try srcset
                element
                    .value()
                    .attr("srcset")
                    .and_then(pick_best_srcset)
                    .map(|s| resolve_url(&s, base_url))
                    .unwrap_or_default()
            } else {
                src
            };

            if !src.is_empty() {
                assets.images.push(Image {
                    alt: alt.clone(),
                    src: src.clone(),
                });
                format!("![{alt}]({src})")
            } else {
                String::new()
            }
        }

        // Bold — if it contains block elements (e.g., Drudge wraps entire columns
        // in <b>), treat as a container instead of inline bold.
        "strong" | "b" => {
            if cell_has_block_content(element) {
                children_to_md(element, base_url, assets, list_depth, exclude, depth)
            } else {
                format!("**{}**", inline_text(element, base_url, assets, exclude, depth))
            }
        }

        // Italic — same block-content check as bold.
        "em" | "i" => {
            if cell_has_block_content(element) {
                children_to_md(element, base_url, assets, list_depth, exclude, depth)
            } else {
                format!("*{}*", inline_text(element, base_url, assets, exclude, depth))
            }
        }

        // Inline code
        "code" => {
            // If parent is <pre>, this is handled by the "pre" arm
            if is_inside_pre(element) {
                // Just return raw text — the pre handler wraps it
                collect_text(element)
            } else {
                let text = collect_text(element);
                if text.is_empty() {
                    String::new()
                } else {
                    format!("`{text}`")
                }
            }
        }

        // Fenced code blocks
        "pre" => {
            let code_el = element.select(&CODE_SELECTOR).next();
            let (code, lang) = if let Some(code_el) = code_el {
                // Try <code> class first, then fall back to <pre> class
                let lang = code_el
                    .value()
                    .attr("class")
                    .and_then(extract_language_from_class)
                    .or_else(|| {
                        element
                            .value()
                            .attr("class")
                            .and_then(extract_language_from_class)
                    });
                (collect_preformatted_text(code_el, depth), lang)
            } else {
                let lang = element
                    .value()
                    .attr("class")
                    .and_then(extract_language_from_class);
                (collect_preformatted_text(element, depth), lang)
            };

            let code = code.trim_matches('\n').to_string();
            assets.code_blocks.push(CodeBlock {
                language: lang.clone(),
                code: code.clone(),
            });

            let fence_lang = lang.as_deref().unwrap_or("");
            format!("\n\n```{fence_lang}\n{code}\n```\n\n")
        }

        // Blockquote
        "blockquote" => {
            let inner = children_to_md(element, base_url, assets, list_depth, exclude, depth);
            let quoted = inner
                .trim()
                .lines()
                .map(|line| format!("> {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("\n\n{quoted}\n\n")
        }

        // Unordered list
        "ul" => {
            let items = list_items(element, base_url, assets, list_depth, false, exclude, depth);
            format!("\n\n{items}\n\n")
        }

        // Ordered list
        "ol" => {
            let items = list_items(element, base_url, assets, list_depth, true, exclude, depth);
            format!("\n\n{items}\n\n")
        }

        // List item — handled by ul/ol parent, but if encountered standalone:
        "li" => {
            let text = inline_text(element, base_url, assets, exclude, depth);
            format!("- {text}\n")
        }

        // Horizontal rule
        "hr" => "\n\n---\n\n".to_string(),

        // Line break
        "br" => "\n".to_string(),

        // Table
        "table" => format!(
            "\n\n{}\n\n",
            table_to_md(element, base_url, assets, exclude, depth)
        ),

        // Divs and other containers — just recurse
        _ => children_to_md(element, base_url, assets, list_depth, exclude, depth),
    }
}

/// Collect markdown from all children of an element.
fn children_to_md(
    element: ElementRef<'_>,
    base_url: Option<&Url>,
    assets: &mut ConvertedAssets,
    list_depth: usize,
    exclude: &HashSet<NodeId>,
    depth: usize,
) -> String {
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    let chunk = node_to_md(child_el, base_url, assets, list_depth, exclude, depth + 1);
                    if !chunk.is_empty() && !out.is_empty() && needs_separator(&out, &chunk) {
                        out.push(' ');
                    }
                    out.push_str(&chunk);
                }
            }
            Node::Text(text) => {
                out.push_str(text);
            }
            _ => {}
        }
    }
    out
}

/// Collect inline text — walks children, converting inline elements to markdown.
/// This is for contexts where we want inline content (headings, paragraphs, links).
fn inline_text(
    element: ElementRef<'_>,
    base_url: Option<&Url>,
    assets: &mut ConvertedAssets,
    exclude: &HashSet<NodeId>,
    depth: usize,
) -> String {
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Element(_) => {
                if let Some(child_el) = ElementRef::wrap(child) {
                    let chunk = node_to_md(child_el, base_url, assets, 0, exclude, depth + 1);
                    if !chunk.is_empty() && !out.is_empty() && needs_separator(&out, &chunk) {
                        out.push(' ');
                    }
                    out.push_str(&chunk);
                }
            }
            Node::Text(text) => {
                out.push_str(text);
            }
            _ => {}
        }
    }
    // Collapse internal whitespace for inline content
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Check whether a space is needed between two adjacent chunks of output.
/// Returns true when the left side doesn't end with whitespace and the right
/// side doesn't start with whitespace — i.e., two words would be mashed together.
fn needs_separator(left: &str, right: &str) -> bool {
    let l = left.as_bytes().last().copied().unwrap_or(b' ');
    let r = right.as_bytes().first().copied().unwrap_or(b' ');
    !l.is_ascii_whitespace() && !r.is_ascii_whitespace()
}

/// Collect raw text content (no markdown formatting).
fn collect_text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>()
}

/// Collect text from a preformatted element, preserving all whitespace.
/// Every text node is pushed verbatim -- no trimming, no collapsing.
/// Handles `<br>` as newlines and inserts newlines between block-level children
/// (e.g., `<div>` lines produced by some syntax highlighters).
fn collect_preformatted_text(element: ElementRef<'_>, depth: usize) -> String {
    if depth > MAX_DOM_DEPTH {
        return element.text().collect::<String>();
    }
    let mut out = String::new();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(el) => {
                let tag = el.name.local.as_ref();
                if tag == "br" {
                    out.push('\n');
                } else if let Some(child_el) = ElementRef::wrap(child) {
                    if tag == "div" || tag == "p" {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out.push_str(&collect_preformatted_text(child_el, depth + 1));
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                    } else {
                        out.push_str(&collect_preformatted_text(child_el, depth + 1));
                    }
                }
            }
            _ => {}
        }
    }
    out
}

fn is_inside_pre(element: ElementRef<'_>) -> bool {
    let mut node = element.parent();
    while let Some(parent) = node {
        if let Some(el) = ElementRef::wrap(parent)
            && el.value().name() == "pre"
        {
            return true;
        }
        node = parent.parent();
    }
    false
}

fn list_items(
    list_el: ElementRef<'_>,
    base_url: Option<&Url>,
    assets: &mut ConvertedAssets,
    depth: usize,
    ordered: bool,
    exclude: &HashSet<NodeId>,
    dom_depth: usize,
) -> String {
    let indent = "  ".repeat(depth);
    let mut out = String::new();
    let mut index = 1;

    for child in list_el.children() {
        if let Some(child_el) = ElementRef::wrap(child) {
            if exclude.contains(&child_el.id()) {
                continue;
            }
            let tag = child_el.value().name();
            if tag == "li" {
                let bullet = if ordered {
                    let b = format!("{index}.");
                    index += 1;
                    b
                } else {
                    "-".to_string()
                };

                // Separate nested lists from inline content
                let mut inline_parts = String::new();
                let mut nested_lists = String::new();

                for li_child in child_el.children() {
                    if let Some(li_child_el) = ElementRef::wrap(li_child) {
                        if exclude.contains(&li_child_el.id()) {
                            continue;
                        }
                        let child_tag = li_child_el.value().name();
                        if child_tag == "ul" || child_tag == "ol" {
                            nested_lists.push_str(&list_items(
                                li_child_el,
                                base_url,
                                assets,
                                depth + 1,
                                child_tag == "ol",
                                exclude,
                                dom_depth + 1,
                            ));
                        } else {
                            inline_parts.push_str(&node_to_md(
                                li_child_el,
                                base_url,
                                assets,
                                depth,
                                exclude,
                                dom_depth + 1,
                            ));
                        }
                    } else if let Some(text) = li_child.value().as_text() {
                        inline_parts.push_str(text);
                    }
                }

                let text = inline_parts
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                out.push_str(&format!("{indent}{bullet} {text}\n"));

                if !nested_lists.is_empty() {
                    out.push_str(&nested_lists);
                }
            }
        }
    }
    out.trim_end_matches('\n').to_string()
}

/// Check whether a table cell contains block-level elements, indicating a layout
/// table rather than a data table.
fn cell_has_block_content(cell: ElementRef<'_>) -> bool {
    const BLOCK_TAGS: &[&str] = &[
        "p", "div", "ul", "ol", "blockquote", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "pre",
        "table", "section", "article", "header", "footer", "nav", "aside",
    ];
    for desc in cell.descendants() {
        if let Some(el) = ElementRef::wrap(desc) {
            if BLOCK_TAGS.contains(&el.value().name()) {
                return true;
            }
        }
    }
    false
}

fn table_to_md(
    table_el: ElementRef<'_>,
    base_url: Option<&Url>,
    assets: &mut ConvertedAssets,
    exclude: &HashSet<NodeId>,
    depth: usize,
) -> String {
    // Collect all <td>/<th> cells grouped by row, and detect layout tables
    let mut raw_rows: Vec<Vec<ElementRef<'_>>> = Vec::new();
    let mut has_header = false;
    let mut is_layout = false;

    for child in table_el.descendants() {
        if let Some(el) = ElementRef::wrap(child) {
            if exclude.contains(&el.id()) {
                continue;
            }
            if el.value().name() == "tr" {
                let cells: Vec<ElementRef<'_>> = el
                    .children()
                    .filter_map(ElementRef::wrap)
                    .filter(|c| {
                        !exclude.contains(&c.id())
                            && (c.value().name() == "th" || c.value().name() == "td")
                    })
                    .map(|c| {
                        if c.value().name() == "th" {
                            has_header = true;
                        }
                        if !is_layout && cell_has_block_content(c) {
                            is_layout = true;
                        }
                        c
                    })
                    .collect();

                if !cells.is_empty() {
                    raw_rows.push(cells);
                }
            }
        }
    }

    if raw_rows.is_empty() {
        return String::new();
    }

    // Layout table: render each cell as a standalone block section
    if is_layout {
        let mut out = String::new();
        for row in &raw_rows {
            for cell in row {
                let content =
                    children_to_md(*cell, base_url, assets, 0, exclude, depth);
                let content = content.trim();
                if !content.is_empty() {
                    if !out.is_empty() {
                        out.push_str("\n\n");
                    }
                    out.push_str(content);
                }
            }
        }
        return out;
    }

    // Data table: render as markdown table
    let mut rows: Vec<Vec<String>> = raw_rows
        .iter()
        .map(|row| {
            row.iter()
                .map(|c| inline_text(*c, base_url, assets, exclude, depth))
                .collect()
        })
        .collect();

    // Find max column count
    let cols = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if cols == 0 {
        return String::new();
    }

    // Normalize row lengths
    for row in &mut rows {
        while row.len() < cols {
            row.push(String::new());
        }
    }

    let mut out = String::new();

    // Header row
    let header = &rows[0];
    out.push_str("| ");
    out.push_str(&header.join(" | "));
    out.push_str(" |\n");

    // Separator
    out.push_str("| ");
    out.push_str(&(0..cols).map(|_| "---").collect::<Vec<_>>().join(" | "));
    out.push_str(" |\n");

    // Data rows (skip first if it was a header)
    let start = if has_header { 1 } else { 0 };
    for row in &rows[start..] {
        out.push_str("| ");
        out.push_str(&row.join(" | "));
        out.push_str(" |\n");
    }

    out.trim_end().to_string()
}

/// Extract language hint from code element class (e.g., "language-rust", "lang-js", "highlight-python")
/// Known language names to match as bare class values (e.g., `class="javascript"`).
const KNOWN_LANGS: &[&str] = &[
    "javascript",
    "typescript",
    "python",
    "rust",
    "go",
    "java",
    "c",
    "cpp",
    "csharp",
    "ruby",
    "php",
    "swift",
    "kotlin",
    "scala",
    "shell",
    "bash",
    "zsh",
    "fish",
    "sql",
    "html",
    "css",
    "scss",
    "sass",
    "less",
    "json",
    "yaml",
    "yml",
    "toml",
    "xml",
    "markdown",
    "md",
    "jsx",
    "tsx",
    "vue",
    "svelte",
    "graphql",
    "protobuf",
    "dockerfile",
    "makefile",
    "lua",
    "perl",
    "r",
    "matlab",
    "haskell",
    "elixir",
    "erlang",
    "clojure",
    "dart",
    "zig",
    "nim",
    "wasm",
    "diff",
    "text",
    "plaintext",
    "console",
];

fn extract_language_from_class(class: &str) -> Option<String> {
    for cls in class.split_whitespace() {
        // Standard prefixes: language-js, lang-python, highlight-rust
        for prefix in &["language-", "lang-", "highlight-"] {
            if let Some(lang) = cls.strip_prefix(prefix)
                && !lang.is_empty()
                && lang.len() < 20
            {
                return Some(normalize_lang(lang));
            }
        }
        // Sandpack prefix (sp-javascript, sp-python) — validate against known langs
        if let Some(lang) = cls.strip_prefix("sp-") {
            let lower = lang.to_lowercase();
            if KNOWN_LANGS.contains(&lower.as_str()) {
                return Some(normalize_lang(&lower));
            }
        }
        // Bare language name as class: class="javascript" or class="python"
        let lower = cls.to_lowercase();
        if KNOWN_LANGS.contains(&lower.as_str()) {
            return Some(normalize_lang(&lower));
        }
    }
    None
}

/// Normalize language identifiers to common short forms.
fn normalize_lang(lang: &str) -> String {
    match lang.to_lowercase().as_str() {
        "javascript" | "js" => "js".to_string(),
        "typescript" | "ts" => "ts".to_string(),
        "python" | "py" => "python".to_string(),
        "csharp" | "cs" | "c#" => "csharp".to_string(),
        "cpp" | "c++" => "cpp".to_string(),
        "shell" | "bash" | "zsh" | "sh" => "bash".to_string(),
        "yaml" | "yml" => "yaml".to_string(),
        "markdown" | "md" => "markdown".to_string(),
        "plaintext" | "text" => "text".to_string(),
        other => other.to_string(),
    }
}

/// Pick the best (largest) image from an HTML srcset attribute.
/// srcset format: "url1 300w, url2 600w, url3 1200w" or "url1 1x, url2 2x"
fn pick_best_srcset(srcset: &str) -> Option<String> {
    let mut best_url = None;
    let mut best_size: u32 = 0;

    for entry in srcset.split(',') {
        let parts: Vec<&str> = entry.split_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        let url = parts[0];
        // Skip data URIs
        if url.starts_with("data:") || url.starts_with("blob:") {
            continue;
        }
        let size = if parts.len() > 1 {
            let descriptor = parts[1];
            // Parse "300w" or "2x"
            descriptor
                .trim_end_matches(|c: char| !c.is_ascii_digit())
                .parse::<u32>()
                .unwrap_or(1)
        } else {
            1
        };
        if size > best_size {
            best_size = size;
            best_url = Some(url.to_string());
        }
    }

    best_url
}

/// Collect images and links from a noise element without adding text to markdown.
/// This preserves valuable metadata (links, images) from nav/header/footer
/// that would otherwise be completely lost.
fn collect_assets_from_noise(
    element: ElementRef<'_>,
    base_url: Option<&Url>,
    assets: &mut ConvertedAssets,
) {
    // Collect images with alt text
    for img in element.select(&Selector::parse("img[alt]").unwrap()) {
        let alt = img.value().attr("alt").unwrap_or("").to_string();
        let src = img
            .value()
            .attr("src")
            .map(|s| resolve_url(s, base_url))
            .unwrap_or_default();
        if !src.is_empty() && !alt.is_empty() {
            assets.images.push(Image { alt, src });
        }
    }

    // Collect links
    for link in element.select(&Selector::parse("a[href]").unwrap()) {
        let href = link
            .value()
            .attr("href")
            .map(|h| resolve_url(h, base_url))
            .unwrap_or_default();
        let text: String = link.text().collect::<String>().trim().to_string();
        if !href.is_empty() && !text.is_empty() && href.starts_with("http") {
            assets.links.push(Link { text, href });
        }
    }
}

pub fn resolve_url(href: &str, base_url: Option<&Url>) -> String {
    // Absolute URLs pass through
    if href.starts_with("http://") || href.starts_with("https://") || href.starts_with("//") {
        return href.to_string();
    }

    // Try resolving against base
    if let Some(base) = base_url
        && let Ok(resolved) = base.join(href)
    {
        return resolved.to_string();
    }

    href.to_string()
}

/// Collapse excessive whitespace: max 2 consecutive newlines, trim trailing
/// whitespace from lines. Content inside fenced code blocks (``` ... ```) is
/// passed through verbatim to preserve indentation and preformatted layout.
fn collapse_whitespace(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut consecutive_newlines = 0;
    let mut in_code_fence = false;

    for line in s.lines() {
        // Detect code fence boundaries
        if line.trim_start().starts_with("```") {
            in_code_fence = !in_code_fence;
            consecutive_newlines = 0;
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(line.trim_end());
            result.push('\n');
            continue;
        }

        // Inside code fences: preserve content exactly (only trim trailing whitespace)
        if in_code_fence {
            result.push_str(line.trim_end());
            result.push('\n');
            continue;
        }

        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                result.push('\n');
            }
        } else {
            consecutive_newlines = 0;
            if !result.is_empty() && !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(trimmed);
            result.push('\n');
        }
    }

    result.trim().to_string()
}

/// Crude markdown stripping for plain_text output.
fn strip_markdown(md: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;

    static LINK_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").unwrap());
    static IMG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").unwrap());
    static BOLD_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*\*([^*]+)\*\*").unwrap());
    static ITALIC_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\*([^*]+)\*").unwrap());
    static CODE_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"`([^`]+)`").unwrap());
    static HEADING_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^#{1,6}\s+").unwrap());
    // Table separator rows: | --- | --- | (with optional colons for alignment)
    static TABLE_SEP_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"^\|\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|$").unwrap());

    let s = IMG_RE.replace_all(md, "$1");
    let s = LINK_RE.replace_all(&s, "$1");
    let s = BOLD_RE.replace_all(&s, "$1");
    let s = ITALIC_RE.replace_all(&s, "$1");
    let s = CODE_RE.replace_all(&s, "$1");
    let s = HEADING_RE.replace_all(&s, "");

    // Remove fenced code block markers + strip table syntax
    let mut lines: Vec<String> = Vec::new();
    let mut in_fence = false;
    for line in s.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }

        let trimmed = line.trim();

        // Skip table separator rows (| --- | --- |)
        if TABLE_SEP_RE.is_match(trimmed) {
            continue;
        }

        // Convert table data rows: strip leading/trailing pipes, replace inner pipes with tabs
        if trimmed.starts_with('|') && trimmed.ends_with('|') {
            let inner = &trimmed[1..trimmed.len() - 1];
            let cells: Vec<&str> = inner.split('|').map(|c| c.trim()).collect();
            lines.push(cells.join("\t"));
            continue;
        }

        lines.push(line.to_string());
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use scraper::Html;

    fn convert_html(html: &str, base: Option<&str>) -> (String, String, ConvertedAssets) {
        let doc = Html::parse_fragment(html);
        let root = doc.root_element();
        let base_url = base.and_then(|u| Url::parse(u).ok());
        convert(root, base_url.as_ref(), &HashSet::new())
    }

    #[test]
    fn headings() {
        let (md, _, _) = convert_html("<h1>Title</h1>", None);
        assert!(md.contains("# Title"));

        let (md, _, _) = convert_html("<h3>Sub</h3>", None);
        assert!(md.contains("### Sub"));
    }

    #[test]
    fn paragraphs_and_inline() {
        let (md, _, _) = convert_html(
            "<p>Hello <strong>world</strong> and <em>stuff</em></p>",
            None,
        );
        assert!(md.contains("Hello **world** and *stuff*"));
    }

    #[test]
    fn links_collected() {
        let (md, _, assets) = convert_html(
            r#"<p><a href="https://example.com">Click here</a></p>"#,
            None,
        );
        assert!(md.contains("[Click here](https://example.com)"));
        assert_eq!(assets.links.len(), 1);
        assert_eq!(assets.links[0].href, "https://example.com");
    }

    #[test]
    fn relative_url_resolution() {
        let (md, _, _) = convert_html(
            r#"<a href="/about">About</a>"#,
            Some("https://example.com/page"),
        );
        assert!(md.contains("[About](https://example.com/about)"));
    }

    #[test]
    fn images_collected() {
        let (md, _, assets) = convert_html(
            r#"<img src="https://img.example.com/photo.jpg" alt="A photo">"#,
            None,
        );
        assert!(md.contains("![A photo](https://img.example.com/photo.jpg)"));
        assert_eq!(assets.images.len(), 1);
    }

    #[test]
    fn code_blocks() {
        let (md, _, assets) = convert_html(
            r#"<pre><code class="language-rust">fn main() {}</code></pre>"#,
            None,
        );
        assert!(md.contains("```rust"));
        assert!(md.contains("fn main() {}"));
        assert_eq!(assets.code_blocks.len(), 1);
        assert_eq!(assets.code_blocks[0].language.as_deref(), Some("rust"));
    }

    #[test]
    fn multiline_code_preserves_newlines() {
        let html = "<pre><code class=\"language-js\">function App() {\n  const [count, setCount] = useState(0);\n  return count;\n}</code></pre>";
        let (md, _, assets) = convert_html(html, None);
        assert!(md.contains("```js"), "missing language fence: {md}");
        assert!(
            md.contains("function App() {\n  const [count, setCount] = useState(0);"),
            "newlines collapsed in code block: {md}"
        );
        assert_eq!(assets.code_blocks.len(), 1);
        assert_eq!(assets.code_blocks[0].language.as_deref(), Some("js"));
    }

    #[test]
    fn multiline_code_with_br_tags() {
        let html = "<pre><code class=\"language-js\">function App() {<br>  const x = 1;<br>  return x;<br>}</code></pre>";
        let (md, _, _) = convert_html(html, None);
        assert!(md.contains("```js"), "missing language fence: {md}");
        assert!(
            md.contains("function App() {\n  const x = 1;\n  return x;\n}"),
            "br tags not converted to newlines in code block: {md}"
        );
    }

    #[test]
    fn multiline_code_with_div_lines() {
        let html = "<pre><code class=\"language-py\"><div>def hello():</div><div>    print(\"hi\")</div></code></pre>";
        let (md, _, _) = convert_html(html, None);
        assert!(md.contains("```py"), "missing language fence: {md}");
        assert!(
            md.contains("def hello():\n"),
            "div-separated lines not preserved in code block: {md}"
        );
    }

    #[test]
    fn multiline_code_with_span_children() {
        let html = "<pre><code class=\"language-js\"><span class=\"token keyword\">function</span> <span class=\"token function\">App</span>() {\n  <span class=\"token keyword\">const</span> [count, setCount] = useState(0);\n  <span class=\"token keyword\">return</span> count;\n}</code></pre>";
        let (md, _, assets) = convert_html(html, None);
        assert!(md.contains("```js"), "missing language fence: {md}");
        assert!(
            md.contains("function App() {\n  const"),
            "newlines collapsed in highlighted code block: {md}"
        );
        assert_eq!(assets.code_blocks.len(), 1);
    }

    #[test]
    fn multiline_code_no_inline_markdown() {
        let html = "<pre><code>let **x** = *y*;\nlet a = b;</code></pre>";
        let (md, _, _) = convert_html(html, None);
        assert!(
            md.contains("let **x** = *y*;"),
            "code block content was processed for inline markdown: {md}"
        );
    }

    #[test]
    fn inline_code() {
        let (md, _, _) = convert_html("<p>Use <code>cargo build</code> to compile</p>", None);
        assert!(md.contains("`cargo build`"));
    }

    #[test]
    fn unordered_list() {
        let (md, _, _) = convert_html("<ul><li>Alpha</li><li>Beta</li></ul>", None);
        assert!(md.contains("- Alpha"));
        assert!(md.contains("- Beta"));
    }

    #[test]
    fn ordered_list() {
        let (md, _, _) = convert_html("<ol><li>First</li><li>Second</li></ol>", None);
        assert!(md.contains("1. First"));
        assert!(md.contains("2. Second"));
    }

    #[test]
    fn blockquote() {
        let (md, _, _) = convert_html("<blockquote><p>A wise quote</p></blockquote>", None);
        assert!(md.contains("> A wise quote"));
    }

    #[test]
    fn table() {
        let html = r##"
        <table>
            <thead><tr><th>Name</th><th>Age</th></tr></thead>
            <tbody><tr><td>Alice</td><td>30</td></tr></tbody>
        </table>"##;
        let (md, _, _) = convert_html(html, None);
        assert!(md.contains("| Name | Age |"));
        assert!(md.contains("| --- | --- |"));
        assert!(md.contains("| Alice | 30 |"));
    }

    #[test]
    fn layout_table() {
        // Layout tables (cells with block elements) should render as sections, not markdown tables
        let html = r##"
        <table>
            <tr>
                <td>
                    <p>Column one first paragraph</p>
                    <p>Column one second paragraph</p>
                </td>
                <td>
                    <p>Column two content</p>
                    <hr>
                    <p>Column two after rule</p>
                </td>
            </tr>
        </table>"##;
        let (md, _, _) = convert_html(html, None);
        // Should NOT produce markdown table syntax
        assert!(!md.contains("| "), "layout table should not use pipe syntax: {md}");
        // Should contain the content as separate blocks
        assert!(md.contains("Column one first paragraph"), "missing content: {md}");
        assert!(md.contains("Column two content"), "missing content: {md}");
        assert!(md.contains("Column two after rule"), "missing content: {md}");
    }

    #[test]
    fn layout_table_with_links() {
        // Drudge-style layout: cells full of links and divs
        let html = r##"
        <table>
            <tr>
                <td>
                    <div><a href="https://example.com/1">Headline One</a></div>
                    <div><a href="https://example.com/2">Headline Two</a></div>
                </td>
                <td>
                    <div><a href="https://example.com/3">Headline Three</a></div>
                </td>
            </tr>
        </table>"##;
        let (md, _, _) = convert_html(html, None);
        assert!(!md.contains("| "), "layout table should not use pipe syntax: {md}");
        assert!(md.contains("[Headline One](https://example.com/1)"), "missing link: {md}");
        assert!(md.contains("[Headline Two](https://example.com/2)"), "missing link: {md}");
        assert!(md.contains("[Headline Three](https://example.com/3)"), "missing link: {md}");
    }

    #[test]
    fn horizontal_rule() {
        let (md, _, _) = convert_html("<p>Above</p><hr><p>Below</p>", None);
        assert!(md.contains("---"));
    }

    #[test]
    fn strips_to_plain_text() {
        let (_, plain, _) = convert_html(
            "<p>Hello <strong>bold</strong> <a href='#'>link</a></p>",
            None,
        );
        assert!(plain.contains("Hello bold link"));
        assert!(!plain.contains("**"));
        assert!(!plain.contains("["));
    }

    #[test]
    fn strips_table_syntax_from_plain_text() {
        let html = r##"
        <table>
            <thead><tr><th>Name</th><th>Age</th></tr></thead>
            <tbody><tr><td>Alice</td><td>30</td></tr></tbody>
        </table>"##;
        let (md, plain, _) = convert_html(html, None);
        // Markdown should have table syntax
        assert!(md.contains("| --- |"));
        // Plain text should NOT have any pipe or separator syntax
        assert!(!plain.contains("| --- |"), "separator row leaked: {plain}");
        assert!(!plain.contains("| Name"), "pipe syntax leaked: {plain}");
        assert!(plain.contains("Name"), "table content missing: {plain}");
        assert!(plain.contains("Alice"), "table content missing: {plain}");
    }

    #[test]
    fn nested_list() {
        let html = r##"
        <ul>
            <li>Top
                <ul>
                    <li>Nested</li>
                </ul>
            </li>
        </ul>"##;
        let (md, _, _) = convert_html(html, None);
        assert!(md.contains("- Top"));
        assert!(md.contains("  - Nested"));
    }

    // --- Noise stripping tests ---

    #[test]
    fn strips_nav_sidebar_from_content() {
        let html = r##"
        <div>
            <nav>
                <ul>
                    <li><a href="/">Home</a></li>
                    <li><a href="/about">About</a></li>
                    <li><a href="/contact">Contact</a></li>
                </ul>
            </nav>
            <div class="sidebar">
                <h3>Related Articles</h3>
                <ul><li><a href="/other">Other article</a></li></ul>
            </div>
            <article>
                <h1>Main Article Title</h1>
                <p>This is the actual content that readers care about.</p>
            </article>
        </div>"##;
        let (md, plain, _) = convert_html(html, None);

        assert!(md.contains("Main Article Title"));
        assert!(md.contains("actual content"));
        assert!(!md.contains("Home"), "nav link 'Home' leaked into output");
        assert!(!md.contains("About"), "nav link 'About' leaked into output");
        assert!(
            !md.contains("Related Articles"),
            "sidebar heading leaked into output"
        );
        assert!(
            !plain.contains("Other article"),
            "sidebar link leaked into plain text"
        );
    }

    #[test]
    fn strips_script_content() {
        let html = r##"
        <div>
            <p>Real content here.</p>
            <script>
                var React = require('react');
                window.__NEXT_DATA__ = {"props":{"pageProps":{}}};
                console.log("hydration complete");
            </script>
            <script type="application/json">{"key": "value"}</script>
            <p>More real content.</p>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(md.contains("Real content here"));
        assert!(md.contains("More real content"));
        assert!(!md.contains("React"), "script variable leaked into output");
        assert!(
            !md.contains("NEXT_DATA"),
            "React hydration data leaked into output"
        );
        assert!(!md.contains("console.log"), "JS code leaked into output");
        assert!(
            !md.contains(r#""key""#),
            "JSON script content leaked into output"
        );
    }

    #[test]
    fn strips_style_content() {
        let html = r##"
        <div>
            <style>
                .article { font-size: 16px; color: #333; }
                body { margin: 0; }
            </style>
            <p>Styled paragraph content.</p>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(md.contains("Styled paragraph content"));
        assert!(!md.contains("font-size"), "CSS leaked into output");
        assert!(!md.contains("margin"), "CSS leaked into output");
    }

    #[test]
    fn strips_footer_content() {
        let html = r##"
        <div>
            <p>Article body text with important information.</p>
            <footer>
                <p>Copyright 2025 Example Corp. All rights reserved.</p>
                <nav>
                    <a href="/privacy">Privacy Policy</a>
                    <a href="/terms">Terms of Service</a>
                </nav>
            </footer>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(md.contains("Article body text"));
        assert!(!md.contains("Copyright"), "footer text leaked into output");
        assert!(
            !md.contains("Privacy Policy"),
            "footer nav leaked into output"
        );
    }

    #[test]
    fn strips_by_role_attribute() {
        let html = r##"
        <div>
            <div role="navigation"><a href="/">Home</a><a href="/docs">Docs</a></div>
            <div role="banner"><h1>Site Banner</h1></div>
            <div role="main">
                <p>The main content lives here.</p>
            </div>
            <div role="complementary"><p>Sidebar widget</p></div>
            <div role="contentinfo"><p>Footer info</p></div>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(md.contains("main content lives here"));
        assert!(!md.contains("Site Banner"), "banner role leaked");
        assert!(!md.contains("Sidebar widget"), "complementary role leaked");
        assert!(!md.contains("Footer info"), "contentinfo role leaked");
        assert!(!md.contains("Docs"), "navigation role leaked");
    }

    #[test]
    fn strips_by_class_patterns() {
        // Uses exact class token matching.
        // "cookie" matches class="cookie", not class="cookie-banner".
        let html = r##"
        <div>
            <div class="cookie"><p>We use cookies</p></div>
            <div class="social"><a href="#">Share on Twitter</a></div>
            <div class="sidebar"><p>Sidebar content here</p></div>
            <div class="modal"><p>Subscribe to newsletter</p></div>
            <p>This is the real article content.</p>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(md.contains("real article content"));
        assert!(!md.contains("cookies"), "cookie class leaked");
        assert!(!md.contains("Twitter"), "social class leaked");
        assert!(!md.contains("Sidebar content"), "sidebar class leaked");
        assert!(!md.contains("Subscribe"), "modal class leaked");
    }

    #[test]
    fn compound_classes_not_noise() {
        // Compound class names should NOT trigger noise filter.
        // "free-modal-container" is Vice.com's content wrapper, not a modal.
        let html = r##"
        <div>
            <div class="free-modal-container"><p>Vice article content here</p></div>
            <div class="social-share"><a href="#">Share link</a></div>
            <div class="cookie-banner"><p>Cookie notice</p></div>
            <p>Main content.</p>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(
            md.contains("Vice article content"),
            "compound modal class should not be noise"
        );
        assert!(
            md.contains("Share link"),
            "social-share should not be noise"
        );
        assert!(
            md.contains("Cookie notice"),
            "cookie-banner should not be noise"
        );
    }

    #[test]
    fn strips_by_id_patterns() {
        // Exact ID matching — "sidebar" matches, "sidebar-left" does NOT.
        let html = r##"
        <div>
            <div id="sidebar"><p>Sidebar content</p></div>
            <div id="nav"><a href="/">Home</a></div>
            <div id="cookie"><p>Accept cookies?</p></div>
            <p>Article text that matters.</p>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(md.contains("Article text that matters"));
        assert!(!md.contains("Sidebar content"), "sidebar id leaked");
        assert!(!md.contains("Accept cookies"), "cookie id leaked");
    }

    #[test]
    fn preserves_content_with_no_noise() {
        let html = r##"
        <div>
            <h1>Clean Article</h1>
            <p>First paragraph with <strong>bold</strong> and <em>italic</em>.</p>
            <p>Second paragraph with a <a href="https://example.com">link</a>.</p>
            <pre><code class="language-python">print("hello")</code></pre>
            <blockquote><p>A great quote.</p></blockquote>
        </div>"##;
        let (md, _, assets) = convert_html(html, None);

        assert!(md.contains("# Clean Article"));
        assert!(md.contains("**bold**"));
        assert!(md.contains("*italic*"));
        assert!(md.contains("[link](https://example.com)"));
        assert!(md.contains("```python"));
        assert!(md.contains("> A great quote."));
        assert_eq!(assets.links.len(), 1);
        assert_eq!(assets.code_blocks.len(), 1);
    }

    #[test]
    fn ad_class_does_not_false_positive() {
        // "ad" as substring in "read", "loading", "load" should NOT be stripped
        let html = r##"
        <div>
            <div class="reading-time"><p>5 min read</p></div>
            <div class="loading-indicator"><p>Loading content</p></div>
            <p>Main text.</p>
        </div>"##;
        let (md, _, _) = convert_html(html, None);

        assert!(
            md.contains("5 min read"),
            "reading-time was incorrectly stripped"
        );
        assert!(
            md.contains("Loading content"),
            "loading-indicator was incorrectly stripped"
        );
        assert!(md.contains("Main text"));
    }

    // --- Adjacent inline element spacing tests ---

    #[test]
    fn adjacent_buttons_get_separated() {
        let html =
            r#"<div><button>search</button><button>extract</button><button>crawl</button></div>"#;
        let (md, _, _) = convert_html(html, None);
        assert!(
            !md.contains("searchextract"),
            "adjacent buttons mashed: {md}"
        );
        assert!(
            !md.contains("extractcrawl"),
            "adjacent buttons mashed: {md}"
        );
    }

    #[test]
    fn adjacent_links_get_separated() {
        let html = r#"<div><a href="/a">Talk to an expert</a><a href="/b">Try it out</a></div>"#;
        let (md, _, _) = convert_html(html, None);
        assert!(
            !md.contains("expert)["),
            "adjacent links should have space: {md}"
        );
    }

    #[test]
    fn adjacent_spans_get_separated() {
        let html = r#"<div><span>Hello</span><span>World</span></div>"#;
        let (md, _, _) = convert_html(html, None);
        assert!(!md.contains("HelloWorld"), "adjacent spans mashed: {md}");
    }

    #[test]
    fn inline_text_with_adjacent_elements() {
        // Inside a <p>, adjacent inline elements should also be separated
        let html = r#"<p><a href="/a">One</a><a href="/b">Two</a><a href="/c">Three</a></p>"#;
        let (md, _, _) = convert_html(html, None);
        assert!(
            !md.contains(")("),
            "adjacent links in paragraph mashed: {md}"
        );
    }

    #[test]
    fn no_extra_space_when_whitespace_exists() {
        // When HTML already has whitespace, don't double-space
        let html = r#"<p><a href="/a">One</a> <a href="/b">Two</a></p>"#;
        let (md, _, _) = convert_html(html, None);
        assert!(!md.contains("  "), "double space introduced: {md}");
    }

    // --- Code block indentation tests ---
    // Syntax highlighters (Prism.js, Shiki, highlight.js) wrap tokens in <span>
    // elements. Leading whitespace (indentation) appears as text nodes between
    // these spans. collect_preformatted_text must preserve all whitespace verbatim,
    // and collapse_whitespace must not strip leading spaces inside fenced code blocks.

    #[test]
    fn syntax_highlighted_code_preserves_indentation() {
        // Mimics React docs Prism.js output where each token is a <span>
        // and indentation is a text node between closing/opening spans.
        let html = r#"<pre><code class="language-js"><span class="token keyword">function</span> <span class="token function">MyComponent</span><span class="token punctuation">(</span><span class="token punctuation">)</span> <span class="token punctuation">{</span>
  <span class="token keyword">const</span> <span class="token punctuation">[</span>age<span class="token punctuation">,</span> setAge<span class="token punctuation">]</span> <span class="token operator">=</span> <span class="token function">useState</span><span class="token punctuation">(</span><span class="token number">28</span><span class="token punctuation">)</span><span class="token punctuation">;</span>
<span class="token punctuation">}</span></code></pre>"#;

        let (md, _, assets) = convert_html(html, None);

        assert!(md.contains("```js"), "missing language fence: {md}");
        assert!(
            md.contains("function MyComponent() {"),
            "first line wrong: {md}"
        );
        assert!(
            md.contains("  const [age, setAge] = useState(28);"),
            "indentation not preserved in syntax-highlighted code: {md}"
        );
        assert!(md.contains("\n}"), "closing brace missing: {md}");
        assert_eq!(assets.code_blocks.len(), 1);
        assert_eq!(assets.code_blocks[0].language.as_deref(), Some("js"));
    }

    #[test]
    fn shiki_line_spans_preserve_indentation() {
        // Shiki wraps each line in <span class="line">, indentation is a text
        // node inside the line span.
        let html = concat!(
            r#"<pre><code class="language-js">"#,
            r#"<span class="line"><span class="token keyword">function</span> foo() {</span>"#,
            "\n",
            r#"<span class="line">  <span class="token keyword">return</span> 1;</span>"#,
            "\n",
            r#"<span class="line">}</span>"#,
            r#"</code></pre>"#,
        );
        let (md, _, _) = convert_html(html, None);
        assert!(
            md.contains("  return 1;"),
            "Shiki-style indentation lost: {md}"
        );
    }

    #[test]
    fn deep_indentation_preserved_in_code() {
        // Multiple nesting levels -- 4-space indentation
        let html = concat!(
            "<pre><code class=\"language-py\">",
            "def outer():\n",
            "    def inner():\n",
            "        return 42\n",
            "    return inner",
            "</code></pre>"
        );
        let (md, _, _) = convert_html(html, None);
        assert!(md.contains("    def inner():"), "4-space indent lost: {md}");
        assert!(
            md.contains("        return 42"),
            "8-space indent lost: {md}"
        );
    }

    #[test]
    fn tab_indentation_preserved_in_code() {
        let html = "<pre><code>if (x) {\n\treturn;\n}</code></pre>";
        let (md, _, _) = convert_html(html, None);
        assert!(md.contains("\treturn;"), "tab indentation lost: {md}");
    }

    #[test]
    fn collapse_whitespace_skips_code_fences() {
        // Directly test that collapse_whitespace bypasses code block content
        let input = "text\n\n```js\nfunction foo() {\n  const x = 1;\n    if (true) {\n      return;\n    }\n}\n```\n\nmore text";
        let output = collapse_whitespace(input);
        assert!(
            output.contains("  const x = 1;"),
            "collapse_whitespace stripped 2-space indent: {output}"
        );
        assert!(
            output.contains("    if (true) {"),
            "collapse_whitespace stripped 4-space indent: {output}"
        );
        assert!(
            output.contains("      return;"),
            "collapse_whitespace stripped 6-space indent: {output}"
        );
    }
}
