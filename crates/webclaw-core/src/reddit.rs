//! Reddit thread extractor — parses old.reddit.com HTML directly.
//!
//! old.reddit.com serves fully server-rendered HTML with stable class names
//! and data attributes. No JS, no API key, no `.json` trick needed.

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

use crate::{Content, DomainData, DomainType, ExtractionResult, Metadata};

// ─── Public types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RedditPost {
    pub id: Option<String>,
    pub title: String,
    pub author: String,
    pub subreddit: Option<String>,
    pub score: i64,
    pub body: Option<String>,
    pub num_comments: usize,
    pub permalink: String,
    pub url: Option<String>,
    pub is_self: bool,
    pub flair: Option<String>,
    pub created_utc: Option<String>,
}

#[derive(Serialize)]
pub struct RedditComment {
    pub id: Option<String>,
    pub author: String,
    pub body: String,
    /// `None` when Reddit hides the score (fresh comments). Distinct from
    /// `Some(0)`, which is a real net-zero score.
    pub score: Option<i64>,
    pub depth: usize,
    pub is_op: bool,
    pub created_utc: Option<String>,
    pub replies: Vec<RedditComment>,
}

#[derive(Serialize)]
pub struct RedditThread {
    #[serde(rename = "url")]
    pub source_url: String,
    pub post: Option<RedditPost>,
    pub comments: Vec<RedditComment>,
}

// ─── Public API ────────────────────────────────────────────────────────────────

pub fn is_reddit_url(url: &str) -> bool {
    matches!(
        host_of(url),
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "np.reddit.com" | "new.reddit.com"
    )
}

/// Try to parse a Reddit thread from old.reddit.com HTML.
/// Returns `None` if the page doesn't have recognisable Reddit structure.
pub fn try_extract_thread(html: &str, url: &str) -> Option<RedditThread> {
    if !url.contains("/comments/") {
        return None;
    }
    let doc = Html::parse_document(html);
    let post = parse_post(&doc);
    let op = post.as_ref().map(|p| p.author.as_str()).unwrap_or("");
    let comments = parse_comments(&doc, op);

    if post.is_none() && comments.is_empty() {
        return None;
    }

    Some(RedditThread {
        source_url: url.to_string(),
        post,
        comments,
    })
}

/// Entry point for `webclaw-core`'s extraction fast path.
pub fn try_extract(html: &str, url: &str) -> Option<ExtractionResult> {
    let thread = try_extract_thread(html, url)?;
    Some(to_extraction_result(&thread))
}

// ─── ExtractionResult builder ──────────────────────────────────────────────────

fn to_extraction_result(thread: &RedditThread) -> ExtractionResult {
    let md = to_markdown(thread);
    let plain = plain_text(&md);
    let wc = md.split_whitespace().count();

    let (title, author, site_name) = thread
        .post
        .as_ref()
        .map(|p| {
            (
                Some(p.title.clone()),
                Some(p.author.clone()),
                p.subreddit.clone(),
            )
        })
        .unwrap_or_default();

    ExtractionResult {
        metadata: Metadata {
            title,
            description: None,
            author,
            published_date: None,
            language: Some("en".to_string()),
            url: Some(thread.source_url.clone()),
            site_name,
            image: None,
            favicon: None,
            word_count: wc,
        },
        content: Content {
            markdown: md,
            plain_text: plain,
            links: vec![],
            images: vec![],
            code_blocks: vec![],
            raw_html: None,
        },
        domain_data: Some(DomainData {
            domain_type: DomainType::Social,
        }),
        structured_data: vec![],
    }
}

// ─── Markdown rendering ────────────────────────────────────────────────────────

pub fn to_markdown(thread: &RedditThread) -> String {
    let mut out = String::new();

    if let Some(p) = &thread.post {
        out.push_str(&format!("# {}\n\n", p.title));

        let pts = pt_label(Some(p.score));
        let cmt = match p.num_comments {
            0 => String::new(),
            1 => " · 1 comment".to_string(),
            n => format!(" · {n} comments"),
        };
        let sub = p.subreddit.as_deref().unwrap_or("?");
        out.push_str(&format!("**u/{}** · r/{sub} · {pts}{cmt}\n\n", p.author));

        if let Some(ref body) = p.body
            && !body.is_empty()
        {
            out.push_str(body);
            out.push_str("\n\n");
        }
        if let Some(ref link) = p.url
            && !p.is_self
        {
            out.push_str(&format!("[Link]({link})\n\n"));
        }
        out.push_str("---\n\n");
    }

    if !thread.comments.is_empty() {
        out.push_str("## Comments\n\n");
        for c in &thread.comments {
            render_comment(c, &mut out);
        }
    }

    collapse_blank_lines(out.trim_end())
}

