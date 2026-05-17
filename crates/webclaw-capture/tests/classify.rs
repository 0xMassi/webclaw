use chrono::{TimeZone, Utc};
use serde_json::{Map, Value, json};
use webclaw_capture::classify::{classify_exchange, filter_api_exchanges};
use webclaw_capture::types::CapturedExchange;

fn headers(entries: &[(&str, &str)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

fn exchange(url: &str) -> CapturedExchange {
    CapturedExchange {
        method: "GET".to_owned(),
        url: url.to_owned(),
        request_headers: Map::new(),
        request_body_sample: None,
        resource_type: Some("document".to_owned()),
        status: 200,
        response_headers: Map::new(),
        response_body_sample: None,
        started_at: Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap(),
        duration_ms: 25,
        redirect_chain: Vec::new(),
    }
}

fn with_resource_type(mut exchange: CapturedExchange, resource_type: &str) -> CapturedExchange {
    exchange.resource_type = Some(resource_type.to_owned());
    exchange
}

fn with_response_header(
    mut exchange: CapturedExchange,
    name: &str,
    value: &str,
) -> CapturedExchange {
    exchange.response_headers = headers(&[(name, value)]);
    exchange
}

fn with_request_body(mut exchange: CapturedExchange, body: serde_json::Value) -> CapturedExchange {
    exchange.method = "POST".to_owned();
    exchange.request_headers = headers(&[("Content-Type", "application/json")]);
    exchange.request_body_sample = Some(body.to_string());
    exchange
}

fn assert_included(exchange: &CapturedExchange, label: &str) {
    let classification = classify_exchange(exchange);

    assert!(
        classification.include,
        "{label} should be included, got {classification:?}"
    );
    assert!(
        classification.confidence >= 0.5,
        "{label} should have useful confidence, got {classification:?}"
    );
    assert!(
        !classification.reasons.is_empty(),
        "{label} should explain why it was classified as API traffic"
    );
}

fn assert_excluded(exchange: &CapturedExchange, label: &str) {
    let classification = classify_exchange(exchange);

    assert!(
        !classification.include,
        "{label} should be excluded, got {classification:?}"
    );
    assert!(
        classification.confidence <= 0.5,
        "{label} should not look like confident API traffic, got {classification:?}"
    );
    assert!(
        !classification.reasons.is_empty(),
        "{label} should explain why it was excluded"
    );
}

#[test]
fn includes_fetch_and_xhr_resource_types() {
    let cases = [
        with_resource_type(exchange("https://example.test/products"), "fetch"),
        with_resource_type(exchange("https://example.test/products"), "xhr"),
    ];

    for case in cases {
        assert_included(
            &case,
            case.resource_type
                .as_deref()
                .expect("resource type should be set"),
        );
    }
}

#[test]
fn includes_json_responses() {
    let case = with_response_header(
        exchange("https://example.test/products"),
        "Content-Type",
        "application/json; charset=utf-8",
    );

    assert_included(&case, "JSON response");
}

#[test]
fn includes_common_api_path_prefixes() {
    let cases = [
        exchange("https://example.test/api/products"),
        exchange("https://example.test/v1/products"),
        exchange("https://example.test/v2/products"),
    ];

    for case in cases {
        assert_included(&case, &case.url);
    }
}

#[test]
fn includes_graphql_paths() {
    let case = exchange("https://example.test/graphql");

    assert_included(&case, "GraphQL path");
}

#[test]
fn includes_graphql_request_bodies() {
    let case = with_request_body(
        exchange("https://example.test/query"),
        json!({
            "operationName": "Products",
            "query": "query Products { products { id name } }",
            "variables": {
                "first": 25
            }
        }),
    );

    assert_included(&case, "GraphQL request body");
}

#[test]
fn excludes_static_assets_by_extension() {
    let cases = [
        exchange("https://example.test/static/logo.png"),
        exchange("https://example.test/static/photo.jpg"),
        exchange("https://example.test/static/icon.svg"),
        exchange("https://example.test/static/site.css"),
        exchange("https://example.test/static/app.js"),
        exchange("https://example.test/static/font.woff2"),
        exchange("https://example.test/static/app.js.map"),
    ];

    for case in cases {
        assert_excluded(&case, &case.url);
    }
}

#[test]
fn excludes_tracking_hosts() {
    let cases = [
        with_response_header(
            exchange("https://www.google-analytics.com/g/collect?v=2"),
            "Content-Type",
            "application/json",
        ),
        with_response_header(
            exchange("https://ads.doubleclick.net/pagead/id"),
            "Content-Type",
            "application/json",
        ),
        with_response_header(
            exchange("https://telemetry.example.test/v1/events"),
            "Content-Type",
            "application/json",
        ),
    ];

    for case in cases {
        assert_excluded(&case, &case.url);
    }
}

#[test]
fn excludes_browser_extension_urls() {
    let cases = [
        with_resource_type(exchange("chrome-extension://abcdef/options.html"), "fetch"),
        with_resource_type(exchange("moz-extension://abcdef/options.html"), "xhr"),
    ];

    for case in cases {
        assert_excluded(&case, &case.url);
    }
}

#[test]
fn filter_api_exchanges_returns_only_included_traffic() {
    let api = exchange("https://example.test/api/products");
    let asset = exchange("https://example.test/static/app.js");
    let tracking = with_response_header(
        exchange("https://telemetry.example.test/v1/events"),
        "Content-Type",
        "application/json",
    );
    let exchanges = vec![api.clone(), asset, tracking];

    let filtered = filter_api_exchanges(&exchanges);

    assert_eq!(filtered, vec![api]);
}
