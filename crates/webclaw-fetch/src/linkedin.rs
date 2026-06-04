/// LinkedIn post extraction from authenticated HTML.
///
/// LinkedIn's SPA stores all data in `<code>` tags as HTML-escaped JSON.
/// The `included` array contains typed entities: Update (post), Comment,
/// Profile, etc. We parse these to reconstruct post + comments as markdown.
use serde_json::Value;
use tracing::debug;
use webclaw_core::{Content, ExtractionResult, Metadata};

/// Check if a URL is a LinkedIn post/activity.
pub fn is_linkedin_post(url: &str) -> bool {
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("");
    (host == "www.linkedin.com" || host == "linkedin.com")
        && (url.contains("/feed/update/") || url.contains("/posts/"))
}

/// Extract `<code>` block contents from HTML using simple string scanning.
/// LinkedIn wraps JSON data in `<code>` tags with HTML-escaped content.
fn extract_code_blocks(html: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    let mut search_from = 0;
    while let Some(start) = html[search_from..].find("<code") {
        let abs_start = search_from + start;
        // Find end of opening tag
        let Some(tag_end) = html[abs_start..].find('>') else {
            break;
        };
        let content_start = abs_start + tag_end + 1;
        let Some(end) = html[content_start..].find("</code>") else {
            break;
        };
        let content = &html[content_start..content_start + end];
        if content.len() > 1000 {
            blocks.push(html_unescape(content));
        }
        search_from = content_start + end + 7;
    }
    blocks
}

/// Extract post + comments from LinkedIn's SSR HTML (requires auth cookies).
pub fn extract_linkedin_post(html: &str, url: &str) -> Option<ExtractionResult> {
    let code_blocks = extract_code_blocks(html);

    // Find the largest <code> block with "included" — that's the main data payload
    let mut best_included: Option<Vec<Value>> = None;
    for raw in &code_blocks {
        if let Ok(obj) = serde_json::from_str::<Value>(raw)
            && let Some(arr) = obj.get("included").and_then(|v| v.as_array())
        {
            let current_len = best_included.as_ref().map(|a| a.len()).unwrap_or(0);
            if arr.len() > current_len {
                best_included = Some(arr.clone());
            }
        }
    }

    let included = best_included?;
    debug!(entities = included.len(), "linkedin: found included array");

    // Collect profiles (entityUrn → "First Last")
    let mut profiles = std::collections::HashMap::new();
    for item in &included {
        let t = item.get("$type").and_then(|v| v.as_str()).unwrap_or("");
        if t.contains("Profile") {
            let urn = item.get("entityUrn").and_then(|v| v.as_str()).unwrap_or("");
            let first = item.get("firstName").and_then(|v| v.as_str()).unwrap_or("");
            let last = item.get("lastName").and_then(|v| v.as_str()).unwrap_or("");
            let headline = item.get("headline").and_then(|v| v.as_str()).unwrap_or("");
            if !first.is_empty() {
                profiles.insert(
                    urn.to_string(),
                    (
                        format!("{first} {last}").trim().to_string(),
                        headline.to_string(),
                    ),
                );
            }
        }
    }

    // Find the main post (Update type)
    let mut markdown = String::new();
    let mut post_author = String::new();
    let mut post_headline = String::new();

    for item in &included {
        let t = item.get("$type").and_then(|v| v.as_str()).unwrap_or("");
        if !t.contains("Update") {
            continue;
        }

        // Get author from actor profile
        if let Some(actor) = item.get("actor") {
            // actor can have a nested profile reference or inline data
            let author_urn = actor
                .get("*author")
                .or(actor.get("author"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if let Some((name, headline)) = profiles.get(author_urn) {
                post_author = name.clone();
                post_headline = headline.clone();
            }
            // Or inline name
            if post_author.is_empty()
                && let Some(name) = actor.get("name").and_then(|v| v.as_object())
            {
                let text = name.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if !text.is_empty() {
                    post_author = text.to_string();
                }
            }
            if post_headline.is_empty()
                && let Some(desc) = actor.get("description").and_then(|v| v.as_object())
            {
                let text = desc.get("text").and_then(|v| v.as_str()).unwrap_or("");
                if !text.is_empty() {
                    post_headline = text.to_string();
                }
            }
        }

        // Get post body from commentary
        if let Some(commentary) = item.get("commentary")
            && let Some(text) = commentary
                .get("text")
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("text"))
                .and_then(|v| v.as_str())
        {
            if !post_author.is_empty() {
                markdown.push_str(&format!("# {post_author}\n\n"));
            }
            if !post_headline.is_empty() {
                markdown.push_str(&format!("*{post_headline}*\n\n"));
            }
            markdown.push_str("---\n\n");
            // Unescape literal \n from JSON
            markdown.push_str(&text.replace("\\n", "\n"));
            markdown.push_str("\n\n");
        }
    }

    if markdown.is_empty() {
        return None;
    }

    // Collect comments — LinkedIn stores comment text in `commentary.text`
    // and commenter name in `commenter.name.text`
    let mut comments: Vec<(String, String)> = Vec::new();
    for item in &included {
        let t = item.get("$type").and_then(|v| v.as_str()).unwrap_or("");
        if !t.contains("Comment") {
            continue;
        }

        // Get comment text from commentary.text
        let text = item
            .get("commentary")
            .and_then(|c| c.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if text.is_empty() {
            continue;
        }

        // Get commenter name from commenter.title.text
        let name = item
            .get("commenter")
            .and_then(|c| c.get("title"))
            .and_then(|n| n.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or("Someone");

        comments.push((name.to_string(), text.to_string()));
    }

    if !comments.is_empty() {
        markdown.push_str("---\n\n## Comments\n\n");
        for (name, text) in &comments {
            markdown.push_str(&format!("- **{name}**: {text}\n\n"));
        }
    }

    let word_count = markdown.split_whitespace().count();
    debug!(
        word_count,
        comments = comments.len(),
        "linkedin extraction done"
    );

    let title = if post_author.is_empty() {
        None
    } else {
        Some(format!("{post_author}'s LinkedIn Post"))
    };
    let author = if post_author.is_empty() {
        None
    } else {
        Some(post_author)
    };
    let metadata = Metadata::default()
        .with_title(title)
        .with_author(author)
        .with_url(Some(url.to_string()))
        .with_site_name(Some("LinkedIn".into()))
        .with_word_count(word_count);
    let content = Content::default().with_markdown(markdown);
    Some(ExtractionResult::new(metadata, content))
}

/// Unescape HTML entities (named + numeric decimal).
fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '&' {
            out.push(c);
            continue;
        }
        // Collect until ';'
        let mut entity = String::new();
        for c2 in chars.by_ref() {
            if c2 == ';' {
                break;
            }
            entity.push(c2);
            if entity.len() > 10 {
                break;
            }
        }
        match entity.as_str() {
            "quot" => out.push('"'),
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "apos" => out.push('\''),
            s if s.starts_with('#') => {
                let num = &s[1..];
                if let Ok(n) = num.parse::<u32>()
                    && let Some(ch) = char::from_u32(n)
                {
                    out.push(ch);
                    continue;
                }
                out.push('&');
                out.push_str(&entity);
                out.push(';');
            }
            _ => {
                out.push('&');
                out.push_str(&entity);
                out.push(';');
            }
        }
    }
    out
}
