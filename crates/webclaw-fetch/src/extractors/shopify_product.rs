//! Shopify product structured extractor.
//!
//! Every Shopify store exposes a public JSON endpoint for each product
//! by appending `.json` to the product URL:
//!
//!   https://shop.example.com/products/cool-tshirt
//!   → https://shop.example.com/products/cool-tshirt.json
//!
//! There are ~4 million Shopify stores. The `.json` endpoint is
//! undocumented but has been stable for 10+ years. When a store puts
//! Cloudflare / antibot in front of the shop, this path can 403 just
//! like any other — for those cases the caller should fall back to
//! `ecommerce_product` (JSON-LD) or the cloud tier.
//!
//! This extractor is **explicit-call only** — it is NOT auto-dispatched
//! from `/v1/scrape` because we cannot tell ahead of time whether an
//! arbitrary `/products/{slug}` URL is a Shopify store. Callers hit
//! `/v1/scrape/shopify_product` when they know.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "shopify_product",
    label: "Shopify product",
    description: "Returns product metadata on ANY Shopify store via the public /products/{handle}.json endpoint: title, vendor, variants with prices + stock, images, options.",
    url_patterns: &[
        "https://{shop}/products/{handle}",
        "https://{shop}.myshopify.com/products/{handle}",
    ],
};

pub fn matches(url: &str) -> bool {
    // Any URL whose path contains /products/{something}. We do not
    // filter by host — Shopify powers custom-domain stores. The
    // extractor's /.json fallback is what confirms Shopify; `matches`
    // just says "this is a plausible shape." Still reject obviously
    // non-Shopify known hosts to save a failed request.
    let host = host_of(url);
    if host.is_empty() || NON_SHOPIFY_HOSTS.iter().any(|h| host.ends_with(h)) {
        return false;
    }
    url.contains("/products/") && !url.ends_with("/products/")
}

/// Hosts we know are not Shopify — reject so we don't burn a request.
const NON_SHOPIFY_HOSTS: &[&str] = &[
    "amazon.com",
    "amazon.co.uk",
    "amazon.de",
    "amazon.fr",
    "amazon.it",
    "ebay.com",
    "etsy.com",
    "walmart.com",
    "target.com",
    "aliexpress.com",
    "bestbuy.com",
    "wayfair.com",
    "homedepot.com",
    "github.com", // /products is a marketing page
];

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let json_url = build_json_url(url);
    let resp = client.fetch(&json_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "shopify_product: '{url}' not found (got 404 from {json_url})"
        )));
    }
    if resp.status == 403 {
        return Err(FetchError::Build(format!(
            "shopify_product: {json_url} returned 403 — the store has antibot in front of the .json endpoint. Try /v1/scrape/ecommerce_product for the HTML + JSON-LD fallback."
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "shopify returned status {} for {json_url}",
            resp.status
        )));
    }

    let body: Wrapper = serde_json::from_str(&resp.html).map_err(|e| {
        FetchError::BodyDecode(format!(
            "shopify_product: '{url}' didn't return Shopify JSON — likely not a Shopify store ({e})"
        ))
    })?;
    let p = body.product;

    let variants: Vec<Value> = p
        .variants
        .iter()
        .map(|v| {
            json!({
                "id":                  v.id,
                "title":               v.title,
                "sku":                 v.sku,
                "barcode":             v.barcode,
                "price":               v.price,
                "compare_at_price":    v.compare_at_price,
                "available":           v.available,
                "inventory_quantity":  v.inventory_quantity,
                "position":            v.position,
                "weight":              v.weight,
                "weight_unit":         v.weight_unit,
                "requires_shipping":   v.requires_shipping,
                "taxable":             v.taxable,
                "option1":             v.option1,
                "option2":             v.option2,
                "option3":             v.option3,
            })
        })
        .collect();

    let images: Vec<Value> = p
        .images
        .iter()
        .map(|i| {
            json!({
                "src":      i.src,
                "width":    i.width,
                "height":   i.height,
                "position": i.position,
                "alt":      i.alt,
            })
        })
        .collect();

    let options: Vec<Value> = p
        .options
        .iter()
        .map(|o| json!({"name": o.name, "values": o.values, "position": o.position}))
        .collect();

    // Price range + availability summary across variants (the shape
    // agents typically want without walking the variants array).
    let prices: Vec<f64> = p
        .variants
        .iter()
        .filter_map(|v| v.price.as_deref().and_then(|s| s.parse::<f64>().ok()))
        .collect();
    let any_available = p.variants.iter().any(|v| v.available.unwrap_or(false));

    Ok(json!({
        "url":             url,
        "json_url":        json_url,
        "product_id":      p.id,
        "handle":          p.handle,
        "title":           p.title,
        "vendor":          p.vendor,
        "product_type":    p.product_type,
        "tags":            p.tags,
        "description_html":p.body_html,
        "published_at":    p.published_at,
        "created_at":      p.created_at,
        "updated_at":      p.updated_at,
        "variant_count":   variants.len(),
        "image_count":     images.len(),
        "any_available":   any_available,
        "price_min":       prices.iter().cloned().fold(f64::INFINITY, f64::min).is_finite().then(|| prices.iter().cloned().fold(f64::INFINITY, f64::min)),
        "price_max":       prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max).is_finite().then(|| prices.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
        "variants":        variants,
        "images":          images,
        "options":         options,
    }))
}

