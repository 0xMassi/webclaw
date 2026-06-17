//! Shared GitHub REST API helper for the `github_*` vertical extractors.
//!
//! repo / pr / issue / release all call api.github.com and handle the same
//! 404 / 403-rate-limit / non-200 responses identically. This centralizes
//! the fetch + status handling so each extractor keeps only its URL parsing
//! and response shaping.
use crate::error::FetchError;
use crate::fetcher::Fetcher;

/// Fetch a GitHub API URL and return the response body, mapping the common
/// error statuses to actionable messages.
///
/// `vertical` is the extractor name used as the error prefix (e.g.
/// `"github_repo"`); `not_found` is the 404 detail (e.g.
/// `"repo 'owner/repo' not found"`).
pub(crate) async fn fetch_api(
    client: &dyn Fetcher,
    api_url: &str,
    vertical: &str,
    not_found: &str,
) -> Result<String, FetchError> {
    let resp = client.fetch(api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!("{vertical}: {not_found}")));
    }
    if resp.status == 403 {
        return Err(FetchError::Build(format!(
            "{vertical}: rate limited (60/hour unauth). Set GITHUB_TOKEN for 5,000/hour."
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "github api returned status {}",
            resp.status
        )));
    }
    Ok(resp.html)
}
