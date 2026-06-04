//! webclaw-server — minimal REST API for self-hosting webclaw extraction.
//!
//! This is the OSS reference server. It is intentionally small:
//! single binary, stateless, no database, no job queue. It wraps the
//! same extraction crates the CLI and MCP server use, exposed over
//! HTTP with JSON shapes that mirror the hosted API at
//! api.webclaw.io where the underlying capability exists in OSS.
//!
//! Hosted-only features (anti-bot bypass, JS rendering, async crawl
//! jobs, multi-tenant auth, billing) are *not* implemented here and
//! never will be — they're closed-source. See the docs for the full
//! "what self-hosting gives you vs. what the cloud gives you" matrix.

mod auth;
mod error;
mod routes;
mod state;

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use axum::{
    Router,
    middleware::from_fn_with_state,
    routing::{get, post},
};
use clap::Parser;
use tower_http::cors::{Any, CorsLayer};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

use crate::state::AppState;

/// Hard ceiling on how long any single request may run before the server
/// returns `408 Request Timeout` and drops the work. Generous enough for a
/// cold scrape + LLM round-trip, but bounds the inline `/v1/crawl` handler
/// (up to 500 pages, no job queue) so a slow crawl can't pin a connection
/// and a worker indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Parser, Debug)]
#[command(
    name = "webclaw-server",
    version,
    about = "Minimal self-hosted REST API for webclaw extraction.",
    long_about = "Stateless single-binary REST API. Wraps the OSS extraction \
                  crates over HTTP. For the full hosted platform (anti-bot, \
                  JS render, async jobs, multi-tenant), use api.webclaw.io."
)]
struct Args {
    /// Port to listen on. Env: WEBCLAW_PORT.
    #[arg(short, long, env = "WEBCLAW_PORT", default_value_t = 3000)]
    port: u16,

    /// Host to bind to. Env: WEBCLAW_HOST.
    /// Default `127.0.0.1` keeps the server local-only; set to
    /// `0.0.0.0` to expose on all interfaces (only do this with
    /// `--api-key` set or behind a reverse proxy that adds auth).
    #[arg(long, env = "WEBCLAW_HOST", default_value = "127.0.0.1")]
    host: IpAddr,

    /// Optional bearer token. Env: WEBCLAW_API_KEY. When set, every
    /// `/v1/*` request must present `Authorization: Bearer <key>`.
    /// When unset, the server runs in open mode (no auth) — only
    /// safe on a local-bound interface or behind another auth layer.
    #[arg(long, env = "WEBCLAW_API_KEY")]
    api_key: Option<String>,

    /// Tracing filter. Env: RUST_LOG.
    #[arg(long, env = "RUST_LOG", default_value = "info,webclaw_server=info")]
    log: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    fmt()
        .with_env_filter(EnvFilter::try_new(&args.log).unwrap_or_else(|_| EnvFilter::new("info")))
        .with_target(false)
        .compact()
        .init();

    if is_unspecified_addr(args.host)
        && args.api_key.is_none()
        && std::env::var_os("WEBCLAW_ALLOW_OPEN_PUBLIC").is_none()
    {
        anyhow::bail!(
            "refusing to bind 0.0.0.0/[::] without WEBCLAW_API_KEY; set WEBCLAW_API_KEY or WEBCLAW_ALLOW_OPEN_PUBLIC=1 to override"
        );
    }

    let state = AppState::new(args.api_key.clone()).await?;

    let app = build_app(state);

    let addr = SocketAddr::from((args.host, args.port));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let auth_status = if args.api_key.is_some() {
        "bearer auth required"
    } else {
        "open mode (no auth)"
    };
    info!(%addr, mode = auth_status, "webclaw-server listening");

    axum::serve(listener, app).await?;
    Ok(())
}

