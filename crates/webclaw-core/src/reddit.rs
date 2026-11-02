//! Reddit extractor over Reddit's public Atom feeds.
//!
//! The fetch layer maps Reddit page URLs to their `.rss` feed; this module
//! turns that feed into markdown, and a comment thread into [`RedditThread`].

use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use sxd_document::dom::{ChildOfElement, ChildOfRoot, Element};

use crate::{Content, DomainData, DomainType, ExtractionResult, Metadata};

#[derive(Serialize)]
pub struct RedditPost {
    pub id: Option<String>,
    pub title: String,
    pub author: String,
    pub subreddit: Option<String>,
    pub score: Option<i64>,
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

pub fn is_reddit_url(url: &str) -> bool {
    matches!(
        host_of(url),
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "np.reddit.com" | "new.reddit.com"
    )
}

/// Fill comment scores from Reddit's server-rendered comment tree, matched
/// by comment id. The tree lists the first page of comments, so comments it
/// leaves out keep `None`, and so does the post.
pub fn apply_comment_scores(thread: &mut RedditThread, comment_tree: &str) {
    static COMMENT: std::sync::LazyLock<Selector> = std::sync::LazyLock::new(|| {
        Selector::parse("shreddit-comment[thingid][score]").expect("valid selector")
    });
    let doc = Html::parse_document(comment_tree);
    let scores: std::collections::HashMap<&str, i64> = doc
        .select(&COMMENT)
        .filter_map(|c| {
            let id = c.value().attr("thingid")?.strip_prefix("t1_")?;
            Some((id, c.value().attr("score")?.parse().ok()?))
        })
        .collect();
    for comment in &mut thread.comments {
        if let Some(score) = comment.id.as_deref().and_then(|id| scores.get(id)) {
            comment.score = Some(*score);
        }
    }
}

/// Parse a comment thread from its Atom feed. `None` when `url` is not a
/// thread or `feed` is not a Reddit Atom feed.
pub fn try_extract_thread(feed: &str, url: &str) -> Option<RedditThread> {
    if !url.contains("/comments/") {
        return None;
    }
    thread_from_feed(&parse_feed(feed)?, url)
}

/// Entry point for `webclaw-core`'s extraction fast path: a thread or a
/// listing (subreddit, search, user) from its Atom feed.
pub fn try_extract(feed: &str, url: &str) -> Option<ExtractionResult> {
    let feed = parse_feed(feed)?;
    if url.contains("/comments/") {
        let thread = thread_from_feed(&feed, url)?;
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
        return Some(result(to_markdown(&thread), url, title, author, site_name));
    }
    let title = feed.title.clone();
    Some(result(listing_markdown(&feed), url, title, None, None))
}

struct Feed {
    title: Option<String>,
    entries: Vec<Entry>,
}

struct Entry {
    id: String,
    author: String,
    title: String,
    link: String,
    content: Html,
    published: Option<String>,
    subreddit: Option<String>,
}

fn parse_feed(body: &str) -> Option<Feed> {
    let body = body.trim_start_matches('\u{feff}');
    let head = &body.as_bytes()[..body.len().min(512)];
    if !head.windows(5).any(|w| w == b"<feed") {
        return None;
    }
    let package = sxd_document::parser::parse(body).ok()?;
    let doc = package.as_document();
    let feed = doc.root().children().into_iter().find_map(|c| match c {
        ChildOfRoot::Element(e) if e.name().local_part() == "feed" => Some(e),
        _ => None,
    })?;
    let entries = elements(feed, "entry")
        .into_iter()
        .filter_map(parse_entry)
        .collect();
    Some(Feed {
        title: child_text(feed, "title"),
        entries,
    })
}

fn parse_entry(e: Element) -> Option<Entry> {
    let author = elements(e, "author")
        .first()
        .and_then(|a| child_text(*a, "name"))
        .map(|n| n.trim_start_matches("/u/").to_string())
        .unwrap_or_else(|| "[deleted]".to_string());
    Some(Entry {
        id: child_text(e, "id")?,
        author,
        title: child_text(e, "title").unwrap_or_default(),
        link: elements(e, "link")
            .first()
            .and_then(|l| l.attribute_value("href"))
            .unwrap_or_default()
            .to_string(),
        content: Html::parse_fragment(&child_text(e, "content").unwrap_or_default()),
        published: child_text(e, "published").or_else(|| child_text(e, "updated")),
        subreddit: elements(e, "category")
            .first()
            .and_then(|c| c.attribute_value("term"))
            .filter(|t| !t.starts_with("u/"))
            .map(str::to_string),
    })
}

fn elements<'d>(el: Element<'d>, name: &str) -> Vec<Element<'d>> {
    el.children()
        .into_iter()
        .filter_map(|c| match c {
            ChildOfElement::Element(e) if e.name().local_part() == name => Some(e),
            _ => None,
        })
        .collect()
}

