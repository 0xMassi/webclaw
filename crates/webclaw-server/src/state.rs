//! Shared application state. Cheap to clone via Arc; held by the axum
//! Router for the life of the process.
//!
//! Two unrelated keys get carried here:
//!
//! 1. [`AppState::api_key`] — the **bearer token clients must present**
//!    to call this server. Set via `WEBCLAW_API_KEY` / `--api-key`.
//!    Unset = open mode.
//! 2. The inner [`webclaw_fetch::cloud::CloudClient`] (if any) — our
//!    **outbound** credential for api.webclaw.io, used by extractors
//!    that escalate on antibot. Set via `WEBCLAW_CLOUD_API_KEY`.
//!    Unset = hard-site extractors return a "set WEBCLAW_CLOUD_API_KEY"
//!    error with a signup link.
//!
//! Different variables on purpose: conflating the two means operators
//! who want their server behind an auth token can't also enable cloud
//! fallback, and vice versa.

use std::sync::Arc;
use tracing::info;
use webclaw_fetch::cloud::CloudClient;
use webclaw_fetch::{BrowserProfile, FetchClient, FetchConfig};
use webclaw_llm::ProviderChain;

/// Single-process state shared across all request handlers.
#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    /// Wrapped in `Arc` because `fetch_and_extract_batch_with_options`
    /// (used by the /v1/batch handler) takes `self: &Arc<Self>` so it
    /// can clone the client into spawned tasks. The single-call handlers
    /// auto-deref `&Arc<FetchClient>` -> `&FetchClient`, so this costs
    /// them nothing.
    pub fetch: Arc<FetchClient>,
    /// The exact [`FetchConfig`] the shared `fetch` client was built from.
    /// Endpoints that spin up their own client (e.g. `/v1/crawl`, which
    /// builds a `Crawler` with its own internal `FetchClient`) clone this
    /// so they inherit the same browser profile / proxy / timeout instead
    /// of silently falling back to `FetchConfig::default()` (Chrome).
    pub fetch_config: FetchConfig,
    /// LLM provider chain (Ollama -> OpenAI -> Anthropic), built once at
    /// startup. `/v1/extract` and `/v1/summarize` borrow this instead of
    /// rebuilding the chain (and re-probing Ollama) on every request.
    pub llm_chain: Arc<ProviderChain>,
    /// Inbound bearer-auth token for this server's own `/v1/*` surface.
    pub api_key: Option<String>,
}

impl AppState {
    /// Build the application state. The fetch client is constructed once
    /// and shared across requests so connection pools + browser profile
    /// state don't churn per request.
    ///
    /// `inbound_api_key` is the bearer token clients must present;
    /// cloud-fallback credentials come from the env (checked here).
    ///
    /// Async because the LLM provider chain probes Ollama for availability
    /// once at startup; doing it here keeps it off the per-request hot path.
    pub async fn new(inbound_api_key: Option<String>) -> anyhow::Result<Self> {
        let config = FetchConfig {
            browser: BrowserProfile::Firefox,
            ..FetchConfig::default()
        };
        let mut fetch = FetchClient::new(config.clone())
            .map_err(|e| anyhow::anyhow!("failed to build fetch client: {e}"))?;

        // Cloud fallback: only activates when the operator has provided
        // an api.webclaw.io key. Supports both WEBCLAW_CLOUD_API_KEY
        // (preferred, disambiguates from the inbound-auth key) and
        // WEBCLAW_API_KEY as a fallback when there's no inbound key
        // configured (backwards compat with MCP / CLI conventions).
        if let Some(cloud) = build_cloud_client(inbound_api_key.as_deref()) {
            info!(
                base = cloud.base_url(),
                "cloud fallback enabled — antibot-protected sites will escalate via api.webclaw.io"
            );
            fetch = fetch.with_cloud(cloud);
        }

        let llm_chain = Arc::new(ProviderChain::default().await);

        Ok(Self {
            inner: Arc::new(Inner {
                fetch: Arc::new(fetch),
                fetch_config: config,
                llm_chain,
                api_key: inbound_api_key,
            }),
        })
    }

    pub fn fetch(&self) -> &Arc<FetchClient> {
        &self.inner.fetch
    }

    /// The [`FetchConfig`] the shared client was built from. Cloned by
    /// endpoints that need to construct their own client with identical
    /// settings (currently `/v1/crawl`).
    pub fn fetch_config(&self) -> &FetchConfig {
        &self.inner.fetch_config
    }

    /// The shared LLM provider chain. Borrowed by `/v1/extract` and
    /// `/v1/summarize`; `&ProviderChain` coerces to `&dyn LlmProvider`.
    pub fn llm_chain(&self) -> &ProviderChain {
        &self.inner.llm_chain
    }

    pub fn api_key(&self) -> Option<&str> {
        self.inner.api_key.as_deref()
    }
}

/// Resolve the outbound cloud key. Prefers `WEBCLAW_CLOUD_API_KEY`;
/// falls back to `WEBCLAW_API_KEY` *only* when no inbound key is
/// configured (i.e. open mode — the same env var can't mean two
/// things to one process).
fn build_cloud_client(inbound_api_key: Option<&str>) -> Option<CloudClient> {
    let cloud_key = std::env::var("WEBCLAW_CLOUD_API_KEY").ok();
    if let Some(k) = cloud_key.as_deref()
        && !k.trim().is_empty()
    {
        return Some(CloudClient::with_key(k));
    }
    // Reuse WEBCLAW_API_KEY only when not also acting as our own
    // inbound-auth token — otherwise we'd be telling the operator
    // they can't have both.
    if inbound_api_key.is_none()
        && let Ok(k) = std::env::var("WEBCLAW_API_KEY")
        && !k.trim().is_empty()
    {
        return Some(CloudClient::with_key(k));
    }
    None
}
