/// Known-bad-sites registry (M3, iter 3).
///
/// Declarative list of hosts that webclaw cannot usefully fetch — Cloudflare
/// interstitials, JS+adblock walls, and (eventually) hard paywalls. Checked
/// BEFORE any DNS resolution or HTTP request, so the registered hosts
/// short-circuit with a stderr message naming a substitute domain rather than
/// burning wall-clock on a doomed fetch.
///
/// Initial entries (phase A measured pre-baseline, see
/// `baselines/iter-3-pre-baseline.json`):
///   - `ambito.com` — Cloudflare "Just a moment..." interstitial. Pre-M3:
///     exit 0, 75 B stdout (metadata only), 218 ms. Chrome retry does not
///     bypass.
///   - `liberation.fr` — JS + adblock wall. Pre-M3: exit 0, 148 B stub
///     ("Please enable JS and disable any ad blocker"), 344 ms, silent
///     stderr.
///
/// WSJ / FT / Bloomberg / NYT are explicitly DEFERRED to a later milestone
/// (M11) because hard paywalls behave differently and the substitute logic
/// is different.
///
/// Host matching:
///   `lowercase(strip_leading_www(url.host))` then exact-match against the
///   normalized `host` field of each registry entry. So `ambito.com`,
///   `www.ambito.com`, and `Ambito.COM` all collapse to `ambito.com` and
///   hit the same entry. Subpaths (`/economia/`) match because the
///   comparison is host-only.
///
/// IDN / punycode (e.g. the Spanish display name "Ámbito") is not handled
/// this iter — the actual DNS for ambito.com is plain ASCII. If a future
/// entry needs IDN, switch to `url::Host` matching.
use std::fmt;

/// Why a host is registered as bad. Determines the `<category>` segment of
/// the stderr error line: `error: <host> is <category>-walled; ...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BadSiteCategory {
    /// Cloudflare "Just a moment..." interstitial / challenge page.
    Cloudflare,
    /// JS + adblock wall (page renders an "enable adblock disable" stub).
    Adblock,
    /// Reserved for M11 (NYT/WSJ/FT/Bloomberg). Not used by any current
    /// registry entry — kept in the enum so the matching/formatting code
    /// already covers the variant when M11 lands.
    #[allow(dead_code)]
    HardPaywall,
}

impl fmt::Display for BadSiteCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            BadSiteCategory::Cloudflare => "cloudflare",
            BadSiteCategory::Adblock => "adblock",
            BadSiteCategory::HardPaywall => "paywall",
        };
        f.write_str(s)
    }
}

/// One registry entry. `host` is the normalized form (lowercase, no `www.`).
#[derive(Debug, Clone, Copy)]
pub struct KnownBadSite {
    /// Normalized host: lowercase, no leading `www.`.
    pub host: &'static str,
    pub category: BadSiteCategory,
    /// Suggested alternative domains the caller can try instead. Order
    /// matters: the first is the strongest recommendation.
    pub substitutes: &'static [&'static str],
    /// Human-readable note explaining why this host is registered. Not
    /// emitted to stderr by default but available to library callers.
    #[allow(dead_code)]
    pub reason: &'static str,
}

/// Compile-time registry. Linear scan is fine at this size; swap to a
/// `phf` perfect-hash if it grows past ~50 entries.
pub const KNOWN_BAD_SITES: &[KnownBadSite] = &[
    KnownBadSite {
        host: "ambito.com",
        category: BadSiteCategory::Cloudflare,
        substitutes: &["cronista.com", "iprofesional.com"],
        reason: "Cloudflare 'Just a moment...' interstitial; chrome retry does not bypass",
    },
    KnownBadSite {
        host: "liberation.fr",
        category: BadSiteCategory::Adblock,
        substitutes: &["lemonde.fr", "lepoint.fr"],
        reason: "JS + adblock wall; returns 148-byte stub asking to disable adblock",
    },
];

/// Normalize a host string for registry matching: lowercase, strip a single
/// leading `www.` label if present. Returns owned `String` because the
/// lowercase operation may allocate.
fn normalize_host(host: &str) -> String {
    let lower = host.to_ascii_lowercase();
    lower.strip_prefix("www.").map(|s| s.to_string()).unwrap_or(lower)
}

