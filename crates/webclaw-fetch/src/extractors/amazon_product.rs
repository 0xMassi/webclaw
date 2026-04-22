//! Amazon product detail page extractor.
//!
//! Amazon product pages (`/dp/{ASIN}/` on every locale) are
//! inconsistently protected. Sometimes our local TLS fingerprint gets
//! a real HTML page; sometimes we land on a CAPTCHA interstitial;
//! sometimes we land on a real page that for whatever reason ships
//! no Product JSON-LD (Amazon A/B-tests this regularly). So the
//! extractor has a two-stage fallback:
//!
//! 1. Try local fetch + parse. If we got Product JSON-LD back, great:
//!    we have everything (title, brand, price, availability, rating).
//! 2. If local fetch worked *but the page has no Product JSON-LD* AND
//!    a cloud client is configured, force-escalate to api.webclaw.io.
//!    Cloud's render + antibot pipeline reliably surfaces the
//!    structured data. Without a cloud client we return whatever we
//!    got from local (usually just title via `#productTitle` or OG
//!    meta tags).
//!
//! Parsing tries JSON-LD first, DOM regex (`#productTitle`,
//! `#landingImage`) second, OG `<meta>` tags third. The OG path
//! matters because the cloud's synthesized HTML ships metadata as
//! OG tags but lacks Amazon's DOM IDs.
//!
//! Auto-dispatch: we accept any amazon.* host with a `/dp/{ASIN}/`
//! path. ASINs are a stable Amazon identifier so we extract that as
//! part of the response even when everything else is empty (tells
//! callers the URL was at least recognised).

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::cloud::{self, CloudError};
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "amazon_product",
    label: "Amazon product",
    description: "Returns product detail: title, brand, price, currency, availability, rating, image, ASIN. Requires WEBCLAW_API_KEY — Amazon's antibot means we always go through the cloud.",
    url_patterns: &[
        "https://www.amazon.com/dp/{ASIN}",
        "https://www.amazon.co.uk/dp/{ASIN}",
        "https://www.amazon.de/dp/{ASIN}",
        "https://www.amazon.fr/dp/{ASIN}",
        "https://www.amazon.it/dp/{ASIN}",
        "https://www.amazon.es/dp/{ASIN}",
        "https://www.amazon.co.jp/dp/{ASIN}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if !is_amazon_host(host) {
        return false;
    }
    parse_asin(url).is_some()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let asin = parse_asin(url)
        .ok_or_else(|| FetchError::Build(format!("amazon_product: no ASIN in '{url}'")))?;

    let mut fetched = cloud::smart_fetch_html(client, client.cloud(), url)
        .await
        .map_err(cloud_to_fetch_err)?;

    // Amazon ships Product JSON-LD inconsistently even on non-CAPTCHA
    // pages (they A/B-test it). When local fetch succeeded but has no
    // Product JSON-LD, force-escalate to the cloud which runs the
    // render pipeline and reliably surfaces structured data. No-op
    // when cloud isn't configured — we return whatever local gave us.
    if fetched.source == cloud::FetchSource::Local
        && find_product_jsonld(&fetched.html).is_none()
        && let Some(c) = client.cloud()
    {
        match c.fetch_html(url).await {
            Ok(cloud_html) => {
                fetched = cloud::FetchedHtml {
                    html: cloud_html,
                    final_url: url.to_string(),
                    source: cloud::FetchSource::Cloud,
                };
            }
            Err(e) => {
                tracing::debug!(
                    error = %e,
                    "amazon_product: cloud escalation failed, keeping local"
                );
            }
        }
    }

    let mut data = parse(&fetched.html, url, &asin);
    if let Some(obj) = data.as_object_mut() {
        obj.insert(
            "data_source".into(),
            match fetched.source {
                cloud::FetchSource::Local => json!("local"),
                cloud::FetchSource::Cloud => json!("cloud"),
            },
        );
    }
    Ok(data)
}

/// Pure parser. Given HTML (from anywhere — direct, cloud, or a fixture
/// file) and the source URL, extract Amazon product detail. Returns a
/// `Value` rather than a typed struct so callers can pass it through
/// without carrying webclaw_fetch types.
pub fn parse(html: &str, url: &str, asin: &str) -> Value {
    let jsonld = find_product_jsonld(html);
    // Three-tier title: JSON-LD `name` > Amazon's `#productTitle` span
    // (only present on real static HTML) > cloud-synthesized og:title.
    let title = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "name"))
        .or_else(|| dom_title(html))
        .or_else(|| og(html, "title"));
    let image = jsonld
        .as_ref()
        .and_then(get_first_image)
        .or_else(|| dom_image(html))
        .or_else(|| og(html, "image"));
    let brand = jsonld.as_ref().and_then(get_brand);
    let description = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "description"))
        .or_else(|| og(html, "description"));
    let aggregate_rating = jsonld.as_ref().and_then(get_aggregate_rating);
    let offer = jsonld.as_ref().and_then(first_offer);

    let sku = jsonld.as_ref().and_then(|v| get_text(v, "sku"));
    let mpn = jsonld.as_ref().and_then(|v| get_text(v, "mpn"));

    json!({
        "url":              url,
        "asin":             asin,
        "title":            title,
        "brand":            brand,
        "description":      description,
        "image":            image,
        "price":            offer.as_ref().and_then(|o| get_text(o, "price")),
        "currency":         offer.as_ref().and_then(|o| get_text(o, "priceCurrency")),
        "availability":     offer.as_ref().and_then(|o| {
            get_text(o, "availability").map(|s|
                s.replace("http://schema.org/", "").replace("https://schema.org/", ""))
        }),
        "condition":        offer.as_ref().and_then(|o| {
            get_text(o, "itemCondition").map(|s|
                s.replace("http://schema.org/", "").replace("https://schema.org/", ""))
        }),
        "sku":              sku,
        "mpn":              mpn,
        "aggregate_rating": aggregate_rating,
    })
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

