use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use webclaw_capture::infer::{
    endpoint_id, infer_endpoints, infer_json_schema, normalize_path_template,
};
use webclaw_capture::types::{CapturedExchange, EndpointDefinition};

fn fixture_exchanges() -> Vec<CapturedExchange> {
    let har: Value =
        serde_json::from_str(include_str!("fixtures/sample.har.json")).expect("valid HAR fixture");
    let entries = har
        .pointer("/log/entries")
        .and_then(Value::as_array)
        .expect("HAR fixture entries");

    entries.iter().map(har_entry_to_exchange).collect()
}

fn har_entry_to_exchange(entry: &Value) -> CapturedExchange {
    let request = entry.get("request").expect("request");
    let response = entry.get("response").expect("response");

    CapturedExchange {
        method: string_at(request, "method"),
        url: string_at(request, "url"),
        request_headers: har_headers(request),
        request_body_sample: request
            .pointer("/postData/text")
            .and_then(Value::as_str)
            .map(str::to_owned),
        resource_type: entry
            .get("_resourceType")
            .and_then(Value::as_str)
            .map(str::to_owned),
        status: response
            .get("status")
            .and_then(Value::as_u64)
            .expect("response status") as u16,
        response_headers: har_headers(response),
        response_body_sample: response
            .pointer("/content/text")
            .and_then(Value::as_str)
            .map(str::to_owned),
        started_at: DateTime::parse_from_rfc3339(&string_at(entry, "startedDateTime"))
            .expect("RFC3339 startedDateTime")
            .with_timezone(&Utc),
        duration_ms: entry.get("time").and_then(Value::as_u64).expect("duration"),
        redirect_chain: Vec::new(),
    }
}

fn har_headers(container: &Value) -> Map<String, Value> {
    container
        .get("headers")
        .and_then(Value::as_array)
        .expect("headers")
        .iter()
        .map(|header| {
            (
                string_at(header, "name"),
                Value::String(string_at(header, "value")),
            )
        })
        .collect()
}

fn string_at(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("{key} should be a string"))
        .to_owned()
}

fn find_endpoint<'a>(
    endpoints: &'a [EndpointDefinition],
    method: &str,
    path_template: &str,
) -> &'a EndpointDefinition {
    endpoints
        .iter()
        .find(|endpoint| endpoint.method == method && endpoint.path_template == path_template)
        .unwrap_or_else(|| panic!("missing endpoint {method} {path_template}; got {endpoints:#?}"))
}

