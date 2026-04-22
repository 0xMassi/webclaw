//! Docker Hub repository structured extractor.
//!
//! Uses the v2 JSON API at `hub.docker.com/v2/repositories/{namespace}/{name}`.
//! Anonymous access is allowed for public images. The official-image
//! shorthand (e.g. `nginx`, `redis`) is normalized to `library/{name}`.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "docker_hub",
    label: "Docker Hub repository",
    description: "Returns image metadata: pull count, star count, last_updated, official flag, description.",
    url_patterns: &[
        "https://hub.docker.com/_/{name}",
        "https://hub.docker.com/r/{namespace}/{name}",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "hub.docker.com" {
        return false;
    }
    url.contains("/_/") || url.contains("/r/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (namespace, name) = parse_repo(url)
        .ok_or_else(|| FetchError::Build(format!("docker_hub: cannot parse repo from '{url}'")))?;

    let api_url = format!("https://hub.docker.com/v2/repositories/{namespace}/{name}");
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "docker_hub: repo '{namespace}/{name}' not found"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "docker_hub api returned status {}",
            resp.status
        )));
    }

    let r: RepoResponse = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("docker_hub parse: {e}")))?;

    Ok(json!({
        "url":               url,
        "namespace":         r.namespace,
        "name":              r.name,
        "full_name":         format!("{namespace}/{name}"),
        "pull_count":        r.pull_count,
        "star_count":        r.star_count,
        "description":       r.description,
        "full_description":  r.full_description,
        "last_updated":      r.last_updated,
        "date_registered":   r.date_registered,
        "is_official":       namespace == "library",
        "is_private":        r.is_private,
        "status_description":r.status_description,
        "categories":        r.categories,
    }))
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Parse `(namespace, name)` from a Docker Hub URL. The official-image
/// shorthand `/_/nginx` maps to `(library, nginx)`. Personal repos
/// `/r/foo/bar` map to `(foo, bar)`.
fn parse_repo(url: &str) -> Option<(String, String)> {
    if let Some(after) = url.split("/_/").nth(1) {
        let stripped = after.split(['?', '#']).next()?.trim_end_matches('/');
        let name = stripped.split('/').next().filter(|s| !s.is_empty())?;
        return Some(("library".into(), name.to_string()));
    }
    let after = url.split("/r/").nth(1)?;
    let stripped = after.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let ns = segs.next()?;
    let nm = segs.next()?;
    Some((ns.to_string(), nm.to_string()))
}

#[derive(Deserialize)]
struct RepoResponse {
    namespace: Option<String>,
    name: Option<String>,
    pull_count: Option<i64>,
    star_count: Option<i64>,
    description: Option<String>,
    full_description: Option<String>,
    last_updated: Option<String>,
    date_registered: Option<String>,
    is_private: Option<bool>,
    status_description: Option<String>,
    #[serde(default)]
    categories: Vec<DockerCategory>,
}

#[derive(Deserialize, serde::Serialize)]
struct DockerCategory {
    name: Option<String>,
    slug: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_docker_urls() {
        assert!(matches("https://hub.docker.com/_/nginx"));
        assert!(matches("https://hub.docker.com/r/grafana/grafana"));
        assert!(!matches("https://hub.docker.com/"));
        assert!(!matches("https://example.com/_/nginx"));
    }

    #[test]
    fn parse_repo_handles_official_and_personal() {
        assert_eq!(
            parse_repo("https://hub.docker.com/_/nginx"),
            Some(("library".into(), "nginx".into()))
        );
        assert_eq!(
            parse_repo("https://hub.docker.com/_/nginx/tags"),
            Some(("library".into(), "nginx".into()))
        );
        assert_eq!(
            parse_repo("https://hub.docker.com/r/grafana/grafana"),
            Some(("grafana".into(), "grafana".into()))
        );
        assert_eq!(
            parse_repo("https://hub.docker.com/r/grafana/grafana/?foo=bar"),
            Some(("grafana".into(), "grafana".into()))
        );
    }
}