/// Render one comment + its replies. Nesting is expressed with blockquote
/// depth (`> ` per level) rather than leading spaces: space-indentation of
/// 4+ would turn ordinary text and ``` fences into CommonMark indented code
/// blocks, corrupting any comment at depth ≥ 2.
fn render_comment(c: &RedditComment, out: &mut String) {
    let q = "> ".repeat(c.depth);
    let blank = ">".repeat(c.depth);
    let author = if c.is_op {
        format!("**u/{} [OP]**", c.author)
    } else {
        format!("**u/{}**", c.author)
    };
    out.push_str(&format!("{q}{author} · {}\n", pt_label(c.score)));
    for line in c.body.lines() {
        if line.is_empty() {
            out.push_str(&blank);
            out.push('\n');
        } else {
            out.push_str(&q);
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push('\n');
    for reply in &c.replies {
        render_comment(reply, out);
    }
}

fn pt_label(n: Option<i64>) -> String {
    match n {
        None => "score hidden".to_string(),
        Some(1) => "1 pt".to_string(),
        Some(-1) => "-1 pt".to_string(),
        Some(n) => format!("{n} pts"),
    }
}

/// Collapse runs of 3+ newlines down to a blank-line separator so the
/// blockquote prefixes and `<pre>` spacing don't leave large gaps.
fn collapse_blank_lines(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut newlines = 0;
    for ch in s.chars() {
        if ch == '\n' {
            newlines += 1;
            if newlines <= 2 {
                out.push(ch);
            }
        } else {
            newlines = 0;
            out.push(ch);
        }
    }
    out
}

fn plain_text(md: &str) -> String {
    md.lines()
        .map(|l| {
            // Strip a single leading blockquote / heading marker, then drop
            // emphasis markers. Greedy char-class stripping (the old approach)
            // ate legitimate content like ">"-prefixed quotes.
            let l = l.trim_start();
            let l = l
                .strip_prefix("> ")
                .or_else(|| l.strip_prefix('>'))
                .unwrap_or(l);
            let l = l.trim_start_matches('#').trim_start();
            l.replace("**", "")
                .replace("~~", "")
                .replace(['*', '`'], "")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// ─── HTML parsing ──────────────────────────────────────────────────────────────

fn parse_post(doc: &Html) -> Option<RedditPost> {
    let sel = Selector::parse("#siteTable .thing.link").ok()?;
    let thing = doc.select(&sel).next()?;
    let v = thing.value();

    let id = v
        .attr("data-fullname")
        .map(|s| s.trim_start_matches("t3_").to_string());
    let author = v.attr("data-author").unwrap_or("[deleted]").to_string();
    let subreddit = v.attr("data-subreddit").map(str::to_string);
    let score: i64 = v
        .attr("data-score")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let num_comments: usize = v
        .attr("data-comments-count")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let permalink_path = v.attr("data-permalink").unwrap_or("");
    let permalink = format!("https://old.reddit.com{permalink_path}");
    // Self-posts carry the `self` class and a `self.<sub>` domain; their
    // data-url points back at the permalink rather than an external site.
    let is_self = v.has_class("self", scraper::CaseSensitivity::AsciiCaseInsensitive)
        || v.attr("data-domain")
            .is_some_and(|d| d.starts_with("self."));
    let link_url = v.attr("data-url").map(str::to_string);
    let url = if is_self { None } else { link_url };

    // Title
    let sel_title = Selector::parse(".title a.title").ok()?;
    let title = thing
        .select(&sel_title)
        .next()
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty())?;

    // Flair
    let flair = Selector::parse(".linkflairlabel")
        .ok()
        .and_then(|s| thing.select(&s).next())
        .map(|el| el.text().collect::<String>().trim().to_string())
        .filter(|s| !s.is_empty());

    // Self-text body: thing > .entry > .expando > .usertext-body [> .md]
    let body = direct_child(thing, "entry")
        .and_then(|entry| find_class(entry, "expando"))
        .and_then(|expando| find_class(expando, "usertext-body"))
        .and_then(|ut| find_class(ut, "md"))
        .map(md_to_markdown)
        .filter(|s| !s.is_empty());

    // Datetime
    let created_utc = Selector::parse("time[datetime]")
        .ok()
        .and_then(|s| thing.select(&s).next())
        .and_then(|t| t.value().attr("datetime"))
        .map(str::to_string);

    Some(RedditPost {
        id,
        title,
        author,
        subreddit,
        score,
        body,
        num_comments,
        permalink,
        url,
        is_self,
        flair,
        created_utc,
    })
}

// ─── Comment parsing ───────────────────────────────────────────────────────────
//
// old.reddit.com nests comments structurally, not via a depth attribute:
//
//   .commentarea
//     .sitetable.nestedlisting
//       .comment.thing                          ← root comment
//         .entry → form → .usertext-body → .md  ← its own body
//         .child
//           .sitetable.listing
//             .comment.thing                    ← reply (recurse)
//
// `data-depth`/`data-replies` are absent or always "0" in the logged-out
// HTML, so we walk the tree by recursing into each comment's `.child`.

fn parse_comments(doc: &Html, op: &str) -> Vec<RedditComment> {
    // Root listing is `.sitetable.nestedlisting` inside `.commentarea`
    // (note: `commentarea` is a class on old.reddit, not an id). Fall back
    // to the first `.nestedlisting` anywhere for comment-permalink pages.
    let listing = Selector::parse(".commentarea .sitetable.nestedlisting")
        .ok()
        .and_then(|s| doc.select(&s).next())
        .or_else(|| {
            Selector::parse(".sitetable.nestedlisting")
                .ok()
                .and_then(|s| doc.select(&s).next())
        });

    match listing {
        Some(l) => walk_comment_level(l, op, 0),
        None => vec![],
    }
}

/// Parse the direct-child `.comment.thing` elements of a comment listing.
fn walk_comment_level(listing: ElementRef, op: &str, depth: usize) -> Vec<RedditComment> {
    listing
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|c| {
            let val = c.value();
            val.has_class("comment", scraper::CaseSensitivity::AsciiCaseInsensitive)
                && val.has_class("thing", scraper::CaseSensitivity::AsciiCaseInsensitive)
        })
        .filter_map(|c| parse_one_comment(c, op, depth))
        .collect()
}

fn parse_one_comment(c: ElementRef, op: &str, depth: usize) -> Option<RedditComment> {
    let v = c.value();

    // "load more comments" placeholders are `.thing` with type=morechildren.
    // They carry a t1_ fullname but no real content — skip them.
    if v.attr("data-type") == Some("morechildren")
        || v.has_class(
            "morechildren",
            scraper::CaseSensitivity::AsciiCaseInsensitive,
        )
    {
        return None;
    }

    let is_deleted = v.has_class("deleted", scraper::CaseSensitivity::AsciiCaseInsensitive);
    let id = v
        .attr("data-fullname")
        .map(|s| s.trim_start_matches("t1_").to_string());
    let author = v
        .attr("data-author")
        .filter(|a| !a.is_empty())
        .unwrap_or("[deleted]")
        .to_string();

    // Own body lives in `.entry > form > .usertext-body > .md`. `.child`
    // (nested replies) is a sibling of `.entry`, so descending within
    // `.entry` never crosses into a reply's body.
    let entry = direct_child(c, "entry");
    let body = entry
        .and_then(|e| find_class(e, "usertext-body"))
        .and_then(|ut| find_class(ut, "md"))
        .map(md_to_markdown)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            if is_deleted {
                "[removed]".into()
            } else {
                String::new()
            }
        });

    // Displayed score is `.score.unvoted`, whose `title` holds the exact
    // integer (the sibling likes/dislikes spans are ±1). Hidden-score
    // comments have no `.score.unvoted` span, so `comment_score` returns
    // None — kept distinct from a genuine 0.
    let score = entry.and_then(comment_score);

    let created_utc = entry
        .and_then(|e| Selector::parse("time[datetime]").ok().map(|s| (e, s)))
        .and_then(|(e, s)| e.select(&s).next())
        .and_then(|t| t.value().attr("datetime"))
        .map(str::to_string);

    let is_op = !is_deleted && author != "[deleted]" && author == op;

    // Replies: `.comment > .child > .sitetable > .comment`.
    let replies = direct_child(c, "child")
        .and_then(|child| direct_child(child, "sitetable"))
        .map(|st| walk_comment_level(st, op, depth + 1))
        .unwrap_or_default();

    Some(RedditComment {
        id,
        author,
        body,
        score,
        depth,
        is_op,
        created_utc,
        replies,
    })
}

/// Read a comment's score from the `.score.unvoted` span inside `.entry`.
/// Prefers the `title` attribute (exact integer); falls back to the text.
/// Returns `None` when Reddit hides the score (no `.score.unvoted` span).
fn comment_score(entry: ElementRef) -> Option<i64> {
    let sel = Selector::parse("span.score.unvoted").ok()?;
    let span = entry.select(&sel).next()?;
    span.value()
        .attr("title")
        .and_then(|t| t.trim().parse().ok())
        .or_else(|| parse_score(&span.text().collect::<String>()))
}

// ─── DOM helpers ───────────────────────────────────────────────────────────────

/// First direct child element whose class list includes `class`.
fn direct_child<'a>(el: ElementRef<'a>, class: &str) -> Option<ElementRef<'a>> {
    el.children().filter_map(ElementRef::wrap).find(|c| {
        c.value()
            .has_class(class, scraper::CaseSensitivity::AsciiCaseInsensitive)
    })
}

