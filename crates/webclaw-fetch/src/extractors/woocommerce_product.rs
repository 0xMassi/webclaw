//! WooCommerce product structured extractor.
//!
//! Targets WooCommerce's Store API: `/wp-json/wc/store/v1/products?slug={slug}`.
//! About 30-50% of WooCommerce stores expose this endpoint publicly
//! (it's on by default, but common security plugins disable it).
//! When it's off, the server returns 404 at /wp-json. We surface a
//! clean error and point callers at `/v1/scrape/ecommerce_product`
//! which works on any store with Schema.org JSON-LD.
//!
//! Explicit-call only. `/product/{slug}` is the default permalink for
//! WooCommerce but custom stores use every variation imaginable, so
//! auto-dispatch is unreliable.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::client::FetchClient;
use crate::error::FetchError;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "woocommerce_product",
    label: "WooCommerce product",
    description: "Returns product via the WooCommerce Store REST API (requires the /wp-json/wc/store endpoint to be enabled on the target store).",
    url_patterns: &[
        "https://{shop}/product/{slug}",
        "https://{shop}/shop/{slug}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host.is_empty() {
        return false;
    }
    // Permissive: WooCommerce stores use custom domains + custom
    // permalinks. The extractor's API probe is what confirms it's
    // really WooCommerce.
    url.contains("/product/")
        || url.contains("/shop/")
        || url.contains("/producto/") // common es locale
        || url.contains("/produit/") // common fr locale
}

pub async fn extract(client: &FetchClient, url: &str) -> Result<Value, FetchError> {
    let slug = parse_slug(url).ok_or_else(|| {
        FetchError::Build(format!(
            "woocommerce_product: cannot parse slug from '{url}'"
        ))
    })?;
    let host = host_of(url);
    if host.is_empty() {
        return Err(FetchError::Build(format!(
            "woocommerce_product: empty host in '{url}'"
        )));
    }
    let scheme = if url.starts_with("http://") {
        "http"
    } else {
        "https"
    };
    let api_url = format!("{scheme}://{host}/wp-json/wc/store/v1/products?slug={slug}&per_page=1");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "woocommerce_product: {host} does not expose /wp-json/wc/store (404). \
             Use /v1/scrape/ecommerce_product for JSON-LD fallback."
        )));
    }
    if resp.status == 401 || resp.status == 403 {
        return Err(FetchError::Build(format!(
            "woocommerce_product: {host} requires auth for /wp-json/wc/store ({}). \
             Use /v1/scrape/ecommerce_product for the public JSON-LD fallback.",
            resp.status
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "woocommerce api returned status {} for {api_url}",
            resp.status
        )));
    }

    let products: Vec<Product> = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("woocommerce parse: {e}")))?;
    let p = products.into_iter().next().ok_or_else(|| {
        FetchError::Build(format!(
            "woocommerce_product: no product found for slug '{slug}' on {host}"
        ))
    })?;

    let images: Vec<Value> = p
        .images
        .iter()
        .map(|i| json!({"src": i.src, "thumbnail": i.thumbnail, "alt": i.alt}))
        .collect();
    let variations_count = p.variations.as_ref().map(|v| v.len()).unwrap_or(0);

    Ok(json!({
        "url":             url,
        "api_url":         api_url,
        "product_id":      p.id,
        "name":            p.name,
        "slug":            p.slug,
        "sku":             p.sku,
        "permalink":       p.permalink,
        "on_sale":         p.on_sale,
        "in_stock":        p.is_in_stock,
        "is_purchasable":  p.is_purchasable,
        "price":           p.prices.as_ref().and_then(|pr| pr.price.clone()),
        "regular_price":   p.prices.as_ref().and_then(|pr| pr.regular_price.clone()),
        "sale_price":      p.prices.as_ref().and_then(|pr| pr.sale_price.clone()),
        "currency":        p.prices.as_ref().and_then(|pr| pr.currency_code.clone()),
        "currency_minor":  p.prices.as_ref().and_then(|pr| pr.currency_minor_unit),
        "price_range":     p.prices.as_ref().and_then(|pr| pr.price_range.clone()),
        "average_rating":  p.average_rating,
        "review_count":    p.review_count,
        "description":     p.description,
        "short_description": p.short_description,
        "categories":      p.categories.iter().filter_map(|c| c.name.clone()).collect::<Vec<_>>(),
        "tags":            p.tags.iter().filter_map(|t| t.name.clone()).collect::<Vec<_>>(),
        "variation_count": variations_count,
        "image_count":     images.len(),
        "images":          images,
    }))
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

/// Extract the product slug from common WooCommerce permalinks.
fn parse_slug(url: &str) -> Option<String> {
    for needle in ["/product/", "/shop/", "/producto/", "/produit/"] {
        if let Some(after) = url.split(needle).nth(1) {
            let stripped = after
                .split(['?', '#'])
                .next()?
                .trim_end_matches('/')
                .split('/')
                .next()
                .unwrap_or("");
            if !stripped.is_empty() {
                return Some(stripped.to_string());
            }
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Store API types (subset of the full response)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct Product {
    id: Option<i64>,
    name: Option<String>,
    slug: Option<String>,
    sku: Option<String>,
    permalink: Option<String>,
    description: Option<String>,
    short_description: Option<String>,
    on_sale: Option<bool>,
    is_in_stock: Option<bool>,
    is_purchasable: Option<bool>,
    average_rating: Option<serde_json::Value>, // string or number
    review_count: Option<i64>,
    prices: Option<Prices>,
    #[serde(default)]
    categories: Vec<Term>,
    #[serde(default)]
    tags: Vec<Term>,
    #[serde(default)]
    images: Vec<Img>,
    variations: Option<Vec<serde_json::Value>>,
}

#[derive(Deserialize)]
struct Prices {
    price: Option<String>,
    regular_price: Option<String>,
    sale_price: Option<String>,
    currency_code: Option<String>,
    currency_minor_unit: Option<i64>,
    price_range: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Term {
    name: Option<String>,
}

#[derive(Deserialize)]
struct Img {
    src: Option<String>,
    thumbnail: Option<String>,
    alt: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_common_permalinks() {
        assert!(matches("https://shop.example.com/product/cool-widget"));
        assert!(matches("https://shop.example.com/shop/cool-widget"));
        assert!(matches("https://tienda.example.com/producto/cosa"));
        assert!(matches("https://boutique.example.com/produit/chose"));
    }

    #[test]
    fn parse_slug_handles_locale_and_suffix() {
        assert_eq!(
            parse_slug("https://shop.example.com/product/cool-widget"),
            Some("cool-widget".into())
        );
        assert_eq!(
            parse_slug("https://shop.example.com/product/cool-widget/?attr=red"),
            Some("cool-widget".into())
        );
        assert_eq!(
            parse_slug("https://tienda.example.com/producto/cosa/"),
            Some("cosa".into())
        );
    }
}