fn is_amazon_host(host: &str) -> bool {
    host.starts_with("www.amazon.") || host.starts_with("amazon.")
}

/// Pull a 10-char ASIN out of any recognised Amazon URL shape:
/// - /dp/{ASIN}
/// - /gp/product/{ASIN}
/// - /product/{ASIN}
/// - /exec/obidos/ASIN/{ASIN}
fn parse_asin(url: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r"/(?:dp|gp/product|product|ASIN)/([A-Z0-9]{10})(?:[/?#]|$)").unwrap()
    });
    re.captures(url)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

// ---------------------------------------------------------------------------
// JSON-LD walkers — light reuse of ecommerce_product's style
// ---------------------------------------------------------------------------

fn find_product_jsonld(html: &str) -> Option<Value> {
    let blocks = webclaw_core::structured_data::extract_json_ld(html);
    for b in blocks {
        if let Some(found) = find_product_in(&b) {
            return Some(found);
        }
    }
    None
}

fn find_product_in(v: &Value) -> Option<Value> {
    if is_product_type(v) {
        return Some(v.clone());
    }
    if let Some(graph) = v.get("@graph").and_then(|g| g.as_array()) {
        for item in graph {
            if let Some(found) = find_product_in(item) {
                return Some(found);
            }
        }
    }
    if let Some(arr) = v.as_array() {
        for item in arr {
            if let Some(found) = find_product_in(item) {
                return Some(found);
            }
        }
    }
    None
}

fn is_product_type(v: &Value) -> bool {
    let Some(t) = v.get("@type") else {
        return false;
    };
    let is_prod = |s: &str| matches!(s, "Product" | "ProductGroup" | "IndividualProduct");
    match t {
        Value::String(s) => is_prod(s),
        Value::Array(arr) => arr.iter().any(|x| x.as_str().is_some_and(is_prod)),
        _ => false,
    }
}