/// Build the .json path from a product URL. Handles pre-.jsoned URLs,
/// trailing slashes, and query strings.
fn build_json_url(url: &str) -> String {
    let (path_part, query_part) = match url.split_once('?') {
        Some((a, b)) => (a, Some(b)),
        None => (url, None),
    };
    let clean = path_part.trim_end_matches('/');
    let with_json = if clean.ends_with(".json") {
        clean.to_string()
    } else {
        format!("{clean}.json")
    };
    match query_part {
        Some(q) => format!("{with_json}?{q}"),
        None => with_json,
    }
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
// Shopify product JSON shape (a subset of the full response)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Wrapper {
    product: Product,
}

#[derive(Deserialize)]
struct Product {
    id: Option<i64>,
    title: Option<String>,
    handle: Option<String>,
    vendor: Option<String>,
    product_type: Option<String>,
    body_html: Option<String>,
    published_at: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    tags: serde_json::Value, // array OR comma-joined string depending on store
    #[serde(default)]
    variants: Vec<Variant>,
    #[serde(default)]
    images: Vec<Image>,
    #[serde(default)]
    options: Vec<Option_>,
}

#[derive(Deserialize)]
struct Variant {
    id: Option<i64>,
    title: Option<String>,
    sku: Option<String>,
    barcode: Option<String>,
    price: Option<String>,
    compare_at_price: Option<String>,
    available: Option<bool>,
    inventory_quantity: Option<i64>,
    position: Option<i64>,
    weight: Option<f64>,
    weight_unit: Option<String>,
    requires_shipping: Option<bool>,
    taxable: Option<bool>,
    option1: Option<String>,
    option2: Option<String>,
    option3: Option<String>,
}

#[derive(Deserialize)]
struct Image {
    src: Option<String>,
    width: Option<i64>,
    height: Option<i64>,
    position: Option<i64>,
    alt: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
struct Option_ {
    name: Option<String>,
    position: Option<i64>,
    #[serde(default)]
    values: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_plausible_shopify_urls() {
        assert!(matches(
            "https://www.allbirds.com/products/mens-tree-runners"
        ));
        assert!(matches(
            "https://shop.example.com/products/cool-tshirt?variant=123"
        ));
        assert!(matches("https://somestore.myshopify.com/products/thing-1"));
    }

    #[test]
    fn rejects_known_non_shopify() {
        assert!(!matches("https://www.amazon.com/dp/B0C123"));
        assert!(!matches("https://www.etsy.com/listing/12345/foo"));
        assert!(!matches("https://www.amazon.co.uk/products/thing"));
        assert!(!matches("https://github.com/products"));
    }

    #[test]
    fn rejects_non_product_urls() {
        assert!(!matches("https://example.com/"));
        assert!(!matches("https://example.com/products/"));
        assert!(!matches("https://example.com/collections/all"));
    }

    #[test]
    fn build_json_url_handles_slash_and_query() {
        assert_eq!(
            build_json_url("https://shop.example.com/products/foo"),
            "https://shop.example.com/products/foo.json"
        );
        assert_eq!(
            build_json_url("https://shop.example.com/products/foo/"),
            "https://shop.example.com/products/foo.json"
        );
        assert_eq!(
            build_json_url("https://shop.example.com/products/foo?variant=123"),
            "https://shop.example.com/products/foo.json?variant=123"
        );
        assert_eq!(
            build_json_url("https://shop.example.com/products/foo.json"),
            "https://shop.example.com/products/foo.json"
        );
    }
}
