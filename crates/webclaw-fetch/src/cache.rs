//! On-disk cache of fetched responses; see [`crate::FetchConfig::cache`].

use std::hash::{Hash, Hasher};
use std::io::Write;
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
struct Meta {
    request: String,
    url: String,
    status: u16,
    headers: Vec<(String, String)>,
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
        let entry = std::fs::read(self.dir.join(key)).ok()?;
        let split = entry.iter().position(|b| *b == b'\n')?;
        let meta: Meta = serde_json::from_slice(&entry[..split]).ok()?;
        if meta.request != url {
            return None;
        }
        let body = &entry[split + 1..];
        let (html, document_bytes) = if meta.document {
            (String::new(), Some(bytes::Bytes::copy_from_slice(body)))
        } else {
            (String::from_utf8(body.to_vec()).ok()?, None)
        };
        let mut headers = http::HeaderMap::new();
        for (name, value) in meta.headers {
            if let (Ok(name), Ok(value)) = (
                http::header::HeaderName::from_bytes(name.as_bytes()),
                http::header::HeaderValue::from_str(&value),
            ) {
                headers.append(name, value);
            }
        }
        Some(FetchResult {
            script_results: Vec::new(),
            html,
            document_bytes,
            status: meta.status,
            url: meta.url,
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
        private_dir(&self.dir)?;
        let meta = Meta {
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
            document: fetched.document_bytes.is_some(),
        };
        let body = fetched
            .document_bytes
            .as_deref()
            .unwrap_or(fetched.html.as_bytes());
        static WRITES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let partial = self.dir.join(format!(
            "{key}.{}-{}.partial",
            std::process::id(),
            WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let published = private_file(&partial).and_then(|mut file| {
            file.write_all(&serde_json::to_vec(&meta)?)?;
            file.write_all(b"\n")?;
            file.write_all(body)?;
            drop(file);
            std::fs::rename(&partial, self.dir.join(key))
        });
        if published.is_err() {
            let _ = std::fs::remove_file(&partial);
        }
        published
    }
}

#[cfg(unix)]
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(unix)]
fn private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

#[cfg(not(unix))]
fn private_file(path: &std::path::Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
}

#[cfg(test)]
mod tests {
    use super::ResponseCache;

    fn page(html: &str, document: Option<&[u8]>) -> crate::client::FetchResult {
        let mut headers = http::HeaderMap::new();
        headers.insert("content-type", "text/html".parse().unwrap());
        headers.insert("set-cookie", "session=secret".parse().unwrap());
        crate::client::FetchResult {
            script_results: Vec::new(),
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
        cache.write(
            &key,
            "https://example.com/",
            &page("<p>hi\nthere</p>", None),
        );
        let hit = cache.read(&key, "https://example.com/").unwrap();
        assert_eq!(hit.html, "<p>hi\nthere</p>");
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
        let pdf = b"%PDF-1.7\n\xff\x00 body";
        cache.write(&key, "https://example.com/a.pdf", &page("", Some(pdf)));
        let hit = cache.read(&key, "https://example.com/a.pdf").unwrap();
        assert_eq!(hit.document_bytes.as_deref(), Some(&pdf[..]));
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
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

    #[test]
    fn a_failed_write_leaves_no_partial_file() {
        let root = tempfile::tempdir().unwrap();
        let cache = ResponseCache::new(root.path().into(), &"chrome");
        let key = cache.key("https://example.com/", &[]);
        std::fs::create_dir(root.path().join(&key)).unwrap();
        std::fs::write(root.path().join(&key).join("occupied"), b"x").unwrap();
        cache.write(&key, "https://example.com/", &page("<p>hi</p>", None));
        let names: Vec<_> = std::fs::read_dir(root.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [key]);
    }

    #[cfg(unix)]
    #[test]
    fn only_the_owner_can_read_the_cache() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("cache");
        let cache = ResponseCache::new(dir.clone(), &"chrome");
        let key = cache.key("https://example.com/", &[]);
        cache.write(&key, "https://example.com/", &page("<p>private</p>", None));
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join(&key)), 0o600);
    }
}