fn child_text(el: Element, name: &str) -> Option<String> {
    let child = *elements(el, name).first()?;
    let text: String = child
        .children()
        .into_iter()
        .filter_map(|c| match c {
            ChildOfElement::Text(t) => Some(t.text()),
            _ => None,
        })
        .collect();
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn thread_from_feed(feed: &Feed, url: &str) -> Option<RedditThread> {
    let comments: Vec<_> = feed
        .entries
        .iter()
        .filter(|e| e.id.starts_with("t1_"))
        .collect();
    let post = feed.entries.iter().find(|e| e.id.starts_with("t3_"));
    if post.is_none() && comments.is_empty() {
        return None;
    }
    let op = post.map(|p| p.author.as_str()).unwrap_or("");
    Some(RedditThread {
        source_url: url.to_string(),
        post: post.map(|p| to_post(p, comments.len())),
        comments: comments
            .into_iter()
            .map(|c| RedditComment {
                id: Some(c.id.trim_start_matches("t1_").to_string()),
                author: c.author.clone(),
                body: entry_body(c).unwrap_or_default(),
                score: None,
                depth: 0,
                is_op: !op.is_empty() && c.author == op,
                created_utc: c.published.clone(),
                replies: vec![],
            })
            .collect(),
    })
}

fn to_post(e: &Entry, num_comments: usize) -> RedditPost {
    let url = external_link(e);
    RedditPost {
        id: Some(e.id.trim_start_matches("t3_").to_string()),
        title: e.title.clone(),
        author: e.author.clone(),
        subreddit: e.subreddit.clone(),
        score: None,
        body: entry_body(e),
        num_comments,
        permalink: e.link.clone(),
        is_self: url.is_none(),
        url,
        flair: None,
        created_utc: e.published.clone(),
    }
}

fn entry_body(e: &Entry) -> Option<String> {
    let sel = Selector::parse("div.md").ok()?;
    e.content
        .select(&sel)
        .next()
        .map(md_to_markdown)
        .filter(|s| !s.is_empty())
}

fn external_link(e: &Entry) -> Option<String> {
    let sel = Selector::parse("a[href]").ok()?;
    let href = e
        .content
        .select(&sel)
        .find(|a| a.text().collect::<String>().trim() == "[link]")?
        .value()
        .attr("href")?;
    (href.trim_end_matches('/') != e.link.trim_end_matches('/')).then(|| href.to_string())
}

fn result(
    md: String,
    url: &str,
    title: Option<String>,
    author: Option<String>,
    site_name: Option<String>,
) -> ExtractionResult {
    ExtractionResult {
        metadata: Metadata {
            title,
            description: None,
            author,
            published_date: None,
            language: Some("en".to_string()),
            url: Some(url.to_string()),
            site_name,
            image: None,
            favicon: None,
            word_count: md.split_whitespace().count(),
        },
        content: Content {
            plain_text: plain_text(&md),
            markdown: md,
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

pub fn to_markdown(thread: &RedditThread) -> String {
    let mut out = String::new();

    if let Some(p) = &thread.post {
        out.push_str(&format!("# {}\n\n", p.title));
        let mut line = vec![format!("**u/{}**", p.author)];
        line.extend(p.subreddit.as_ref().map(|s| format!("r/{s}")));
        line.extend(p.score.map(pt_label));
        match p.num_comments {
            0 => {}
            1 => line.push("1 comment".to_string()),
            n => line.push(format!("{n} comments")),
        }
        out.push_str(&line.join(" · "));
        out.push_str("\n\n");
        if let Some(body) = &p.body {
            out.push_str(body);
            out.push_str("\n\n");
        }
        if let Some(link) = &p.url {
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

fn listing_markdown(feed: &Feed) -> String {
    let mut out = String::new();
    if let Some(title) = &feed.title {
        out.push_str(&format!("# {title}\n\n"));
    }
    for e in &feed.entries {
        out.push_str(&format!("## [{}]({})\n\n", e.title, e.link));
        let mut line = vec![format!("**u/{}**", e.author)];
        line.extend(e.subreddit.as_ref().map(|s| format!("r/{s}")));
        line.extend(e.published.clone());
        out.push_str(&line.join(" · "));
        out.push_str("\n\n");
        if let Some(body) = entry_body(e) {
            out.push_str(&body);
            out.push_str("\n\n");
        }
        if let Some(link) = external_link(e) {
            out.push_str(&format!("[Link]({link})\n\n"));
        }
    }
    collapse_blank_lines(out.trim_end())
}

fn render_comment(c: &RedditComment, out: &mut String) {
    let q = "> ".repeat(c.depth);
    let blank = ">".repeat(c.depth);
    let mut line = vec![if c.is_op {
        format!("**u/{} [OP]**", c.author)
    } else {
        format!("**u/{}**", c.author)
    }];
    line.extend(c.score.map(pt_label));
    out.push_str(&format!("{q}{}\n", line.join(" · ")));
    for l in c.body.lines() {
        if l.is_empty() {
            out.push_str(&blank);
        } else {
            out.push_str(&q);
            out.push_str(l);
        }
        out.push('\n');
    }
    out.push('\n');
    for reply in &c.replies {
        render_comment(reply, out);
    }
}

fn pt_label(n: i64) -> String {
    match n {
        1 | -1 => format!("{n} pt"),
        n => format!("{n} pts"),
    }
}

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

fn md_to_markdown(el: ElementRef) -> String {
    let mut out = String::new();
    render_children(el, &mut out);
    out.trim().to_string()
}

fn render_children(el: ElementRef, out: &mut String) {
    use scraper::node::Node;
    for child in el.children() {
        match child.value() {
            Node::Text(t) if out.ends_with('\n') => out.push_str(t.trim_start()),
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
                let href = el.value().attr("href").unwrap_or("");
                if href.starts_with("http://") || href.starts_with("https://") {
                    out.push_str(&format!("[{text}]({href})"));
                } else if href.starts_with('/') {
                    out.push_str(&format!("[{text}](https://www.reddit.com{href})"));
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
        _ => render_children(el, out),
    }
}

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

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    const THREAD: &str = "https://www.reddit.com/r/AskReddit/comments/1xh2sk9/scientists_of_reddit_whats_a_discovery_that/";
    const LINK_THREAD: &str = "https://old.reddit.com/r/technology/comments/1xhjlt8/meta_plotted/";

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!("testdata/reddit-feed/{name}")).unwrap()
    }

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
    fn listing_url_is_not_a_thread() {
        assert!(
            try_extract_thread(
                &fixture("subreddit-new.atom"),
                "https://www.reddit.com/r/technology/new/"
            )
            .is_none()
        );
    }

    #[test]
    fn html_is_not_a_feed() {
        let html = "<html><body><p>You've been blocked by network security.</p></body></html>";
        assert!(try_extract(html, THREAD).is_none());
        assert!(try_extract_thread(html, THREAD).is_none());
    }

    #[test]
    fn multibyte_character_at_the_sniff_boundary_is_not_a_feed() {
        let html = format!("<html>{}é{}", "a".repeat(505), "b".repeat(100));
        assert!(!html.is_char_boundary(512));
        assert!(try_extract(&html, THREAD).is_none());
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
        let sel = Selector::parse(".md").unwrap();
        let abs = Html::parse_fragment(
            r#"<div class="md"><p>see <a href="https://example.com/x">this</a></p></div>"#,
        );
        let el = abs.select(&sel).next().unwrap();
        assert!(md_to_markdown(el).contains("[this](https://example.com/x)"));

        let rel = Html::parse_fragment(
            r#"<div class="md"><p><a href="/r/rust/wiki/faq">faq</a></p></div>"#,
        );
        let el = rel.select(&sel).next().unwrap();
        assert!(md_to_markdown(el).contains("[faq](https://www.reddit.com/r/rust/wiki/faq)"));

        let js = Html::parse_fragment(
            r#"<div class="md"><p><a href="javascript:void(0)">x</a></p></div>"#,
        );
        let el = js.select(&sel).next().unwrap();
        let out = md_to_markdown(el);
        assert!(out.contains('x') && !out.contains("javascript"));
    }

    #[test]
    fn thread_feed_matches_golden_json() {
        let thread = try_extract_thread(&fixture("thread-small.atom"), LINK_THREAD).unwrap();
        let got = serde_json::to_value(&thread).unwrap();
        let want: serde_json::Value = serde_json::from_str(&fixture("thread-small.json")).unwrap();
        assert_eq!(got, want);
    }

    #[test]
    fn self_post_thread_has_post_and_every_comment() {
        let t = try_extract_thread(&fixture("thread-busy.atom"), THREAD).unwrap();
        let p = t.post.as_ref().unwrap();
        assert_eq!(p.id.as_deref(), Some("1xh2sk9"));
        assert_eq!(p.author, "user1");
        assert_eq!(p.subreddit.as_deref(), Some("AskReddit"));
        assert!(p.title.starts_with("Scientists of Reddit"));
        assert!(p.is_self);
        assert_eq!(p.url, None);
        assert_eq!(p.score, None);
        assert_eq!(p.created_utc.as_deref(), Some("2026-10-27T19:51:01+00:00"));
        assert_eq!(t.comments.len(), 29);
        assert_eq!(p.num_comments, 29);
        assert_eq!(t.comments[0].id.as_deref(), Some("19rr19znbxl5v"));
        assert!(t.comments[0].body.contains("cystic fibrosis"));
        assert!(t.comments[0].created_utc.is_some());
        assert!(
            t.comments
                .iter()
                .all(|c| !c.body.is_empty() && c.score.is_none())
        );
        assert_eq!(t.source_url, THREAD);
    }

    #[test]
    fn comment_scores_come_from_the_comment_tree() {
        let url = "https://www.reddit.com/r/rust/comments/1x9w6by/hey_rustaceans_got_an_easy_question_ask_here/";
        let mut t = try_extract_thread(&fixture("thread-scored.atom"), url).unwrap();
        apply_comment_scores(&mut t, &fixture("thread-scored-comments.html"));
        let scores: Vec<_> = t
            .comments
            .iter()
            .map(|c| (c.id.as_deref().unwrap(), c.score))
            .collect();
        assert_eq!(
            scores,
            [
                ("19rnkcav91gmd", Some(2)),
                ("19rjjnurcui5d", Some(2)),
                ("19reyh3zxamw5", Some(3)),
                ("phosst6", Some(1)),
                ("php29hk", Some(1)),
                ("phq9ijv", Some(5)),
                ("phqbljf", None),
            ]
        );
        assert_eq!(t.post.unwrap().score, None);
    }

    #[test]
    fn comment_scores_ignore_html_without_comments() {
        let url = "https://www.reddit.com/r/rust/comments/1x9w6by/x/";
        let mut t = try_extract_thread(&fixture("thread-scored.atom"), url).unwrap();
        apply_comment_scores(&mut t, "<html><title>Reddit</title></html>");
        assert!(t.comments.iter().all(|c| c.score.is_none()));
    }

    #[test]
    fn thread_markdown_is_clean() {
        let result = try_extract(&fixture("thread-busy.atom"), THREAD).unwrap();
        let md = &result.content.markdown;
        assert!(md.starts_with("# Scientists of Reddit"));
        assert!(md.contains("**u/user1** · r/AskReddit · 29 comments"));
        assert!(md.contains("## Comments"));
        assert!(!md.contains("submitted by"));
        assert!(!md.contains("score hidden"));
        assert!(md.lines().all(|l| !l.starts_with(' ')));
        assert_eq!(result.metadata.url.as_deref(), Some(THREAD));
        assert_eq!(result.metadata.site_name.as_deref(), Some("AskReddit"));
        assert!(result.metadata.word_count > 500);
    }

    #[test]
    fn subreddit_listing_renders_every_post() {
        let url = "https://www.reddit.com/r/technology/new/";
        let result = try_extract(&fixture("subreddit-new.atom"), url).unwrap();
        let md = &result.content.markdown;
        assert!(md.starts_with("# newest submissions : technology"));
        assert_eq!(md.matches("\n## [").count(), 6);
        assert!(md.contains("## [Meta plotted to discredit critic Jonathan Haidt...](https://www.reddit.com/r/technology/comments/1xhjlt8/meta_plotted_to_discredit_critic_jonathan_haidt/)"));
        assert!(md.contains("**u/user1** · r/technology · 2026-10-28T09:42:31+00:00"));
        assert!(md.contains("[Link](https://pluang.com/"));
        assert_eq!(result.metadata.url.as_deref(), Some(url));
    }

    #[test]
    fn search_listing_keeps_subreddits_and_posts() {
        let url = "https://www.reddit.com/search/?q=openai&sort=new";
        let md = try_extract(&fixture("search-new.atom"), url)
            .unwrap()
            .content
            .markdown;
        assert!(md.contains("## [OpenAI](https://www.reddit.com/r/OpenAI/)"));
        assert!(md.contains("n8n has released version 3.0.0 Beta"));
        assert_eq!(md.matches("\n## [").count(), 6);
    }
}
