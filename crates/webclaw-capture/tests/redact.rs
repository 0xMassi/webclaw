use chrono::{TimeZone, Utc};
use serde_json::{Map, Value, json};
use url::Url;
use webclaw_capture::redact::{redact_artifact, redact_headers, redact_json, redact_url};
use webclaw_capture::types::{CaptureArtifact, CapturedExchange};

const REDACTED: &str = "[REDACTED]";

fn header_map(entries: &[(&str, &str)]) -> Map<String, Value> {
    entries
        .iter()
        .map(|(name, value)| ((*name).to_owned(), Value::String((*value).to_owned())))
        .collect()
}

fn query_value(url: &str, name: &str) -> Option<String> {
    Url::parse(url)
        .unwrap()
        .query_pairs()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

#[test]
fn redacts_sensitive_header_and_cookie_values_by_name() {
    let headers = header_map(&[
        ("Authorization", "Bearer secret-token"),
        ("Cookie", "session=secret-session; theme=dark"),
        ("Set-Cookie", "account=secret-cookie; HttpOnly"),
        ("X-Api-Key", "secret-api-key"),
        ("X-CSRF-Token", "secret-csrf-token"),
        ("X-Session-Id", "secret-session-id"),
        ("X-Password-Hash", "secret-password"),
        ("X-User-Email", "person@example.test"),
        ("Content-Type", "application/json"),
    ]);

    let redacted = redact_headers(&headers);

    assert_eq!(redacted["Authorization"], REDACTED);
    assert_eq!(redacted["Cookie"], REDACTED);
    assert_eq!(redacted["Set-Cookie"], REDACTED);
    assert_eq!(redacted["X-Api-Key"], REDACTED);
    assert_eq!(redacted["X-CSRF-Token"], REDACTED);
    assert_eq!(redacted["X-Session-Id"], REDACTED);
    assert_eq!(redacted["X-Password-Hash"], REDACTED);
    assert_eq!(redacted["X-User-Email"], REDACTED);
    assert_eq!(redacted["Content-Type"], "application/json");
}

#[test]
fn redacts_sensitive_query_parameter_values_by_name() {
    let url = concat!(
        "https://example.test/api/products?",
        "authorization=Bearer%20secret-token&",
        "api-key=secret-api-key&",
        "csrf=secret-csrf&",
        "access_token=secret-access-token&",
        "session_id=secret-session&",
        "password=secret-password&",
        "email=person%40example.test&",
        "cookie=secret-cookie&",
        "page=2"
    );

    let redacted = redact_url(url);

    assert_eq!(
        query_value(&redacted, "authorization").as_deref(),
        Some(REDACTED)
    );
    assert_eq!(query_value(&redacted, "api-key").as_deref(), Some(REDACTED));
    assert_eq!(query_value(&redacted, "csrf").as_deref(), Some(REDACTED));
    assert_eq!(
        query_value(&redacted, "access_token").as_deref(),
        Some(REDACTED)
    );
    assert_eq!(
        query_value(&redacted, "session_id").as_deref(),
        Some(REDACTED)
    );
    assert_eq!(
        query_value(&redacted, "password").as_deref(),
        Some(REDACTED)
    );
    assert_eq!(query_value(&redacted, "email").as_deref(), Some(REDACTED));
    assert_eq!(query_value(&redacted, "cookie").as_deref(), Some(REDACTED));
    assert_eq!(query_value(&redacted, "page").as_deref(), Some("2"));
    assert!(!redacted.contains("secret"));
    assert!(!redacted.contains("person%40example.test"));
}

#[test]
fn redacts_sensitive_json_body_keys_recursively() {
    let body = json!({
        "authorization": "Bearer secret-token",
        "cookie": "session=secret-session",
        "set-cookie": "session=secret-session",
        "api-key": "secret-api-key",
        "csrf": "secret-csrf",
        "access_token": "secret-access-token",
        "session_id": "secret-session",
        "password": "secret-password",
        "email": "person@example.test",
        "profile": {
            "backupEmail": "backup@example.test",
            "display_name": "Visible Name"
        },
        "items": [
            {
                "sessionToken": "nested-secret-session-token",
                "quantity": 3
            }
        ]
    });

    let redacted = redact_json(&body);

    assert_eq!(redacted["authorization"], REDACTED);
    assert_eq!(redacted["cookie"], REDACTED);
    assert_eq!(redacted["set-cookie"], REDACTED);
    assert_eq!(redacted["api-key"], REDACTED);
    assert_eq!(redacted["csrf"], REDACTED);
    assert_eq!(redacted["access_token"], REDACTED);
    assert_eq!(redacted["session_id"], REDACTED);
    assert_eq!(redacted["password"], REDACTED);
    assert_eq!(redacted["email"], REDACTED);
    assert_eq!(redacted["profile"]["backupEmail"], REDACTED);
    assert_eq!(redacted["profile"]["display_name"], "Visible Name");
    assert_eq!(redacted["items"][0]["sessionToken"], REDACTED);
    assert_eq!(redacted["items"][0]["quantity"], 3);
}

#[test]
fn redacts_capture_artifact_headers_urls_and_json_body_samples() {
    let captured_at = Utc.with_ymd_and_hms(2026, 5, 16, 12, 0, 0).unwrap();
    let artifact = CaptureArtifact {
        id: "example.test/2026-05-16T12-00-00Z".to_owned(),
        source_url: "https://example.test/app?email=person@example.test".to_owned(),
        intent: Some("discover public API".to_owned()),
        started_at: captured_at,
        completed_at: Some(captured_at),
        exchanges: vec![CapturedExchange {
            method: "POST".to_owned(),
            url: "https://example.test/api/session?token=secret-token&page=2".to_owned(),
            request_headers: header_map(&[
                ("Authorization", "Bearer secret-token"),
                ("Content-Type", "application/json"),
            ]),
            request_body_sample: Some(
                json!({
                    "email": "person@example.test",
                    "password": "secret-password",
                    "name": "Visible Name"
                })
                .to_string(),
            ),
            resource_type: Some("fetch".to_owned()),
            status: 200,
            response_headers: header_map(&[
                ("Set-Cookie", "session=secret-session; HttpOnly"),
                ("Content-Type", "application/json"),
            ]),
            response_body_sample: Some(
                json!({
                    "sessionToken": "secret-session-token",
                    "status": "ok"
                })
                .to_string(),
            ),
            started_at: captured_at,
            duration_ms: 25,
            redirect_chain: vec!["https://example.test/login?csrf=secret-csrf".to_owned()],
        }],
        endpoints: Vec::new(),
        metadata: Map::new(),
    };

    let redacted = redact_artifact(&artifact);
    let exchange = &redacted.exchanges[0];

    assert_eq!(
        query_value(&redacted.source_url, "email").as_deref(),
        Some(REDACTED)
    );
    assert_eq!(
        query_value(&exchange.url, "token").as_deref(),
        Some(REDACTED)
    );
    assert_eq!(query_value(&exchange.url, "page").as_deref(), Some("2"));
    assert_eq!(exchange.request_headers["Authorization"], REDACTED);
    assert_eq!(exchange.request_headers["Content-Type"], "application/json");
    assert_eq!(exchange.response_headers["Set-Cookie"], REDACTED);
    assert_eq!(
        query_value(&exchange.redirect_chain[0], "csrf").as_deref(),
        Some(REDACTED)
    );

    let request_body = exchange.request_body_sample.as_deref().unwrap();
    assert!(request_body.contains(REDACTED));
    assert!(request_body.contains("Visible Name"));
    assert!(!request_body.contains("person@example.test"));
    assert!(!request_body.contains("secret-password"));

    let response_body = exchange.response_body_sample.as_deref().unwrap();
    assert!(response_body.contains(REDACTED));
    assert!(response_body.contains("ok"));
    assert!(!response_body.contains("secret-session-token"));
}
