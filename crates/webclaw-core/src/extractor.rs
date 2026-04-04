/// Readability-style content extraction.
/// Strips noise (nav, ads, sidebars), scores remaining nodes by text density
/// and structural signals, then converts the best candidate to markdown.
use std::collections::HashSet;

use ego_tree::NodeId;
use once_cell::sync::Lazy;
use scraper::{ElementRef, Html, Selector};
use tracing::{debug, warn};
use url::Url;

use crate::markdown;
use crate::noise;
use crate::types::{Content, ExtractionOptions, Link};

static CANDIDATE_SELECTOR: Lazy<Selector> =
    Lazy::new(|| Selector::parse("article, main, [role='main'], div, section, td").unwrap());
static BODY_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("body").unwrap());
static H1_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("h1").unwrap());
static H2_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("h2").unwrap());
static P_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("p").unwrap());
static A_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("a").unwrap());
static ANNOUNCEMENT_SELECTOR: Lazy<Selector> =
    Lazy::new(|| Selector::parse("[role='region'][aria-label]").unwrap());
static FOOTER_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("footer").unwrap());
static FOOTER_HEADING_SELECTOR: Lazy<Selector> =
    Lazy::new(|| Selector::parse("h2, h3, h4, h5, h6").unwrap());

/// Selector for only_main_content: article, main, [role="main"]
static MAIN_CONTENT_SELECTOR: Lazy<Selector> =
    Lazy::new(|| Selector::parse("article, main, [role='main']").unwrap());

const MAX_SELECTORS: usize = 100;

/// Build a HashSet of NodeIds to exclude based on CSS selector strings.
/// Invalid selectors are skipped with a warning.
fn build_exclude_set(doc: &Html, selectors: &[String]) -> HashSet<NodeId> {
    if selectors.len() > MAX_SELECTORS {
        warn!(
            "too many CSS selectors ({}, max {}), truncating",
            selectors.len(),
            MAX_SELECTORS
        );
    }

    let mut exclude = HashSet::new();
    for selector_str in selectors.iter().take(MAX_SELECTORS) {
        let Ok(selector) = Selector::parse(selector_str) else {
            warn!(
                selector = selector_str.as_str(),
                "invalid CSS selector, skipping"
            );
            continue;
        };
        for el in doc.select(&selector) {
            // Add the element itself and all descendants
            exclude.insert(el.id());
            for descendant in el.descendants() {
                if let Some(child_el) = ElementRef::wrap(descendant) {
                    exclude.insert(child_el.id());
                }
            }
        }
    }
    exclude
}

/// Parse CSS selector strings into Selectors, skipping invalid ones.
fn parse_selectors(strings: &[String]) -> Vec<Selector> {
    strings
        .iter()
        .filter_map(|s| {
            Selector::parse(s)
                .map_err(|_| warn!(selector = s.as_str(), "invalid CSS selector, skipping"))
                .ok()
        })
        .collect()
}

/// Extract the main content from a parsed HTML document with options.
pub fn extract_content(doc: &Html, base_url: Option<&Url>, options: &ExtractionOptions) -> Content {
    let exclude = build_exclude_set(doc, &options.exclude_selectors);

    // Path 1: Include selectors — skip scoring, extract only matching elements
    if !options.include_selectors.is_empty() {
        return extract_with_include(doc, base_url, &options.include_selectors, &exclude, options);
    }

    // Path 2: only_main_content — pick first article/main/[role="main"]
    if options.only_main_content {
        if let Some(main_el) = doc.select(&MAIN_CONTENT_SELECTOR).next() {
            debug!(
                tag = main_el.value().name(),
                "only_main_content: selected element"
            );
            let (markdown, plain_text, assets) = markdown::convert(main_el, base_url, &exclude);

            let raw_html = if options.include_raw_html {
                Some(main_el.html())
            } else {
                None
            };

            return Content {
                markdown,
                plain_text,
                links: assets.links,
                images: assets.images,
                code_blocks: assets.code_blocks,
                raw_html,
            };
        }
        debug!("only_main_content: no article/main found, falling back to scoring");
    }

    // Path 3: Default scoring algorithm
    let best = find_best_node(doc);

    let (content_element, mut markdown, plain_text, mut assets) = if let Some(node) = best {
        debug!(tag = node.value().name(), "selected content node");
        let (md, pt, a) = markdown::convert(node, base_url, &exclude);
        (Some(node), md, pt, a)
    } else {
        debug!("no strong candidate, falling back to body");
        if let Some(body) = doc.select(&BODY_SELECTOR).next() {
            let (md, pt, a) = markdown::convert(body, base_url, &exclude);
            (Some(body), md, pt, a)
        } else {
            let root = doc.root_element();
            let (md, pt, a) = markdown::convert(root, base_url, &exclude);
            (Some(root), md, pt, a)
        }
    };

    // The best content node often excludes the page's primary H1 (e.g., in a
    // hero/banner section). If the document has an H1 and its text isn't already
    // in the markdown, prepend it so the output always starts with the title.
    if let Some(h1) = doc.select(&H1_SELECTOR).next() {
        let h1_text = h1
            .text()
            .collect::<String>()
            .trim()
            .trim_end_matches(|c: char| !c.is_alphanumeric())
            .trim()
            .to_string();
        if !h1_text.is_empty() && !markdown.contains(&h1_text) {
            markdown = format!("# {h1_text}\n\n{markdown}");
            // Recover hero paragraph: H1 was outside the content node (noise-stripped),
            // so adjacent tagline/mission paragraphs are also lost. Recover them.
            recover_hero_paragraph(h1, &mut markdown);
        }
    }

    // Recover announcement banners (role="region" with announcement-like aria-label).
    // These are often stripped by class-based noise filters ("banner" class) but
    // contain genuinely important content like product announcements.
    recover_announcements(doc, base_url, &mut markdown, &mut assets.links);

    // Recover section headings that were stripped because their wrapper had a
    // noise class (e.g., <div class="section-header">). If an <h2> is missing
    // from the markdown but nearby content from the same section IS present,
    // the heading was likely a false-positive noise strip.
    recover_section_headings(doc, &mut markdown);

    // Recover prominent CTA links from the footer (e.g., documentation links).
    // The footer tag is noise, but "call to action" sections inside it often
    // contain high-value links and headings worth capturing.
    recover_footer_cta(doc, base_url, &mut markdown, &mut assets.links);

    // Recover structured site navigation from footer (product/service listings).
    // Many homepages have organized footer sitemaps (Products, Solutions, etc.)
    // that are genuinely useful for LLM consumption.
    recover_footer_sitemap(doc, base_url, &mut markdown, &mut assets.links);

    let raw_html = if options.include_raw_html {
        content_element.map(|el| el.html())
    } else {
        None
    };

    Content {
        markdown,
        plain_text,
        links: assets.links,
        images: assets.images,
        code_blocks: assets.code_blocks,
        raw_html,
    }
}

