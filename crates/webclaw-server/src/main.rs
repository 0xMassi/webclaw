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
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt};

use crate::state::AppState;

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

    let state = AppState::new(args.api_key.clone())?;

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

    let app = Router::new()
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
        .layer(TraceLayer::new_for_http())
        .with_state(state);

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

fn is_unspecified_addr(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(ip) => ip.is_unspecified(),
        IpAddr::V6(ip) => ip.is_unspecified(),
    }
}
