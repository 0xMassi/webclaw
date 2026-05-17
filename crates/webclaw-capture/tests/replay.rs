use std::collections::BTreeMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use webclaw_capture::replay::replay_endpoint;
use webclaw_capture::types::{
    EndpointDefinition, EndpointExample, EndpointSafety, ReplayOptions, ReplayResult,
};

struct LocalServer {
    base_url: String,
    requests: mpsc::UnboundedReceiver<String>,
    shutdown: Option<oneshot::Sender<()>>,
}

impl LocalServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local replay test server");
        let address = listener.local_addr().expect("local replay server address");
        let (shutdown, mut shutdown_rx) = oneshot::channel::<()>();
        let (requests_tx, requests_rx) = mpsc::unbounded_channel::<String>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else {
                            continue;
                        };

                        tokio::spawn(handle_connection(stream, requests_tx.clone()));
                    }
                }
            }
        });

        Self {
            base_url: format!("http://{address}"),
            requests: requests_rx,
            shutdown: Some(shutdown),
        }
    }

    async fn next_request(&mut self) -> String {
        tokio::time::timeout(Duration::from_secs(2), self.requests.recv())
            .await
            .expect("local replay server should receive a request")
            .expect("local replay server request channel should remain open")
    }
}

impl Drop for LocalServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_endpoint_executes_when_dry_run_is_false() {
    let mut server = LocalServer::start().await;
    let endpoint = get_endpoint(&server.base_url, headers(&[("Accept", "application/json")]));

    let result = replay_endpoint(
        &endpoint,
        ReplayOptions {
            dry_run: false,
            confirm_unsafe: false,
            params_json: Some(json!({ "category": "tools" })),
            headers: Map::new(),
            body_json: None,
        },
    )
    .await
    .expect("replay GET endpoint");