/// Check whether `url` is a registered known-bad host. Returns the matching
/// entry or `None`. Accepts a full URL string; parsing failures yield `None`
/// (the caller should hit its normal "invalid URL" path).
pub fn check(url: &str) -> Option<&'static KnownBadSite> {
    let parsed = url::Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let normalized = normalize_host(host);
    KNOWN_BAD_SITES.iter().find(|entry| entry.host == normalized)
}

/// Format the stderr error line for a registry hit. Phase A's contract:
///
///   `error: <host> is <category>-walled; suggested substitute: <a>, <b>`
///
/// `<host>` is the normalized host (so even if the caller passed
/// `https://WWW.Ambito.COM/economia/` we emit `ambito.com`). `<category>`
/// is the lowercase `Display` form of the enum. `requested_url` is accepted
/// for future use (e.g. echoing the caller's URL in a debug-level field);
/// it's intentionally unused in the canonical one-liner so probe.py's regex
/// stays simple.
pub fn format_fail_message(site: &KnownBadSite, _requested_url: &str) -> String {
    format!(
        "error: {host} is {category}-walled; suggested substitute: {subs}",
        host = site.host,
        category = site.category,
        subs = site.substitutes.join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_matches_ambito_root() {
        let hit = check("https://www.ambito.com/").expect("ambito.com should be in registry");
        assert_eq!(hit.host, "ambito.com");
        assert_eq!(hit.category, BadSiteCategory::Cloudflare);
    }

    #[test]
    fn test_registry_matches_ambito_path() {
        // Host-only match: any path under a registered host fires.
        let hit = check("https://www.ambito.com/economia/")
            .expect("ambito subpath should still match");
        assert_eq!(hit.host, "ambito.com");
    }

    #[test]
    fn test_registry_matches_ambito_without_www() {
        // www stripping: bare apex matches the same entry as the www form.
        let hit = check("https://ambito.com/")
            .expect("bare apex ambito.com should match");
        assert_eq!(hit.host, "ambito.com");
    }

    #[test]
    fn test_registry_matches_liberation_subpath() {
        let hit = check("https://www.liberation.fr/culture/cinema/")
            .expect("liberation subpath should match");
        assert_eq!(hit.host, "liberation.fr");
        assert_eq!(hit.category, BadSiteCategory::Adblock);
    }

    #[test]
    fn test_registry_skips_unknown_domain() {
        assert!(check("https://example.com/").is_none());
        // Also reject the "lookalike + word" false-positive — we want
        // EXACT host match after normalization, not substring matching.
        assert!(check("https://evilambito.com/").is_none());
    }

    #[test]
    fn test_registry_case_insensitive() {
        // All-caps scheme + host. url::Url already lowercases scheme/host
        // on parse, but our normalize_host belt-and-braces it anyway.
        let hit = check("HTTPS://AMBITO.COM/").expect("uppercase host should match");
        assert_eq!(hit.host, "ambito.com");

        // Mixed case with www prefix.
        let hit2 = check("https://WWW.Ambito.com/").expect("mixed-case www should match");
        assert_eq!(hit2.host, "ambito.com");
    }

    #[test]
    fn test_format_fail_message_includes_substitutes() {
        let site = check("https://www.ambito.com/").unwrap();
        let msg = format_fail_message(site, "https://www.ambito.com/");
        assert!(msg.contains("ambito.com"), "msg should contain normalized host: {msg}");
        assert!(msg.contains("cloudflare-walled"), "category segment expected: {msg}");
        assert!(msg.contains("cronista.com"), "first substitute missing: {msg}");
        assert!(msg.contains("iprofesional.com"), "second substitute missing: {msg}");
    }

    #[test]
    fn test_format_fail_message_liberation_shape() {
        let site = check("https://www.liberation.fr/culture/cinema/").unwrap();
        let msg = format_fail_message(site, "https://www.liberation.fr/culture/cinema/");
        assert_eq!(
            msg,
            "error: liberation.fr is adblock-walled; suggested substitute: lemonde.fr, lepoint.fr"
        );
    }

    #[test]
    fn test_check_returns_none_on_invalid_url() {
        // Garbage input should not panic; we expect None so the caller
        // falls through to its normal invalid-URL handling.
        assert!(check("not a url at all").is_none());
        assert!(check("").is_none());
    }
}
