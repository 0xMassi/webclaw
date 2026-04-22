//! Etsy listing extractor.
//!
//! Etsy product pages at `etsy.com/listing/{id}` (and a sluggy variant
//! `etsy.com/listing/{id}/{slug}`) ship a Schema.org `Product` JSON-LD
//! block with title, price, currency, availability, shop seller, and
//! an `AggregateRating` for the listing.
//!
//! Etsy puts Cloudflare + custom WAF in front of product pages with a
//! high variance: the Firefox profile gets clean HTML most of the time
//! but some listings return a CF interstitial. We route through
//! `cloud::smart_fetch_html` so both paths resolve to the same parser,
//! same as `ebay_listing`.
//!
//! ## URL slug as last-resort title
//!
//! Even with cloud antibot bypass, Etsy frequently serves a generic
//! page with minimal metadata (`og:title = "etsy.com"`, no JSON-LD,
//! empty markdown). In that case we humanise the slug from the URL
//! (`/listing/{id}/personalized-stainless-steel-tumbler` becomes
//! "Personalized Stainless Steel Tumbler") so callers always get a
//! meaningful title. Degrades gracefully when the URL has no slug.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::cloud::{self, CloudError};
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "etsy_listing",
    label: "Etsy listing",
    description: "Returns listing title, price, currency, availability, shop, rating, and image. Heavy listings may need WEBCLAW_API_KEY for antibot.",
    url_patterns: &[
        "https://www.etsy.com/listing/{id}",
        "https://www.etsy.com/listing/{id}/{slug}",
        "https://www.etsy.com/{locale}/listing/{id}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if !is_etsy_host(host) {
        return false;
    }
    parse_listing_id(url).is_some()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let listing_id = parse_listing_id(url)
        .ok_or_else(|| FetchError::Build(format!("etsy_listing: no listing id in '{url}'")))?;

    let fetched = cloud::smart_fetch_html(client, client.cloud(), url)
        .await
        .map_err(cloud_to_fetch_err)?;

    let mut data = parse(&fetched.html, url, &listing_id);
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

pub fn parse(html: &str, url: &str, listing_id: &str) -> Value {
    let jsonld = find_product_jsonld(html);
    let slug_title = humanise_slug(parse_slug(url).as_deref());

    let title = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "name"))
        .or_else(|| og(html, "title").filter(|t| !is_generic_title(t)))
        .or(slug_title);
    let description = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "description"))
        .or_else(|| og(html, "description").filter(|d| !is_generic_description(d)));
    let image = jsonld
        .as_ref()
        .and_then(get_first_image)
        .or_else(|| og(html, "image"));
    let brand = jsonld.as_ref().and_then(get_brand);

    // Etsy listings often ship either a single Offer or an
    // AggregateOffer when the listing has variants with different prices.
    let offer = jsonld.as_ref().and_then(first_offer);
    let (low_price, high_price, single_price) = match offer.as_ref() {
        Some(o) => (
            get_text(o, "lowPrice"),
            get_text(o, "highPrice"),
            get_text(o, "price"),
        ),
        None => (None, None, None),
    };
    let currency = offer.as_ref().and_then(|o| get_text(o, "priceCurrency"));
    let availability = offer
        .as_ref()
        .and_then(|o| get_text(o, "availability").map(strip_schema_prefix));
    let item_condition = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "itemCondition"))
        .map(strip_schema_prefix);

    // Shop name: offers[0].seller.name on newer listings, top-level
    // `brand` on older listings (Etsy changed the schema around 2022).
    // Fall back through both so either shape resolves.
    let shop = offer
        .as_ref()
        .and_then(|o| {
            o.get("seller")
                .and_then(|s| s.get("name"))
                .and_then(|n| n.as_str())
                .map(String::from)
        })
        .or_else(|| brand.clone());
    let shop_url = shop_url_from_html(html);

    let aggregate_rating = jsonld.as_ref().and_then(get_aggregate_rating);

    json!({
        "url":              url,
        "listing_id":       listing_id,
        "title":            title,
        "description":      description,
        "image":            image,
        "brand":            brand,
        "price":            single_price,
        "low_price":        low_price,
        "high_price":       high_price,
        "currency":         currency,
        "availability":     availability,
        "item_condition":   item_condition,
        "shop":             shop,
        "shop_url":         shop_url,
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

fn is_etsy_host(host: &str) -> bool {
    host == "etsy.com" || host == "www.etsy.com" || host.ends_with(".etsy.com")
}

/// Extract the numeric listing id. Etsy ids are 9-11 digits today but
/// we accept any all-digit segment right after `/listing/`.
///
/// Handles `/listing/{id}`, `/listing/{id}/{slug}`, and the localised
/// `/{locale}/listing/{id}` shape (e.g. `/fr/listing/...`).
fn parse_listing_id(url: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"/listing/(\d{6,})(?:[/?#]|$)").unwrap());
    re.captures(url)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract the URL slug after the listing id, e.g.
/// `personalized-stainless-steel-tumbler`. Returns `None` when the URL
/// is the bare `/listing/{id}` shape.
fn parse_slug(url: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"/listing/\d{6,}/([^/?#]+)").unwrap());
    re.captures(url)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// Turn a URL slug into a human-ish title:
