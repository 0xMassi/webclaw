//! Shared JSON-LD `Product` walker for the marketplace vertical extractors.
//!
//! Amazon, eBay, and Etsy all surface their product data as schema.org
//! `Product` JSON-LD. The logic to find the product node (walking `@graph`
//! and arrays) and pull the common fields (title, brand, image, first offer,
//! aggregate rating) is identical across them, so it lives here once. Each
//! site keeps only its site-specific shell (ASIN, item id, slug handling,
//! antibot escalation) on top.
//!
//! `ecommerce_product` has its own divergent walker (carries an OG-merge
//! guard) and intentionally does not share this module.
use serde_json::{Value, json};

/// Find the first schema.org `Product` node across a page's JSON-LD blocks.
pub(crate) fn find_product_jsonld(html: &str) -> Option<Value> {
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

pub(crate) fn get_text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| match x {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

pub(crate) fn get_brand(v: &Value) -> Option<String> {
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

pub(crate) fn get_first_image(v: &Value) -> Option<String> {
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

pub(crate) fn first_offer(v: &Value) -> Option<Value> {
    let offers = v.get("offers")?;
    match offers {
        Value::Array(arr) => arr.first().cloned(),
        Value::Object(_) => Some(offers.clone()),
        _ => None,
    }
}

pub(crate) fn get_aggregate_rating(v: &Value) -> Option<Value> {
    let r = v.get("aggregateRating")?;
    Some(json!({
        "rating_value": get_text(r, "ratingValue"),
        "review_count": get_text(r, "reviewCount"),
        "best_rating":  get_text(r, "bestRating"),
    }))
}