/// Extract content using include selectors. Each matching element is converted
/// to markdown and the results are concatenated.
fn extract_with_include(
    doc: &Html,
    base_url: Option<&Url>,
    include_selectors: &[String],
    exclude: &HashSet<NodeId>,
    options: &ExtractionOptions,
) -> Content {
    let selectors = parse_selectors(include_selectors);

    let mut all_md = String::new();
    let mut all_plain = String::new();
    let mut all_links = Vec::new();
    let mut all_images = Vec::new();
    let mut all_code_blocks = Vec::new();
    let mut all_raw_html = if options.include_raw_html {
        Some(String::new())
    } else {
        None
    };

    for selector in &selectors {
        for el in doc.select(selector) {
            if exclude.contains(&el.id()) {
                continue;
            }

            let (md, plain, assets) = markdown::convert(el, base_url, exclude);

            if !md.is_empty() {
                if !all_md.is_empty() {
                    all_md.push_str("\n\n");
                }
                all_md.push_str(&md);
            }
            if !plain.is_empty() {
                if !all_plain.is_empty() {
                    all_plain.push('\n');
                }
                all_plain.push_str(&plain);
            }

            all_links.extend(assets.links);
            all_images.extend(assets.images);
            all_code_blocks.extend(assets.code_blocks);

            if let Some(ref mut raw) = all_raw_html {
                raw.push_str(&el.html());
            }
        }
    }

    Content {
        markdown: all_md,
        plain_text: all_plain,
        links: all_links,
        images: all_images,
        code_blocks: all_code_blocks,
        raw_html: all_raw_html,
    }
}

/// Recover announcement banners that were stripped as noise.
/// Pattern: `<div role="region" aria-label="Announcement">` with short, meaningful text.
fn recover_announcements(
    doc: &Html,
    base_url: Option<&Url>,
    markdown: &mut String,
    links: &mut Vec<Link>,
) {
    for el in doc.select(&ANNOUNCEMENT_SELECTOR) {
        let label = el.value().attr("aria-label").unwrap_or("");
        if !label.to_lowercase().contains("announcement") {
            continue;
        }

        let text = el.text().collect::<String>();
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() || markdown.contains(&text) {
            continue;
        }

        // Build markdown for the announcement, including any links
        let mut announcement = format!("> **{text}**");
        for a in el.select(&A_SELECTOR) {
            let link_text = a.text().collect::<String>().trim().to_string();
            let href = a
                .value()
                .attr("href")
                .map(|h| markdown::resolve_url(h, base_url))
                .unwrap_or_default();
            if !link_text.is_empty() && !href.is_empty() {
                links.push(Link {
                    text: link_text,
                    href,
                });
            }
        }
        announcement.push_str("\n\n");

        debug!("recovered announcement banner");
        *markdown = format!("{announcement}{markdown}");
    }
}

/// Recover the hero paragraph (mission/tagline) that's near the H1 but inside
/// a noise-stripped container like `<header>`. Walk siblings/cousins of the H1
/// to find a substantial `<p>` that isn't in the markdown.
fn recover_hero_paragraph(h1: ElementRef<'_>, markdown: &mut String) {
    // Walk up to find a container that holds both H1 and sibling content
    let mut node = h1.parent();
    for _ in 0..4 {
        let Some(parent) = node else { break };
        let Some(parent_el) = ElementRef::wrap(parent) else {
            node = parent.parent();
            continue;
        };

        // Search all <p> descendants of this container
        for descendant in parent_el.descendants() {
            let Some(el) = ElementRef::wrap(descendant) else {
                continue;
            };
            if el.value().name() != "p" {
                continue;
            }
            let text = el
                .text()
                .collect::<String>()
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            // Only recover substantial paragraphs (taglines, mission statements)
            if text.len() < 40 || text.len() > 300 {
                continue;
            }
            if markdown.contains(&text) {
                continue;
            }
            // Insert right after the H1 heading line
            debug!(text = text.as_str(), "recovered hero paragraph");
            let insert = format!("\n{text}\n");
            if let Some(pos) = markdown.find('\n') {
                markdown.insert_str(pos + 1, &insert);
            } else {
                markdown.push_str(&insert);
            }
            return;
        }
        node = parent.parent();
    }
}