/// `personalized-stainless-steel-tumbler` → `Personalized Stainless
/// Steel Tumbler`. Word-cap each dash-separated token; preserves
/// underscores as spaces too. Returns `None` on empty input.
fn humanise_slug(slug: Option<&str>) -> Option<String> {
    let raw = slug?.trim();
    if raw.is_empty() {
        return None;
    }
    let words: Vec<String> = raw
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(capitalise_word)
        .collect();
    if words.is_empty() {
        None
    } else {
        Some(words.join(" "))
    }
}

fn capitalise_word(w: &str) -> String {
    let mut chars = w.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// True when the OG title is Etsy's fallback-page title rather than a
/// listing-specific title. Expired / region-blocked / antibot-filtered
/// pages return Etsy's sitewide tagline:
/// `"Etsy - Your place to buy and sell all things handmade..."`, or
/// simply `"etsy.com"`. A real listing title always starts with the
/// item name, never with "Etsy - " or the domain.
fn is_generic_title(t: &str) -> bool {
    let normalised = t.trim().to_lowercase();
    if matches!(
        normalised.as_str(),
        "etsy.com" | "etsy" | "www.etsy.com" | ""
    ) {
        return true;
    }
    // Etsy's sitewide marketing tagline, served on 404 / blocked pages.
    if normalised.starts_with("etsy - ")
        || normalised.starts_with("etsy.com - ")
        || normalised.starts_with("etsy uk - ")
    {
        return true;
    }
    // Etsy's "item unavailable" placeholder, served on delisted
    // products. Keep the slug fallback so callers still see what the
    // URL was about.
    normalised.starts_with("this item is unavailable")
        || normalised.starts_with("sorry, this item is")
        || normalised == "item not available - etsy"
}

/// True when the OG description is an Etsy error-page placeholder or
/// sitewide marketing blurb rather than a real listing description.
fn is_generic_description(d: &str) -> bool {
    let normalised = d.trim().to_lowercase();
    if normalised.is_empty() {
        return true;
    }
    normalised.starts_with("sorry, the page you were looking for")
        || normalised.starts_with("page not found")
        || normalised.starts_with("find the perfect handmade gift")
}

// ---------------------------------------------------------------------------
// JSON-LD walkers (same shape as ebay_listing; kept separate so the two
// extractors can diverge without cross-impact)
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

fn strip_schema_prefix(s: String) -> String {
    s.replace("http://schema.org/", "")
        .replace("https://schema.org/", "")
}

fn og(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

/// Etsy links the owning shop with a canonical anchor like
/// `<a href="/shop/ShopName" ...>`. Grab the first one after the
/// breadcrumb boundary.
fn shop_url_from_html(html: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"href="(/shop/[A-Za-z0-9_-]+)""#).unwrap());
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| format!("https://www.etsy.com{}", m.as_str()))
}

fn cloud_to_fetch_err(e: CloudError) -> FetchError {
    FetchError::Build(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_etsy_listing_urls() {
        assert!(matches("https://www.etsy.com/listing/123456789"));
        assert!(matches(
            "https://www.etsy.com/listing/123456789/vintage-typewriter"
        ));
        assert!(matches(
            "https://www.etsy.com/fr/listing/123456789/vintage-typewriter"
        ));
        assert!(!matches("https://www.etsy.com/"));
        assert!(!matches("https://www.etsy.com/shop/SomeShop"));
        assert!(!matches("https://example.com/listing/123456789"));
    }

    #[test]
    fn parse_listing_id_handles_slug_and_locale() {
        assert_eq!(
            parse_listing_id("https://www.etsy.com/listing/123456789"),
            Some("123456789".into())
        );
        assert_eq!(
            parse_listing_id("https://www.etsy.com/listing/123456789/slug-here"),
            Some("123456789".into())
        );
        assert_eq!(
            parse_listing_id("https://www.etsy.com/fr/listing/123456789/slug"),
            Some("123456789".into())
        );
        assert_eq!(
            parse_listing_id("https://www.etsy.com/listing/123456789?ref=foo"),
            Some("123456789".into())
        );
    }

    #[test]
    fn parse_extracts_from_fixture_jsonld() {
        let html = r##"
<html><head>
<script type="application/ld+json">
{"@context":"https://schema.org","@type":"Product",
 "name":"Handmade Ceramic Mug","sku":"MUG-001",
 "brand":{"@type":"Brand","name":"Studio Clay"},
 "image":["https://i.etsystatic.com/abc.jpg","https://i.etsystatic.com/xyz.jpg"],
 "itemCondition":"https://schema.org/NewCondition",
 "offers":{"@type":"Offer","price":"24.00","priceCurrency":"USD",
           "availability":"https://schema.org/InStock",
           "seller":{"@type":"Organization","name":"StudioClay"}},
 "aggregateRating":{"@type":"AggregateRating","ratingValue":"4.9","reviewCount":"127","bestRating":"5"}}
</script>
<a href="/shop/StudioClay" class="wt-text-link">StudioClay</a>
</head></html>"##;
        let v = parse(html, "https://www.etsy.com/listing/1", "1");
        assert_eq!(v["title"], "Handmade Ceramic Mug");
        assert_eq!(v["price"], "24.00");
        assert_eq!(v["currency"], "USD");
        assert_eq!(v["availability"], "InStock");
        assert_eq!(v["item_condition"], "NewCondition");
        assert_eq!(v["shop"], "StudioClay");
        assert_eq!(v["shop_url"], "https://www.etsy.com/shop/StudioClay");
        assert_eq!(v["brand"], "Studio Clay");
        assert_eq!(v["aggregate_rating"]["rating_value"], "4.9");
        assert_eq!(v["aggregate_rating"]["review_count"], "127");
    }

    #[test]
    fn parse_handles_aggregate_offer_price_range() {
        let html = r##"
<script type="application/ld+json">
{"@type":"Product","name":"Mug Set",
 "offers":{"@type":"AggregateOffer",
           "lowPrice":"18.00","highPrice":"36.00","priceCurrency":"USD"}}
</script>
"##;
        let v = parse(html, "https://www.etsy.com/listing/2", "2");
        assert_eq!(v["low_price"], "18.00");
        assert_eq!(v["high_price"], "36.00");
        assert_eq!(v["currency"], "USD");
    }

    #[test]
    fn parse_falls_back_to_og_when_no_jsonld() {
        let html = r#"
<html><head>
<meta property="og:title" content="Minimal Fallback Item">
<meta property="og:description" content="OG-only extraction test.">
<meta property="og:image" content="https://i.etsystatic.com/fallback.jpg">
</head></html>"#;
        let v = parse(html, "https://www.etsy.com/listing/3", "3");
        assert_eq!(v["title"], "Minimal Fallback Item");
        assert_eq!(v["description"], "OG-only extraction test.");
        assert_eq!(v["image"], "https://i.etsystatic.com/fallback.jpg");
        // No price fields when we only have OG.
        assert!(v["price"].is_null());
    }

    #[test]
    fn parse_slug_from_url() {
        assert_eq!(
            parse_slug("https://www.etsy.com/listing/123456789/vintage-typewriter"),
            Some("vintage-typewriter".into())
        );
        assert_eq!(
            parse_slug("https://www.etsy.com/listing/123456789/slug?ref=shop"),
            Some("slug".into())
        );
        assert_eq!(parse_slug("https://www.etsy.com/listing/123456789"), None);
        assert_eq!(
            parse_slug("https://www.etsy.com/fr/listing/123456789/slug"),
            Some("slug".into())
        );
    }

    #[test]
    fn humanise_slug_capitalises_each_word() {
        assert_eq!(
            humanise_slug(Some("personalized-stainless-steel-tumbler")).as_deref(),
            Some("Personalized Stainless Steel Tumbler")
        );
        assert_eq!(
            humanise_slug(Some("hand_crafted_mug")).as_deref(),
            Some("Hand Crafted Mug")
        );
        assert_eq!(humanise_slug(Some("")), None);
        assert_eq!(humanise_slug(None), None);
    }

    #[test]
    fn is_generic_title_catches_common_shapes() {
        assert!(is_generic_title("etsy.com"));
        assert!(is_generic_title("Etsy"));
        assert!(is_generic_title("  etsy.com  "));
        assert!(is_generic_title(
            "Etsy - Your place to buy and sell all things handmade, vintage, and supplies"
        ));
        assert!(is_generic_title("Etsy UK - Vintage & Handmade"));
        assert!(!is_generic_title("Vintage Typewriter"));
        assert!(!is_generic_title("Handmade Etsy-style Mug"));
    }

    #[test]
    fn is_generic_description_catches_404_shapes() {
        assert!(is_generic_description(""));
        assert!(is_generic_description(
            "Sorry, the page you were looking for was not found."
        ));
        assert!(is_generic_description("Page not found"));
        assert!(!is_generic_description(
            "Hand-thrown ceramic mug, dishwasher safe."
        ));
    }

    #[test]
    fn parse_uses_slug_when_og_is_generic() {
        // Cloud-blocked Etsy listing: og:title is a site-wide generic
        // placeholder, no JSON-LD, no description. Slug should win.
        let html = r#"<html><head>
<meta property="og:title" content="etsy.com">
</head></html>"#;
        let v = parse(
            html,
            "https://www.etsy.com/listing/1079113183/personalized-stainless-steel-tumbler",
            "1079113183",
        );
        assert_eq!(v["title"], "Personalized Stainless Steel Tumbler");
    }

    #[test]
    fn parse_prefers_real_og_over_slug() {
        let html = r#"<html><head>
<meta property="og:title" content="Real Listing Title">
</head></html>"#;
        let v = parse(
            html,
            "https://www.etsy.com/listing/1079113183/the-url-slug",
            "1079113183",
        );
        assert_eq!(v["title"], "Real Listing Title");
    }
}
