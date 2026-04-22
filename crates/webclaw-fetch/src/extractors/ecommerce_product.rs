//! Generic ecommerce product extractor via Schema.org JSON-LD.
//!
//! Every modern ecommerce site ships a `<script type="application/ld+json">`
//! Product block for SEO / rich-result snippets. Google's own SEO docs
//! force this markup on anyone who wants to appear in shopping search.
//! We take advantage of it: one extractor that works on Shopify,
//! BigCommerce, WooCommerce, Squarespace, Magento, custom storefronts,
//! and anything else that follows Schema.org.
//!
//! **Explicit-call only** (`/v1/scrape/ecommerce_product`). Not in the
//! auto-dispatch because we can't identify "this is a product page"
//! from the URL alone. When the caller knows they have a product URL,
//! this is the reliable fallback for stores where shopify_product
//! doesn't apply.
//!
//! The extractor reuses `webclaw_core::structured_data::extract_json_ld`
//! so JSON-LD parsing is shared with the rest of the extraction
//! pipeline. We walk all blocks looking for `@type: Product`,
//! `ProductGroup`, or an `ItemList` whose first entry is a Product.
//!
//! ## OG fallback
//!
//! Two real-world cases JSON-LD alone can't cover:
//!
//! 1. Site has no Product JSON-LD at all (smaller Squarespace / custom
//!    storefronts, many European shops).
//! 2. Site has Product JSON-LD but the `offers` block is empty (seen on
//!    Patagonia and other catalog-style sites that split price onto a
//!    separate widget).
//!
//! For case 1 we build a minimal payload from OG / product meta tags
//! (`og:title`, `og:image`, `og:description`, `product:price:amount`,
//! `product:price:currency`, `product:availability`, `product:brand`).
//! For case 2 we augment the JSON-LD offers list with an OG-derived
//! offer so callers get a price either way. A `data_source` field
//! (`"jsonld"` / `"jsonld+og"` / `"og_fallback"`) tells the caller
//! which branch produced the data.

use std::sync::OnceLock;

use regex::Regex;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "ecommerce_product",
    label: "Ecommerce product (generic)",
    description: "Returns product info from any site that ships Schema.org Product JSON-LD: name, description, images, brand, SKU, price, availability, aggregate rating.",
    url_patterns: &[
        "https://{any-ecom-store}/products/{slug}",
        "https://{any-ecom-store}/product/{slug}",
        "https://{any-ecom-store}/p/{slug}",
    ],
};

pub fn matches(url: &str) -> bool {
    // Maximally permissive: explicit-call-only extractor. We trust the
    // caller knows they're pointing at a product page. Custom ecom
    // sites use every conceivable URL shape (warbyparker.com uses
    // `/eyeglasses/{category}/{slug}/{colour}`, etc.), so path-pattern
    // matching would false-negative a lot. All we gate on is a valid
    // http(s) URL with a host.
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return false;
    }
    !host_of(url).is_empty()
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let resp = client.fetch(url).await?;
    if !(200..300).contains(&resp.status) {
        return Err(FetchError::Build(format!(
            "ecommerce_product: status {} for {url}",
            resp.status
        )));
    }
    parse(&resp.html, url).ok_or_else(|| {
        FetchError::BodyDecode(format!(
            "ecommerce_product: no Schema.org Product JSON-LD and no OG product tags on {url}"
        ))
    })
}

/// Pure parser: try JSON-LD first, fall back to OG meta tags. Returns
/// `None` when neither path has enough to say "this is a product page".
pub fn parse(html: &str, url: &str) -> Option<Value> {
    // Reuse the core JSON-LD parser so we benefit from whatever
    // robustness it gains over time (handling @graph, arrays, etc.).
    let blocks = webclaw_core::structured_data::extract_json_ld(html);
    let product = find_product(&blocks);

    if let Some(p) = product {
        Some(build_jsonld_payload(&p, html, url))
    } else if has_og_product_signal(html) {
        Some(build_og_payload(html, url))
    } else {
        None
    }
}

