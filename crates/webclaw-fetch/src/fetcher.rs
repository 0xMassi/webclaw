//! Pluggable fetcher abstraction for vertical extractors.
//!
//! Extractors call the network through this trait instead of hard-
//! coding [`FetchClient`]. The OSS CLI / MCP / self-hosted server all
//! pass `&FetchClient` (wreq-backed BoringSSL). The production API
//! server, which must not use in-process TLS fingerprinting, provides
//! its own implementation that routes through the Go tls-sidecar.
//!
//! Both paths expose the same [`FetchResult`] shape and the same
//! optional cloud-escalation client, so extractor logic stays
//! identical across environments.
//!
//! ## Choosing an implementation
//!
//! - CLI, MCP, self-hosted `webclaw-server`: build a [`FetchClient`]
//!   with [`FetchClient::with_cloud`] to attach cloud fallback, pass
//!   it to extractors as `&client`.
//! - `api.webclaw.io` production server: build a `TlsSidecarFetcher`
//!   (in `server/src/engine/`) that delegates to `engine::tls_client`
//!   and wraps it in `Arc<dyn Fetcher>` for handler injection.
//!
//! ## Why a trait and not a free function
//!
//! Extractors need state beyond a single fetch: the cloud client for
//! antibot escalation, and in the future per-user proxy pools, tenant
//! headers, circuit breakers. A trait keeps that state encapsulated
//! behind the fetch interface instead of threading it through every
//! extractor signature.

use async_trait::async_trait;

use crate::client::FetchResult;
use crate::cloud::CloudClient;
use crate::error::FetchError;

/// HTTP fetch surface used by vertical extractors.
///
/// Implementations must be `Send + Sync` because extractor dispatchers
/// run them inside tokio tasks, potentially across many requests.
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// Fetch a URL and return the raw response body + metadata. The
    /// body is in `FetchResult::html` regardless of the actual content
    /// type — JSON API endpoints put JSON there, HTML pages put HTML.
    /// Extractors branch on response status and body shape.
    async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError>;

    /// Fetch with additional request headers. Needed for endpoints
    /// that authenticate via a specific header (Instagram's
    /// `x-ig-app-id`, for example). Default implementation routes to
    /// [`Self::fetch`] so implementers without header support stay
    /// functional, though the `Option<String>` field they'd set won't
    /// be populated on the request.
    async fn fetch_with_headers(
        &self,
        url: &str,
        _headers: &[(&str, &str)],
    ) -> Result<FetchResult, FetchError> {
        self.fetch(url).await
    }

    /// Optional cloud-escalation client for antibot bypass. Returning
    /// `Some` tells extractors they can call into the hosted API when
    /// local fetch hits a challenge page. Returning `None` makes
    /// cloud-gated extractors emit [`CloudError::NotConfigured`] with
    /// an actionable signup link.
    ///
    /// The default implementation returns `None` because not every
    /// deployment wants cloud fallback (self-hosts that don't have a
    /// webclaw.io subscription, for instance).
    ///
    /// [`CloudError::NotConfigured`]: crate::cloud::CloudError::NotConfigured
    fn cloud(&self) -> Option<&CloudClient> {
        None
    }
}

// ---------------------------------------------------------------------------
// Blanket impls: make `&T` and `Arc<T>` behave like the wrapped `T`.
// ---------------------------------------------------------------------------

#[async_trait]
impl<T: Fetcher + ?Sized> Fetcher for &T {
    async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError> {
        (**self).fetch(url).await
    }

    async fn fetch_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<FetchResult, FetchError> {
        (**self).fetch_with_headers(url, headers).await
    }

    fn cloud(&self) -> Option<&CloudClient> {
        (**self).cloud()
    }
}

#[async_trait]
impl<T: Fetcher + ?Sized> Fetcher for std::sync::Arc<T> {
    async fn fetch(&self, url: &str) -> Result<FetchResult, FetchError> {
        (**self).fetch(url).await
    }

    async fn fetch_with_headers(
        &self,
        url: &str,
        headers: &[(&str, &str)],
    ) -> Result<FetchResult, FetchError> {
        (**self).fetch_with_headers(url, headers).await
    }

    fn cloud(&self) -> Option<&CloudClient> {
        (**self).cloud()
    }
}
