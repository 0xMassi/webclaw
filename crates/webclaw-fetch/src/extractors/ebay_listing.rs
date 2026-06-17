//! eBay listing extractor.
//!
//! eBay item pages at `ebay.com/itm/{id}` and international variants
//! usually ship a `Product` JSON-LD block with title, price, currency,
//! condition, and an `AggregateOffer` when bidding. eBay applies
//! Cloudflare + custom WAF selectively — some item IDs return normal
//! HTML to the Firefox profile, others 403 / get the "Pardon our
//! interruption" page. We route through `cloud::smart_fetch_html` so
//! both paths resolve to the same parser.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use super::host_of;
use super::jsonld_product::{
    find_product_jsonld, first_offer, get_aggregate_rating, get_brand, get_first_image, get_text,
};
use super::og::parse_og;
use crate::cloud::{self, CloudError};
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "ebay_listing",
    label: "eBay listing",
    description: "Returns item title, price, currency, condition, seller, shipping, and bid info. Heavy listings may need WEBCLAW_API_KEY for antibot.",
    url_patterns: &[
        "https://www.ebay.com/itm/{id}",
        "https://www.ebay.co.uk/itm/{id}",
        "https://www.ebay.de/itm/{id}",
        "https://www.ebay.fr/itm/{id}",
        "https://www.ebay.it/itm/{id}",
    ],
};

pub fn matches(url: &str) -> bool {
    let Some(host) = host_of(url) else {
        return false;
    };
    if !is_ebay_host(&host) {
        return false;
    }
    parse_item_id(url).is_some()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let item_id = parse_item_id(url)
        .ok_or_else(|| FetchError::Build(format!("ebay_listing: no item id in '{url}'")))?;

    let fetched = cloud::smart_fetch_html(client, client.cloud(), url)
        .await
        .map_err(cloud_to_fetch_err)?;

    let mut data = parse(&fetched.html, url, &item_id);
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

pub fn parse(html: &str, url: &str, item_id: &str) -> Value {
    let jsonld = find_product_jsonld(html);
    // Single scan for the three og:* fields read as fallbacks below.
    let og_meta = parse_og(html);
    let title = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "name"))
        .or_else(|| og_meta.raw("title"));
    let image = jsonld
        .as_ref()
        .and_then(get_first_image)
        .or_else(|| og_meta.raw("image"));
    let brand = jsonld.as_ref().and_then(get_brand);
    let description = jsonld
        .as_ref()
        .and_then(|v| get_text(v, "description"))
        .or_else(|| og_meta.raw("description"));
    let offer = jsonld.as_ref().and_then(first_offer);

    // eBay's AggregateOffer uses lowPrice/highPrice. Offer uses price.
    let (low_price, high_price, single_price) = match offer.as_ref() {
        Some(o) => (
            get_text(o, "lowPrice"),
            get_text(o, "highPrice"),
            get_text(o, "price"),
        ),
        None => (None, None, None),
    };
    let offer_count = offer.as_ref().and_then(|o| get_text(o, "offerCount"));

    let aggregate_rating = jsonld.as_ref().and_then(get_aggregate_rating);

    json!({
        "url":             url,
        "item_id":         item_id,
        "title":           title,
        "brand":           brand,
        "description":     description,
        "image":           image,
        "price":           single_price,
        "low_price":       low_price,
        "high_price":      high_price,
        "offer_count":     offer_count,
        "currency":        offer.as_ref().and_then(|o| get_text(o, "priceCurrency")),
        "availability":    offer.as_ref().and_then(|o| {
            get_text(o, "availability").map(|s|
                s.replace("http://schema.org/", "").replace("https://schema.org/", ""))
        }),
        "condition":       offer.as_ref().and_then(|o| {
            get_text(o, "itemCondition").map(|s|
                s.replace("http://schema.org/", "").replace("https://schema.org/", ""))
        }),
        "seller":          offer.as_ref().and_then(|o|
            o.get("seller").and_then(|s| s.get("name")).and_then(|n| n.as_str()).map(String::from)),
        "aggregate_rating": aggregate_rating,
    })
}

// ---------------------------------------------------------------------------
// URL helpers
// ---------------------------------------------------------------------------

