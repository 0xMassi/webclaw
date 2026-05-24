/// Paywall HTML-signature detection (M11, iter 11).
///
/// Declarative registry of known paywall overlay markers (CSS class names,
/// data-attributes, element IDs) for major publishers. Detection is
/// host-gated and runs AFTER a successful fetch, scanning the raw HTML for
/// any registered marker that belongs to the responding host's suffix.
///
/// This is ADVISORY ONLY. Webclaw uses wreq for TLS impersonation and has
/// no headless browser, so true paywall bypass (cookie injection + JS
/// rendering + session auth) is not possible from this layer. When a
/// paywall is detected, the CLI emits a stderr warning:
///
///   `# webclaw: warning: paywall detected on <name> (<host>); full article
///    may not be accessible. Try --paywall-bypass or https://archive.is/<url>`
///
/// The `--paywall-bypass` flag is a best-effort attempt: it injects a
/// Googlebot User-Agent (some publishers serve full content to crawlers
/// for SEO). If detection still fires post-bypass, the stderr message
/// adds a note pointing the user at archive.is as an external fallback.
///
/// Host matching: `normalize_host(host).ends_with(host_suffix)` — so
/// `www.nytimes.com`, `nytimes.com`, `cooking.nytimes.com` all match the
/// `nytimes.com` entry. This is intentionally suffix-based (not exact
/// like M3 known-bad-sites) because paywalls span subdomains uniformly
/// within a publisher.
///
/// Marker matching: any-of substring scan on the html (case-sensitive,
/// since CSS class names and data-attribute values are spec'd case-
/// sensitive in HTML/CSS).
///
/// False-positive resistance: critical sentinel — detection MUST NOT
/// fire on example.com, BBC, Reuters, AP News, or any non-registered
/// host. The host gate is checked FIRST; if it doesn't match, the html
/// is never scanned. See `test_signature_only_fires_for_correct_host`.

/// One paywall signature entry. Static by construction.
#[derive(Debug, Clone, Copy)]
pub struct PaywallSignature {
    /// Human-readable publisher name. Used in the stderr warning.
    pub name: &'static str,
    /// Host suffix to match against the responding host (after `www.`
    /// stripping + lowercasing). Subdomain-tolerant: `nytimes.com`
    /// matches `cooking.nytimes.com`.
    pub host_suffix: &'static str,
    /// CSS classes, data-attributes, and element IDs whose presence in
    /// the response body indicates a paywall overlay. Any-of match: the
    /// signature fires when at least one marker is present.
    pub markers: &'static [&'static str],
}

/// Compile-time registry. Linear scan is fine at this size.
pub const PAYWALL_SIGNATURES: &[PaywallSignature] = &[
    PaywallSignature {
        name: "New York Times",
        host_suffix: "nytimes.com",
        // Observed live on www.nytimes.com/<date>/<slug>.html pages:
        //   - `vi-gateway-container` is the JS gateway div NYT injects
        //     around paywall-eligible content (verified iter-11 phase B).
        //   - `"isAccessibleForFree":false` is in the NewsArticle JSON-LD
        //     for metered articles.
        //   - `meteredContent` covers the CSS class + JSON-LD cssSelector
        //     references; appears on metered articles only.
        markers: &[
            "vi-gateway-container",
            "\"isAccessibleForFree\":false",
            "meteredContent",
        ],
    },
    PaywallSignature {
        name: "Wall Street Journal",
        host_suffix: "wsj.com",
        markers: &[
            "paywall-overlay",
            "wsj-paywall",
            "snippet-promotion",
        ],
    },
    PaywallSignature {
        name: "Financial Times",
        host_suffix: "ft.com",
        markers: &[
            "js-paywall",
            "subscribe-prompt",
            "data-trackable=\"paywall\"",
            "id=\"paywall-app\"",
        ],
    },
    PaywallSignature {
        name: "Bloomberg",
        host_suffix: "bloomberg.com",
        markers: &[
            "paywall-inline",
            "terminal-promo",
            "paywall-inline-promo",
        ],
    },
    PaywallSignature {
        name: "Substack",
        host_suffix: "substack.com",
        markers: &[
            "paywall-content",
            "subscribe-widget--paywall",
            "class=\"paywall\"",
        ],
    },
];

/// Normalize a host string for registry matching: lowercase + strip a
/// single leading `www.` label if present.
fn normalize_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    lower.strip_prefix("www.").map(|s| s.to_string()).unwrap_or(lower)
}

