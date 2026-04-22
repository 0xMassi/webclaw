//! Vertical extractors: site-specific parsers that return typed JSON
//! instead of generic markdown.
//!
//! Each extractor handles a single site or platform and exposes:
//! - `matches(url)` to claim ownership of a URL pattern
//! - `extract(client, url)` to fetch + parse into a typed JSON `Value`
//! - `INFO` static for the catalog (`/v1/extractors`)
//!
//! The dispatch in this module is a simple `match`-style chain rather than
//! a trait registry. With ~30 extractors that's still fast and avoids the
//! ceremony of dynamic dispatch. If we hit 50+ we'll revisit.
//!
//! Extractors prefer official JSON APIs over HTML scraping where one
//! exists (Reddit, HN/Algolia, PyPI, npm, GitHub, HuggingFace all have
//! one). HTML extraction is the fallback for sites that don't.

pub mod arxiv;
pub mod crates_io;
pub mod dev_to;
pub mod docker_hub;
pub mod github_pr;
pub mod github_release;
pub mod github_repo;
pub mod hackernews;
pub mod huggingface_dataset;
pub mod huggingface_model;
pub mod npm;
pub mod pypi;
pub mod reddit;
pub mod stackoverflow;

use serde::Serialize;
use serde_json::Value;

use crate::client::FetchClient;
use crate::error::FetchError;

/// Public catalog entry for `/v1/extractors`. Stable shape — clients
/// rely on `name` to pick the right `/v1/scrape/{name}` route.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractorInfo {
    /// URL-safe identifier (`reddit`, `hackernews`, `github_repo`, ...).
    pub name: &'static str,
    /// Human-friendly display name.
    pub label: &'static str,
    /// One-line description of what the extractor returns.
    pub description: &'static str,
    /// Glob-ish URL pattern(s) the extractor claims. For documentation;
    /// the actual matching is done by the extractor's `matches` fn.
    pub url_patterns: &'static [&'static str],
}

/// Full catalog. Order is stable; new entries append.
pub fn list() -> Vec<ExtractorInfo> {
    vec![
        reddit::INFO,
        hackernews::INFO,
        github_repo::INFO,
        github_pr::INFO,
        github_release::INFO,
        pypi::INFO,
        npm::INFO,
        crates_io::INFO,
        huggingface_model::INFO,
        huggingface_dataset::INFO,
        arxiv::INFO,
        docker_hub::INFO,
        dev_to::INFO,
        stackoverflow::INFO,
    ]
}

/// Auto-detect mode: try every extractor's `matches`, return the first
/// one that claims the URL. Used by `/v1/scrape` when the caller doesn't
/// pick a vertical explicitly.
pub async fn dispatch_by_url(
    client: &FetchClient,
    url: &str,
) -> Option<Result<(&'static str, Value), FetchError>> {
    if reddit::matches(url) {
        return Some(
            reddit::extract(client, url)
                .await
                .map(|v| (reddit::INFO.name, v)),
        );
    }
    if hackernews::matches(url) {
        return Some(
            hackernews::extract(client, url)
                .await
                .map(|v| (hackernews::INFO.name, v)),
        );
    }
    if github_repo::matches(url) {
        return Some(
            github_repo::extract(client, url)
                .await
                .map(|v| (github_repo::INFO.name, v)),
        );
    }
    if pypi::matches(url) {
        return Some(
            pypi::extract(client, url)
                .await
                .map(|v| (pypi::INFO.name, v)),
        );
    }
    if npm::matches(url) {
        return Some(npm::extract(client, url).await.map(|v| (npm::INFO.name, v)));
    }
    if github_pr::matches(url) {
        return Some(
            github_pr::extract(client, url)
                .await
                .map(|v| (github_pr::INFO.name, v)),
        );
    }
    if github_release::matches(url) {
        return Some(
            github_release::extract(client, url)
                .await
                .map(|v| (github_release::INFO.name, v)),
        );
    }
    if crates_io::matches(url) {
        return Some(
            crates_io::extract(client, url)
                .await
                .map(|v| (crates_io::INFO.name, v)),
        );
    }
    if huggingface_model::matches(url) {
        return Some(
            huggingface_model::extract(client, url)
                .await
                .map(|v| (huggingface_model::INFO.name, v)),
        );
    }
    if huggingface_dataset::matches(url) {
        return Some(
            huggingface_dataset::extract(client, url)
                .await
                .map(|v| (huggingface_dataset::INFO.name, v)),
        );
    }
    if arxiv::matches(url) {
        return Some(
            arxiv::extract(client, url)
                .await
                .map(|v| (arxiv::INFO.name, v)),
        );
    }
    if docker_hub::matches(url) {
        return Some(
            docker_hub::extract(client, url)
                .await
                .map(|v| (docker_hub::INFO.name, v)),
        );
    }
    if dev_to::matches(url) {
        return Some(
            dev_to::extract(client, url)
                .await
                .map(|v| (dev_to::INFO.name, v)),
        );
    }
    if stackoverflow::matches(url) {
        return Some(
            stackoverflow::extract(client, url)
                .await
                .map(|v| (stackoverflow::INFO.name, v)),
        );
    }
    None
}