    match result {
        ReplayResult::Executed {
            status,
            body_sample,
            ..
        } => {
            assert_eq!(status, 200);
            assert!(
                body_sample
                    .as_deref()
                    .unwrap_or_default()
                    .contains(r#""ok":true"#),
                "executed replay should return the response body sample"
            );
        }
        other => panic!("GET replay should execute, got {other:#?}"),
    }

    let request = server.next_request().await;
    assert!(
        request.starts_with("GET /api/products"),
        "server should receive the replayed GET request, got {request:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_endpoint_with_dry_run_returns_preview_without_network() {
    let endpoint = get_endpoint(
        "http://127.0.0.1:9",
        headers(&[("Accept", "application/json")]),
    );

    let result = replay_endpoint(
        &endpoint,
        ReplayOptions {
            dry_run: true,
            confirm_unsafe: false,
            params_json: Some(json!({ "category": "tools" })),
            headers: headers(&[("X-Replay-Trace", "dry-run")]),
            body_json: None,
        },
    )
    .await
    .expect("preview GET endpoint");

    match result {
        ReplayResult::Preview {
            method,
            url,
            headers,
            body_sample,
        } => {
            assert_eq!(method, "GET");
            assert!(url.starts_with("http://127.0.0.1:9/api/products"));
            assert!(url.contains("category=tools"));
            assert_eq!(header_string(&headers, "X-Replay-Trace"), Some("dry-run"));
            assert_eq!(body_sample, None);
        }
        other => panic!("dry-run GET replay should return a preview, got {other:#?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_without_confirmation_is_blocked() {
    let endpoint = post_endpoint("http://127.0.0.1:9");

    let result = replay_endpoint(
        &endpoint,
        ReplayOptions {
            dry_run: false,
            confirm_unsafe: false,
            params_json: None,
            headers: Map::new(),
            body_json: Some(graphql_body()),
        },
    )
    .await
    .expect("block unsafe POST replay");

    match result {
        ReplayResult::Blocked { reason } => {
            let reason = reason.to_ascii_lowercase();
            assert!(
                reason.contains("confirm") || reason.contains("unsafe"),
                "blocked replay should explain confirmation is required, got {reason:?}"
            );
        }
        other => {
            panic!("unsafe POST replay without confirmation should be blocked, got {other:#?}")
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn post_with_dry_run_returns_preview_only() {
    let endpoint = post_endpoint("http://127.0.0.1:9");

    let result = replay_endpoint(
        &endpoint,
        ReplayOptions {
            dry_run: true,
            confirm_unsafe: false,
            params_json: None,
            headers: headers(&[("Content-Type", "application/json")]),
            body_json: Some(graphql_body()),
        },
    )
    .await
    .expect("preview unsafe POST replay");

    match result {
        ReplayResult::Preview {
            method,
            url,
            body_sample,
            ..
        } => {
            assert_eq!(method, "POST");
            assert_eq!(url, "http://127.0.0.1:9/graphql");
            assert!(
                body_sample
                    .as_deref()
                    .unwrap_or_default()
                    .contains("CreateProduct"),
                "dry-run POST preview should include the request body sample"
            );
        }
        other => panic!("dry-run POST replay should return a preview, got {other:#?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn redacted_headers_are_never_sent() {
    let mut server = LocalServer::start().await;
    let endpoint = get_endpoint(
        &server.base_url,
        headers(&[
            ("Authorization", "[REDACTED]"),
            ("Cookie", "[REDACTED]"),
            ("X-Api-Key", "[REDACTED]"),
            ("X-Trace-Id", "captured-trace"),
        ]),
    );

    let result = replay_endpoint(
        &endpoint,
        ReplayOptions {
            dry_run: false,
            confirm_unsafe: false,
            params_json: None,
            headers: headers(&[
                ("X-User-Email", "[REDACTED]"),
                ("X-Allowed-Override", "override-ok"),
            ]),
            body_json: None,
        },
    )
    .await
    .expect("replay GET endpoint without redacted headers");

    assert!(
        matches!(result, ReplayResult::Executed { status: 200, .. }),
        "GET replay should execute, got {result:#?}"
    );

    let request = server.next_request().await;
    let lower_request = request.to_ascii_lowercase();

    for forbidden in [
        "authorization:",
        "cookie:",
        "x-api-key:",
        "x-user-email:",
        "[redacted]",
    ] {
        assert!(
            !lower_request.contains(forbidden),
            "replay request should not send redacted header material {forbidden:?}: {request}"
        );
    }
    assert!(
        lower_request.contains("x-allowed-override: override-ok"),
        "non-redacted caller-supplied headers should still be sent: {request}"
    );
}

async fn handle_connection(mut stream: TcpStream, requests: mpsc::UnboundedSender<String>) {
    let mut buffer = vec![0_u8; 8192];
    let Ok(bytes_read) = stream.read(&mut buffer).await else {
        return;
    };
    if bytes_read == 0 {
        return;
    }

    let request = String::from_utf8_lossy(&buffer[..bytes_read]).to_string();
    let status = if request.starts_with("GET /api/products") {
        "200 OK"
    } else {
        "404 Not Found"
    };
    let body = if status == "200 OK" {
        r#"{"ok":true,"items":[{"id":12345,"name":"Hammer"}]}"#
    } else {
        r#"{"ok":false}"#
    };
    let response = http_response(status, &[("Content-Type", "application/json")], body);

    let _ = requests.send(request);
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn http_response(status: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut response = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n",
        body.len()
    );

    for (name, value) in headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }

    response.push_str("\r\n");
    response.push_str(body);
    response
}

fn get_endpoint(origin: &str, request_headers: Map<String, Value>) -> EndpointDefinition {
    let mut query_params = BTreeMap::new();
    query_params.insert("category".to_owned(), vec!["tools".to_owned()]);

    EndpointDefinition {
        id: format!("GET {origin}/api/products"),
        method: "GET".to_owned(),
        origin: origin.to_owned(),
        path_template: "/api/products".to_owned(),
        query_params,
        request_schema: None,
        response_schema: Some(json!({
            "type": "object",
            "properties": {
                "items": { "type": "array" }
            }
        })),
        auth_evidence: Vec::new(),
        safety: EndpointSafety {
            safe_to_replay: true,
            requires_confirmation: false,
            reason: "GET is a read-oriented HTTP method".to_owned(),
        },
        examples: vec![EndpointExample {
            url: format!("{origin}/api/products?category=tools"),
            request_headers,
            request_body_sample: None,
            response_status: 200,
            response_headers: headers(&[("Content-Type", "application/json")]),
            response_body_sample: Some(r#"{"items":[{"id":12345,"name":"Hammer"}]}"#.to_owned()),
            captured_at: test_time(),
        }],
    }
}

fn post_endpoint(origin: &str) -> EndpointDefinition {
    EndpointDefinition {
        id: format!("POST {origin}/graphql"),
        method: "POST".to_owned(),
        origin: origin.to_owned(),
        path_template: "/graphql".to_owned(),
        query_params: BTreeMap::new(),
        request_schema: Some(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string" },
                "variables": { "type": "object" }
            }
        })),
        response_schema: Some(json!({ "type": "object" })),
        auth_evidence: vec!["X-CSRF-Token header observed".to_owned()],
        safety: EndpointSafety {
            safe_to_replay: false,
            requires_confirmation: true,
            reason: "POST may mutate server state and requires confirmation".to_owned(),
        },
        examples: vec![EndpointExample {
            url: format!("{origin}/graphql"),
            request_headers: headers(&[
                ("Content-Type", "application/json"),
                ("X-CSRF-Token", "[REDACTED]"),
            ]),
            request_body_sample: Some(graphql_body().to_string()),
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

fn header_string<'a>(headers: &'a Map<String, Value>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(header_name, _value)| header_name.eq_ignore_ascii_case(name))
        .and_then(|(_header_name, value)| value.as_str())
}

fn graphql_body() -> Value {
    json!({
        "query": "mutation CreateProduct($name: String!) { createProduct(name: $name) { id } }",
        "variables": {
            "name": "Hammer"
        }
    })
}

fn test_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-05-16T12:00:00Z")
        .expect("valid test timestamp")
        .with_timezone(&Utc)
}
