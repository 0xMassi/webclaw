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

/// Words of text a browser would show in the page body before any script
/// runs: text inside `script`, `style`, `noscript`, `template` and `svg`
/// does not count.
pub fn visible_word_count(html: &str) -> usize {
    const HIDDEN: [&str; 5] = ["script", "style", "noscript", "template", "svg"];
    let document = scraper::Html::parse_document(html);
    let Some(body) = document
        .select(&scraper::Selector::parse("body").expect("valid selector"))
        .next()
    else {
        return 0;
    };
    body.descendants()
        .filter_map(|node| node.value().as_text().map(|text| (node, text)))
        .filter(|(node, _)| {
            !node.ancestors().any(|a| {
                a.value()
                    .as_element()
                    .is_some_and(|e| HIDDEN.contains(&e.name()))
            })
        })
        .map(|(_, text)| text.split_whitespace().count())
        .sum()
}

/// Empty output is intentional only when the caller selected a DOM scope.
/// Main-content mode without a matching region falls back to automatic extraction.
pub fn allows_empty_content(html: &str, options: &crate::ExtractionOptions) -> bool {
    !options.include_selectors.is_empty()
        || (options.only_main_content
            && crate::extractor::has_main_content(&scraper::Html::parse_document(html)))
}

const CONSENT_URL_FRAGMENTS: &[&str] = &[
    "://consent.",
    "/consent?",
    "/consent/",
    "collectconsent",
    "consentcheck",
    "/cmp/",
    "guce.advertising.com",
];

const CONSENT_TITLES: &[&str] = &[
    "before you continue",
    "your privacy choices",
    "we value your privacy",
    "we care about your privacy",
    "cookie consent",
    "consent required",
];

/// Whether the extraction is a GDPR/cookie consent wall instead of the
/// requested page.
///
/// True when `metadata.url` (set it to the final URL after redirects) points
/// at a known consent host or path, such as `consent.google.com`, or when the
/// title starts with an English consent-wall phrase and the page has at most
/// 50 words. Providers localize their consent pages, so the URL is the primary
/// signal and the title a best-effort fallback.
pub fn is_consent_wall(result: &ExtractionResult) -> bool {
    if let Some(ref url) = result.metadata.url {
        let lower = url.to_ascii_lowercase();
        if CONSENT_URL_FRAGMENTS
            .iter()
            .any(|fragment| lower.contains(fragment))
        {
            return true;
        }
    }

    if result.metadata.word_count <= 50
        && let Some(ref title) = result.metadata.title
    {
        let lower = title.to_lowercase();
        if CONSENT_TITLES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            return true;
        }
    }

    false
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
    fn visible_words_skip_scripts_styles_and_noscript() {
        let html = "<html><head><title>two words</title></head><body><p>one two</p><script>var a = 1;</script><style>p { color: red }</style><noscript>enable js</noscript><template>x y</template><svg><text>chart label</text></svg><div>three</div></body></html>";
        assert_eq!(visible_word_count(html), 3);
    }

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

    #[test]
    fn consent_walls_are_detected_by_final_url_or_short_titled_page() {
        let google = extract(
            "<title>Bevor Sie zu Google weitergehen</title><p>Wir verwenden Cookies und Daten.</p><button>Alle akzeptieren</button>",
            Some("https://consent.google.com/ml?continue=https://chromewebstore.google.com/&gl=DE&hl=de"),
        )
        .unwrap();
        assert!(is_consent_wall(&google));

        let short_english = extract(
            "<title>Before you continue to Google</title><p>We use cookies and data to deliver services.</p>",
            Some("https://www.google.com/"),
        )
        .unwrap();
        assert!(is_consent_wall(&short_english));

        let localized_elsewhere = extract(
            "<title>Bevor Sie zu Google weitergehen</title><p>Wir verwenden Cookies und Daten.</p>",
            Some("https://www.google.de/"),
        )
        .unwrap();
        assert!(!is_consent_wall(&localized_elsewhere));
    }

    #[test]
    fn privacy_pages_with_real_content_are_not_consent_walls() {
        let html = format!(
            "<title>Your privacy choices explained</title><article><h1>Privacy policy</h1><p>{}</p></article>",
            "This section explains how we collect, store and delete account data for customers. "
                .repeat(10)
        );
        let result = extract(&html, Some("https://example.com/privacy")).unwrap();
        assert!(result.metadata.word_count > 50);
        assert!(!is_consent_wall(&result));
    }
}
