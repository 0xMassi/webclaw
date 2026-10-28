//! Reddit URL helpers for the fetch layer.
//!
//! Reddit answers logged-out HTML and `.json` requests with a block page, but
//! serves its Atom feeds. Every Reddit URL is fetched as its `.rss` feed on
//! www.reddit.com, which `webclaw-core::reddit` parses.

pub fn is_reddit_url(url: &str) -> bool {
    webclaw_core::reddit::is_reddit_url(url)
}

/// The Atom feed that carries the content of a Reddit page: a thread's post
/// and up to 500 comments, a listing's posts, or search results.
pub fn to_feed_url(url: &str) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    let segments: Vec<String> = parsed
        .path_segments()
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    let comments = segments.iter().position(|s| s == "comments");
    let path = match (comments, segments.last().map(String::as_str)) {
        (_, Some(last)) if last.ends_with(".rss") => format!("/{}", segments.join("/")),
        (Some(i), _) if i + 1 < segments.len() => {
            parsed.set_query(Some("limit=500"));
            format!("/{}/.rss", segments[..=i + 1].join("/"))
        }
        (_, Some("search")) => format!("/{}.rss", segments.join("/")),
        _ if segments.is_empty() => "/.rss".to_string(),
        _ => format!("/{}/.rss", segments.join("/")),
    };
    parsed.set_path(&path);
    parsed.set_fragment(None);
    let _ = parsed.set_scheme("https");
    let _ = parsed.set_host(Some("www.reddit.com"));
    parsed.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_maps_to_its_comment_feed() {
        for url in [
            "https://www.reddit.com/r/rust/comments/abc123/some_title/",
            "https://old.reddit.com/r/rust/comments/abc123/some_title/?context=3#c",
            "https://reddit.com/r/rust/comments/abc123",
            "http://np.reddit.com/r/rust/comments/abc123/some_title/def456/",
        ] {
            assert_eq!(
                to_feed_url(url),
                "https://www.reddit.com/r/rust/comments/abc123/.rss?limit=500",
                "{url}"
            );
        }
    }

    #[test]
    fn listings_map_to_their_feed() {
        assert_eq!(
            to_feed_url("https://www.reddit.com/r/rust"),
            "https://www.reddit.com/r/rust/.rss"
        );
        assert_eq!(
            to_feed_url("https://old.reddit.com/r/AskReddit/top/?t=day"),
            "https://www.reddit.com/r/AskReddit/top/.rss?t=day"
        );
        assert_eq!(
            to_feed_url("https://www.reddit.com/user/someone/submitted/"),
            "https://www.reddit.com/user/someone/submitted/.rss"
        );
        assert_eq!(
            to_feed_url("https://reddit.com/"),
            "https://www.reddit.com/.rss"
        );
    }

    #[test]
    fn search_maps_to_search_feed() {
        assert_eq!(
            to_feed_url("https://www.reddit.com/search/?q=web+scraping&sort=new"),
            "https://www.reddit.com/search.rss?q=web+scraping&sort=new"
        );
        assert_eq!(
            to_feed_url("https://old.reddit.com/r/rust/search?q=async&restrict_sr=on"),
            "https://www.reddit.com/r/rust/search.rss?q=async&restrict_sr=on"
        );
    }

    #[test]
    fn feed_urls_are_unchanged() {
        for url in [
            "https://www.reddit.com/r/rust/comments/abc123/.rss?limit=500",
            "https://www.reddit.com/r/rust/new/.rss",
            "https://www.reddit.com/search.rss?q=x&sort=new",
        ] {
            assert_eq!(to_feed_url(url), url);
        }
    }
}
