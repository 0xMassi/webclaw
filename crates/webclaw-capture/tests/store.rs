use std::collections::BTreeMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use url::Url;
use webclaw_capture::redact::redact_artifact;
use webclaw_capture::store::{
    capture_id_for, capture_root, find_endpoint, load_endpoints, save_capture,
};
use webclaw_capture::types::{
    CaptureArtifact, CapturedExchange, EndpointDefinition, EndpointExample, EndpointSafety,
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

fn unique_temp_root(test_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_nanos();

    env::temp_dir().join(format!(
        "webclaw-capture-store-{test_name}-{}-{nanos}",
        std::process::id()
    ))
}

fn test_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-05-16T12:00:00Z")
        .expect("valid test timestamp")
        .with_timezone(&Utc)
}

fn headers(entries: &[(&str, &str)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

fn sample_endpoint() -> EndpointDefinition {
    let mut query_params = BTreeMap::new();
    query_params.insert("category".to_owned(), vec!["tools".to_owned()]);

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
                    "items": { "type": "object" }
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
            url: "https://example.test/api/products?category=tools".to_owned(),
            request_headers: headers(&[
                ("Authorization", "Bearer raw-secret"),
                ("Accept", "application/json"),
            ]),
            request_body_sample: None,
            response_status: 200,
            response_headers: headers(&[("Content-Type", "application/json")]),
            response_body_sample: Some(r#"{"items":[{"id":12345,"name":"Hammer"}]}"#.to_owned()),
            captured_at: test_time(),
        }],
    }
}

fn sample_exchange() -> CapturedExchange {
    CapturedExchange {
        method: "GET".to_owned(),
        url: "https://example.test/api/products?category=tools&token=raw-secret".to_owned(),
        request_headers: headers(&[
            ("Authorization", "Bearer raw-secret"),
            ("Accept", "application/json"),
        ]),
        request_body_sample: None,
        resource_type: Some("fetch".to_owned()),
        status: 200,
        response_headers: headers(&[("Content-Type", "application/json")]),
        response_body_sample: Some(r#"{"items":[{"id":12345,"name":"Hammer"}]}"#.to_owned()),
        started_at: test_time(),
        duration_ms: 42,
        redirect_chain: vec!["https://example.test/login?session=raw-secret".to_owned()],
    }
}

fn sample_artifact() -> CaptureArtifact {
    let mut metadata = Map::new();
    metadata.insert("runner".to_owned(), json!("store-test"));

    CaptureArtifact {
        id: "example.test/2026-05-16T12-00-00Z".to_owned(),
        source_url: "https://example.test/products?email=user@example.test".to_owned(),
        intent: Some("discover product listing API".to_owned()),
        started_at: test_time(),
        completed_at: Some(test_time()),
        exchanges: vec![sample_exchange()],
        endpoints: vec![sample_endpoint()],
        metadata,
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let contents = fs::read_to_string(path).expect("read JSON file");
    serde_json::from_str(&contents).expect("valid JSON file")
}

#[test]
fn default_capture_root_resolves_under_user_profile_webclaw_api_captures() {
    with_capture_dir(None, || {
        let home = env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
            .expect("home directory");

        assert_eq!(capture_root(), home.join(".webclaw").join("api-captures"));
    });
}

#[test]
fn capture_root_uses_webclaw_capture_dir_override() {
    let root = unique_temp_root("override");

    with_capture_dir(Some(&root), || {
        assert_eq!(capture_root(), root);
    });
}

#[test]
fn capture_id_for_uses_domain_and_filesystem_safe_utc_timestamp() {
    let url = Url::parse("https://example.test/api/products?category=tools").expect("valid URL");

    assert_eq!(
        capture_id_for(&url, test_time()),
        "example.test/2026-05-16T12-00-00Z"
    );
}

#[test]
fn save_capture_writes_raw_redacted_endpoints_and_metadata_files() {
    let root = unique_temp_root("save");

    with_capture_dir(Some(&root), || {
        let artifact = sample_artifact();
        let saved = save_capture(&artifact).expect("save capture");

        assert_eq!(saved.id, artifact.id);
        assert_eq!(
            saved.capture_dir,
            root.join("example.test").join("2026-05-16T12-00-00Z")
        );
        assert_eq!(
            saved.raw_capture_path,
            saved.capture_dir.join("raw-capture.json")
        );
        assert_eq!(
            saved.redacted_capture_path,
            saved.capture_dir.join("redacted-capture.json")
        );
        assert_eq!(
            saved.endpoints_path,
            saved.capture_dir.join("endpoints.json")
        );
        assert_eq!(saved.metadata_path, saved.capture_dir.join("metadata.json"));

        assert!(saved.raw_capture_path.is_file());
        assert!(saved.redacted_capture_path.is_file());
        assert!(saved.endpoints_path.is_file());
        assert!(saved.metadata_path.is_file());

        let raw_capture: CaptureArtifact = read_json(&saved.raw_capture_path);
        assert_eq!(raw_capture, artifact);

        let redacted_capture: CaptureArtifact = read_json(&saved.redacted_capture_path);
        assert_ne!(redacted_capture, artifact);
        assert!(
            !serde_json::to_string(&redacted_capture)
                .expect("serialize redacted capture")
                .contains("raw-secret"),
            "redacted capture should not contain raw secrets"
        );

        let endpoints: Vec<EndpointDefinition> = read_json(&saved.endpoints_path);
        assert_eq!(endpoints, redact_artifact(&artifact).endpoints);
        assert!(
            !serde_json::to_string(&endpoints)
                .expect("serialize endpoints")
                .contains("raw-secret"),
            "endpoints.json should not contain raw secrets"
        );

        let metadata: Value = read_json(&saved.metadata_path);
        assert!(
            metadata.is_object(),
            "metadata.json should contain a JSON object"
        );
        let metadata_text = serde_json::to_string(&metadata).expect("serialize metadata");
        assert!(
            !metadata_text.contains("user@example.test"),
            "metadata.json should redact PII from source_url"
        );
        assert!(
            metadata_text.contains("REDACTED"),
            "metadata.json should preserve the redaction marker"
        );
    });

    let _ = fs::remove_dir_all(root);
}

#[test]
fn load_endpoints_by_capture_id_reads_endpoints_json() {
    let root = unique_temp_root("load");

    with_capture_dir(Some(&root), || {
        let artifact = sample_artifact();
        save_capture(&artifact).expect("save capture");

        let loaded = load_endpoints(&artifact.id).expect("load endpoints");

        assert_eq!(loaded, redact_artifact(&artifact).endpoints);
        assert!(
            !serde_json::to_string(&loaded)
                .expect("serialize loaded endpoints")
                .contains("raw-secret"),
            "loaded endpoints should not contain raw secrets"
        );
    });

    let _ = fs::remove_dir_all(root);
}

#[test]
fn find_endpoint_scans_saved_capture_endpoints() {
    let root = unique_temp_root("find");

    with_capture_dir(Some(&root), || {
        let artifact = sample_artifact();
        let expected = redact_artifact(&artifact).endpoints[0].clone();
        save_capture(&artifact).expect("save capture");

        let found = find_endpoint(&expected.id).expect("find endpoint");

        assert_eq!(found, expected);
        assert!(
            !serde_json::to_string(&found)
                .expect("serialize found endpoint")
                .contains("raw-secret"),
            "found endpoint should not contain raw secrets"
        );
    });

    let _ = fs::remove_dir_all(root);
}
