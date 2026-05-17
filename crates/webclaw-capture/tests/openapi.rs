use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use webclaw_capture::openapi::{export_openapi, write_openapi};
use webclaw_capture::store::save_capture;
use webclaw_capture::types::{
    CaptureArtifact, EndpointDefinition, EndpointExample, EndpointSafety,
};

static ENV_LOCK: Mutex<()> = Mutex::new(());
const CAPTURE_DIR_ENV: &str = "WEBCLAW_CAPTURE_DIR";

struct EnvVarGuard {
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set_capture_dir(value: Option<&Path>) -> Self {
        let original = env::var_os(CAPTURE_DIR_ENV);

        unsafe {
            match value {
                Some(path) => env::set_var(CAPTURE_DIR_ENV, path),
                None => env::remove_var(CAPTURE_DIR_ENV),
            }
        }

        Self { original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => env::set_var(CAPTURE_DIR_ENV, value),
                None => env::remove_var(CAPTURE_DIR_ENV),
            }
        }
    }
}

fn with_capture_dir<T>(value: Option<&Path>, test: impl FnOnce() -> T) -> T {
    let _lock = ENV_LOCK.lock().expect("capture env lock");
    let _guard = EnvVarGuard::set_capture_dir(value);

    test()
}

#[test]
fn exports_openapi_31_and_an_operation_for_every_endpoint() {
    let doc = export_openapi(&sample_endpoints());

    assert_eq!(doc.get("openapi").and_then(Value::as_str), Some("3.1.0"));

    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .expect("OpenAPI document should contain paths");

    assert!(
        operation(&doc, "/api/products", "get").is_some(),
        "GET product endpoint should become an OpenAPI operation"
    );
    assert!(
        operation(&doc, "/graphql", "post").is_some(),
        "POST GraphQL endpoint should become an OpenAPI operation"
    );
    assert_eq!(
        operation_count(paths),
        2,
        "every learned endpoint should become exactly one operation"
    );
}

#[test]
fn unsafe_operations_require_confirmation_extension() {
    let doc = export_openapi(&sample_endpoints());

    let get_operation =
        operation(&doc, "/api/products", "get").expect("GET product endpoint should be exported");
    let post_operation =
        operation(&doc, "/graphql", "post").expect("POST GraphQL endpoint should be exported");

    assert_ne!(
        get_operation.get("x-webclaw-requires-confirmation"),
        Some(&json!(true)),
        "safe GET operations should not require unsafe replay confirmation"
    );
    assert_eq!(
        post_operation.get("x-webclaw-requires-confirmation"),
        Some(&json!(true)),
        "unsafe POST operations should require explicit replay confirmation"
    );
}

#[test]
fn generated_examples_do_not_leak_secret_values() {
    let doc = export_openapi(&sample_endpoints());

    assert!(
        contains_example_node(&doc),
        "OpenAPI export should include examples derived from captured endpoint examples"
    );

    let doc_text = serde_json::to_string(&doc).expect("serialize OpenAPI document");
    for forbidden in [
        "Bearer raw-secret",
        "raw-api-key",
        "raw-csrf-token",
        "raw-session-id",
        "raw-password",
        "user@example.test",
    ] {
        assert!(
            !doc_text.contains(forbidden),
            "OpenAPI examples should not leak secret value {forbidden:?}"
        );
    }
    assert!(
        doc_text.contains("[REDACTED]"),
        "OpenAPI examples should preserve redaction markers instead of raw secrets"
    );
}

#[test]
fn write_openapi_writes_openapi_json_next_to_saved_endpoints() {
    let root = unique_temp_root("write");

    with_capture_dir(Some(&root), || {
        let artifact = sample_artifact();
        save_capture(&artifact).expect("save capture before OpenAPI export");

        let openapi_path = write_openapi(&artifact.id).expect("write OpenAPI document");

        assert_eq!(
            openapi_path,
            root.join("example.test")
                .join("2026-05-16T12-00-00Z")
                .join("openapi.json")
        );
        assert!(openapi_path.is_file());

        let doc: Value = read_json(&openapi_path);
        assert_eq!(doc.get("openapi").and_then(Value::as_str), Some("3.1.0"));
        assert!(
            operation(&doc, "/api/products", "get").is_some(),
            "written OpenAPI document should contain saved capture endpoints"
        );
    });

    let _ = fs::remove_dir_all(root);
}

fn sample_artifact() -> CaptureArtifact {
    CaptureArtifact {
        id: "example.test/2026-05-16T12-00-00Z".to_owned(),
        source_url: "https://example.test/products?email=user@example.test".to_owned(),
        intent: Some("discover product listing API".to_owned()),
        started_at: test_time(),
        completed_at: Some(test_time()),
        exchanges: Vec::new(),
        endpoints: sample_endpoints(),
        metadata: Map::new(),
    }
}

fn sample_endpoints() -> Vec<EndpointDefinition> {
    vec![product_endpoint(), graphql_endpoint()]
}

