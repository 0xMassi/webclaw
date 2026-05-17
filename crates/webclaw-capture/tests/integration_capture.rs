use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use webclaw_capture::cdp::{CaptureOptions, capture_network};
use webclaw_capture::types::{CaptureArtifact, EndpointDefinition};

const CAPTURE_DIR_ENV: &str = "WEBCLAW_CAPTURE_DIR";

struct CaptureDirGuard {
    original: Option<OsString>,
}

impl CaptureDirGuard {
    fn set(path: &Path) -> Self {
        let original = env::var_os(CAPTURE_DIR_ENV);

        unsafe {
            env::set_var(CAPTURE_DIR_ENV, path);
        }

        Self { original }
    }
}

impl Drop for CaptureDirGuard {
    fn drop(&mut self) {
        unsafe {
            match &self.original {
                Some(value) => env::set_var(CAPTURE_DIR_ENV, value),
                None => env::remove_var(CAPTURE_DIR_ENV),
            }
        }
    }
}

struct LocalServer {
    base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
}

impl LocalServer {
    async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local test server");
        let address = listener.local_addr().expect("local test server address");
        let (shutdown, mut shutdown_rx) = oneshot::channel::<()>();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = &mut shutdown_rx => break,
                    accepted = listener.accept() => {
                        let Ok((stream, _peer)) = accepted else {
                            continue;
                        };

                        tokio::spawn(handle_connection(stream));
                    }
                }
            }
        });

        Self {
            base_url: format!("http://{address}"),
            shutdown: Some(shutdown),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
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
async fn capture_network_records_fetches_redacts_secrets_and_learns_api_endpoints() {
    let capture_root = unique_temp_root("integration-capture");
    let _capture_dir = CaptureDirGuard::set(&capture_root);
    let server = LocalServer::start().await;

    let saved = capture_network(CaptureOptions {
        url: server.url("/"),
        intent: Some("discover product listing API".to_owned()),
        wait_ms: 1_500,
        headed: false,
    })
    .await
    .expect("capture network traffic");

    let raw_capture: CaptureArtifact = read_json(&saved.raw_capture_path);
    assert!(
        raw_capture
            .exchanges
            .iter()
            .any(|exchange| exchange.url.contains("/api/products?category=tools")),
        "raw capture should include the fetch to /api/products"
    );

    let redacted_capture_text =
        fs::read_to_string(&saved.redacted_capture_path).expect("read redacted capture");
    for secret in [
        "browser-authorization-secret",
        "browser-api-key-secret",
        "browser-csrf-secret",
        "page-session-secret",
        "api-session-secret",
    ] {
        assert!(
            !redacted_capture_text.contains(secret),
            "redacted capture should not contain raw secret value {secret}"
        );
    }

    let endpoints: Vec<EndpointDefinition> = read_json(&saved.endpoints_path);
    let api_endpoints = endpoints
        .iter()
        .filter(|endpoint| endpoint.method == "GET" && endpoint.path_template == "/api/products")
        .collect::<Vec<_>>();

    assert_eq!(
        api_endpoints.len(),
        1,
        "inferred endpoints should contain one GET /api/products endpoint"
    );
    assert!(
        endpoints
            .iter()
            .all(|endpoint| endpoint.path_template != "/static/app.js"),
        "static assets should not be included as learned endpoints"
    );

    let _ = fs::remove_dir_all(capture_root);
}

async fn handle_connection(mut stream: TcpStream) {
    let mut buffer = vec![0_u8; 8192];
    let Ok(bytes_read) = stream.read(&mut buffer).await else {
        return;
    };
    if bytes_read == 0 {
        return;
    }

    let request = String::from_utf8_lossy(&buffer[..bytes_read]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/");

    let response = match path.split('?').next().unwrap_or(path) {
        "/" => http_response(
            "200 OK",
            &[
                ("Content-Type", "text/html; charset=utf-8"),
                ("Set-Cookie", "session=page-session-secret; HttpOnly"),
            ],
            r#"<!doctype html>
<html>
  <head><title>Webclaw capture test</title></head>
  <body>
    <script src="/static/app.js"></script>
  </body>
</html>"#,
        ),
        "/static/app.js" => http_response(
            "200 OK",
            &[("Content-Type", "application/javascript; charset=utf-8")],
            r#"fetch('/api/products?category=tools', {
  headers: {
    'Authorization': 'Bearer browser-authorization-secret',
    'X-Api-Key': 'browser-api-key-secret',
    'X-CSRF-Token': 'browser-csrf-secret'
  }
}).then(response => response.json()).then(products => {
  window.__webclawProducts = products;
});"#,
        ),
        "/api/products" => http_response(
            "200 OK",
            &[
                ("Content-Type", "application/json"),
                ("Set-Cookie", "session=api-session-secret; HttpOnly"),
            ],
            r#"{"items":[{"id":12345,"name":"Hammer","category":"tools"}]}"#,
        ),
        _ => http_response(
            "404 Not Found",
            &[("Content-Type", "text/plain; charset=utf-8")],
            "not found",
        ),
    };

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

fn unique_temp_root(test_name: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time after unix epoch")
        .as_nanos();

    env::temp_dir().join(format!(
        "webclaw-capture-{test_name}-{}-{nanos}",
        std::process::id()
    ))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> T {
    let contents = fs::read_to_string(path).expect("read JSON file");
    serde_json::from_str(&contents).expect("valid JSON file")
}
