//! API/endpoint surface discovery from HTML + JS bundle text.
//!
//! Pure and zero-network: callers fetch the page and its `<script src>`
//! bundles, then hand the raw text here. We surface API paths, absolute
//! API URLs, GraphQL and WebSocket endpoints that live in inline scripts
//! and bundles — the surface a sitemap/`map` can never see.
//!
//! Heuristic by design: regex over string literals, not JS dataflow.
//! High-signal patterns only; bounded for DoS safety.

use once_cell::sync::Lazy;
use regex::Regex;
use scraper::{Html, Selector};
use std::collections::BTreeSet;
use url::Url;

/// Hard caps so a hostile/huge bundle set can't blow up CPU or memory.
const MAX_SCAN_BYTES: usize = 8 * 1024 * 1024;
const MAX_ENDPOINTS: usize = 2000;
/// Cap on `<script src>` URLs returned for the caller to fetch.
const MAX_SCRIPT_SRCS: usize = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    RelativePath,
    AbsoluteUrl,
    GraphQl,
    WebSocket,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscoveredEndpoint {
    pub value: String,
    pub kind: EndpointKind,
    pub first_party: bool,
    /// `"inline"` or the bundle URL the match came from.
    pub source: String,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct EndpointReport {
    pub endpoints: Vec<DiscoveredEndpoint>,
    /// Distinct hosts seen across absolute URLs (first- and third-party).
    pub hosts: Vec<String>,
    pub bundles_scanned: usize,
    /// True if a cap was hit and results may be incomplete.
    pub truncated: bool,
}

// Quoted relative path that looks API-ish. Bounded quantifiers; the `regex`
// crate is linear-time (RE2) so this cannot catastrophically backtrack.
static RE_REL_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"["'`](/[A-Za-z0-9_\-./]{0,200}?(?:api|graphql|gql|/v[0-9]|/rest|/gateway|/internal|/discovery)[A-Za-z0-9_\-./]{0,200})["'`]"#,
    )
    .expect("RE_REL_PATH")
});

static RE_ABS_URL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"https?://[A-Za-z0-9.\-]{1,253}(?:/[A-Za-z0-9_\-./%]{0,400})?"#)
        .expect("RE_ABS_URL")
});

static RE_WS: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"wss?://[A-Za-z0-9.\-]{1,253}(?:/[A-Za-z0-9_\-./%]{0,256})?"#).expect("RE_WS")
});

static SCRIPT_SEL: Lazy<Selector> = Lazy::new(|| Selector::parse("script").expect("script sel"));

/// Common multi-label public suffixes so `ticketmaster.co.uk` resolves to
/// `ticketmaster.co.uk` (not `co.uk`). Not a full PSL — pragmatic v1.
const SUFFIX2: &[&str] = &[
    "co.uk", "org.uk", "gov.uk", "ac.uk", "me.uk", "com.au", "net.au", "org.au", "co.jp", "co.nz",
    "co.za", "com.br", "com.mx", "com.sg", "co.in", "co.kr", "com.tr", "com.cn",
];

fn registrable_domain(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() < 2 {
        return host;
    }
    let last2 = labels[labels.len() - 2..].join(".");
    if SUFFIX2.contains(&last2.as_str()) && labels.len() >= 3 {
        labels[labels.len() - 3..].join(".")
    } else {
        last2
    }
}

fn is_first_party(candidate_host: &str, base_reg: &str) -> bool {
    let ch = candidate_host.to_ascii_lowercase();
    ch == base_reg || ch.ends_with(&format!(".{base_reg}"))
}

/// Resolved absolute `<script src>` URLs (http/https only), deduped, capped.
/// Inline scripts have no `src` and are scanned via [`extract_endpoints`].
pub fn script_srcs(html: &str, base_url: &str) -> Vec<String> {
    let base = Url::parse(base_url).ok();
    let doc = Html::parse_document(html);
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for el in doc.select(&SCRIPT_SEL) {
        if out.len() >= MAX_SCRIPT_SRCS {
            break;
        }
        let Some(src) = el.value().attr("src") else {
            continue;
        };
        let resolved = match Url::parse(src) {
            Ok(u) => Some(u),
            Err(_) => base.as_ref().and_then(|b| b.join(src).ok()),
        };
        let Some(u) = resolved else {
            continue;
        };
        if (u.scheme() == "http" || u.scheme() == "https") && seen.insert(u.to_string()) {
            out.push(u.to_string());
        }
    }
    out
}

/// Extract endpoints from inline HTML scripts plus pre-fetched JS bundles.
/// `bundles` is `(bundle_url, bundle_text)`.
pub fn extract_endpoints(
    html: &str,
    base_url: &str,
    bundles: &[(String, String)],
) -> EndpointReport {
    let base_reg = Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(registrable_domain))
        .unwrap_or_default();

    let mut endpoints: Vec<DiscoveredEndpoint> = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    let mut hosts: BTreeSet<String> = BTreeSet::new();
    let mut budget = MAX_SCAN_BYTES;
    let mut truncated = false;

    let push = |value: String,
                kind: EndpointKind,
                source: &str,
                endpoints: &mut Vec<DiscoveredEndpoint>,
                seen: &mut BTreeSet<(String, String)>,
                hosts: &mut BTreeSet<String>|
     -> bool {
        if endpoints.len() >= MAX_ENDPOINTS {
            return false;
        }
        let first_party = match Url::parse(&value) {
            Ok(u) => {
                if let Some(h) = u.host_str() {
                    hosts.insert(h.to_ascii_lowercase());
                    is_first_party(h, &base_reg)
                } else {
                    false
                }
            }
            // Relative path: same origin as the page by definition.
            Err(_) => true,
        };
        if seen.insert((value.clone(), source.to_string())) {
            endpoints.push(DiscoveredEndpoint {
                value,
                kind,
                first_party,
                source: source.to_string(),
            });
        }
        true
    };

    let scan = |text: &str,
                source: &str,
                endpoints: &mut Vec<DiscoveredEndpoint>,
                seen: &mut BTreeSet<(String, String)>,
                hosts: &mut BTreeSet<String>,
                budget: &mut usize,
                truncated: &mut bool| {
        if *budget == 0 {
            return;
        }
        let slice = if text.len() > *budget {
            *truncated = true;
            &text[..*budget]
        } else {
            text
        };
        *budget -= slice.len();

        for c in RE_REL_PATH.captures_iter(slice) {
            if let Some(m) = c.get(1) {
                let v = m.as_str().to_string();
                let kind = if v.contains("graphql") || v.contains("/gql") {
                    EndpointKind::GraphQl
                } else {
                    EndpointKind::RelativePath
                };
                if !push(v, kind, source, endpoints, seen, hosts) {
                    *truncated = true;
                    return;
                }
            }
        }
        for m in RE_WS.find_iter(slice) {
            if !push(
                m.as_str().to_string(),
                EndpointKind::WebSocket,
                source,
                endpoints,
                seen,
                hosts,
            ) {
                *truncated = true;
                return;
            }
        }
        for m in RE_ABS_URL.find_iter(slice) {
            let v = m.as_str().to_string();
            // Skip obvious static assets — we want API surface, not CDN files.
            let lower = v.to_ascii_lowercase();
            if lower.ends_with(".js")
                || lower.ends_with(".css")
                || lower.ends_with(".png")
                || lower.ends_with(".jpg")
                || lower.ends_with(".svg")
                || lower.ends_with(".woff2")
            {
                // still record the host for visibility
                if let Some(h) = Url::parse(&v)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_string))
                {
                    hosts.insert(h.to_ascii_lowercase());
                }
                continue;
            }
            let kind = if lower.contains("graphql") || lower.contains("/gql") {
                EndpointKind::GraphQl
            } else {
                EndpointKind::AbsoluteUrl
            };
            if !push(v, kind, source, endpoints, seen, hosts) {
                *truncated = true;
                return;
            }
        }
    };

    // Inline scripts.
    let doc = Html::parse_document(html);
    let mut inline = String::new();
    for el in doc.select(&SCRIPT_SEL) {
        if el.value().attr("src").is_none() {
            inline.push_str(&el.text().collect::<String>());
            inline.push('\n');
        }
    }
    scan(
        &inline,
        "inline",
        &mut endpoints,
        &mut seen,
        &mut hosts,
        &mut budget,
        &mut truncated,
    );

    // Bundles.
    let mut bundles_scanned = 0usize;
    for (src, text) in bundles {
        if budget == 0 {
            truncated = true;
            break;
        }
        bundles_scanned += 1;
        scan(
            text,
            src,
            &mut endpoints,
            &mut seen,
            &mut hosts,
            &mut budget,
            &mut truncated,
        );
    }

    endpoints.sort_by(|a, b| (a.kind, &a.value, &a.source).cmp(&(b.kind, &b.value, &b.source)));

    EndpointReport {
        endpoints,
        hosts: hosts.into_iter().collect(),
        bundles_scanned,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registrable_domain_handles_cc_tlds() {
        assert_eq!(
            registrable_domain("www.ticketmaster.co.uk"),
            "ticketmaster.co.uk"
        );
        assert_eq!(
            registrable_domain("api.ticketmaster.com"),
            "ticketmaster.com"
        );
        assert_eq!(
            registrable_domain("pubapi.ticketmaster.co.uk"),
            "ticketmaster.co.uk"
        );
        assert_eq!(registrable_domain("localhost"), "localhost");
    }

    #[test]
    fn script_srcs_resolves_and_filters() {
        let html = r#"<html><head>
            <script src="/_next/static/chunks/main-abc.js"></script>
            <script src="https://cdn.example.net/lib.js"></script>
            <script>var inline = 1;</script>
            <script src="data:text/javascript,1"></script>
        </head></html>"#;
        let srcs = script_srcs(html, "https://www.ticketmaster.co.uk/");
        assert!(srcs.contains(
            &"https://www.ticketmaster.co.uk/_next/static/chunks/main-abc.js".to_string()
        ));
        assert!(srcs.contains(&"https://cdn.example.net/lib.js".to_string()));
        assert_eq!(srcs.len(), 2, "inline + data: ignored");
    }

    #[test]
    fn extracts_inline_and_bundle_endpoints_with_classification() {
        let html = r#"<html><body>
            <script>
              var cfg = { search: "/api/search/events", suggest: "/api/search/search-suggest" };
              fetch("/api/venue/info");
            </script>
            <script src="/app.js"></script>
        </body></html>"#;
        let bundles = vec![(
            "https://www.ticketmaster.co.uk/app.js".to_string(),
            r#"
              const GQL = "https://pubapi.ticketmaster.co.uk/graphql";
              axios.post("https://services.ticketmaster.co.uk/discovery/v2/events");
              new WebSocket("wss://live.ticketmaster.co.uk/socket");
              const ga = "https://www.googletagservices.com/tag/js/gpt.js";
              const img = "https://cdn.tmol.co/hero.png";
            "#
            .to_string(),
        )];
        let r = extract_endpoints(html, "https://www.ticketmaster.co.uk/", &bundles);
        let vals: Vec<&str> = r.endpoints.iter().map(|e| e.value.as_str()).collect();

        assert!(vals.contains(&"/api/search/events"));
        assert!(vals.contains(&"/api/search/search-suggest"));
        assert!(vals.contains(&"/api/venue/info"));
        assert!(vals.contains(&"https://pubapi.ticketmaster.co.uk/graphql"));
        assert!(vals.contains(&"https://services.ticketmaster.co.uk/discovery/v2/events"));
        assert!(vals.contains(&"wss://live.ticketmaster.co.uk/socket"));
        // static .js asset is not an endpoint, but its host is recorded
        assert!(!vals.contains(&"https://www.googletagservices.com/tag/js/gpt.js"));
        assert!(r.hosts.iter().any(|h| h == "www.googletagservices.com"));

        let gql = r
            .endpoints
            .iter()
            .find(|e| e.value.contains("graphql"))
            .unwrap();
        assert_eq!(gql.kind, EndpointKind::GraphQl);
        assert!(
            gql.first_party,
            "pubapi.ticketmaster.co.uk is first-party to .co.uk"
        );

        let third = r
            .endpoints
            .iter()
            .find(|e| e.value.starts_with("/api/venue"));
        assert!(third.unwrap().first_party, "relative path is same-origin");
        assert_eq!(r.bundles_scanned, 1);
    }

    #[test]
    fn third_party_absolute_is_flagged_not_first_party() {
        let bundles = vec![(
            "b".to_string(),
            r#"x="https://api.stripe.com/v1/charges""#.to_string(),
        )];
        let r = extract_endpoints("<html></html>", "https://www.ticketmaster.co.uk/", &bundles);
        let e = r
            .endpoints
            .iter()
            .find(|e| e.value.contains("stripe"))
            .unwrap();
        assert!(!e.first_party);
    }

    #[test]
    fn caps_bound_pathological_input() {
        // A huge blob of fake endpoints must not exceed MAX_ENDPOINTS and
        // must return promptly (regex crate is linear-time).
        let mut big = String::new();
        for i in 0..50_000 {
            big.push_str(&format!("\"/api/v1/item/{i}\" "));
        }
        let bundles = vec![("big".to_string(), big)];
        let r = extract_endpoints("<html></html>", "https://x.com/", &bundles);
        assert!(r.endpoints.len() <= MAX_ENDPOINTS);
        assert!(r.truncated);
    }

    #[test]
    fn empty_inputs_are_safe() {
        let r = extract_endpoints("", "not a url", &[]);
        assert!(r.endpoints.is_empty());
        assert_eq!(r.bundles_scanned, 0);
        assert!(!r.truncated);
    }
}
