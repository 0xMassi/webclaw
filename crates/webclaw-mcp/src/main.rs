/// webclaw-mcp: MCP (Model Context Protocol) server for webclaw.
/// Exposes web extraction tools to AI agents over stdio, or over streamable
/// HTTP with `--http`.
mod handshake_guard;
mod server;
mod tools;

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::Request;
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::ServiceExt;
use rmcp::service::RoleServer;
use rmcp::transport::async_rw::AsyncRwTransport;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};

use handshake_guard::PreHandshakeGuard;
use server::WebclawMcp;

const DEFAULT_HTTP_ADDR: &str = "127.0.0.1:8000";
const USAGE: &str = "usage: webclaw-mcp [--http [ADDR] [--no-auth]]

  (no flags)     serve MCP over stdio
  --http [ADDR]  serve MCP over streamable HTTP at http://ADDR/mcp (default 127.0.0.1:8000)
  --no-auth      serve HTTP without a token

Over HTTP every request needs `Authorization: Bearer <token>` with the token in
WEBCLAW_MCP_TOKEN, because anyone who reaches the server can make this machine
fetch any URL. --no-auth turns that off.";

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{USAGE}");
        return Ok(());
    }
    let (http, no_auth) = http_args(&args).unwrap_or_else(|e| fail(&e));
    let token = std::env::var("WEBCLAW_MCP_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    if let Some(addr) = http {
        match (&token, no_auth) {
            (None, false) => fail(&format!(
                "refusing to serve MCP on {addr} without a token: anyone who reaches it could make \
                 this machine fetch any URL. Set WEBCLAW_MCP_TOKEN, or pass --no-auth to serve it open anyway"
            )),
            (None, true) => eprintln!(
                "webclaw-mcp: warning: serving without a token; anyone who reaches {addr} can use every tool"
            ),
            (Some(_), true) => eprintln!(
                "webclaw-mcp: warning: WEBCLAW_MCP_TOKEN is set, so --no-auth is ignored and the token is required"
            ),
            (Some(_), false) => {}
        }
    }

    #[cfg(all(unix, feature = "browser"))]
    webclaw_fetch::render::close_profiles_on_terminate();
    let mcp = WebclawMcp::new().await;
    let close_browser = mcp.close_browser();
    match http {
        Some(addr) => serve_http(mcp, addr, token).await?,
        None => {
            let transport =
                PreHandshakeGuard::new(AsyncRwTransport::<RoleServer, _, _>::new_server(
                    tokio::io::stdin(),
                    tokio::io::stdout(),
                ));
            mcp.serve(transport).await?.waiting().await?;
        }
    }
    close_browser.await;
    Ok(())
}

fn fail(message: &str) -> ! {
    eprintln!("error: {message}");
    std::process::exit(1)
}

fn http_args(args: &[String]) -> Result<(Option<SocketAddr>, bool), String> {
    let no_auth = args.iter().any(|a| a == "--no-auth");
    let rest: Vec<&str> = args
        .iter()
        .map(String::as_str)
        .filter(|a| *a != "--no-auth")
        .collect();
    let addr = match rest.as_slice() {
        [] if !no_auth => None,
        ["--http"] => DEFAULT_HTTP_ADDR.parse().ok(),
        ["--http", addr] => Some(addr.parse().map_err(|_| {
            format!("invalid address `{addr}`; expected HOST:PORT such as 127.0.0.1:8000")
        })?),
        _ => return Err(USAGE.to_string()),
    };
    Ok((addr, no_auth))
}

async fn serve_http(
    mcp: WebclawMcp,
    addr: SocketAddr,
    token: Option<String>,
) -> std::io::Result<()> {
    let config = StreamableHttpServerConfig::default();
    let cancel = config.cancellation_token.clone();
    let listener = tokio::net::TcpListener::bind(addr).await?;
    eprintln!(
        "webclaw-mcp: serving MCP at http://{}/mcp{}",
        listener.local_addr()?,
        if token.is_some() {
            " (bearer token required)"
        } else {
            ""
        }
    );
    axum::serve(
        listener,
        http_app(mcp, addr.ip().is_loopback(), token, config),
    )
    .with_graceful_shutdown(async move {
        let _ = tokio::signal::ctrl_c().await;
        cancel.cancel();
    })
    .await
}