fn is_ebay_host(host: &str) -> bool {
    const EBAY_HOSTS: &[&str] = &[
        "ebay.at",
        "ebay.be",
        "ebay.ca",
        "ebay.ch",
        "ebay.co.uk",
        "ebay.com",
        "ebay.com.au",
        "ebay.com.hk",
        "ebay.com.my",
        "ebay.com.sg",
        "ebay.de",
        "ebay.es",
        "ebay.fr",
        "ebay.ie",
        "ebay.it",
        "ebay.nl",
        "ebay.ph",
        "ebay.pl",
    ];
    let normalized = host.strip_prefix("www.").unwrap_or(host);
    EBAY_HOSTS.contains(&normalized)
}

/// Pull the numeric item id out of `/itm/{id}` or `/itm/{slug}/{id}`
/// URLs. IDs are 10-15 digits today, but we accept any all-digit
/// trailing segment so the extractor stays forward-compatible.
fn parse_item_id(url: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        // /itm/(optional-slug/)?(digits)([/?#]|end)
        Regex::new(r"/itm/(?:[^/]+/)?(\d{8,})(?:[/?#]|$)").unwrap()
    });
    re.captures(url)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

fn cloud_to_fetch_err(e: CloudError) -> FetchError {
    FetchError::Build(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ebay_item_urls() {
        assert!(matches("https://www.ebay.com/itm/325478156234"));
        assert!(matches(
            "https://www.ebay.com/itm/vintage-typewriter/325478156234"
        ));
        assert!(matches("https://www.ebay.co.uk/itm/325478156234"));
        assert!(matches("https://www.ebay.ca/itm/325478156234"));
        assert!(matches("https://www.ebay.com.au/itm/325478156234"));
        assert!(matches("https://www.ebay.es/itm/325478156234"));
        assert!(!matches("https://www.ebay.com/"));
        assert!(!matches("https://www.ebay.com/sch/foo"));
        assert!(!matches("https://example.com/itm/325478156234"));
        assert!(!matches("https://www.ebay.com@127.0.0.1/itm/325478156234"));
        assert!(!matches("https://www.ebay.attacker.com/itm/325478156234"));
    }

    #[test]
    fn parse_item_id_handles_slugged_urls() {
        assert_eq!(
            parse_item_id("https://www.ebay.com/itm/325478156234"),
            Some("325478156234".into())
        );
        assert_eq!(
            parse_item_id("https://www.ebay.com/itm/vintage-typewriter/325478156234"),
            Some("325478156234".into())
        );
        assert_eq!(
            parse_item_id("https://www.ebay.com/itm/325478156234?hash=abc"),
            Some("325478156234".into())
        );
    }

    #[test]
    fn parse_extracts_from_fixture_jsonld() {
        let html = r##"
<html><head>
<script type="application/ld+json">
{"@context":"https://schema.org","@type":"Product",
 "name":"Vintage Typewriter","sku":"TW-001",
 "brand":{"@type":"Brand","name":"Olivetti"},
 "image":"https://i.ebayimg.com/images/abc.jpg",
 "offers":{"@type":"Offer","price":"79.99","priceCurrency":"GBP",
           "availability":"https://schema.org/InStock",
           "itemCondition":"https://schema.org/UsedCondition",
           "seller":{"@type":"Person","name":"vintage_seller_99"}}}
</script>
</head></html>"##;
        let v = parse(html, "https://www.ebay.co.uk/itm/325", "325");
        assert_eq!(v["title"], "Vintage Typewriter");
        assert_eq!(v["price"], "79.99");
        assert_eq!(v["currency"], "GBP");
        assert_eq!(v["availability"], "InStock");
        assert_eq!(v["condition"], "UsedCondition");
        assert_eq!(v["seller"], "vintage_seller_99");
        assert_eq!(v["brand"], "Olivetti");
    }

    #[test]
    fn parse_handles_aggregate_offer_price_range() {
        let html = r##"
<script type="application/ld+json">
{"@type":"Product","name":"Used Copies",
 "offers":{"@type":"AggregateOffer","offerCount":"5",
           "lowPrice":"10.00","highPrice":"50.00","priceCurrency":"USD"}}
</script>
"##;
        let v = parse(html, "https://www.ebay.com/itm/1", "1");
        assert_eq!(v["low_price"], "10.00");
        assert_eq!(v["high_price"], "50.00");
        assert_eq!(v["offer_count"], "5");
        assert_eq!(v["currency"], "USD");
    }
}