/// Explicit mode: caller picked the vertical (`POST /v1/scrape/reddit`).
/// We still validate that the URL plausibly belongs to that vertical so
/// users get a clear "wrong route" error instead of a confusing parse
/// failure deep in the extractor.
pub async fn dispatch_by_name(
    client: &FetchClient,
    name: &str,
    url: &str,
) -> Result<Value, ExtractorDispatchError> {
    match name {
        n if n == reddit::INFO.name => {
            run_or_mismatch(reddit::matches(url), n, url, || {
                reddit::extract(client, url)
            })
            .await
        }
        n if n == hackernews::INFO.name => {
            run_or_mismatch(hackernews::matches(url), n, url, || {
                hackernews::extract(client, url)
            })
            .await
        }
        n if n == github_repo::INFO.name => {
            run_or_mismatch(github_repo::matches(url), n, url, || {
                github_repo::extract(client, url)
            })
            .await
        }
        n if n == pypi::INFO.name => {
            run_or_mismatch(pypi::matches(url), n, url, || pypi::extract(client, url)).await
        }
        n if n == npm::INFO.name => {
            run_or_mismatch(npm::matches(url), n, url, || npm::extract(client, url)).await
        }
        n if n == github_pr::INFO.name => {
            run_or_mismatch(github_pr::matches(url), n, url, || {
                github_pr::extract(client, url)
            })
            .await
        }
        n if n == github_release::INFO.name => {
            run_or_mismatch(github_release::matches(url), n, url, || {
                github_release::extract(client, url)
            })
            .await
        }
        n if n == crates_io::INFO.name => {
            run_or_mismatch(crates_io::matches(url), n, url, || {
                crates_io::extract(client, url)
            })
            .await
        }
        n if n == huggingface_model::INFO.name => {
            run_or_mismatch(huggingface_model::matches(url), n, url, || {
                huggingface_model::extract(client, url)
            })
            .await
        }
        n if n == huggingface_dataset::INFO.name => {
            run_or_mismatch(huggingface_dataset::matches(url), n, url, || {
                huggingface_dataset::extract(client, url)
            })
            .await
        }
        n if n == arxiv::INFO.name => {
            run_or_mismatch(arxiv::matches(url), n, url, || arxiv::extract(client, url)).await
        }
        n if n == docker_hub::INFO.name => {
            run_or_mismatch(docker_hub::matches(url), n, url, || {
                docker_hub::extract(client, url)
            })
            .await
        }
        n if n == dev_to::INFO.name => {
            run_or_mismatch(dev_to::matches(url), n, url, || {
                dev_to::extract(client, url)
            })
            .await
        }
        n if n == stackoverflow::INFO.name => {
            run_or_mismatch(stackoverflow::matches(url), n, url, || {
                stackoverflow::extract(client, url)
            })
            .await
        }
        _ => Err(ExtractorDispatchError::UnknownVertical(name.to_string())),
    }
}

/// Errors that the dispatcher itself raises (vs. errors from inside an
/// extractor, which come back wrapped in `Fetch`).
#[derive(Debug, thiserror::Error)]
pub enum ExtractorDispatchError {
    #[error("unknown vertical: '{0}'")]
    UnknownVertical(String),

    #[error("URL '{url}' does not match the '{vertical}' extractor")]
    UrlMismatch { vertical: String, url: String },

    #[error(transparent)]
    Fetch(#[from] FetchError),
}

/// Helper: when the caller explicitly picked a vertical but their URL
/// doesn't match it, return `UrlMismatch` instead of running the
/// extractor (which would just fail with a less-clear error).
async fn run_or_mismatch<F, Fut>(
    matches: bool,
    vertical: &str,
    url: &str,
    f: F,
) -> Result<Value, ExtractorDispatchError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<Value, FetchError>>,
{
    if !matches {
        return Err(ExtractorDispatchError::UrlMismatch {
            vertical: vertical.to_string(),
            url: url.to_string(),
        });
    }
    f().await.map_err(ExtractorDispatchError::Fetch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_is_non_empty_and_unique() {
        let entries = list();
        assert!(!entries.is_empty());
        let mut names: Vec<_> = entries.iter().map(|e| e.name).collect();
        names.sort();
        let before = names.len();
        names.dedup();
        assert_eq!(before, names.len(), "extractor names must be unique");
    }
}
