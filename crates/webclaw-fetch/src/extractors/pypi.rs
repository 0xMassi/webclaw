//! PyPI package structured extractor.
//!
//! PyPI exposes a stable JSON API at `pypi.org/pypi/{name}/json` and
//! a versioned form at `pypi.org/pypi/{name}/{version}/json`. Both
//! return the full release info plus history. No auth, no rate limits
//! that we hit at normal usage.

use serde::Deserialize;
use serde_json::{Value, json};

use super::ExtractorInfo;
use crate::error::FetchError;
use crate::fetcher::Fetcher;

pub const INFO: ExtractorInfo = ExtractorInfo {
    name: "pypi",
    label: "PyPI package",
    description: "Returns package metadata: latest version, dependencies, license, release history.",
    url_patterns: &[
        "https://pypi.org/project/{name}/",
        "https://pypi.org/project/{name}/{version}/",
    ],
};

pub fn matches(url: &str) -> bool {
    let host = host_of(url);
    if host != "pypi.org" && host != "www.pypi.org" {
        return false;
    }
    url.contains("/project/")
}

pub async fn extract(client: &dyn Fetcher, url: &str) -> Result<Value, FetchError> {
    let (name, version) = parse_project(url).ok_or_else(|| {
        FetchError::Build(format!("pypi: cannot parse package name from '{url}'"))
    })?;

    let api_url = match &version {
        Some(v) => format!("https://pypi.org/pypi/{name}/{v}/json"),
        None => format!("https://pypi.org/pypi/{name}/json"),
    };
    let resp = client.fetch(&api_url).await?;
    if resp.status == 404 {
        return Err(FetchError::Build(format!(
            "pypi: package '{name}' not found"
        )));
    }
    if resp.status != 200 {
        return Err(FetchError::Build(format!(
            "pypi api returned status {}",
            resp.status
        )));
    }

    let pkg: PypiResponse = serde_json::from_str(&resp.html)
        .map_err(|e| FetchError::BodyDecode(format!("pypi parse: {e}")))?;

    let info = pkg.info;
    let release_count = pkg.releases.as_ref().map(|r| r.len()).unwrap_or(0);

    // Latest release date = max upload time across files in the latest version.
    let latest_release_date = pkg
        .releases
        .as_ref()
        .and_then(|map| info.version.as_deref().and_then(|v| map.get(v)))
        .and_then(|files| files.iter().filter_map(|f| f.upload_time.clone()).max());

    // Drop the long description from the JSON shape — it's frequently a 50KB
    // README and bloats responses. Callers who need it can hit /v1/scrape.
    Ok(json!({
        "url":                 url,
        "name":                info.name,
        "version":             info.version,
        "summary":             info.summary,
        "homepage":            info.home_page,
        "license":             info.license,
        "license_classifier":  pick_license_classifier(&info.classifiers),
        "author":              info.author,
        "author_email":        info.author_email,
        "maintainer":          info.maintainer,
        "requires_python":     info.requires_python,
        "requires_dist":       info.requires_dist,
        "keywords":            info.keywords,
        "classifiers":         info.classifiers,
        "yanked":              info.yanked,
        "yanked_reason":       info.yanked_reason,
        "project_urls":        info.project_urls,
        "release_count":       release_count,
        "latest_release_date": latest_release_date,
    }))
}

/// PyPI puts the SPDX-ish license under classifiers like
/// `License :: OSI Approved :: Apache Software License`. Surface the most
/// specific one when the `license` field itself is empty/junk.
fn pick_license_classifier(classifiers: &Option<Vec<String>>) -> Option<String> {
    classifiers
        .as_ref()?
        .iter()
        .filter(|c| c.starts_with("License ::"))
        .max_by_key(|c| c.len())
        .cloned()
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or("")
}

fn parse_project(url: &str) -> Option<(String, Option<String>)> {
    let after = url.split("/project/").nth(1)?;
    let stripped = after.split(['?', '#']).next()?.trim_end_matches('/');
    let mut segs = stripped.split('/').filter(|s| !s.is_empty());
    let name = segs.next()?.to_string();
    let version = segs.next().map(|v| v.to_string());
    Some((name, version))
}

// ---------------------------------------------------------------------------
// PyPI API types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PypiResponse {
    info: Info,
    releases: Option<std::collections::BTreeMap<String, Vec<File>>>,
}

#[derive(Deserialize)]
struct Info {
    name: Option<String>,
    version: Option<String>,
    summary: Option<String>,
    home_page: Option<String>,
    license: Option<String>,
    author: Option<String>,
    author_email: Option<String>,
    maintainer: Option<String>,
    requires_python: Option<String>,
    requires_dist: Option<Vec<String>>,
    keywords: Option<String>,
    classifiers: Option<Vec<String>>,
    yanked: Option<bool>,
    yanked_reason: Option<String>,
    project_urls: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Deserialize)]
struct File {
    upload_time: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_project_urls() {
        assert!(matches("https://pypi.org/project/requests/"));
        assert!(matches("https://pypi.org/project/numpy/1.26.0/"));
        assert!(!matches("https://pypi.org/"));
        assert!(!matches("https://example.com/project/foo"));
    }

    #[test]
    fn parse_project_pulls_name_and_version() {
        assert_eq!(
            parse_project("https://pypi.org/project/requests/"),
            Some(("requests".into(), None))
        );
        assert_eq!(
            parse_project("https://pypi.org/project/numpy/1.26.0/"),
            Some(("numpy".into(), Some("1.26.0".into())))
        );
        assert_eq!(
            parse_project("https://pypi.org/project/scikit-learn/?foo=bar"),
            Some(("scikit-learn".into(), None))
        );
    }
}
