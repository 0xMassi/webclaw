//! Reddit thread structured extractor over the thread's Atom feed.
//!
//! Fetches the feed `crate::reddit::to_feed_url` names and delegates parsing
//! to `webclaw_core::reddit`, then fills comment scores from the thread's
//! server-rendered comment tree when Reddit serves it. Returns a typed JSON
//! value with `{ url, post, comments }` structure.

use serde_json::Value;

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "reddit",
    label: "Reddit thread",
    description: "Returns the post and up to 500 comments with authors and timestamps, and scores for the first page of comments.",
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
    let resp = client.fetch(&crate::reddit::to_feed_url(url)).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "reddit: unexpected status {}",
            resp.status
        )));
    }

    let mut thread =
        webclaw_core::reddit::try_extract_thread(&resp.html, url).ok_or_else(|| {
            FetchError::BodyDecode("reddit: no thread in the feed — is this a thread URL?".into())
        })?;
    if let Some(tree) = crate::reddit::to_comments_url(url)
        && let Ok(scored) = client.fetch(&tree).await
        && scored.status == 200
    {
        webclaw_core::reddit::apply_comment_scores(&mut thread, &scored.html);
    }

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

    struct RecordedReddit {
        tree_status: u16,
    }

    #[async_trait::async_trait]
    impl Fetcher for RecordedReddit {
        async fn fetch(&self, url: &str) -> Result<crate::client::FetchResult, FetchError> {
            let (file, status) = if url.contains("/svc/shreddit/") {
                ("thread-scored-comments.html", self.tree_status)
            } else {
                ("thread-scored.atom", 200)
            };
            Ok(crate::client::FetchResult {
                html: std::fs::read_to_string(format!(
                    "../webclaw-core/testdata/reddit-feed/{file}"
                ))
                .unwrap(),
                document_bytes: None,
                status,
                url: url.to_string(),
                headers: http::HeaderMap::new(),
                elapsed: std::time::Duration::ZERO,
            })
        }
    }

    const THREAD: &str = "https://www.reddit.com/r/rust/comments/1x9w6by/hey_rustaceans_got_an_easy_question_ask_here/";

    #[tokio::test]
    async fn thread_comments_carry_scores() {
        let v = extract(&RecordedReddit { tree_status: 200 }, THREAD)
            .await
            .unwrap();
        assert_eq!(v["comments"][2]["score"], 3);
        assert_eq!(v["comments"][5]["score"], 5);
        assert!(v["comments"][6]["score"].is_null());
    }

    #[tokio::test]
    async fn blocked_comment_tree_keeps_the_feed_thread() {
        let v = extract(&RecordedReddit { tree_status: 403 }, THREAD)
            .await
            .unwrap();
        assert_eq!(v["comments"].as_array().unwrap().len(), 7);
        assert!(v["comments"][2]["score"].is_null());
    }

    #[test]
    fn rejects_listing_and_non_reddit() {
        assert!(!matches("https://www.reddit.com/r/rust"));
        assert!(!matches("https://example.com/r/rust/comments/abc/x"));
    }
}
