//! Shopify collection structured extractor.
//!
//! Every Shopify store exposes `/collections/{handle}.json` and
//! `/collections/{handle}/products.json` on the public surface. This
//! extractor hits `.json` (collection metadata) and falls through to
//! `/products.json` for the first page of products. Same caveat as
//! `shopify_product`: stores with Cloudflare in front of the shop
//! will 403 the public path.
//!
//! Explicit-call only (like `shopify_product`). `/collections/{slug}`
//! is a URL shape used by non-Shopify stores too, so auto-dispatch
//! would claim too many URLs.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "shopify_collection",
    label: "Shopify collection",
    description: "Returns collection metadata + first page of products (handle, title, vendor, price, available) on ANY Shopify store via /collections/{handle}.json + /products.json.",
    url_patterns: &[
        "https://{shop}/collections/{handle}",
        "https://{shop}.myshopify.com/collections/{handle}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host.is_empty() || NON_SHOPIFY_HOSTS.iter().any(|h| host.ends_with(h)) {
        return false;
    }
    url.contains("/collections/") && !url.ends_with("/collections/")
}

const NON_SHOPIFY_HOSTS: &[&str] = &[
    "amazon.com",
    "amazon.co.uk",
    "amazon.de",
    "ebay.com",
    "etsy.com",
    "walmart.com",
    "target.com",
    "aliexpress.com",
    "huggingface.co", // has /collections/ for models
    "github.com",
];

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (coll_meta_url, coll_products_url) = build_json_urls(url);

    // Step 1: collection metadata. Shopify returns 200 on missing
    // collections sometimes; check "collection" key below.
    let meta_resp = client.fetch(&coll_meta_url).await?;
    if meta_resp.status == 404 {
        return Err(FetchError::Build(format!(
            "shopify_collection: '{url}' not found"
        )));
    }
    if meta_resp.status == 403 {
        return Err(FetchError::Build(format!(
            "shopify_collection: {coll_meta_url} returned 403. The store has antibot in front of the .json endpoint. Use /v1/scrape/ecommerce_product or api.webclaw.io for this store."
        )));
    }
    if meta_resp.status != 200 {
        return Err(FetchError::Build(format!(
            "shopify returned status {} for {coll_meta_url}",
            meta_resp.status
        )));
    }

    let meta: MetaWrapper = serde_json::from_str(&meta_resp.html).map_err(|e| {
        FetchError::BodyDecode(format!(
            "shopify_collection: '{url}' didn't return Shopify JSON, likely not a Shopify store ({e})"
        ))
    })?;

    // Step 2: first page of products for this collection.
    let products = match client.fetch(&coll_products_url).await {
        Ok(r) if r.status == 200 => serde_json::from_str::<ProductsWrapper>(&r.html)
            .ok()
            .map(|pw| pw.products)
            .unwrap_or_default(),
        _ => Vec::new(),
    };

    let product_summaries: Vec<Value> = products
        .iter()
        .map(|p| {
            let first_variant = p.variants.first();
            json!({
                "id":              p.id,
                "handle":          p.handle,
                "title":           p.title,
                "vendor":          p.vendor,
                "product_type":    p.product_type,
                "price":           first_variant.and_then(|v| v.price.clone()),
                "compare_at_price":first_variant.and_then(|v| v.compare_at_price.clone()),
                "available":       p.variants.iter().any(|v| v.available.unwrap_or(false)),
                "variant_count":   p.variants.len(),
                "image":           p.images.first().and_then(|i| i.src.clone()),
                "created_at":      p.created_at,
                "updated_at":      p.updated_at,
            })
        })
        .collect();

    let c = meta.collection;
    Ok(json!({
        "url":               url,
        "meta_json_url":     coll_meta_url,
        "products_json_url": coll_products_url,
        "collection_id":     c.id,
        "handle":            c.handle,
        "title":             c.title,
        "description_html":  c.body_html,
        "published_at":      c.published_at,
        "updated_at":        c.updated_at,
        "sort_order":        c.sort_order,
        "products_in_page":  product_summaries.len(),
        "products":          product_summaries,
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

/// Build `(collection.json, collection/products.json)` from a user URL.
fn build_json_urls(url: &str) -> (String, String) {
    let (path_part, _query_part) = match url.split_once('?') {
        Some((a, b)) => (a, Some(b)),
        None => (url, None),
    };
    let clean = path_part.trim_end_matches('/').trim_end_matches(".json");
    (
        format!("{clean}.json"),
        format!("{clean}/products.json?limit=50"),
    )
}

// ---------------------------------------------------------------------------
// Shopify collection + product JSON shapes (subsets)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct MetaWrapper {
    collection: Collection,
}

#[derive(Deserialize)]
struct Collection {
    id: Option<i64>,
    handle: Option<String>,
    title: Option<String>,
    body_html: Option<String>,
    published_at: Option<String>,
    updated_at: Option<String>,
    sort_order: Option<String>,
}

#[derive(Deserialize)]
struct ProductsWrapper {
    #[serde(default)]
    products: Vec<ProductSummary>,
}

#[derive(Deserialize)]
struct ProductSummary {
    id: Option<i64>,
    handle: Option<String>,
    title: Option<String>,
    vendor: Option<String>,
    product_type: Option<String>,
    created_at: Option<String>,
    updated_at: Option<String>,
    #[serde(default)]
    variants: Vec<VariantSummary>,
    #[serde(default)]
    images: Vec<ImageSummary>,
}

#[derive(Deserialize)]
struct VariantSummary {
    price: Option<String>,
    compare_at_price: Option<String>,
    available: Option<bool>,
}

#[derive(Deserialize)]
struct ImageSummary {
    src: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_shopify_collection_urls() {
        assert!(matches("https://www.allbirds.com/collections/mens"));
        assert!(matches(
            "https://shop.example.com/collections/new-arrivals?page=2"
        ));
    }

    #[test]
    fn rejects_non_shopify() {
        assert!(!matches("https://github.com/collections/foo"));
        assert!(!matches("https://huggingface.co/collections/foo"));
        assert!(!matches("https://example.com/"));
        assert!(!matches("https://example.com/collections/"));
    }

    #[test]
    fn build_json_urls_derives_both_paths() {
        let (meta, products) = build_json_urls("https://shop.example.com/collections/mens");
        assert_eq!(meta, "https://shop.example.com/collections/mens.json");
        assert_eq!(
            products,
            "https://shop.example.com/collections/mens/products.json?limit=50"
        );
    }

    #[test]
    fn build_json_urls_handles_trailing_slash() {
        let (meta, _) = build_json_urls("https://shop.example.com/collections/mens/");
        assert_eq!(meta, "https://shop.example.com/collections/mens.json");
    }
}