fn product_endpoint() -> EndpointDefinition {
    let mut query_params = BTreeMap::new();
    query_params.insert("category".to_owned(), vec!["tools".to_owned()]);
    query_params.insert("page".to_owned(), vec!["2".to_owned()]);

    EndpointDefinition {
        id: "GET https://example.test/api/products".to_owned(),
        method: "GET".to_owned(),
        origin: "https://example.test".to_owned(),
        path_template: "/api/products".to_owned(),
        query_params,
        request_schema: None,
        response_schema: Some(json!({
            "type": "object",
            "properties": {
                "items": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": { "type": "integer" },
                            "name": { "type": "string" }
                        }
                    }
                }
            }
        })),
        auth_evidence: vec!["Authorization header observed".to_owned()],
        safety: EndpointSafety {
            safe_to_replay: true,
            requires_confirmation: false,
            reason: "GET is a read-oriented HTTP method".to_owned(),
        },
        examples: vec![EndpointExample {
            url: "https://example.test/api/products?category=tools&page=2&api_key=raw-api-key"
                .to_owned(),
            request_headers: headers(&[
                ("Authorization", "Bearer raw-secret"),
                ("Accept", "application/json"),
                ("X-Api-Key", "raw-api-key"),
            ]),
            request_body_sample: None,
            response_status: 200,
            response_headers: headers(&[
                ("Content-Type", "application/json"),
                ("Set-Cookie", "session=raw-session-id"),
            ]),
            response_body_sample: Some(
                r#"{"items":[{"id":12345,"name":"Hammer","email":"user@example.test"}]}"#
                    .to_owned(),
            ),
            captured_at: test_time(),
        }],
    }
}

fn graphql_endpoint() -> EndpointDefinition {
    EndpointDefinition {
        id: "POST https://example.test/graphql".to_owned(),
        method: "POST".to_owned(),
        origin: "https://example.test".to_owned(),
        path_template: "/graphql".to_owned(),
        query_params: BTreeMap::new(),
        request_schema: Some(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "variables": { "type": "object" }
            }
        })),
        response_schema: Some(json!({
            "type": "object",
            "properties": {
                "data": { "type": "object" }
            }
        })),
        auth_evidence: vec!["X-CSRF-Token header observed".to_owned()],
        safety: EndpointSafety {
            safe_to_replay: false,
            requires_confirmation: true,
            reason: "POST may mutate server state and requires confirmation".to_owned(),
        },
        examples: vec![EndpointExample {
            url: concat!(
                "https://example.test/graphql?",
                "ref=user%40example.test&",
                "debug=Bearer%20raw-secret&",
                "trace=raw-session-id"
            )
            .to_owned(),
            request_headers: headers(&[
                ("Content-Type", "application/json"),
                ("X-CSRF-Token", "raw-csrf-token"),
            ]),
            request_body_sample: Some(
                json!({
                    "query": "mutation CreateProduct($name: String!) { createProduct(name: $name) { id } }",
                    "variables": {
                        "name": "Hammer",
                        "password": "raw-password"
                    }
                })
                .to_string(),
            ),
            response_status: 200,
            response_headers: headers(&[("Content-Type", "application/json")]),
            response_body_sample: Some(r#"{"data":{"createProduct":{"id":"12345"}}}"#.to_owned()),
            captured_at: test_time(),
        }],
    }
}

fn headers(entries: &[(&str, &str)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

fn operation<'a>(doc: &'a Value, path: &str, method: &str) -> Option<&'a Map<String, Value>> {
    doc.get("paths")
        .and_then(Value::as_object)
        .and_then(|paths| paths.get(path))
        .and_then(Value::as_object)
        .and_then(|path_item| path_item.get(method))
        .and_then(Value::as_object)
}

fn operation_count(paths: &Map<String, Value>) -> usize {
    const HTTP_METHODS: &[&str] = &[
        "get", "put", "post", "delete", "options", "head", "patch", "trace",
    ];

    paths
        .values()
        .filter_map(Value::as_object)
        .map(|path_item| {
            HTTP_METHODS
                .iter()
                .filter(|method| path_item.contains_key(**method))
                .count()
        })
        .sum()
}

fn contains_example_node(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            object
                .keys()
                .any(|key| matches!(key.as_str(), "example" | "examples" | "x-webclaw-examples"))
                || object.values().any(contains_example_node)
        }
        Value::Array(items) => items.iter().any(contains_example_node),
        _ => false,
    }
}

fn unique_temp_root(test_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_nanos();

    env::temp_dir().join(format!(
        "webclaw-capture-openapi-{test_name}-{}-{nanos}",
        std::process::id()
    ))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let contents = fs::read_to_string(path).expect("read JSON file");
    serde_json::from_str(&contents).expect("valid JSON file")
}

fn test_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-05-16T12:00:00Z")
        .expect("valid test timestamp")
        .with_timezone(&Utc)
}
