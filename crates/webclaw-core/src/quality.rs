//! Conservative checks for empty pages and recognizable access/error screens.
//! These describe the returned content, not whether a target can be retrieved.
use crate::ExtractionResult;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ContentIssue {
    #[error("The target returned no readable page content.")]
    Empty,
    #[error("The target returned an access challenge instead of page content.")]
    AccessDenied,
    #[error("The target returned an error page instead of the requested content.")]
    Unavailable,
    #[error("The target requires sign-in to view the requested content.")]
    LoginRequired,
}

impl ContentIssue {
    pub fn code(self) -> &'static str {
        match self {
            Self::Empty => "empty_content",
            Self::AccessDenied => "bot_protection_unbypassed",
            Self::Unavailable => "upstream_error_page",
            Self::LoginRequired => "login_required",
        }
    }
}

/// Empty output is intentional only when the caller selected a DOM scope.
/// Main-content mode without a matching region falls back to automatic extraction.
pub fn allows_empty_content(html: &str, options: &crate::ExtractionOptions) -> bool {
    !options.include_selectors.is_empty()
        || (options.only_main_content
            && crate::extractor::has_main_content(&scraper::Html::parse_document(html)))
}

pub fn content_issue(result: &ExtractionResult) -> Option<ContentIssue> {
    let text = result
        .content
        .plain_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let title = result
        .metadata
        .title
        .as_deref()
        .unwrap_or("")
        .trim()
        .trim_end_matches(['.', '…', '!'])
        .to_lowercase();
    // Phrase mentions inside substantial articles are not error screens.
    if text.split_whitespace().count() < 200 {
        if matches!(
            title.as_str(),
            "just a moment" | "access denied" | "attention required" | "security check"
        ) || matches!(
            text.trim_end_matches(['.', '…', '!']),
            "please enable js and disable any ad blocker"
                | "please enable javascript and disable any ad blocker"
                | "verifying your connection"
        ) {
            return Some(ContentIssue::AccessDenied);
        }
        if text.contains("this is taking longer than usual") && text.contains("refresh") {
            return Some(ContentIssue::Unavailable);
        }
        if matches!(title.as_str(), "sign in" | "log in" | "login") {
            return Some(ContentIssue::LoginRequired);
        }
    }
    if !result.content.markdown.chars().any(char::is_alphanumeric) {
        return Some(ContentIssue::Empty);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract;

    #[test]
    fn main_mode_only_permits_empty_when_a_region_exists() {
        let options = crate::ExtractionOptions {
            only_main_content: true,
            ..Default::default()
        };
        assert!(allows_empty_content("<main></main>", &options));
        assert!(!allows_empty_content(
            "<body><div id='app'></div></body>",
            &options
        ));
        let options = crate::ExtractionOptions {
            exclude_selectors: vec![".cookie-banner".into()],
            ..Default::default()
        };
        assert!(!allows_empty_content("<div id='app'></div>", &options));
    }

    #[test]
    fn distinguishes_error_screens_from_short_pages_and_articles() {
        for (html, expected) in [
            ("<p>\u{200b}\u{feff}</p>", Some(ContentIssue::Empty)),
            (
                "<p>Please enable JS and disable any ad blocker</p>",
                Some(ContentIssue::AccessDenied),
            ),
            (
                "<h1>This is taking longer than usual</h1><p>Please refresh the page.</p>",
                Some(ContentIssue::Unavailable),
            ),
            (
                "<title>Sign in</title><p>Enter your password to continue.</p>",
                Some(ContentIssue::LoginRequired),
            ),
            ("<p>Hello world.</p>", None),
            (
                "<p>Verifying your connection to the database is the first troubleshooting step.</p>",
                None,
            ),
            (
                "<title>Access denied in Python</title><p>How to fix a permissions error.</p>",
                None,
            ),
        ] {
            assert_eq!(content_issue(&extract(html, None).unwrap()), expected);
        }
        let article = format!(
            "<title>Just a moment</title><article>{}</article>",
            "An article about this phrase. ".repeat(60)
        );
        assert_eq!(content_issue(&extract(&article, None).unwrap()), None);
    }
}
