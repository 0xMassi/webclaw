/// Reddit JSON API fallback for extracting posts + comments without JS rendering.
///
/// Reddit's new `shreddit` frontend only SSRs the post body — comments are
/// loaded client-side. Appending `.json` to any Reddit URL returns the full
/// comment tree as structured JSON, which we convert to clean markdown.
use serde::Deserialize;
use tracing::debug;
use webclaw_core::{Content, ExtractionResult, Metadata};

/// Check if a URL points to a Reddit post/comment page.
pub fn is_reddit_url(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    matches!(
        host,
        "reddit.com" | "www.reddit.com" | "old.reddit.com" | "np.reddit.com" | "new.reddit.com"
    )
}

/// Build the `.json` URL from a Reddit page URL.
pub fn json_url(url: &str) -> String {
    let clean = url.split('?').next().unwrap_or(url).trim_end_matches('/');
    format!("{clean}.json")
}

/// Convert Reddit JSON API response into an ExtractionResult.
pub fn parse_reddit_json(json_bytes: &[u8], url: &str) -> Result<ExtractionResult, String> {
    let listings: Vec<Listing> =
        serde_json::from_slice(json_bytes).map_err(|e| format!("reddit json parse: {e}"))?;

    let mut markdown = String::new();
    let mut title = None;
    let mut author = None;
    let mut subreddit = None;

    // First listing = the post itself
    if let Some(post_listing) = listings.first() {
        for child in &post_listing.data.children {
            if child.kind == "t3" {
                let d = &child.data;
                title = d.title.clone();
                author = d.author.clone();
                subreddit = d.subreddit_name_prefixed.clone();

                if let Some(ref t) = title {
                    markdown.push_str(&format!("# {t}\n\n"));
                }
                if let (Some(a), Some(sr)) = (&author, &subreddit) {
                    markdown.push_str(&format!("**u/{a}** in {sr}\n\n"));
                }
                if let Some(ref body) = d.selftext
                    && !body.is_empty()
                {
                    markdown.push_str(body);
                    markdown.push_str("\n\n");
                }
                if let Some(ref url_field) = d.url_overridden_by_dest
                    && !url_field.is_empty()
                {
                    markdown.push_str(&format!("[Link]({url_field})\n\n"));
                }
                markdown.push_str("---\n\n");
            }
        }
    }

    // Second listing = comment tree
    if let Some(comment_listing) = listings.get(1) {
        markdown.push_str("## Comments\n\n");
        for child in &comment_listing.data.children {
            render_comment(child, 0, &mut markdown);
        }
    }

    let word_count = markdown.split_whitespace().count();
    debug!(word_count, "reddit json extracted");

    Ok(ExtractionResult {
        metadata: Metadata {
            title,
            description: None,
            author,
            published_date: None,
            language: Some("en".into()),
            url: Some(url.to_string()),
            site_name: subreddit,
            image: None,
            favicon: None,
            word_count,
            // M12: Reddit JSON path emits post body + comments; no chrome
            // wrapper. All words are article-side content.
            word_count_article: word_count,
            word_count_chrome: 0,
            http_status: None,
        },
        content: Content {
            markdown,
            plain_text: String::new(),
            links: vec![],
            images: vec![],
            code_blocks: vec![],
            raw_html: None,
        },
        domain_data: None,
        structured_data: vec![],
    })
}

fn render_comment(thing: &Thing, depth: usize, out: &mut String) {
    if thing.kind != "t1" {
        return;
    }
    let d = &thing.data;
    let indent = "  ".repeat(depth);
    let author = d.author.as_deref().unwrap_or("[deleted]");
    let body = d.body.as_deref().unwrap_or("[removed]");
    let score = d.score.unwrap_or(0);

    out.push_str(&format!("{indent}- **u/{author}** ({score} pts)\n"));
    for line in body.lines() {
        out.push_str(&format!("{indent}  {line}\n"));
    }
    out.push('\n');

    // Recurse into replies
    if let Some(Replies::Listing(listing)) = &d.replies {
        for child in &listing.data.children {
            render_comment(child, depth + 1, out);
        }
    }
}

// --- Reddit JSON types (minimal) ---

#[derive(Deserialize)]
struct Listing {
    data: ListingData,
}

#[derive(Deserialize)]
struct ListingData {
    children: Vec<Thing>,
}

#[derive(Deserialize)]
struct Thing {
    kind: String,
    data: ThingData,
}

#[derive(Deserialize)]
struct ThingData {
    // Post fields (t3)
    title: Option<String>,
    selftext: Option<String>,
    subreddit_name_prefixed: Option<String>,
    url_overridden_by_dest: Option<String>,
    // Comment fields (t1)
    author: Option<String>,
    body: Option<String>,
    score: Option<i64>,
    replies: Option<Replies>,
}

/// Reddit replies can be either a nested Listing or an empty string.
#[derive(Deserialize)]
#[serde(untagged)]
enum Replies {
    Listing(Listing),
    #[allow(dead_code)]
    Empty(String),
}