fn get_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| match x {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

fn get_brand(v: &Value) -> Option<String> {
    let brand = v.get("brand")?;
    if let Some(s) = brand.as_str() {
        return Some(s.to_string());
    }
    brand
        .as_object()
        .and_then(|o| o.get("name"))
        .and_then(|n| n.as_str())
        .map(String::from)
}

fn get_first_image(v: &Value) -> Option<String> {
    match v.get("image")? {
        Value::String(s) => Some(s.clone()),
        Value::Array(arr) => arr.iter().find_map(|x| match x {
            Value::String(s) => Some(s.clone()),
            Value::Object(_) => x.get("url").and_then(|u| u.as_str()).map(String::from),
            _ => None,
        }),
        Value::Object(o) => o.get("url").and_then(|u| u.as_str()).map(String::from),
        _ => None,
    }
}

fn first_offer(v: &Value) -> Option<Value> {
    let offers = v.get("offers")?;
    match offers {
        Value::Array(arr) => arr.first().cloned(),
        Value::Object(_) => Some(offers.clone()),
        _ => None,
    }
}

fn get_aggregate_rating(v: &Value) -> Option<Value> {
    let r = v.get("aggregateRating")?;
    Some(json!({
        "rating_value": get_text(r, "ratingValue"),
        "review_count": get_text(r, "reviewCount"),
        "best_rating":  get_text(r, "bestRating"),
    }))
}

// ---------------------------------------------------------------------------
// DOM fallbacks — cheap regex for the two fields most likely to be
// missing from JSON-LD on Amazon.
// ---------------------------------------------------------------------------

fn dom_title(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?s)id="productTitle"[^>]*>([^<]+)<"#).unwrap());
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

fn dom_image(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"id="landingImage"[^>]+src="([^"]+)""#).unwrap());
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// OG meta tag lookup. Cloud-synthesized HTML ships these even when
/// JSON-LD and Amazon-DOM-IDs are both absent, so they're the last
/// line of defence for `title`, `image`, `description`.
fn og(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| html_unescape(m.as_str()));
        }
    }
    None
}

/// Undo the synthesize_html attribute escaping for the few entities it
/// emits. Keeps us off a heavier HTML-entity dep.
fn html_unescape(s: &str) -> String {
    s.replace("&quot;", "\"")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
}

fn cloud_to_fetch_err(e: CloudError) -> FetchError {
    FetchError::Build(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_multi_locale() {
        assert!(matches("https://www.amazon.com/dp/B0CHX1W1XY"));
        assert!(matches("https://www.amazon.co.uk/dp/B0CHX1W1XY/"));
        assert!(matches("https://www.amazon.de/dp/B0CHX1W1XY?psc=1"));
        assert!(matches(
            "https://www.amazon.com/gp/product/B0CHX1W1XY/ref=foo"
        ));
    }

    #[test]
    fn rejects_non_product_urls() {
        assert!(!matches("https://www.amazon.com/"));
        assert!(!matches("https://www.amazon.com/gp/cart"));
        assert!(!matches("https://example.com/dp/B0CHX1W1XY"));
    }

    #[test]
    fn parse_asin_extracts_from_multiple_shapes() {
        assert_eq!(
            parse_asin("https://www.amazon.com/dp/B0CHX1W1XY"),
            Some("B0CHX1W1XY".into())
        );
        assert_eq!(
            parse_asin("https://www.amazon.com/dp/B0CHX1W1XY/"),
            Some("B0CHX1W1XY".into())
        );
        assert_eq!(
            parse_asin("https://www.amazon.com/dp/B0CHX1W1XY?psc=1"),
            Some("B0CHX1W1XY".into())
        );
        assert_eq!(
            parse_asin("https://www.amazon.com/gp/product/B0CHX1W1XY/ref=bar"),
            Some("B0CHX1W1XY".into())
        );
        assert_eq!(
            parse_asin("https://www.amazon.com/exec/obidos/ASIN/B0CHX1W1XY/baz"),
            Some("B0CHX1W1XY".into())
        );
        assert_eq!(parse_asin("https://www.amazon.com/"), None);
    }

    #[test]
    fn parse_extracts_from_fixture_jsonld() {
        // Minimal Amazon-style fixture with a Product JSON-LD block.
        let html = r##"
<html><head>
<script type="application/ld+json">
{"@context":"https://schema.org","@type":"Product",
 "name":"ACME Widget","sku":"B0CHX1W1XY",
 "brand":{"@type":"Brand","name":"ACME"},
 "image":"https://m.media-amazon.com/images/I/abc.jpg",
 "offers":{"@type":"Offer","price":"19.99","priceCurrency":"USD",
           "availability":"https://schema.org/InStock"},
 "aggregateRating":{"@type":"AggregateRating","ratingValue":"4.6","reviewCount":"1234"}}
</script>
</head><body></body></html>"##;
        let v = parse(html, "https://www.amazon.com/dp/B0CHX1W1XY", "B0CHX1W1XY");
        assert_eq!(v["asin"], "B0CHX1W1XY");
        assert_eq!(v["title"], "ACME Widget");
        assert_eq!(v["brand"], "ACME");
        assert_eq!(v["price"], "19.99");
        assert_eq!(v["currency"], "USD");
        assert_eq!(v["availability"], "InStock");
        assert_eq!(v["aggregate_rating"]["rating_value"], "4.6");
        assert_eq!(v["aggregate_rating"]["review_count"], "1234");
    }

    #[test]
    fn parse_falls_back_to_dom_when_jsonld_missing_fields() {
        let html = r#"
<html><body>
<span id="productTitle">Fallback Title</span>
<img id="landingImage" src="https://m.media-amazon.com/images/I/fallback.jpg" />
</body></html>
"#;
        let v = parse(html, "https://www.amazon.com/dp/B0CHX1W1XY", "B0CHX1W1XY");
        assert_eq!(v["title"], "Fallback Title");
        assert_eq!(
            v["image"],
            "https://m.media-amazon.com/images/I/fallback.jpg"
        );
    }

    #[test]
    fn parse_falls_back_to_og_meta_when_no_jsonld_no_dom() {
        // Shape we see from the cloud synthesize_html path: OG tags
        // only, no JSON-LD, no Amazon DOM IDs.
        let html = r##"<html><head>
<meta property="og:title" content="Cloud-sourced MacBook Pro">
<meta property="og:image" content="https://m.media-amazon.com/images/I/cloud.jpg">
<meta property="og:description" content="Via api.webclaw.io">
</head></html>"##;
        let v = parse(html, "https://www.amazon.com/dp/B0CHX1W1XY", "B0CHX1W1XY");
        assert_eq!(v["title"], "Cloud-sourced MacBook Pro");
        assert_eq!(v["image"], "https://m.media-amazon.com/images/I/cloud.jpg");
        assert_eq!(v["description"], "Via api.webclaw.io");
    }

    #[test]
    fn og_unescape_handles_quot_entity() {
        let html = r#"<meta property="og:title" content="Apple &quot;M2 Pro&quot; Laptop">"#;
        assert_eq!(
            og(html, "title").as_deref(),
            Some(r#"Apple "M2 Pro" Laptop"#)
        );
    }
}