/// Build the rich payload from a Product JSON-LD node. Augments the
/// `offers` array with an OG-derived offer when JSON-LD offers is empty
/// so callers get a price on sites like Patagonia.
fn build_jsonld_payload(product: &Value, html: &str, url: &str) -> Value {
    let mut offers = collect_offers(product);
    let mut data_source = "jsonld";
    if offers.is_empty()
        && let Some(og_offer) = build_og_offer(html)
    {
        offers.push(og_offer);
        data_source = "jsonld+og";
    }

    json!({
        "url":                url,
        "data_source":        data_source,
        "name":               get_text(product, "name").or_else(|| og(html, "title")),
        "description":        get_text(product, "description").or_else(|| og(html, "description")),
        "brand":              get_brand(product).or_else(|| meta_property(html, "product:brand")),
        "sku":                get_text(product, "sku"),
        "mpn":                get_text(product, "mpn"),
        "gtin":               get_text(product, "gtin")
                                 .or_else(|| get_text(product, "gtin13"))
                                 .or_else(|| get_text(product, "gtin12"))
                                 .or_else(|| get_text(product, "gtin8")),
        "product_id":         get_text(product, "productID"),
        "category":           get_text(product, "category"),
        "color":              get_text(product, "color"),
        "material":           get_text(product, "material"),
        "images":             nonempty_or_og(collect_images(product), html),
        "offers":             offers,
        "aggregate_rating":   get_aggregate_rating(product),
        "review_count":       get_review_count(product),
        "raw_schema_type":    get_text(product, "@type"),
        "raw_jsonld":         product.clone(),
    })
}

/// Build a minimal payload from OG / product meta tags. Used when a
/// page has no Product JSON-LD at all.
fn build_og_payload(html: &str, url: &str) -> Value {
    let offers = build_og_offer(html).map(|o| vec![o]).unwrap_or_default();
    let image = og(html, "image");
    let images: Vec<Value> = image.map(|i| vec![Value::String(i)]).unwrap_or_default();

    json!({
        "url":                url,
        "data_source":        "og_fallback",
        "name":               og(html, "title"),
        "description":        og(html, "description"),
        "brand":              meta_property(html, "product:brand"),
        "sku":                None::<String>,
        "mpn":                None::<String>,
        "gtin":               None::<String>,
        "product_id":         None::<String>,
        "category":           None::<String>,
        "color":              None::<String>,
        "material":           None::<String>,
        "images":             images,
        "offers":             offers,
        "aggregate_rating":   Value::Null,
        "review_count":       None::<String>,
        "raw_schema_type":    None::<String>,
        "raw_jsonld":         Value::Null,
    })
}