/// First descendant (any depth) whose class list includes `class`.
fn find_class<'a>(el: ElementRef<'a>, class: &str) -> Option<ElementRef<'a>> {
    el.children().filter_map(ElementRef::wrap).find_map(|c| {
        if c.value()
            .has_class(class, scraper::CaseSensitivity::AsciiCaseInsensitive)
        {
            Some(c)
        } else {
            find_class(c, class)
        }
    })
}

fn parse_score(text: &str) -> Option<i64> {
    text.split_whitespace()
        .next()
        .map(|w| w.replace('−', "-"))
        .and_then(|w| w.parse().ok())
}

// ─── .md div → markdown ────────────────────────────────────────────────────────

fn md_to_markdown(el: ElementRef) -> String {
    let mut out = String::new();
    render_children(el, &mut out);
    out.trim().to_string()
}

fn render_children(el: ElementRef, out: &mut String) {
    use scraper::node::Node;
    for child in el.children() {
        match child.value() {
            Node::Text(t) => out.push_str(t.as_ref()),
            Node::Element(_) => {
                if let Some(c) = ElementRef::wrap(child) {
                    render_node(c, out);
                }
            }
            _ => {}
        }
    }
}

fn render_node(el: ElementRef, out: &mut String) {
    match el.value().name() {
        "p" | "div" => {
            let mut inner = String::new();
            render_children(el, &mut inner);
            let t = inner.trim();
            if !t.is_empty() {
                out.push_str(t);
                out.push_str("\n\n");
            }
        }
        "br" => out.push('\n'),
        "strong" | "b" => {
            let t: String = el.text().collect();
            let t = t.trim();
            if !t.is_empty() {
                out.push_str(&format!("**{t}**"));
            }
        }
        "em" | "i" => {
            let t: String = el.text().collect();
            let t = t.trim();
            if !t.is_empty() {
                out.push_str(&format!("*{t}*"));
            }
        }
        "del" | "s" | "strike" => {
            let t: String = el.text().collect();
            let t = t.trim();
            if !t.is_empty() {
                out.push_str(&format!("~~{t}~~"));
            }
        }
        "code" => {
            let t: String = el.text().collect();
            out.push('`');
            out.push_str(t.trim());
            out.push('`');
        }
        "pre" => {
            let t: String = el.text().collect();
            out.push_str("```\n");
            out.push_str(t.trim_end_matches('\n'));
            out.push_str("\n```\n\n");
        }
        "a" => {
            let text: String = el.text().collect();
            let text = text.trim();
            if !text.is_empty() {
                // Preserve the destination as a markdown link. Resolve
                // root-relative reddit hrefs (/r/, /user/, /wiki/, ...) and
                // drop non-navigational ones (javascript:, #fragment, mailto:).
                let href = el.value().attr("href").unwrap_or("");
                if href.starts_with("http://") || href.starts_with("https://") {
                    out.push_str(&format!("[{text}]({href})"));
                } else if href.starts_with('/') {
                    out.push_str(&format!("[{text}](https://old.reddit.com{href})"));
                } else {
                    out.push_str(text);
                }
            }
        }
        "blockquote" => {
            let mut inner = String::new();
            render_children(el, &mut inner);
            let trimmed = inner.trim();
            for line in trimmed.lines() {
                out.push('>');
                if !line.is_empty() {
                    out.push(' ');
                    out.push_str(line);
                }
                out.push('\n');
            }
            out.push('\n');
        }
        "ul" => render_list(el, false, 0, out),
        "ol" => render_list(el, true, 0, out),
        "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
            let level = el
                .value()
                .name()
                .chars()
                .nth(1)
                .and_then(|c| c.to_digit(10))
                .unwrap_or(2) as usize;
            let t: String = el.text().collect();
            let t = t.trim();
            if !t.is_empty() {
                out.push_str(&"#".repeat(level));
                out.push(' ');
                out.push_str(t);
                out.push_str("\n\n");
            }
        }
        "hr" => out.push_str("---\n\n"),
        "sup" => {
            let t: String = el.text().collect();
            out.push_str(t.trim());
        }
        // Unknown / generic containers: recurse
        _ => render_children(el, out),
    }
}

