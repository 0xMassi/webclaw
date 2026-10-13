//! On-disk cache of fetched responses; see [`crate::FetchConfig::cache`].

use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::client::FetchResult;

#[derive(Debug, Clone)]
pub(crate) struct ResponseCache {
    dir: PathBuf,
    identity: u64,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    request: String,
    url: String,
    status: u16,
    headers: Vec<(String, String)>,
    html: String,
    document: bool,
}

impl ResponseCache {
    pub(crate) fn new(dir: PathBuf, identity: &impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        identity.hash(&mut hasher);
        Self {
            dir,
            identity: hasher.finish(),
        }
    }

    pub(crate) fn key(&self, url: &str, extra: &[(&str, &str)]) -> String {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.identity.hash(&mut hasher);
        url.hash(&mut hasher);
        extra.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    pub(crate) fn read(&self, key: &str, url: &str) -> Option<FetchResult> {
        let entry: Entry =
            serde_json::from_slice(&std::fs::read(self.dir.join(format!("{key}.json"))).ok()?)
                .ok()?;
        if entry.request != url {
            return None;
        }
        let document_bytes = if entry.document {
            Some(bytes::Bytes::from(
                std::fs::read(self.dir.join(format!("{key}.bin"))).ok()?,
            ))
        } else {
            None
        };
        let mut headers = http::HeaderMap::new();
        for (name, value) in entry.headers {
            if let (Ok(name), Ok(value)) = (
                http::header::HeaderName::from_bytes(name.as_bytes()),
                http::header::HeaderValue::from_str(&value),
            ) {
                headers.append(name, value);
            }
        }
        Some(FetchResult {
            html: entry.html,
            document_bytes,
            status: entry.status,
            url: entry.url,
            headers,
            elapsed: Duration::ZERO,
        })
    }

    pub(crate) fn write(&self, key: &str, url: &str, fetched: &FetchResult) {
        if let Err(e) = self.try_write(key, url, fetched) {
            warn!(url, "could not cache the response: {e}");
        }
    }

    fn try_write(&self, key: &str, url: &str, fetched: &FetchResult) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        if let Some(bytes) = &fetched.document_bytes {
            self.replace(&format!("{key}.bin"), bytes)?;
        }
        let entry = Entry {
            request: url.to_string(),
            url: fetched.url.clone(),
            status: fetched.status,
            headers: fetched
                .headers
                .iter()
                .filter(|(name, _)| *name != http::header::SET_COOKIE)
                .filter_map(|(name, value)| {
                    Some((name.to_string(), value.to_str().ok()?.to_string()))
                })
                .collect(),
            html: fetched.html.clone(),
            document: fetched.document_bytes.is_some(),
        };
        self.replace(&format!("{key}.json"), &serde_json::to_vec(&entry)?)
    }

    fn replace(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let partial = self.dir.join(format!(
            "{name}.{}-{}.partial",
            std::process::id(),
            WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&partial, bytes)?;
        std::fs::rename(&partial, self.dir.join(name))
    }
}

#[cfg(test)]
mod tests {
    use super::ResponseCache;

    fn page(html: &str, document: Option<&[u8]>) -> crate::client::FetchResult {
        let mut headers = http::HeaderMap::new();
        headers.insert("content-type", "text/html".parse().unwrap());
        headers.insert("set-cookie", "session=secret".parse().unwrap());
        crate::client::FetchResult {
            html: html.into(),
            document_bytes: document.map(bytes::Bytes::copy_from_slice),
            status: 200,
            url: "https://example.com/final".into(),
            headers,
            elapsed: std::time::Duration::from_secs(1),
        }
    }

    #[test]
    fn a_cached_page_comes_back_without_its_cookies() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(dir.path().into(), &"chrome");
        let key = cache.key("https://example.com/", &[]);
        cache.write(&key, "https://example.com/", &page("<p>hi</p>", None));
        let hit = cache.read(&key, "https://example.com/").unwrap();
        assert_eq!(hit.html, "<p>hi</p>");
        assert_eq!(hit.url, "https://example.com/final");
        assert_eq!(hit.status, 200);
        assert_eq!(hit.headers["content-type"], "text/html");
        assert!(hit.headers.get("set-cookie").is_none());
        assert!(hit.document_bytes.is_none());
    }

    #[test]
    fn documents_keep_their_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(dir.path().into(), &"chrome");
        let key = cache.key("https://example.com/a.pdf", &[]);
        cache.write(
            &key,
            "https://example.com/a.pdf",
            &page("", Some(b"%PDF-1.7 body")),
        );
        let hit = cache.read(&key, "https://example.com/a.pdf").unwrap();
        assert_eq!(hit.document_bytes.as_deref(), Some(&b"%PDF-1.7 body"[..]));
    }

    #[test]
    fn other_headers_or_identities_miss() {
        let dir = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(dir.path().into(), &"chrome");
        let key = cache.key("https://example.com/", &[]);
        cache.write(&key, "https://example.com/", &page("<p>hi</p>", None));
        let cookie = cache.key("https://example.com/", &[("Cookie", "a=1")]);
        assert_ne!(cookie, key);
        assert!(cache.read(&cookie, "https://example.com/").is_none());
        let firefox = ResponseCache::new(dir.path().into(), &"firefox");
        assert_ne!(firefox.key("https://example.com/", &[]), key);
        assert!(cache.read(&key, "https://example.com/other").is_none());
    }
}