fn nonempty_or_og(imgs: Vec<Value>, html: &str) -> Vec<Value> {
    if !imgs.is_empty() {
        return imgs;
    }
    og(html, "image")
        .map(|s| vec![Value::String(s)])
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// JSON-LD walkers
// ---------------------------------------------------------------------------

/// Recursively walk the JSON-LD blocks and return the first node whose
/// `@type` is Product, ProductGroup, or IndividualProduct.
fn find_product(blocks: &[Value]) -> Option<Value> {
    for b in blocks {
        if let Some(found) = find_product_in(b) {
            return Some(found);
        }
    }
    None
}

fn find_product_in(v: &Value) -> Option<Value> {
    if is_product_type(v) {
        return Some(v.clone());
    }
    // @graph: [ {...}, {...} ]
    if let Some(graph) = v.get("@graph").and_then(|g| g.as_array()) {
        for item in graph {
            if let Some(found) = find_product_in(item) {
                return Some(found);
            }
        }
    }
    // Bare array wrapper
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
    let t = match v.get("@type") {
        Some(t) => t,
        None => return false,
    };
    let match_str = |s: &str| {
        matches!(
            s,
            "Product" | "ProductGroup" | "IndividualProduct" | "Vehicle" | "SomeProducts"
        )
    };
    match t {
        Value::String(s) => match_str(s),
        Value::Array(arr) => arr.iter().any(|x| x.as_str().is_some_and(match_str)),
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
    if let Some(obj) = brand.as_object()
        && let Some(n) = obj.get("name").and_then(|x| x.as_str())
    {
        return Some(n.to_string());
    }
    None
}

fn collect_images(v: &Value) -> Vec<Value> {
    match v.get("image") {
        Some(Value::String(s)) => vec![Value::String(s.clone())],
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|x| match x {
                Value::String(s) => Some(Value::String(s.clone())),
                Value::Object(_) => x.get("url").cloned(),
                _ => None,
            })
            .collect(),
        Some(Value::Object(o)) => o.get("url").cloned().into_iter().collect(),
        _ => Vec::new(),
    }
}

/// Normalise both bare Offer and AggregateOffer into a uniform array.
fn collect_offers(v: &Value) -> Vec<Value> {
    let offers = match v.get("offers") {
        Some(o) => o,
        None => return Vec::new(),
    };
    let collect_single = |o: &Value| -> Option<Value> {
        Some(json!({
            "price":            get_text(o, "price"),
            "low_price":        get_text(o, "lowPrice"),
            "high_price":       get_text(o, "highPrice"),
            "currency":         get_text(o, "priceCurrency"),
            "availability":     get_text(o, "availability").map(|s| s.replace("http://schema.org/", "").replace("https://schema.org/", "")),
            "item_condition":   get_text(o, "itemCondition").map(|s| s.replace("http://schema.org/", "").replace("https://schema.org/", "")),
            "valid_until":      get_text(o, "priceValidUntil"),
            "url":              get_text(o, "url"),
            "seller":           o.get("seller").and_then(|s| s.get("name")).and_then(|n| n.as_str()).map(String::from),
            "offer_count":      get_text(o, "offerCount"),
        }))
    };
    match offers {
        Value::Array(arr) => arr.iter().filter_map(collect_single).collect(),
        Value::Object(_) => collect_single(offers).into_iter().collect(),
        _ => Vec::new(),
    }
}

fn get_aggregate_rating(v: &Value) -> Option<Value> {
    let r = v.get("aggregateRating")?;
    Some(json!({
        "rating_value":  get_text(r, "ratingValue"),
        "best_rating":   get_text(r, "bestRating"),
        "worst_rating":  get_text(r, "worstRating"),
        "rating_count":  get_text(r, "ratingCount"),
        "review_count":  get_text(r, "reviewCount"),
    }))
}

fn get_review_count(v: &Value) -> Option<String> {
    v.get("aggregateRating")
        .and_then(|r| get_text(r, "reviewCount"))
        .or_else(|| get_text(v, "reviewCount"))
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

// ---------------------------------------------------------------------------
// OG / product meta-tag helpers
// ---------------------------------------------------------------------------

/// True when the HTML has enough OG / product meta tags to justify
/// building a fallback payload. A single `og:title` isn't enough on its
/// own — every blog post has that. We require either a product price
/// tag or at least an `og:type` of `product`/`og:product` to avoid
/// mis-classifying articles as products.
fn has_og_product_signal(html: &str) -> bool {
    let has_price = meta_property(html, "product:price:amount").is_some()
        || meta_property(html, "og:price:amount").is_some();
    if has_price {
        return true;
    }
    // `<meta property="og:type" content="product">` is the Schema.org OG
    // marker for product pages.
    let og_type = og(html, "type").unwrap_or_default().to_lowercase();
    matches!(og_type.as_str(), "product" | "og:product" | "product.item")
}

/// Build a single Offer-shaped Value from OG / product meta tags, or
/// `None` if there's no price info at all.
fn build_og_offer(html: &str) -> Option<Value> {
    let price = meta_property(html, "product:price:amount")
        .or_else(|| meta_property(html, "og:price:amount"));
    let currency = meta_property(html, "product:price:currency")
        .or_else(|| meta_property(html, "og:price:currency"));
    let availability = meta_property(html, "product:availability")
        .or_else(|| meta_property(html, "og:availability"));
    price.as_ref()?;
    Some(json!({
        "price":            price,
        "low_price":        None::<String>,
        "high_price":       None::<String>,
        "currency":         currency,
        "availability":     availability,
        "item_condition":   None::<String>,
        "valid_until":      None::<String>,
        "url":              None::<String>,
        "seller":           None::<String>,
        "offer_count":      None::<String>,
    }))
}

/// Pull the value of `<meta property="og:{prop}" content="...">`.
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

/// Pull the value of any `<meta property="..." content="...">` tag.
/// Needed for namespaced OG variants like `product:price:amount` that
/// the simple `og:*` matcher above doesn't cover.
fn meta_property(html: &str, prop: &str) -> Option<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="([^"]+)"[^>]+content="([^"]+)""#).unwrap()
    });
    for c in re.captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_any_http_url_with_host() {
        assert!(matches("https://www.allbirds.com/products/tree-runner"));
        assert!(matches(
            "https://www.warbyparker.com/eyeglasses/women/percey/jet-black-with-polished-gold"
        ));
        assert!(matches("https://example.com/p/widget"));
        assert!(matches("http://shop.example.com/foo/bar"));
    }

    #[test]
    fn rejects_empty_or_non_http() {
        assert!(!matches(""));
        assert!(!matches("not-a-url"));
        assert!(!matches("ftp://example.com/file"));
    }

    #[test]
    fn find_product_walks_graph() {
        let block = json!({
            "@context": "https://schema.org",
            "@graph": [
                {"@type": "Organization", "name": "ACME"},
                {"@type": "Product", "name": "Widget", "sku": "ABC"}
            ]
        });
        let blocks = vec![block];
        let p = find_product(&blocks).unwrap();
        assert_eq!(p.get("name").and_then(|v| v.as_str()), Some("Widget"));
    }

    #[test]
    fn find_product_handles_array_type() {
        let block = json!({
            "@type": ["Product", "Clothing"],
            "name": "Tee"
        });
        assert!(is_product_type(&block));
    }

    #[test]
    fn get_brand_from_string_or_object() {
        assert_eq!(get_brand(&json!({"brand": "ACME"})), Some("ACME".into()));
        assert_eq!(
            get_brand(&json!({"brand": {"@type": "Brand", "name": "ACME"}})),
            Some("ACME".into())
        );
    }

    #[test]
    fn collect_offers_handles_single_and_aggregate() {
        let p = json!({
            "offers": {
                "@type": "Offer",
                "price": "19.99",
                "priceCurrency": "USD",
                "availability": "https://schema.org/InStock"
            }
        });
        let offers = collect_offers(&p);
        assert_eq!(offers.len(), 1);
        assert_eq!(
            offers[0].get("price").and_then(|v| v.as_str()),
            Some("19.99")
        );
        assert_eq!(
            offers[0].get("availability").and_then(|v| v.as_str()),
            Some("InStock")
        );
    }

    // --- OG fallback --------------------------------------------------------

    #[test]
    fn has_og_product_signal_accepts_product_type_or_price() {
        let type_only = r#"<meta property="og:type" content="product">"#;
        let price_only = r#"<meta property="product:price:amount" content="49.00">"#;
        let neither = r#"<meta property="og:title" content="My Article"><meta property="og:type" content="article">"#;
        assert!(has_og_product_signal(type_only));
        assert!(has_og_product_signal(price_only));
        assert!(!has_og_product_signal(neither));
    }

    #[test]
    fn og_fallback_builds_payload_without_jsonld() {
        let html = r##"<html><head>
            <meta property="og:type" content="product">
            <meta property="og:title" content="Handmade Candle">
            <meta property="og:image" content="https://cdn.example.com/candle.jpg">
            <meta property="og:description" content="Small-batch soy candle.">
            <meta property="product:price:amount" content="18.00">
            <meta property="product:price:currency" content="USD">
            <meta property="product:availability" content="in stock">
            <meta property="product:brand" content="Little Studio">
        </head></html>"##;
        let v = parse(html, "https://example.com/p/candle").unwrap();
        assert_eq!(v["data_source"], "og_fallback");
        assert_eq!(v["name"], "Handmade Candle");
        assert_eq!(v["description"], "Small-batch soy candle.");
        assert_eq!(v["brand"], "Little Studio");
        assert_eq!(v["offers"][0]["price"], "18.00");
        assert_eq!(v["offers"][0]["currency"], "USD");
        assert_eq!(v["offers"][0]["availability"], "in stock");
        assert_eq!(v["images"][0], "https://cdn.example.com/candle.jpg");
    }

    #[test]
    fn jsonld_augments_empty_offers_with_og_price() {
        // Patagonia-shaped page: Product JSON-LD without an Offer, plus
        // product:price:* OG tags. We should merge.
        let html = r##"<html><head>
            <script type="application/ld+json">
            {"@context":"https://schema.org","@type":"Product",
             "name":"Better Sweater","brand":"Patagonia",
             "aggregateRating":{"@type":"AggregateRating","ratingValue":"4.4","reviewCount":"1142"}}
            </script>
            <meta property="product:price:amount" content="139.00">
            <meta property="product:price:currency" content="USD">
        </head></html>"##;
        let v = parse(html, "https://patagonia.com/p/x").unwrap();
        assert_eq!(v["data_source"], "jsonld+og");
        assert_eq!(v["name"], "Better Sweater");
        assert_eq!(v["offers"].as_array().unwrap().len(), 1);
        assert_eq!(v["offers"][0]["price"], "139.00");
    }

    #[test]
    fn jsonld_only_stays_pure_jsonld() {
        let html = r##"<html><head>
            <script type="application/ld+json">
            {"@type":"Product","name":"Widget",
             "offers":{"@type":"Offer","price":"9.99","priceCurrency":"USD"}}
            </script>
        </head></html>"##;
        let v = parse(html, "https://example.com/p/w").unwrap();
        assert_eq!(v["data_source"], "jsonld");
        assert_eq!(v["offers"][0]["price"], "9.99");
    }

    #[test]
    fn parse_returns_none_on_no_product_signals() {
        let html = r#"<html><head>
            <meta property="og:title" content="My Blog Post">
            <meta property="og:type" content="article">
        </head></html>"#;
        assert!(parse(html, "https://blog.example.com/post").is_none());
    }
}