/// Render a `<ul>`/`<ol>`, indenting nested lists by two spaces per level so
/// child items keep their own line instead of being glued to the parent.
fn render_list(list: ElementRef, ordered: bool, indent: usize, out: &mut String) {
    use scraper::node::Node;
    let pad = "  ".repeat(indent);
    let mut n = 0;
    for li in list
        .children()
        .filter_map(ElementRef::wrap)
        .filter(|c| c.value().name() == "li")
    {
        n += 1;
        // Inline content of this <li>, excluding nested lists (rendered after).
        let mut inline = String::new();
        for child in li.children() {
            match child.value() {
                Node::Text(t) => inline.push_str(t.as_ref()),
                Node::Element(e) if e.name() == "ul" || e.name() == "ol" => {}
                Node::Element(_) => {
                    if let Some(c) = ElementRef::wrap(child) {
                        render_node(c, &mut inline);
                    }
                }
                _ => {}
            }
        }
        let marker = if ordered {
            format!("{n}. ")
        } else {
            "- ".to_string()
        };
        out.push_str(&format!("{pad}{marker}{}\n", inline.trim()));

        for child in li.children().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "ul" => render_list(child, false, indent + 1, out),
                "ol" => render_list(child, true, indent + 1, out),
                _ => {}
            }
        }
    }
    if indent == 0 {
        out.push('\n');
    }
}