/// Detect a known paywall in the given html for the given host.
///
/// Returns the matching `PaywallSignature` or `None`. Two gates:
///   1. Host gate: normalized host must end with a registered `host_suffix`.
///   2. Marker gate: html must contain at least one of the entry's markers.
///
/// Both gates must pass. Pure function; no I/O.
pub fn detect_in_html(host: &str, html: &str) -> Option<&'static PaywallSignature> {
    let normalized = normalize_host(host);
    for sig in PAYWALL_SIGNATURES {
        if normalized.ends_with(sig.host_suffix)
            && sig.markers.iter().any(|m| html.contains(m))
        {
            return Some(sig);
        }
    }
    None
}

/// Googlebot User-Agent string used by `--paywall-bypass`. Some publishers
/// serve full content to Googlebot for SEO indexing. This is a best-effort
/// soft bypass — many publishers verify the request actually comes from a
/// Google-owned IP, in which case this header alone does nothing.
pub const GOOGLEBOT_USER_AGENT: &str =
    "Mozilla/5.0 (compatible; Googlebot/2.1; +http://www.google.com/bot.html)";

/// Format the stderr warning for a paywall detection. Phase A contract:
///
///   `# webclaw: warning: paywall detected on <name> (<host>); full article
///    may not be accessible. Try --paywall-bypass or https://archive.is/<url>`
///
/// `bypass_attempted=true` (called from the `--paywall-bypass` path when
/// detection STILL fires) appends a note that the soft bypass attempt did
/// not clear the paywall and the archive.is fallback is the next step.
pub fn format_warning(sig: &PaywallSignature, host: &str, url: &str, bypass_attempted: bool) -> String {
    let normalized = normalize_host(host);
    if bypass_attempted {
        format!(
            "# webclaw: warning: paywall still detected on {name} ({host}) after --paywall-bypass attempt (Googlebot UA); try https://archive.is/{url}",
            name = sig.name,
            host = normalized,
            url = url,
        )
    } else {
        format!(
            "# webclaw: warning: paywall detected on {name} ({host}); full article may not be accessible. Try --paywall-bypass or https://archive.is/{url}",
            name = sig.name,
            host = normalized,
            url = url,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_signature_matches_nyt_marker_in_html() {
        // vi-gateway-container is the NYT JS paywall gateway div, observed
        // live on www.nytimes.com/<date>/<slug>.html pages (iter-11 phase B).
        let html = r#"<html><body><div class="vi-gateway-container" data-testid="vi-gateway-container"></div></body></html>"#;
        let hit = detect_in_html("www.nytimes.com", html).expect("nyt should match");
        assert_eq!(hit.name, "New York Times");
        assert_eq!(hit.host_suffix, "nytimes.com");
    }

    #[test]
    fn test_signature_matches_nyt_jsonld_marker() {
        // "isAccessibleForFree":false is the schema.org JSON-LD signal NYT
        // emits in NewsArticle structured data for metered articles.
        // Critically, ":true" (the inverse — free content) must NOT match.
        let html = r#"<script type="application/ld+json">{"@type":"NewsArticle","isAccessibleForFree":false}</script>"#;
        let hit = detect_in_html("www.nytimes.com", html).expect("nyt jsonld marker should match");
        assert_eq!(hit.name, "New York Times");

        // Negative: explicit "isAccessibleForFree":true must NOT fire.
        let free_html = r#"<script>{"isAccessibleForFree":true}</script>"#;
        assert!(detect_in_html("www.nytimes.com", free_html).is_none(),
            "free articles must not trigger the paywall marker");
    }

    #[test]
    fn test_signature_matches_nyt_subdomain() {
        // Subdomain coverage: cooking.nytimes.com should match the nytimes.com suffix.
        let html = r#"<div class="meteredContent">limit</div>"#;
        let hit = detect_in_html("cooking.nytimes.com", html).expect("nyt subdomain should match");
        assert_eq!(hit.name, "New York Times");
    }

    #[test]
    fn test_signature_matches_wsj_marker() {
        let html = r#"<div class="wsj-paywall snippet-promotion"></div>"#;
        let hit = detect_in_html("www.wsj.com", html).expect("wsj should match");
        assert_eq!(hit.name, "Wall Street Journal");
    }

    #[test]
    fn test_signature_matches_ft_marker() {
        let html = r#"<div class="js-paywall"></div>"#;
        let hit = detect_in_html("www.ft.com", html).expect("ft should match");
        assert_eq!(hit.name, "Financial Times");
    }

    #[test]
    fn test_signature_matches_bloomberg_marker() {
        let html = r#"<div class="paywall-inline-promo">subscribe</div>"#;
        let hit = detect_in_html("www.bloomberg.com", html).expect("bloomberg should match");
        assert_eq!(hit.name, "Bloomberg");
    }

    #[test]
    fn test_signature_matches_substack_per_publisher_subdomain() {
        // Substack publishers use <name>.substack.com — subdomain suffix coverage.
        let html = r#"<div class="paywall-content">subscribe</div>"#;
        let hit = detect_in_html("someblog.substack.com", html).expect("substack should match");
        assert_eq!(hit.name, "Substack");
    }

    #[test]
    fn test_signature_skips_clean_html() {
        // example.com: registered NEITHER as host nor marker source.
        let clean_html = r#"<html><body><h1>Example Domain</h1><p>For use in examples.</p></body></html>"#;
        assert!(detect_in_html("example.com", clean_html).is_none());
        assert!(detect_in_html("www.example.com", clean_html).is_none());

        // nytimes.com host but NO marker in html — host gate passes,
        // marker gate fails. Must NOT fire.
        assert!(detect_in_html("www.nytimes.com", clean_html).is_none());

        // BBC: never registered. Must NOT fire even with the same generic html.
        assert!(detect_in_html("www.bbc.com", clean_html).is_none());
        // Reuters: never registered.
        assert!(detect_in_html("www.reuters.com", clean_html).is_none());
        // AP News: never registered.
        assert!(detect_in_html("apnews.com", clean_html).is_none());
    }

    #[test]
    fn test_signature_only_fires_for_correct_host() {
        // CRITICAL false-positive sentinel: an html string containing a
        // paywall marker for a NON-RESPONDING host must NOT trigger
        // detection. The host gate is the first defense.
        let html_with_nyt_marker =
            r#"<div class="vi-gateway-container">subscribe</div>"#;
        assert!(detect_in_html("example.com", html_with_nyt_marker).is_none());
        assert!(detect_in_html("www.bbc.com", html_with_nyt_marker).is_none());
        assert!(detect_in_html("apnews.com", html_with_nyt_marker).is_none());

        // And cross-publisher: a WSJ marker should not match an NYT host.
        let html_with_wsj_marker = r#"<div class="paywall-overlay"></div>"#;
        assert!(detect_in_html("www.nytimes.com", html_with_wsj_marker).is_none());
    }

    #[test]
    fn test_format_warning_message_shape() {
        let sig = &PAYWALL_SIGNATURES[0]; // NYT
        let msg = format_warning(sig, "www.nytimes.com", "https://www.nytimes.com/x", false);
        assert!(msg.starts_with("# webclaw: warning: paywall detected on New York Times"), "msg: {msg}");
        assert!(msg.contains("(nytimes.com)"), "normalized host expected: {msg}");
        assert!(msg.contains("--paywall-bypass"), "bypass hint expected: {msg}");
        assert!(msg.contains("https://archive.is/https://www.nytimes.com/x"), "archive.is suggestion expected: {msg}");
    }

    #[test]
    fn test_format_warning_bypass_attempted_includes_archive_is() {
        let sig = &PAYWALL_SIGNATURES[0]; // NYT
        let msg = format_warning(sig, "www.nytimes.com", "https://www.nytimes.com/x", true);
        assert!(msg.contains("after --paywall-bypass attempt"), "should note bypass attempt: {msg}");
        assert!(msg.contains("Googlebot UA"), "should name the UA strategy: {msg}");
        assert!(msg.contains("https://archive.is/https://www.nytimes.com/x"), "archive.is suggestion expected: {msg}");
    }

    #[test]
    fn test_googlebot_ua_constant() {
        // Pin the exact string so test_paywall_bypass_flag_sets_googlebot_ua
        // in the CLI tests has a known-good reference value.
        assert!(GOOGLEBOT_USER_AGENT.contains("Googlebot/2.1"));
        assert!(GOOGLEBOT_USER_AGENT.starts_with("Mozilla/5.0"));
    }

    #[test]
    fn test_normalize_host_lowercases_and_strips_www() {
        // Belt-and-braces: even if upstream code passes a non-normalized
        // host, detection still works.
        let html = r#"<div class="vi-gateway-container"></div>"#;
        assert!(detect_in_html("WWW.NYTIMES.COM", html).is_some(), "all-caps with www should match");
        assert!(detect_in_html("NYTimes.com", html).is_some(), "mixed-case should match");
    }
}