fn sorted_ids(endpoints: &[EndpointDefinition]) -> Vec<String> {
    let mut ids = endpoints
        .iter()
        .map(|endpoint| endpoint.id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[test]
fn infers_stable_endpoint_ids_and_path_templates_from_har_fixture() {
    let exchanges = fixture_exchanges();

    let endpoints = infer_endpoints(&exchanges);
    let repeated = infer_endpoints(&exchanges);

    assert_eq!(endpoints.len(), 3, "static assets should be ignored");
    assert_eq!(
        sorted_ids(&endpoints),
        sorted_ids(&repeated),
        "endpoint ids should be deterministic across inference runs"
    );

    let products = find_endpoint(&endpoints, "GET", "/api/products");
    assert_eq!(
        products.id,
        endpoint_id("GET", "https://example.test", "/api/products")
    );

    let product_detail = find_endpoint(&endpoints, "GET", "/api/products/{id}");
    assert_eq!(
        product_detail.id,
        endpoint_id("GET", "https://example.test", "/api/products/{id}")
    );

    let graphql = find_endpoint(&endpoints, "POST", "/graphql");
    assert_eq!(
        graphql.id,
        endpoint_id("POST", "https://example.test", "/graphql")
    );
}

#[test]
fn infers_query_examples_schemas_auth_evidence_and_mutation_safety() {
    let endpoints = infer_endpoints(&fixture_exchanges());

    let products = find_endpoint(&endpoints, "GET", "/api/products");
    assert_eq!(
        products.query_params.get("category"),
        Some(&vec!["tools".to_owned()])
    );
    assert_eq!(
        products.query_params.get("page"),
        Some(&vec!["2".to_owned()])
    );
    assert!(
        products
            .auth_evidence
            .iter()
            .any(|evidence| evidence.to_ascii_lowercase().contains("authorization")),
        "Authorization header should be recorded as auth evidence"
    );
    assert!(products.safety.safe_to_replay);
    assert!(!products.safety.requires_confirmation);

    let products_schema = products.response_schema.as_ref().expect("response schema");
    assert_eq!(
        products_schema.pointer("/properties/items/type"),
        Some(&json!("array"))
    );
    assert_eq!(
        products_schema.pointer("/properties/items/items/properties/id/type"),
        Some(&json!("integer"))
    );
    assert_eq!(
        products_schema.pointer("/properties/hasMore/type"),
        Some(&json!("boolean"))
    );

    let graphql = find_endpoint(&endpoints, "POST", "/graphql");
    assert!(!graphql.safety.safe_to_replay);
    assert!(graphql.safety.requires_confirmation);
    assert!(
        graphql
            .auth_evidence
            .iter()
            .any(|evidence| evidence.to_ascii_lowercase().contains("csrf")),
        "CSRF header should be recorded as auth evidence"
    );

    let request_schema = graphql.request_schema.as_ref().expect("request schema");
    assert_eq!(
        request_schema.pointer("/properties/query/type"),
        Some(&json!("string"))
    );
    assert_eq!(
        request_schema.pointer("/properties/variables/properties/name/type"),
        Some(&json!("string"))
    );

    let response_schema = graphql.response_schema.as_ref().expect("response schema");
    assert_eq!(
        response_schema.pointer("/properties/data/properties/createProduct/properties/id/type"),
        Some(&json!("string"))
    );
}

#[test]
fn ignores_static_asset_entries_from_the_fixture() {
    let endpoints = infer_endpoints(&fixture_exchanges());

    assert!(
        endpoints
            .iter()
            .all(|endpoint| !endpoint.path_template.contains("/static/")),
        "static asset requests should not become learned endpoints: {endpoints:#?}"
    );
}

#[test]
fn normalizes_numeric_uuid_and_high_entropy_path_segments() {
    assert_eq!(
        normalize_path_template("/api/products/12345"),
        "/api/products/{id}"
    );
    assert_eq!(
        normalize_path_template("/api/users/550e8400-e29b-41d4-a716-446655440000"),
        "/api/users/{id}"
    );
    assert_eq!(
        normalize_path_template("/api/sessions/a1b2c3d4e5f6a7b8"),
        "/api/sessions/{id}"
    );
    assert_eq!(
        normalize_path_template("/api/categories/tools"),
        "/api/categories/tools"
    );
}

#[test]
fn infers_basic_json_schema_shapes() {
    let schema = infer_json_schema(&json!({
        "id": 12345,
        "name": "Hammer",
        "price": 12.5,
        "inStock": true,
        "tags": ["hand-tool"],
        "metadata": null
    }));

    assert_eq!(schema.pointer("/type"), Some(&json!("object")));
    assert_eq!(
        schema.pointer("/properties/id/type"),
        Some(&json!("integer"))
    );
    assert_eq!(
        schema.pointer("/properties/price/type"),
        Some(&json!("number"))
    );
    assert_eq!(
        schema.pointer("/properties/inStock/type"),
        Some(&json!("boolean"))
    );
    assert_eq!(
        schema.pointer("/properties/tags/type"),
        Some(&json!("array"))
    );
    assert_eq!(
        schema.pointer("/properties/tags/items/type"),
        Some(&json!("string"))
    );
    assert_eq!(
        schema.pointer("/properties/metadata/type"),
        Some(&json!("null"))
    );
}
