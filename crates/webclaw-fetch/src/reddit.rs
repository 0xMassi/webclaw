//! Reddit URL helpers for the fetch layer.
//!
//! The JSON API (`*.json`) is blocked. We rewrite all Reddit hosts to
//! `old.reddit.com`, which serves stable server-rendered HTML that
//! `webclaw-core::reddit` parses directly.

pub fn is_reddit_url(url: &str) -> bool {
    webclaw_core::reddit::is_reddit_url(url)
}

/// Rewrite any Reddit host to old.reddit.com, preserving path and query.
pub fn to_old_reddit_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after = &url[scheme_end + 3..];
    let host_end = after.find(['/', '?', '#']).unwrap_or(after.len());
    let scheme = &url[..scheme_end + 3];
    let rest = &after[host_end..];
    format!("{scheme}old.reddit.com{rest}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_www_to_old() {
        assert_eq!(
            to_old_reddit_url("https://www.reddit.com/r/rust/comments/abc/x/"),
            "https://old.reddit.com/r/rust/comments/abc/x/"
        );
    }

    #[test]
    fn rewrites_bare_to_old() {
        assert_eq!(
            to_old_reddit_url("https://reddit.com/r/rust/"),
            "https://old.reddit.com/r/rust/"
        );
    }

    #[test]
    fn preserves_old_reddit_unchanged() {
        let url = "https://old.reddit.com/r/rust/comments/abc/x/?context=3";
        assert_eq!(to_old_reddit_url(url), url);
    }

    #[test]
    fn preserves_query_and_hash() {
        assert_eq!(
            to_old_reddit_url("https://www.reddit.com/r/rust/?sort=top#anchor"),
            "https://old.reddit.com/r/rust/?sort=top#anchor"
        );
    }
}