fn http_app(
    mcp: WebclawMcp,
    loopback: bool,
    token: Option<String>,
    config: StreamableHttpServerConfig,
) -> axum::Router {
    let config = if loopback {
        config
    } else {
        config.disable_allowed_hosts()
    };
    let service = StreamableHttpService::new(
        move || Ok(mcp.clone()),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let app = axum::Router::new().nest_service("/mcp", service);
    match token {
        Some(token) => {
            let token: Arc<str> = token.into();
            app.layer(axum::middleware::from_fn(
                move |request: Request, next: Next| require_token(token.clone(), request, next),
            ))
        }
        None => app,
    }
}

async fn require_token(token: Arc<str>, request: Request, next: Next) -> Response {
    let sent = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(bearer_token);
    if sent.is_some_and(|sent| same_bytes(sent.as_bytes(), token.as_bytes())) {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "missing or wrong bearer token",
        )
            .into_response()
    }
}

fn bearer_token(authorization: &str) -> Option<&str> {
    let (scheme, token) = authorization.split_once(' ')?;
    scheme.eq_ignore_ascii_case("bearer").then(|| token.trim())
}

fn same_bytes(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#;

    async fn post(addr: SocketAddr, host: &str, token: Option<&str>) -> String {
        let mut stream = tokio::net::TcpStream::connect(addr).await.unwrap();
        let auth = token.map_or(String::new(), |t| format!("Authorization: Bearer {t}\r\n"));
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\n{auth}Content-Length: {}\r\nConnection: close\r\n\r\n{INITIALIZE}",
            INITIALIZE.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        let mut chunk = [0; 4096];
        while let Ok(Ok(n)) =
            tokio::time::timeout(std::time::Duration::from_secs(5), stream.read(&mut chunk)).await
        {
            if n == 0 {
                break;
            }
            response.extend_from_slice(&chunk[..n]);
            let text = String::from_utf8_lossy(&response);
            if text.contains("serverInfo")
                || text.starts_with("HTTP/1.1 4") && text.contains("\r\n\r\n")
            {
                break;
            }
        }
        String::from_utf8_lossy(&response).into_owned()
    }

    async fn serve(loopback: bool, token: Option<&str>) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = http_app(
            WebclawMcp::new().await,
            loopback,
            token.map(str::to_string),
            StreamableHttpServerConfig::default(),
        );
        tokio::spawn(async move { axum::serve(listener, app).await });
        addr
    }

    #[tokio::test]
    async fn http_needs_the_bearer_token_when_one_is_set() {
        let addr = serve(true, Some("s3cret")).await;
        let host = addr.to_string();
        assert!(post(addr, &host, None).await.starts_with("HTTP/1.1 401"));
        assert!(
            post(addr, &host, Some("wrong!"))
                .await
                .starts_with("HTTP/1.1 401")
        );
        let ok = post(addr, &host, Some("s3cret")).await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains("webclaw-mcp"), "{ok}");
    }

    #[tokio::test]
    async fn a_loopback_server_rejects_foreign_host_headers() {
        let addr = serve(true, None).await;
        assert!(
            post(addr, &addr.to_string(), None)
                .await
                .starts_with("HTTP/1.1 200")
        );
        assert!(
            !post(addr, "evil.example", None)
                .await
                .starts_with("HTTP/1.1 200")
        );
    }

    #[test]
    fn http_flags_parse_an_optional_address_and_no_auth() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(http_args(&args(&[])), Ok((None, false)));
        assert_eq!(
            http_args(&args(&["--http"])),
            Ok((Some("127.0.0.1:8000".parse().unwrap()), false))
        );
        assert_eq!(
            http_args(&args(&["--http", "0.0.0.0:9000", "--no-auth"])),
            Ok((Some("0.0.0.0:9000".parse().unwrap()), true))
        );
        assert!(http_args(&args(&["--http", "nope"])).is_err());
        assert!(http_args(&args(&["--no-auth"])).is_err());
        assert!(http_args(&args(&["--stdio"])).is_err());
    }

    #[test]
    fn the_bearer_scheme_matches_in_any_case() {
        assert_eq!(bearer_token("Bearer abc"), Some("abc"));
        assert_eq!(bearer_token("bearer abc"), Some("abc"));
        assert_eq!(bearer_token("BEARER  abc "), Some("abc"));
        assert_eq!(bearer_token("Basic abc"), None);
        assert_eq!(bearer_token("abc"), None);
    }

    #[test]
    fn token_comparison_needs_equal_bytes() {
        assert!(same_bytes(b"abc", b"abc"));
        assert!(!same_bytes(b"abc", b"abd"));
        assert!(!same_bytes(b"abc", b"abcd"));
    }
}