// ─── URL helpers ───────────────────────────────────────────────────────────────

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
}

// ─── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_reddit_url_recognises_variants() {
        assert!(is_reddit_url(
            "https://www.reddit.com/r/rust/comments/abc/x/"
        ));
        assert!(is_reddit_url(
            "https://old.reddit.com/r/rust/comments/abc/x/"
        ));
        assert!(is_reddit_url("https://reddit.com/r/rust/comments/abc/x/"));
        assert!(!is_reddit_url("https://example.com"));
    }

    #[test]
    fn try_extract_thread_returns_none_for_listing_url() {
        let html = "<html><body></body></html>";
        assert!(try_extract_thread(html, "https://old.reddit.com/r/rust/").is_none());
    }

    #[test]
    fn md_to_markdown_basic() {
        let html =
            Html::parse_fragment(r#"<div class="md"><p>Hello <strong>world</strong>!</p></div>"#);
        let sel = Selector::parse(".md").unwrap();
        let el = html.select(&sel).next().unwrap();
        let md = md_to_markdown(el);
        assert!(md.contains("**world**"));
        assert!(md.contains("Hello"));
    }

    #[test]
    fn md_to_markdown_blockquote_and_code() {
        let html = Html::parse_fragment(
            r#"<div class="md"><blockquote><p>Quoted</p></blockquote><pre><code>fn main() {}</code></pre></div>"#,
        );
        let sel = Selector::parse(".md").unwrap();
        let el = html.select(&sel).next().unwrap();
        let md = md_to_markdown(el);
        assert!(md.contains("> Quoted"));
        assert!(md.contains("```"));
        assert!(md.contains("fn main()"));
    }

    #[test]
    fn md_to_markdown_link_preserves_href() {
        let abs = Html::parse_fragment(
            r#"<div class="md"><p>see <a href="https://example.com/x">this</a></p></div>"#,
        );
        let sel = Selector::parse(".md").unwrap();
        let el = abs.select(&sel).next().unwrap();
        assert!(md_to_markdown(el).contains("[this](https://example.com/x)"));

        // Root-relative reddit links resolve against old.reddit.com.
        let rel = Html::parse_fragment(
            r#"<div class="md"><p><a href="/r/rust/wiki/faq">faq</a></p></div>"#,
        );
        let el = rel.select(&sel).next().unwrap();
        assert!(md_to_markdown(el).contains("[faq](https://old.reddit.com/r/rust/wiki/faq)"));

        // javascript: / fragment hrefs degrade to bare text.
        let js = Html::parse_fragment(
            r#"<div class="md"><p><a href="javascript:void(0)">x</a></p></div>"#,
        );
        let el = js.select(&sel).next().unwrap();
        let out = md_to_markdown(el);
        assert!(out.contains('x') && !out.contains("javascript"));
    }

    // ── Regression tests against REAL old.reddit.com HTML ──────────────────
    //
    // These fixtures are genuine pages fetched from old.reddit.com (see
    // testdata/reddit/). They are the ground truth — synthetic HTML is too
    // easy to write to match wrong assumptions, which is exactly how the
    // first version of this parser shipped silently broken.

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("testdata/reddit/{name}")).unwrap()
    }

    fn total_comments(cs: &[RedditComment]) -> usize {
        cs.len() + cs.iter().map(|c| total_comments(&c.replies)).sum::<usize>()
    }

    fn collect<'a>(cs: &'a [RedditComment], out: &mut Vec<&'a RedditComment>) {
        for c in cs {
            out.push(c);
            collect(&c.replies, out);
        }
    }

    #[test]
    fn real_link_post_metadata() {
        // pandas: external-link post (blog.geekuni.com), 34 comments.
        let html = fixture("pandas_34comments.html");
        let t = try_extract_thread(
            &html,
            "https://old.reddit.com/r/programming/comments/abc123/t/",
        )
        .expect("should parse");
        let p = t.post.expect("post");
        assert_eq!(p.author, "Horror-Willingness74");
        assert_eq!(p.subreddit.as_deref(), Some("programming"));
        assert_eq!(p.score, 43);
        assert_eq!(p.num_comments, 34, "data-comments-count");
        assert!(!p.is_self, "external blog link, not a self post");
        assert_eq!(
            p.url.as_deref(),
            Some("https://blog.geekuni.com/2026/06/why-learn-pandas.html")
        );
        assert!(p.title.contains("Pandas"));
    }

    #[test]
    fn real_self_post_metadata() {
        // A self-post (text) on r/rust: `self.rust` domain, self-text body,
        // no external url.
        let html = fixture("rust_selfpost_36comments.html");
        let t = try_extract_thread(&html, "https://old.reddit.com/r/rust/comments/abc123/t/")
            .expect("should parse");
        let p = t.post.expect("post");
        assert!(p.is_self, "self.rust domain → self post");
        assert_eq!(p.url, None, "self posts carry no external url");
        assert_eq!(p.subreddit.as_deref(), Some("rust"));
        assert!(
            p.body
                .as_deref()
                .unwrap_or("")
                .contains("IT project manager"),
            "self-text body should be extracted: {:?}",
            p.body
        );
    }

    #[test]
    fn real_comment_bodies_and_scores() {
        // The original bug: every comment body came back empty because
        // .usertext-body sits inside a <form>, not directly under .entry.
        let html = fixture("ebpf_6comments.html");
        let t = try_extract_thread(
            &html,
            "https://old.reddit.com/r/programming/comments/abc123/t/",
        )
        .expect("should parse");
        // 6 comments total: 5 top-level + 1 nested reply (admalledd under ejrh).
        assert_eq!(t.comments.len(), 5, "5 top-level comments");
        assert_eq!(total_comments(&t.comments), 6, "6 comments incl. nested");
        let teerre = t
            .comments
            .iter()
            .find(|c| c.author == "teerre")
            .expect("teerre");
        assert!(
            teerre.body.contains("Very cool blog"),
            "body must be populated, got {:?}",
            teerre.body
        );
        // Score comes from .score.unvoted title (the real value), not the
        // ±1 likes/dislikes siblings.
        assert_eq!(
            teerre.score,
            Some(10),
            "unvoted score, not dislikes(9)/likes(11)"
        );
        assert!(
            t.comments.iter().all(|c| !c.body.is_empty()),
            "no comment body should be empty"
        );
    }

    #[test]
    fn real_nested_comment_tree() {
        // pandas has structurally-nested replies (.child > .sitetable >
        // .comment). data-depth/data-replies are absent in logged-out HTML.
        let html = fixture("pandas_34comments.html");
        let t = try_extract_thread(
            &html,
            "https://old.reddit.com/r/programming/comments/abc123/t/",
        )
        .expect("should parse");
        // 34 rendered comments with content + 1 [deleted] node that old.reddit
        // still shows because it has live replies = 35 nodes in the tree.
        assert_eq!(
            total_comments(&t.comments),
            35,
            "all comments incl. nested + deleted"
        );
        let nested = t.comments.iter().any(|c| !c.replies.is_empty());
        assert!(nested, "at least one comment must have replies");
        let max_depth = {
            fn d(cs: &[RedditComment]) -> usize {
                cs.iter().map(|c| 1 + d(&c.replies)).max().unwrap_or(0)
            }
            d(&t.comments)
        };
        assert!(max_depth >= 2, "tree should be more than one level deep");
        let a_reply = t.comments.iter().find_map(|c| c.replies.first());
        assert_eq!(a_reply.map(|r| r.depth), Some(1));
    }

    #[test]
    fn real_morechildren_stubs_skipped() {
        // AskReddit deep thread: 259 .thing[data-fullname=t1_] markers, but
        // some are "load more comments" stubs (data-type=morechildren) with
        // no author/body. They must not appear as ghost comments.
        let html = fixture("askreddit_deep_morechildren.html");
        let t = try_extract_thread(
            &html,
            "https://old.reddit.com/r/AskReddit/comments/abc123/t/",
        )
        .expect("should parse");
        fn check(cs: &[RedditComment]) {
            for c in cs {
                let ghost = c.body.is_empty() && c.author == "[deleted]" && c.id.is_some();
                assert!(!ghost, "morechildren stub leaked as comment: {:?}", c.id);
                check(&c.replies);
            }
        }
        check(&t.comments);
    }

    #[test]
    fn real_hidden_score_is_none_not_zero() {
        // AskReddit has fresh comments with `.score-hidden` (no .score.unvoted
        // span). These must be None, distinct from a genuine 0-score comment.
        let html = fixture("askreddit_deep_morechildren.html");
        let t = try_extract_thread(
            &html,
            "https://old.reddit.com/r/AskReddit/comments/abc123/t/",
        )
        .expect("should parse");
        let mut all = Vec::new();
        collect(&t.comments, &mut all);
        assert!(
            all.iter().any(|c| c.score.is_none()),
            "some fresh comments have hidden scores → None"
        );
    }

    #[test]
    fn real_deleted_comment_preserves_subtree() {
        // pandas has a [deleted] comment that still has visible replies. The
        // structural walk must keep it so its children aren't orphaned.
        let html = fixture("pandas_34comments.html");
        let t = try_extract_thread(
            &html,
            "https://old.reddit.com/r/programming/comments/abc123/t/",
        )
        .expect("should parse");
        let mut all = Vec::new();
        collect(&t.comments, &mut all);
        let deleted: Vec<_> = all.iter().filter(|c| c.author == "[deleted]").collect();
        assert!(!deleted.is_empty(), "should keep deleted comments");
        assert!(
            deleted.iter().any(|c| !c.replies.is_empty()),
            "a deleted comment with replies must retain its subtree"
        );
        assert!(deleted.iter().all(|c| !c.is_op));
    }

    #[test]
    fn real_markdown_is_commonmark_clean() {
        // Guards the markdown bugs the verification workflow found: no
        // whitespace-only "blank" lines, and ``` fences never indented 4+
        // spaces (which would turn them into literal indented code blocks).
        let html = fixture("elixir_60comments.html");
        let result = try_extract(
            &html,
            "https://old.reddit.com/r/programming/comments/abc123/t/",
        )
        .expect("should extract");
        let md = &result.content.markdown;
        assert!(md.starts_with("# "));
        assert!(md.contains("## Comments"));
        for line in md.lines() {
            assert!(
                !(line.starts_with(' ') && line.trim().is_empty()),
                "whitespace-only line: {line:?}"
            );
            let trimmed = line.trim_start_matches(['>', ' ']);
            if trimmed.starts_with("```") {
                let indent = line.len() - line.trim_start_matches(' ').len();
                assert!(indent < 4, "code fence indented {indent} spaces: {line:?}");
            }
        }
        assert!(result.metadata.word_count > 20);
    }
}