/// Recover <h2> headings that were stripped because their wrapper div had a
/// noise class like "header". If adjacent content from the same parent section
/// IS in the markdown, the heading should be there too.
fn recover_section_headings(doc: &Html, markdown: &mut String) {
    for h2 in doc.select(&H2_SELECTOR) {
        let h2_text = h2.text().collect::<String>().trim().to_string();
        if h2_text.is_empty() || find_content_position(markdown, &h2_text).is_some() {
            continue;
        }

        // Don't recover headings inside structural noise tags (nav, aside, footer,
        // header). These are genuine noise — not false-positive class matches like
        // <div class="section-header"> inside a content section.
        if is_inside_structural_noise(h2) {
            continue;
        }

        // Walk up to the nearest section/div parent, then check if any sibling
        // content from that parent made it into the markdown.
        let anchor = find_sibling_anchor_text(h2, markdown);
        if let Some(anchor) = anchor {
            debug!(
                heading = h2_text.as_str(),
                "recovered stripped section heading"
            );
            // Insert the heading before the anchor's content block.
            // Walk backwards past short orphan lines (stat numbers etc.)
            // that likely belong to the same section.
            if let Some(pos) = find_content_position(markdown, &anchor) {
                let line_start = markdown[..pos].rfind('\n').map_or(0, |p| p + 1);
                let insert_pos = walk_back_past_orphans(markdown, line_start);
                let heading_md = format!("## {h2_text}\n\n");
                markdown.insert_str(insert_pos, &heading_md);
            }
        }
    }

    // Also recover <p> "eyebrow" text (short taglines above section headings).
    // These are typically inside the same noise-stripped wrapper as the <h2>.
    // Eyebrows are short (e.g., "/the web access layer for agents") — skip full paragraphs.
    for h2 in doc.select(&H2_SELECTOR) {
        let h2_text = h2.text().collect::<String>().trim().to_string();
        if h2_text.is_empty() || find_content_position(markdown, &h2_text).is_none() {
            continue;
        }

        // Look for a preceding <p> sibling inside the same parent
        if let Some(parent) = h2.parent().and_then(ElementRef::wrap) {
            for child in parent.children() {
                if let Some(child_el) = ElementRef::wrap(child) {
                    // Stop when we reach the h2 itself
                    if child_el == h2 {
                        break;
                    }
                    if child_el.value().name() == "p" {
                        let p_text = child_el.text().collect::<String>().trim().to_string();
                        // Only short text qualifies as an eyebrow — full paragraphs
                        // are regular content, not taglines.
                        if p_text.is_empty() || p_text.len() > 80 {
                            continue;
                        }
                        // Skip decorative route-style labels (e.g., "/proof is in
                        // the numbers", "/press room") — common design pattern, not content.
                        if p_text.starts_with('/') {
                            continue;
                        }
                        // Check against a stripped version of the markdown to handle
                        // formatting like **bold** that breaks plain-text matching.
                        let plain_md = strip_md_formatting(markdown);
                        if plain_md.contains(&p_text) {
                            continue;
                        }
                        {
                            // Insert the eyebrow text at the start of the heading's line
                            if let Some(pos) = find_content_position(markdown, &h2_text) {
                                let line_start = markdown[..pos].rfind('\n').map_or(0, |p| p + 1);
                                let eyebrow_md = format!("*{p_text}*\n\n");
                                markdown.insert_str(line_start, &eyebrow_md);
                                debug!(eyebrow = p_text.as_str(), "recovered eyebrow text");
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Find text from a sibling element (in the same section) that IS in the markdown.
/// This confirms the heading belongs to content we already captured.
fn find_sibling_anchor_text(heading: ElementRef<'_>, markdown: &str) -> Option<String> {
    let heading_text = heading.text().collect::<String>();

    // Walk up to find the containing section or significant parent
    let mut node = heading.parent();
    while let Some(parent) = node {
        if let Some(parent_el) = ElementRef::wrap(parent) {
            let tag = parent_el.value().name();
            if tag == "section" || tag == "article" || tag == "main" || tag == "body" {
                // Search descendant <p> and <h3> elements for text in the markdown.
                // Using specific elements avoids the multiline blob issue from
                // concatenating all text nodes of a large container.
                for descendant in parent_el.descendants() {
                    if let Some(el) = ElementRef::wrap(descendant) {
                        let dtag = el.value().name();
                        if dtag != "p" && dtag != "h3" && dtag != "h4" {
                            continue;
                        }
                        // Normalize whitespace to match how the markdown converter collapses it
                        let el_text: String = el
                            .text()
                            .collect::<String>()
                            .split_whitespace()
                            .collect::<Vec<_>>()
                            .join(" ");
                        // Skip if this text is part of the heading itself
                        if el_text.is_empty() || heading_text.contains(&el_text) {
                            continue;
                        }
                        if el_text.len() > 15 && find_content_position(markdown, &el_text).is_some()
                        {
                            return Some(el_text);
                        }
                    }
                }
                break;
            }
        }
        node = parent.parent();
    }
    None
}

/// Recover CTA (call-to-action) links and headings from footer sections.
/// Many sites have a "hero" CTA block in the footer with documentation links
/// or signup prompts. These are valuable content, not navigational noise.
fn recover_footer_cta(
    doc: &Html,
    base_url: Option<&Url>,
    markdown: &mut String,
    links: &mut Vec<Link>,
) {
    for footer in doc.select(&FOOTER_SELECTOR) {
        // Look for h2 headings in the footer (CTA headings like "Power your AI...")
        for h2 in footer.select(&H2_SELECTOR) {
            let h2_text = h2.text().collect::<String>().trim().to_string();
            if h2_text.is_empty() || markdown.contains(&h2_text) {
                continue;
            }
            // Skip meta headings (screen-reader-only "Footer", "Navigation")
            let h2_lower = h2_text.to_lowercase();
            if h2_lower == "footer" || h2_lower == "navigation" || h2_lower == "site map" {
                continue;
            }
            // Skip screen-reader-only headings (sr-only, visually-hidden)
            if let Some(class) = h2.value().attr("class") {
                let cl = class.to_lowercase();
                if cl.contains("sr-only")
                    || cl.contains("visually-hidden")
                    || cl.contains("screen-reader")
                {
                    continue;
                }
            }

            debug!(heading = h2_text.as_str(), "recovered footer CTA heading");
            markdown.push_str(&format!("\n\n## {h2_text}\n\n"));
        }

        // Recover links that point to documentation or app URLs
        for a in footer.select(&A_SELECTOR) {
            let href = match a.value().attr("href") {
                Some(h) => markdown::resolve_url(h, base_url),
                None => continue,
            };
            let text = a.text().collect::<String>().trim().to_string();
            if text.is_empty() || href.is_empty() {
                continue;
            }

            // Only recover links to docs/app/API — not generic footer nav
            let href_lower = href.to_lowercase();
            let is_valuable_cta = href_lower.contains("docs.")
                || href_lower.contains("/docs")
                || href_lower.contains("app.")
                || href_lower.contains("/app")
                || href_lower.contains("api.");

            if is_valuable_cta && !markdown.contains(&text) {
                debug!(
                    text = text.as_str(),
                    href = href.as_str(),
                    "recovered footer CTA link"
                );
                markdown.push_str(&format!("[{text}]({href})\n\n"));
                links.push(Link {
                    text: text.clone(),
                    href: href.clone(),
                });
            }
        }
    }
}

/// Recover structured site navigation from footer when it has organized
/// link categories (Products, Solutions, Resources, etc.). This captures
/// the site's offering structure — useful for LLM queries like "what does
/// this company offer?" Only fires when the footer has 3+ categories.
fn recover_footer_sitemap(
    doc: &Html,
    base_url: Option<&Url>,
    markdown: &mut String,
    links: &mut Vec<Link>,
) {
    for footer in doc.select(&FOOTER_SELECTOR) {
        let mut categories: Vec<(String, Vec<(String, String)>)> = Vec::new();

        for heading in footer.select(&FOOTER_HEADING_SELECTOR) {
            let heading_text = heading.text().collect::<String>().trim().to_string();
            if heading_text.is_empty() || heading_text.len() > 50 {
                continue;
            }
            // Skip meta headings like "Footer" and headings already in the markdown
            if heading_text.eq_ignore_ascii_case("footer") || markdown.contains(&heading_text) {
                continue;
            }

            // Find links in the nearest container that holds both heading + link list.
            // Try parent first, then grandparent (handles wrapper divs).
            let cat_links = collect_sibling_links(heading, base_url);
            // 2–20 links: too few = not a real category, too many = aggregate container
            if cat_links.len() >= 2 && cat_links.len() <= 20 {
                categories.push((heading_text, cat_links));
            }
        }

        if categories.len() < 3 {
            continue;
        }

        // Build compact sitemap — category name + comma-separated link text
        let mut sitemap = String::from("\n\n---\n\n");
        for (heading, cat_links) in &categories {
            let names: Vec<&str> = cat_links.iter().map(|(t, _)| t.as_str()).collect();
            sitemap.push_str(&format!("**{heading}**: {}\n", names.join(", ")));

            for (text, href) in cat_links {
                links.push(Link {
                    text: text.clone(),
                    href: href.clone(),
                });
            }
        }

        debug!(categories = categories.len(), "recovered footer sitemap");
        markdown.push_str(&sitemap);
    }
}

/// Collect links from the same container as a heading element.
/// Walks up the DOM to find the nearest ancestor that contains <a> elements.
fn collect_sibling_links(heading: ElementRef<'_>, base_url: Option<&Url>) -> Vec<(String, String)> {
    let mut node = heading.parent();
    // Try up to 2 levels (parent, grandparent) to find a link container
    for _ in 0..2 {
        let Some(parent) = node else { break };
        let Some(parent_el) = ElementRef::wrap(parent) else {
            node = parent.parent();
            continue;
        };
        let a_elements: Vec<_> = parent_el.select(&A_SELECTOR).collect();
        if a_elements.len() >= 2 {
            return a_elements
                .into_iter()
                .filter_map(|a| {
                    let text = a.text().collect::<String>().trim().to_string();
                    let href = a
                        .value()
                        .attr("href")
                        .map(|h| markdown::resolve_url(h, base_url));
                    match (text.is_empty(), href) {
                        (false, Some(h))
                            if !h.is_empty()
                                && text.len() > 1
                                && text.len() < 60
                                && !matches!(
                                    text.to_lowercase().as_str(),
                                    "here" | "link" | "click" | "more"
                                ) =>
                        {
                            Some((text, h))
                        }
                        _ => None,
                    }
                })
                .collect();
        }
        node = parent.parent();
    }
    Vec::new()
}

/// Walk backwards from `pos` in markdown, skipping blank lines and short
/// orphan lines (<=25 chars, likely stat numbers or labels) that belong to
/// the same section. Stops at headings, long content lines, or start of string.
fn walk_back_past_orphans(markdown: &str, mut pos: usize) -> usize {
    loop {
        if pos == 0 {
            break;
        }
        // Find the previous line
        let prev_end = pos.saturating_sub(1); // skip the \n
        let prev_start = markdown[..prev_end].rfind('\n').map_or(0, |p| p + 1);
        let prev_line = markdown[prev_start..prev_end].trim();

        if prev_line.is_empty() {
            pos = prev_start;
            continue;
        }
        if prev_line.starts_with('#') || prev_line.starts_with('>') || prev_line.len() > 25 {
            break;
        }
        // Short non-structural line — likely a stat number, include it
        pos = prev_start;
    }
    pos
}

/// Quick strip of markdown bold/italic markers for plain-text comparison.
fn strip_md_formatting(md: &str) -> String {
    md.replace("**", "").replace('*', "")
}

/// Find `needle` in `markdown` only at a position that isn't inside image/link
/// alt text (`![...](...)`). Returns the byte offset or None.
fn find_content_position(markdown: &str, needle: &str) -> Option<usize> {
    let mut search_from = 0;
    while let Some(pos) = markdown[search_from..].find(needle) {
        let abs_pos = search_from + pos;
        if !is_inside_image_syntax(markdown, abs_pos) {
            return Some(abs_pos);
        }
        search_from = abs_pos + 1;
    }
    None
}

/// Check if a position in markdown falls inside `![...](...)` image syntax.
fn is_inside_image_syntax(markdown: &str, pos: usize) -> bool {
    // Walk backwards from pos to find the nearest unmatched `![`
    let before = &markdown[..pos];
    // Find the last `![` that hasn't been closed by `](`
    let mut i = before.len();
    while i > 0 {
        i -= 1;
        if i > 0 && before.as_bytes()[i - 1] == b'!' && before.as_bytes()[i] == b'[' {
            // Found `![` — check if there's a matching `](` after pos
            let after = &markdown[pos..];
            if after.contains("](") {
                return true;
            }
        }
        // If we hit a `)` that closes a previous image, stop searching
        if before.as_bytes()[i] == b')' {
            break;
        }
    }
    false
}

/// Check if an element is inside a structural noise tag (nav, aside, footer, header).
/// Unlike class-based noise (e.g., <div class="header">), these are strong signals
/// that the content is genuinely non-content and should NOT be recovered.
const STRUCTURAL_NOISE_TAGS: &[&str] = &["nav", "aside", "footer", "header"];

fn is_inside_structural_noise(el: ElementRef<'_>) -> bool {
    let mut node = el.parent();
    while let Some(parent) = node {
        if let Some(parent_el) = ElementRef::wrap(parent) {
            let tag = parent_el.value().name();
            if STRUCTURAL_NOISE_TAGS.contains(&tag) {
                return true;
            }
            // Also check role-based structural noise
            if let Some(role) = parent_el.value().attr("role")
                && (role == "navigation" || role == "contentinfo")
            {
                return true;
            }
        }
        node = parent.parent();
    }
    false
}

/// Score each candidate node and return the best one.
fn find_best_node(doc: &Html) -> Option<ElementRef<'_>> {
    let mut best: Option<(ElementRef<'_>, f64)> = None;

    for candidate in doc.select(&CANDIDATE_SELECTOR) {
        if noise::is_noise(candidate) || noise::is_noise_descendant(candidate) {
            continue;
        }

        let score = score_node(candidate);

        if score > 0.0 && best.as_ref().is_none_or(|(_, s)| score > *s) {
            best = Some((candidate, score));
        }
    }

    best.map(|(el, score)| {
        debug!(score, tag = el.value().name(), "best content candidate");
        el
    })
}

fn score_node(el: ElementRef<'_>) -> f64 {
    let text = el.text().collect::<String>();
    let text_len = text.len() as f64;

    // Very short nodes aren't content
    if text_len < 50.0 {
        return 0.0;
    }

    let mut score = 0.0;

    // Base score: text length (log scale to avoid huge nodes dominating purely by size)
    score += text_len.ln();

    // Bonus for <article> or <main> — these are strong semantic signals
    let tag = el.value().name();
    match tag {
        "article" => score += 50.0,
        "main" => score += 50.0,
        _ => {}
    }

    // Bonus for role="main"
    if el.value().attr("role") == Some("main") {
        score += 50.0;
    }

    // Bonus for common content class/id patterns
    if let Some(class) = el.value().attr("class") {
        let cl = class.to_lowercase();
        if cl.contains("content")
            || cl.contains("article")
            || cl.contains("post")
            || cl.contains("entry")
        {
            score += 25.0;
        }
    }
    if let Some(id) = el.value().attr("id") {
        let id = id.to_lowercase();
        if id.contains("content")
            || id.contains("article")
            || id.contains("post")
            || id.contains("main")
        {
            score += 25.0;
        }
    }

    // Paragraph density: count <p> children — real content has paragraphs
    let p_count = el.select(&P_SELECTOR).count() as f64;
    score += p_count * 3.0;

    // Link density penalty: nodes that are mostly links (nav, footer) score low.
    // link_text_len / total_text_len — lower is better for content.
    let link_text_len: f64 = el
        .select(&A_SELECTOR)
        .map(|a| a.text().collect::<String>().len() as f64)
        .sum();

    // Semantic nodes (article, main, role=main) get milder link density penalties.
    // Documentation pages often have high link density from TOCs inside the main
    // content container — these are expected, not spam.
    let is_semantic = matches!(tag, "article" | "main") || el.value().attr("role") == Some("main");

    if text_len > 0.0 {
        let link_density = link_text_len / text_len;
        if is_semantic {
            // Semantic nodes: only penalize extreme link density
            if link_density > 0.7 {
                score *= 0.3;
            } else if link_density > 0.5 {
                score *= 0.5;
            }
        } else {
            // Generic divs: heavy penalty for link-dense content
            if link_density > 0.5 {
                score *= 0.1;
            } else if link_density > 0.3 {
                score *= 0.5;
            }
        }
    }

    score
}

/// Count words in text (for word_count metadata).
pub fn word_count(text: &str) -> usize {
    text.split_whitespace().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(html: &str) -> Html {
        Html::parse_document(html)
    }

    /// Helper: extract with default options (backward-compatible).
    fn extract_default(doc: &Html, base_url: Option<&Url>) -> Content {
        extract_content(doc, base_url, &ExtractionOptions::default())
    }

    #[test]
    fn picks_article_over_nav() {
        let html = r##"
        <html>
        <body>
            <nav><ul><li><a href="/">Home</a></li><li><a href="/about">About</a></li></ul></nav>
            <article>
                <h1>Real Article</h1>
                <p>This is the main content of the page. It contains several paragraphs
                of text that make it clearly the main content area.</p>
                <p>Another paragraph with useful information for the reader.</p>
                <p>And a third paragraph to make it really obvious this is content.</p>
            </article>
            <aside class="sidebar">
                <h3>Related Links</h3>
                <ul><li><a href="/1">Link 1</a></li></ul>
            </aside>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);
        assert!(content.markdown.contains("Real Article"));
        assert!(content.markdown.contains("main content"));
    }

    #[test]
    fn falls_back_to_body() {
        let html = r##"<html><body><p>Simple page with just a paragraph.</p></body></html>"##;
        let doc = parse(html);
        let content = extract_default(&doc, None);
        assert!(content.plain_text.contains("Simple page"));
    }

    #[test]
    fn word_count_works() {
        assert_eq!(word_count("hello world foo bar"), 4);
        assert_eq!(word_count(""), 0);
        assert_eq!(word_count("  spaces  everywhere  "), 2);
    }

    #[test]
    fn prefers_content_class() {
        let html = r##"
        <html>
        <body>
            <div class="header"><p>Site header with some branding text content here</p></div>
            <div class="content">
                <h1>Main Content</h1>
                <p>This is the primary content of the page that readers want to see.
                It has multiple sentences and meaningful paragraphs.</p>
                <p>Second paragraph with additional details and context for the article.</p>
                <p>Third paragraph because real articles have substantial text.</p>
            </div>
            <div class="footer"><p>Footer stuff with copyright and legal text here</p></div>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);
        assert!(content.markdown.contains("Main Content"));
    }

    /// Simulates a Wikipedia-like page where the best content node (article/main)
    /// contains a nav sidebar as a child. The markdown converter must strip it.
    #[test]
    fn wikipedia_like_nav_sidebar_stripped() {
        let html = r##"
        <html>
        <body>
            <article>
                <h1>Rust (programming language)</h1>
                <nav class="sidebar-toc">
                    <h2>Contents</h2>
                    <ul>
                        <li><a href="#history">History</a></li>
                        <li><a href="#syntax">Syntax</a></li>
                        <li><a href="#features">Features</a></li>
                    </ul>
                </nav>
                <aside class="infobox">
                    <p>Developer: Mozilla Research</p>
                    <p>First appeared: 2010</p>
                </aside>
                <p>Rust is a multi-paradigm programming language focused on performance
                and safety, especially safe concurrency. It accomplishes these goals
                without a garbage collector.</p>
                <p>Rust was originally designed by Graydon Hoare at Mozilla Research,
                with contributions from several other developers.</p>
                <p>The language grew out of a personal project begun in 2006 by Mozilla
                employee Graydon Hoare, who stated that it was possibly named after
                the rust family of fungi.</p>
            </article>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        // Article content preserved
        assert!(content.markdown.contains("Rust (programming language)"));
        assert!(
            content
                .markdown
                .contains("multi-paradigm programming language")
        );
        assert!(content.markdown.contains("Graydon Hoare"));

        // Nav sidebar stripped
        assert!(
            !content.markdown.contains("Contents"),
            "TOC nav heading leaked"
        );
        assert!(
            !content.markdown.contains("#history"),
            "TOC nav link leaked"
        );

        // Aside infobox stripped
        assert!(
            !content.markdown.contains("First appeared"),
            "infobox aside leaked"
        );
    }

    /// When the best node is a large div that happens to contain script tags,
    /// the JS code must not appear in the markdown.
    #[test]
    fn script_inside_content_node_stripped() {
        let html = r##"
        <html>
        <body>
            <main>
                <h1>Interactive Article</h1>
                <p>This article has some embedded JavaScript for interactivity.
                The content itself is what we want to extract, not the code.</p>
                <script>
                    window.__NEXT_DATA__ = {"props":{"pageProps":{"article":{"id":123}}}};
                    document.addEventListener('DOMContentLoaded', function() {
                        initializeApp();
                    });
                </script>
                <p>The article continues with more useful information for readers
                who want to learn about the topic.</p>
                <style>.highlight { background: yellow; }</style>
            </main>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        assert!(content.markdown.contains("Interactive Article"));
        assert!(content.markdown.contains("embedded JavaScript"));
        assert!(content.markdown.contains("continues with more"));
        assert!(
            !content.markdown.contains("NEXT_DATA"),
            "script content leaked"
        );
        assert!(
            !content.markdown.contains("initializeApp"),
            "JS function call leaked"
        );
        assert!(
            !content.markdown.contains("background: yellow"),
            "CSS leaked"
        );
    }

    /// Full-page simulation: header, nav, main content, footer.
    /// Only the main content should survive.
    #[test]
    fn full_page_noise_stripped() {
        let html = r##"
        <html>
        <body>
            <header>
                <div class="logo">MySite</div>
                <nav>
                    <a href="/">Home</a>
                    <a href="/blog">Blog</a>
                    <a href="/about">About</a>
                </nav>
            </header>
            <main>
                <article>
                    <h1>How to Write Clean Code</h1>
                    <p>Writing clean code is an essential skill for every developer.
                    It makes your codebase easier to maintain and understand.</p>
                    <p>In this article, we will explore several principles that can
                    help you write better, more readable code.</p>
                    <p>The first principle is to use meaningful variable names that
                    clearly describe what the variable holds.</p>
                </article>
            </main>
            <footer>
                <p>Copyright 2025 MySite</p>
                <a href="/privacy">Privacy</a>
                <a href="/terms">Terms</a>
            </footer>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        assert!(content.markdown.contains("How to Write Clean Code"));
        assert!(content.markdown.contains("meaningful variable names"));
        assert!(
            !content.markdown.contains("MySite"),
            "header/footer branding leaked"
        );
        assert!(!content.markdown.contains("Privacy"), "footer link leaked");
        assert!(!content.markdown.contains("Blog"), "nav link leaked");
    }

    /// H1 in a hero/banner section outside the main content node should be
    /// captured and prepended to the markdown output.
    #[test]
    fn h1_outside_content_node_captured() {
        let html = r##"
        <html>
        <body>
            <div class="hero-banner">
                <h1>The Ultimate Guide to Async Rust</h1>
                <p class="subtitle">Everything you need to know</p>
            </div>
            <article>
                <p>Asynchronous programming in Rust is powered by the async/await
                syntax and the Future trait. This guide covers all the fundamentals
                you need to get started with async Rust.</p>
                <p>We will explore tokio, the most popular async runtime, and show
                you how to build concurrent applications efficiently.</p>
                <p>By the end of this guide you will understand how to write
                performant async code that handles thousands of connections.</p>
            </article>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        // H1 must appear in markdown even though it's outside <article>
        assert!(
            content
                .markdown
                .contains("The Ultimate Guide to Async Rust"),
            "H1 from hero banner missing from output"
        );
        // Should be prepended as a heading
        assert!(
            content
                .markdown
                .starts_with("# The Ultimate Guide to Async Rust"),
            "H1 should be prepended as markdown heading"
        );
        // Article content still present
        assert!(content.markdown.contains("async/await"));
        assert!(content.markdown.contains("tokio"));
    }

    /// Announcement banners with role="region" and aria-label="Announcement"
    /// should be recovered even though their class contains "banner" (noise).
    #[test]
    fn announcement_banner_recovered() {
        let html = r##"
        <html>
        <body>
            <div class="announcement-banner" role="region" aria-label="Announcement">
                <p>Big news! We are joining forces with Acme Corp -
                read more in <a href="https://example.com/blog">our blog</a></p>
            </div>
            <header><nav><a href="/">Home</a></nav></header>
            <article>
                <h1>Our Product</h1>
                <p>We build amazing tools for developers that simplify
                complex workflows and boost productivity every day.</p>
                <p>Our platform handles millions of requests per second
                with low latency and high reliability.</p>
            </article>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, Some(&Url::parse("https://example.com").unwrap()));

        assert!(
            content.markdown.contains("joining forces with Acme Corp"),
            "Announcement banner text missing from output"
        );
        assert!(
            content.markdown.contains("Our Product"),
            "Main content missing"
        );
        // The announcement link should be captured
        assert!(
            content
                .links
                .iter()
                .any(|l| l.href.contains("example.com/blog")),
            "Announcement link not captured"
        );
    }

    /// Section headings inside <div class="...header"> wrappers should be
    /// recovered when sibling content from the same section is in the output.
    #[test]
    fn section_heading_in_header_class_recovered() {
        let html = r##"
        <html>
        <body>
            <div class="page-wrapper">
                <section class="features">
                    <div class="section-header">
                        <h2>Built for scale</h2>
                    </div>
                    <div class="feature-grid">
                        <p>Handle thousands of concurrent requests with
                        intelligent load balancing and automatic failover.</p>
                        <p>Deploy globally with edge locations in every
                        major region for minimal latency.</p>
                    </div>
                </section>
            </div>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        assert!(
            content.markdown.contains("## Built for scale"),
            "Section heading should be recovered: {}",
            content.markdown
        );
        assert!(
            content.markdown.contains("concurrent requests"),
            "Section content missing"
        );
    }

    /// Eyebrow text (short tagline above a section heading) should be
    /// recovered when it's inside the same noise-stripped wrapper as the <h2>.
    #[test]
    fn eyebrow_text_recovered() {
        let html = r##"
        <html>
        <body>
            <div class="page-wrapper">
                <section class="users-section">
                    <div class="section-header">
                        <p class="eyebrow">the platform for builders</p>
                        <h2>Loved by developers worldwide</h2>
                    </div>
                    <div class="grid">
                        <p>Thousands of teams rely on our platform daily for
                        mission-critical applications and workflows.</p>
                    </div>
                </section>
            </div>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        assert!(
            content.markdown.contains("the platform for builders"),
            "Eyebrow text missing: {}",
            content.markdown
        );
        assert!(
            content.markdown.contains("Loved by developers worldwide"),
            "Section heading missing"
        );
    }

    /// Decorative route-style labels (starting with "/") should NOT be recovered
    /// as eyebrow text — they're design elements, not content.
    #[test]
    fn route_style_eyebrow_not_recovered() {
        let html = r##"
        <html>
        <body>
            <div class="page-wrapper">
                <section>
                    <div class="section-header">
                        <p class="eyebrow">/proof is in the numbers</p>
                        <h2>Trusted in production</h2>
                    </div>
                    <div class="grid">
                        <p>Our platform handles millions of requests per second
                        with low latency and high reliability.</p>
                    </div>
                </section>
            </div>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        // With exact class matching, "section-header" is NOT noise
        // (only exact "header" class would be). The eyebrow text is now
        // preserved, which is correct — it's content, not navigation.
        assert!(
            content.markdown.contains("Trusted in production"),
            "Section heading should be recovered"
        );
        assert!(
            content.markdown.contains("Our platform"),
            "Grid content should be present"
        );
    }

    /// Footer CTA links to documentation URLs should be recovered.
    #[test]
    fn footer_cta_link_recovered() {
        let html = r##"
        <html>
        <body>
            <article>
                <h1>Our Platform</h1>
                <p>Build powerful applications with our comprehensive API
                and developer tools that handle millions of requests.</p>
                <p>Get started in minutes with our quickstart guide and
                extensive documentation for every feature.</p>
            </article>
            <footer>
                <h2>Start building today</h2>
                <a href="https://docs.example.com">Explore API Docs</a>
                <a href="https://app.example.com">Try it free</a>
                <a href="/privacy">Privacy</a>
                <a href="/terms">Terms</a>
            </footer>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, Some(&Url::parse("https://example.com").unwrap()));

        assert!(
            content.markdown.contains("Start building today"),
            "Footer CTA heading missing: {}",
            content.markdown
        );
        assert!(
            content.markdown.contains("Explore API Docs"),
            "Footer CTA link missing"
        );
        // Non-doc footer links should NOT be recovered
        assert!(
            !content.markdown.contains("Privacy"),
            "Generic footer nav leaked"
        );
        assert!(
            !content.markdown.contains("Terms"),
            "Generic footer nav leaked"
        );
    }

    /// Headings inside genuine noise (nav, aside) should NOT be recovered,
    /// even when sibling content exists in the output.
    #[test]
    fn heading_inside_nav_not_recovered() {
        let html = r##"
        <html>
        <body>
            <article>
                <h1>Programming Guide</h1>
                <nav class="table-of-contents">
                    <h2>Table of Contents</h2>
                    <ul>
                        <li><a href="#ch1">Chapter 1</a></li>
                        <li><a href="#ch2">Chapter 2</a></li>
                    </ul>
                </nav>
                <p>This comprehensive guide covers everything you need
                to know about modern programming practices.</p>
                <p>From basics to advanced topics, we will explore
                patterns and techniques used by professionals.</p>
            </article>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        assert!(
            !content.markdown.contains("Table of Contents"),
            "TOC heading from nav should not be recovered: {}",
            content.markdown
        );
        assert!(content.markdown.contains("comprehensive guide"));
    }

    /// Structured footer sitemaps (3+ categories with headings) should be
    /// recovered as a compact reference section.
    #[test]
    fn footer_sitemap_recovered() {
        let html = r##"
        <html>
        <body>
            <article>
                <h1>Our Company</h1>
                <p>We build tools that help developers create amazing applications
                faster and more efficiently than ever before.</p>
                <p>Join thousands of teams who trust our platform for their
                mission-critical workloads every single day.</p>
            </article>
            <footer>
                <div class="col">
                    <h3>Products</h3>
                    <a href="/product-a">Product A</a>
                    <a href="/product-b">Product B</a>
                    <a href="/product-c">Product C</a>
                </div>
                <div class="col">
                    <h3>Solutions</h3>
                    <a href="/enterprise">Enterprise</a>
                    <a href="/startup">Startup</a>
                    <a href="/education">Education</a>
                </div>
                <div class="col">
                    <h3>Resources</h3>
                    <a href="/blog">Blog</a>
                    <a href="/docs">Documentation</a>
                    <a href="/community">Community</a>
                </div>
            </footer>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, Some(&Url::parse("https://example.com").unwrap()));

        // Categories should be captured
        assert!(
            content.markdown.contains("Products"),
            "Footer sitemap Products missing: {}",
            content.markdown
        );
        assert!(
            content.markdown.contains("Product A"),
            "Footer sitemap link missing"
        );
        assert!(
            content.markdown.contains("Solutions"),
            "Footer sitemap Solutions missing"
        );
        assert!(
            content.markdown.contains("Resources"),
            "Footer sitemap Resources missing"
        );
        // Main content still present
        assert!(content.markdown.contains("Our Company"));
    }

    /// Footer sitemaps with fewer than 3 categories should NOT be recovered
    /// (not enough structure to be confident it's a sitemap).
    #[test]
    fn small_footer_not_treated_as_sitemap() {
        let html = r##"
        <html>
        <body>
            <article>
                <h1>Simple Page</h1>
                <p>This is a simple page with minimal footer structure that
                should not trigger sitemap recovery at all.</p>
                <p>The content here is what matters, not the footer links
                or navigation elements below the main content.</p>
            </article>
            <footer>
                <h3>Legal</h3>
                <a href="/privacy">Privacy</a>
                <a href="/terms">Terms</a>
            </footer>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, None);

        assert!(
            !content.markdown.contains("Legal"),
            "Small footer should not be treated as sitemap: {}",
            content.markdown
        );
    }

    /// Screen-reader-only footer headings (like "Footer") should not leak.
    #[test]
    fn sr_only_footer_heading_not_recovered() {
        let html = r##"
        <html>
        <body>
            <article>
                <h1>Our Platform</h1>
                <p>Build powerful applications with our comprehensive API
                and developer tools that handle millions of requests.</p>
                <p>Get started in minutes with our quickstart guide and
                extensive documentation for every feature.</p>
            </article>
            <footer>
                <h2 class="u-sr-only">Footer</h2>
                <a href="https://docs.example.com">Explore API Docs</a>
            </footer>
        </body>
        </html>"##;

        let doc = parse(html);
        let content = extract_default(&doc, Some(&Url::parse("https://example.com").unwrap()));

        assert!(
            !content.markdown.contains("## Footer"),
            "SR-only 'Footer' heading should not be recovered: {}",
            content.markdown
        );
    }
}

#[cfg(test)]
mod form_integration_tests {
    use super::*;

    #[test]
    fn aspnet_form_content_extraction() {
        let content = "x".repeat(600); // Ensure >500 chars
        let html = format!(r#"<html><body>
            <form method="post" action="./page.aspx" id="form1">
                <div class="wrapper">
                    <div class="header"><a href="/">Logo</a></div>
                    <div class="content">
                        <h2>Section</h2>
                        <h3>Question?</h3>
                        <p>{content}</p>
                    </div>
                </div>
            </form>
        </body></html>"#);
        let doc = Html::parse_document(&html);
        let opts = ExtractionOptions::default();
        let result = extract_content(&doc, None, &opts);
        assert!(result.markdown.contains("Section"), "h2 missing from markdown");
        assert!(result.markdown.contains("Question"), "h3 missing from markdown");
    }

    /// Simulate unclosed header div absorbing the content div.
    /// The header's noise class should NOT propagate to the absorbed content
    /// because the safety valve detects the header has >5000 chars (broken wrapper).
    #[test]
    fn unclosed_header_div_does_not_swallow_content() {
        let faq = "Lorem ipsum dolor sit amet. ".repeat(300); // ~8400 chars
        // The header div is intentionally NOT closed — the HTML parser makes
        // div.content a child of div.header. The safety valve (>5000 chars)
        // should prevent div.header from being treated as noise.
        let html = format!(r#"<html><body>
            <div class="wrapper">
                <div class="header"><a href="/">Logo</a>
                <div class="content">
                    <h2>FAQ Section</h2>
                    <h3>First question?</h3>
                    <p>{faq}</p>
                </div>
            </div>
        </body></html>"#);
        let doc = Html::parse_document(&html);
        let opts = ExtractionOptions::default();
        let result = extract_content(&doc, None, &opts);
        assert!(result.markdown.contains("FAQ Section"), "h2 missing: header swallowed content");
        assert!(result.markdown.contains("First question"), "h3 missing: header swallowed content");
    }
}
