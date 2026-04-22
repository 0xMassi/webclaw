//! npm package structured extractor.
//!
//! Uses two npm-run APIs:
//!   - `registry.npmjs.org/{name}` for full package metadata
//!   - `api.npmjs.org/downloads/point/last-week/{name}` for usage signal
//!
//! The registry API returns the *full* document including every version
//! ever published, which can be tens of MB for popular packages
//! (`@types/node` etc). We strip down to the latest version's manifest
//! and a count of releases — full history would explode the response.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "npm",
    label: "npm package",
    description: "Returns package metadata: latest version manifest, dependencies, weekly downloads, license.",
    url_patterns: &["https://www.npmjs.com/package/{name}"],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "www.npmjs.com" && host != "npmjs.com" {
        return false;
    }
    url.contains("/package/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let name = parse_name(url)
        .ok_or_else(|| FetchError::Build(format!("npm: cannot parse name from '{url}'")))?;

    let registry_url = format!("https://registry.npmjs.org/{}", urlencode_segment(&name));
    let resp = client.fetch(&registry_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "npm: package '{name}' not found"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "npm registry returned status {}",
            resp.status
        )));
    }

    let pkg: PackageDoc = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("npm registry parse: {e}")))?;

    // Resolve "latest" to a concrete version.
    let latest_version = pkg
        .dist_tags
        .as_ref()
        .and_then(|t| t.get("latest"))
        .cloned()
        .or_else(|| pkg.versions.as_ref().and_then(|v| v.keys().last().cloned()));

    let latest_manifest = latest_version
        .as_deref()
        .and_then(|v| pkg.versions.as_ref().and_then(|m| m.get(v)));

    let release_count = pkg.versions.as_ref().map(|v| v.len()).unwrap_or(0);
    let latest_release_date = latest_version
        .as_deref()
        .and_then(|v| pkg.time.as_ref().and_then(|t| t.get(v).cloned()));

    // Best-effort weekly downloads. If the api.npmjs.org call fails we
    // surface `null` rather than failing the whole extractor — npm
    // sometimes 503s the downloads endpoint while the registry is up.
    let weekly_downloads = fetch_weekly_downloads(client, &name).await.ok();

    Ok(json!({
        "url":                 url,
        "name":                pkg.name.clone().unwrap_or(name.clone()),
        "description":         pkg.description,
        "latest_version":      latest_version,
        "license":             latest_manifest.and_then(|m| m.license.clone()),
        "homepage":            pkg.homepage,
        "repository":          pkg.repository.as_ref().and_then(|r| r.url.clone()),
        "dependencies":        latest_manifest.and_then(|m| m.dependencies.clone()),
        "dev_dependencies":    latest_manifest.and_then(|m| m.dev_dependencies.clone()),
        "peer_dependencies":   latest_manifest.and_then(|m| m.peer_dependencies.clone()),
        "keywords":            pkg.keywords,
        "maintainers":         pkg.maintainers,
        "deprecated":          latest_manifest.and_then(|m| m.deprecated.clone()),
        "release_count":       release_count,
        "latest_release_date": latest_release_date,
        "weekly_downloads":    weekly_downloads,
    }))
}

async fn fetch_weekly_downloads(client: &dyn Fetcher, name: &str) -> Result<i64, FetchError> {
    let url = format!(
        "https://api.npmjs.org/downloads/point/last-week/{}",
        urlencode_segment(name)
    );
    let resp = client.fetch(&url).await?;
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "npm downloads api status {}",
            resp.status
        )));
    }
    let dl: Downloads = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("npm downloads parse: {e}")))?;
    Ok(dl.downloads)
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

/// Extract the package name from an npmjs.com URL. Handles scoped packages
/// (`/package/@scope/name`) and trailing path segments (`/v/x.y.z`).
fn parse_name(url: &str) -> Option<String> {
    let after = url.split("/package/").nth(1)?;
    let stripped = after.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let first = segs.next()?;
    if first.starts_with('@') {
        let second = segs.next()?;
        Some(format!("{first}/{second}"))
    } else {
        Some(first.to_string())
    }
}

/// `@scope/name` must encode the `/` for the registry path. Plain names
/// pass through untouched.
fn urlencode_segment(name: &str) -> String {
    name.replace('/', "%2F")
}

// ---------------------------------------------------------------------------
// Registry types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PackageDoc {
    name: Option<String>,
    description: Option<String>,
    homepage: Option<serde_json::Value>, // sometimes string, sometimes object
    repository: Option<Repository>,
    keywords: Option<Vec<String>>,
    maintainers: Option<Vec<Maintainer>>,
    #[serde(rename = "dist-tags")]
    dist_tags: Option<std::collections::BTreeMap<String, String>>,
    versions: Option<std::collections::BTreeMap<String, VersionManifest>>,
    time: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Deserialize, Default, Clone)]
struct VersionManifest {
    license: Option<serde_json::Value>, // string or object
    dependencies: Option<std::collections::BTreeMap<String, String>>,
    #[serde(rename = "devDependencies")]
    dev_dependencies: Option<std::collections::BTreeMap<String, String>>,
    #[serde(rename = "peerDependencies")]
    peer_dependencies: Option<std::collections::BTreeMap<String, String>>,
    // `deprecated` is sometimes a bool and sometimes a string in the
    // registry. serde_json::Value covers both without failing the parse.
    deprecated: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Repository {
    url: Option<String>,
}

#[derive(Deserialize, Clone)]
struct Maintainer {
    name: Option<String>,
    email: Option<String>,
}

impl serde::Serialize for Maintainer {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(2))?;
        m.serialize_entry("name", &self.name)?;
        m.serialize_entry("email", &self.email)?;
        m.end()
    }
}

#[derive(Deserialize)]
struct Downloads {
    downloads: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_npm_package_urls() {
        assert!(matches("https://www.npmjs.com/package/react"));
        assert!(matches("https://www.npmjs.com/package/@types/node"));
        assert!(matches("https://npmjs.com/package/lodash"));
        assert!(!matches("https://www.npmjs.com/"));
        assert!(!matches("https://example.com/package/foo"));
    }

    #[test]
    fn parse_name_handles_scoped_and_unscoped() {
        assert_eq!(
            parse_name("https://www.npmjs.com/package/react"),
            Some("react".into())
        );
        assert_eq!(
            parse_name("https://www.npmjs.com/package/@types/node"),
            Some("@types/node".into())
        );
        assert_eq!(
            parse_name("https://www.npmjs.com/package/lodash/v/4.17.21"),
            Some("lodash".into())
        );
    }

    #[test]
    fn urlencode_only_touches_scope_separator() {
        assert_eq!(urlencode_segment("react"), "react");
        assert_eq!(urlencode_segment("@types/node"), "@types%2Fnode");
    }
}