/// Build the fully-layered axum router for a given [`AppState`].
///
/// Split out from `main` so the handler tests can exercise the exact same
/// routing + middleware stack (auth, timeout) in-process via
/// `tower::ServiceExt::oneshot`, with no TCP listener.
fn build_app(state: AppState) -> Router {
    let v1 = Router::new()
        .route("/scrape", post(routes::scrape::scrape))
        .route(
            "/scrape/{vertical}",
            post(routes::structured::scrape_vertical),
        )
        .route("/crawl", post(routes::crawl::crawl))
        .route("/map", post(routes::map::map))
        .route("/batch", post(routes::batch::batch))
        .route("/extract", post(routes::extract::extract))
        .route("/extractors", get(routes::structured::list_extractors))
        .route("/summarize", post(routes::summarize::summarize_route))
        .route("/diff", post(routes::diff::diff_route))
        .route("/brand", post(routes::brand::brand))
        .layer(from_fn_with_state(state.clone(), auth::require_bearer));

    Router::new()
        .route("/health", get(routes::health::health))
        .nest("/v1", v1)
        .layer(
            // Permissive CORS — same posture as a self-hosted dev tool.
            // Tighten in front with a reverse proxy if you expose this
            // publicly.
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
                .max_age(Duration::from_secs(3600)),
        )
        // Caps total request time; returns 408 if exceeded. Applied
        // outermost so it covers every route, including the inline crawl.
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

fn is_unspecified_addr(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => ip.is_unspecified(),
        IpAddr::V6(ip) => ip.is_unspecified(),
    }
}

#[cfg(test)]
mod tests {
    //! Hermetic handler tests. Each builds the real router via
    //! [`build_app`] and drives it in-process with
    //! [`tower::ServiceExt::oneshot`] — no TCP listener, no outbound
    //! network. Endpoints that would fetch a URL are reached only on paths
    //! that short-circuit before any network call (auth rejection, format
    //! validation, the static `/v1/extractors` catalog, `/health`).

    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    const TEST_KEY: &str = "test-secret-key";

    async fn app_with_key(key: Option<&str>) -> Router {
        // `AppState::new` probes Ollama once at startup. With no Ollama
        // running the probe returns fast (connection refused) and the
        // tests below never touch the chain, so they stay hermetic either
        // way — no env juggling required.
        let state = AppState::new(key.map(str::to_owned))
            .await
            .expect("build state");
        build_app(state)
    }

    fn get(uri: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .body(Body::empty())
            .expect("request")
    }

    fn get_auth(uri: &str, header: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header("authorization", header)
            .body(Body::empty())
            .expect("request")
    }

    async fn json_body(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        serde_json::from_slice(&bytes).expect("json")
    }

    #[tokio::test]
    async fn health_returns_version() {
        let app = app_with_key(None).await;
        let resp = app.oneshot(get("/health")).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
        let body = json_body(resp).await;
        assert_eq!(body["status"], "ok");
        assert_eq!(body["service"], "webclaw-server");
        assert_eq!(body["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn missing_key_is_unauthorized() {
        let app = app_with_key(Some(TEST_KEY)).await;
        let resp = app.oneshot(get("/v1/extractors")).await.expect("response");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn wrong_key_is_unauthorized() {
        let app = app_with_key(Some(TEST_KEY)).await;
        let resp = app
            .oneshot(get_auth("/v1/extractors", "Bearer wrong-key"))
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn correct_key_authorized() {
        let app = app_with_key(Some(TEST_KEY)).await;
        // `/v1/extractors` is a static catalog — passes auth, no network.
        let resp = app
            .oneshot(get_auth("/v1/extractors", &format!("Bearer {TEST_KEY}")))
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn lowercase_bearer_accepted() {
        let app = app_with_key(Some(TEST_KEY)).await;
        let resp = app
            .oneshot(get_auth("/v1/extractors", &format!("bearer {TEST_KEY}")))
            .await
            .expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn open_mode_allows_unauthenticated() {
        // No api key configured => auth middleware passes everything.
        let app = app_with_key(None).await;
        let resp = app.oneshot(get("/v1/extractors")).await.expect("response");
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn unknown_format_is_bad_request() {
        // Format validation now runs before the fetch, so a bogus format
        // returns 400 without any network call.
        let app = app_with_key(None).await;
        let req = Request::builder()
            .method("POST")
            .uri("/v1/scrape")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"{"url":"https://example.com","formats":["bogus"]}"#,
            ))
            .expect("request");
        let resp = app.oneshot(req).await.expect("response");
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let body = json_body(resp).await;
        assert!(
            body["error"]
                .as_str()
                .is_some_and(|e| e.contains("unknown format")),
            "expected unknown-format error, got {body:?}"
        );
    }
}
