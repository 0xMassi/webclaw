//! Reddit structured extractor — parses old.reddit.com HTML.
//!
//! Fetches old.reddit.com (stable server-rendered HTML, no JS required)
//! and delegates parsing to `webclaw_core::reddit`. Returns a typed JSON
//! value with `{ url, post, comments }` structure.

use serde_json::Value;

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "reddit",
    label: "Reddit thread",
    description: "Returns post + nested comment tree with scores, authors, and timestamps.",
    url_patterns: &[
        "https://www.reddit.com/r/*/comments/*",
        "https://reddit.com/r/*/comments/*",
        "https://old.reddit.com/r/*/comments/*",
    ],
};

pub fn matches(url: &str) -> bool {
    webclaw_core::reddit::is_reddit_url(url) && url.contains("/comments/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let fetch_url = crate::reddit::to_old_reddit_url(url);
    let resp = client.fetch(&fetch_url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "reddit: unexpected status {}",
            resp.status
        )));
    }

    let thread = webclaw_core::reddit::try_extract_thread(&resp.html, url).ok_or_else(|| {
        FetchError::BodyDecode(
            "reddit: page structure not recognised — is this a thread URL?".into(),
        )
    })?;

    serde_json::to_value(&thread)
        .map_err(|e| FetchError::BodyDecode(format!("reddit: serialisation error: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_thread_urls() {
        assert!(matches(
            "https://www.reddit.com/r/rust/comments/abc123/some_title/"
        ));
        assert!(matches("https://old.reddit.com/r/rust/comments/abc123/x/"));
        assert!(matches("https://reddit.com/r/rust/comments/abc/x"));
    }

    #[test]
    fn rejects_listing_and_non_reddit() {
        assert!(!matches("https://www.reddit.com/r/rust"));
        assert!(!matches("https://example.com/r/rust/comments/abc/x"));
    }
}
