//! Local JavaScript rendering through Chrome's DevTools protocol.

use std::time::{Duration, Instant};

use chromiumoxide::auth::Credentials;
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams, FulfillRequestParams, HeaderEntry,
};
use chromiumoxide::cdp::browser_protocol::network::{ErrorReason, ResourceType, SetCookieParams};
use futures_util::StreamExt;
use url::Url;

use crate::client::FetchResult;
use crate::error::FetchError;

/// How to render one page.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Also wait until an element matching this CSS selector exists.
    pub wait_for: Option<String>,
    /// Upper bound for navigating, waiting and reading the page.
    pub timeout: Duration,
    /// Headers added to the page's requests to its own origin, such as
    /// `Authorization` or `Accept-Language`. A `Cookie` header is set as
    /// cookies for the page's URL instead.
    pub headers: Vec<(String, String)>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            wait_for: None,
            timeout: Duration::from_secs(30),
            headers: Vec::new(),
        }
    }
}

/// A page already fetched over HTTP, handed to the browser so it does not
/// request the document a second time.
pub struct Prefetched<'a> {
    pub html: &'a str,
    pub headers: &'a http::HeaderMap,
}

/// A Chrome that renders pages so their JavaScript-built content can be extracted.
///
/// Every request the page makes, including scripts and XHR, goes through the
/// same public-address check as webclaw's own HTTP client, so a rendered page
/// cannot reach private or loopback addresses.
pub struct Renderer {
    browser: Browser,
    events: tokio::task::JoinHandle<()>,
    profile: Option<std::path::PathBuf>,
    proxy_login: Option<(String, String)>,
}

impl Renderer {
    /// Launch a local headless Chrome with a throwaway profile, so several
    /// webclaw processes can render at once. `proxy` is an `http://` or
    /// `https://` proxy URL, credentials included. `WEBCLAW_CHROME` points at
    /// the executable; without it the standard install locations are tried,
    /// then the browsers on `PATH`.
    pub async fn launch(proxy: Option<&str>) -> Result<Self, FetchError> {
        let profile = std::env::temp_dir().join(format!(
            "webclaw-chrome-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let mut config = BrowserConfig::builder()
            .headless_mode(HeadlessMode::New)
            .enable_request_intercept()
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .user_data_dir(&profile);
        let installed = std::env::var("WEBCLAW_CHROME").ok().or_else(|| {
            INSTALL_PATHS
                .iter()
                .find(|path| std::path::Path::new(path).exists())
                .map(|path| path.to_string())
        });
        if let Some(path) = installed {
            config = config.chrome_executable(path);
        }
        let mut proxy_login = None;
        if let Some(proxy) = proxy {
            let parsed = Url::parse(proxy)
                .map_err(|_| FetchError::Build("invalid proxy for the browser".into()))?;
            let host = parsed.host_str().unwrap_or_default();
            let port = parsed.port_or_known_default().unwrap_or(80);
            config = config.arg(format!(
                "--proxy-server={}://{host}:{port}",
                parsed.scheme()
            ));
            if !parsed.username().is_empty() {
                proxy_login = Some((
                    percent_decode(parsed.username()),
                    percent_decode(parsed.password().unwrap_or_default()),
                ));
            }
        }
        let config = config.build().map_err(browser_error)?;
        let (browser, handler) = match Browser::launch(config).await {
            Ok(launched) => launched,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(browser_error(e));
            }
        };
        Ok(Self::start(browser, handler, Some(profile), proxy_login))
    }

    /// Use a browser that is already running with its DevTools endpoint at
    /// `cdp_url` (`ws://...` or `http://host:port`). Its own proxy settings apply.
    pub async fn connect(cdp_url: &str) -> Result<Self, FetchError> {
        let config = chromiumoxide::handler::HandlerConfig {
            request_intercept: true,
            ..Default::default()
        };
        let (browser, handler) = Browser::connect_with_config(cdp_url, config)
            .await
            .map_err(browser_error)?;
        Ok(Self::start(browser, handler, None, None))
    }

    fn start(
        browser: Browser,
        mut handler: chromiumoxide::Handler,
        profile: Option<std::path::PathBuf>,
        proxy_login: Option<(String, String)>,
    ) -> Self {
        let events = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        Self {
            browser,
            events,
            profile,
            proxy_login,
        }
    }

    /// Render `url` and return the DOM after its scripts ran. The result has
    /// status 200 and no headers; extract it with [`crate::FetchClient::extract`].
    ///
    /// With `prefetched`, the browser gets that response for the page itself
    /// (body and headers, `Set-Cookie` included) instead of requesting it
    /// again, so a page already fetched over HTTP costs only its scripts and
    /// other subresources.
    pub async fn render(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
    ) -> Result<FetchResult, FetchError> {
        let start = Instant::now();
        let target = crate::url_security::validate_public_http_url(url).await?;
        let url = target.to_string();
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(browser_error)?;

        let mut extra = Vec::new();
        for (name, value) in &options.headers {
            if name.eq_ignore_ascii_case("cookie") {
                for pair in value.split(';') {
                    if let Some((cookie, cookie_value)) = pair.trim().split_once('=') {
                        let mut set = SetCookieParams::new(cookie.trim(), cookie_value.trim());
                        set.url = Some(url.clone());
                        page.execute(set).await.map_err(browser_error)?;
                    }
                }
            } else {
                extra.push((name.clone(), value.clone()));
            }
        }

        let mut paused = page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(browser_error)?;
        if let Some((username, password)) = &self.proxy_login {
            page.authenticate(Credentials {
                username: username.clone(),
                password: password.clone(),
            })
            .await
            .map_err(browser_error)?;
        }

        let mut main_document =
            prefetched.map(|p| (base64(p.html.as_bytes()), response_headers(p.headers)));
        let origin = target.origin();
        let guard_page = page.clone();
        let guard = tokio::spawn(async move {
            while let Some(event) = paused.next().await {
                let request_url = event.request.url.as_str();
                let id = event.request_id.clone();
                if event.resource_type == ResourceType::Document
                    && request_url == target.as_str()
                    && let Some((body, headers)) = main_document.take()
                    && let Ok(fulfill) = FulfillRequestParams::builder()
                        .request_id(id.clone())
                        .response_code(200)
                        .response_headers(headers)
                        .body(body)
                        .build()
                {
                    let _ = guard_page.execute(fulfill).await;
                    continue;
                }
                let parsed = Url::parse(request_url).ok();
                let allowed = !request_url.starts_with("http")
                    || crate::url_security::validate_public_http_url(request_url)
                        .await
                        .is_ok();
                if !allowed {
                    let _ = guard_page
                        .execute(FailRequestParams::new(id, ErrorReason::BlockedByClient))
                        .await;
                    continue;
                }
                let mut resume = ContinueRequestParams::new(id);
                if !extra.is_empty() && parsed.is_some_and(|u| u.origin() == origin) {
                    resume.headers = Some(merged_headers(event.request.headers.inner(), &extra));
                }
                let _ = guard_page.execute(resume).await;
            }
        });
        let outcome = tokio::time::timeout(options.timeout, async {
            page.goto(url.as_str()).await?;
            page.wait_for_network_almost_idle_with_timeout(options.timeout / 3)
                .await?;
            if let Some(selector) = &options.wait_for {
                page.wait_for_selector(selector.as_str(), Some(options.timeout))
                    .await?;
            }
            let html = page.content().await?;
            let final_url = page.url().await?.unwrap_or_else(|| url.clone());
            Ok::<_, chromiumoxide::error::CdpError>((html, final_url))
        })
        .await;
        guard.abort();
        let _ = page.close().await;

        let (html, final_url) = outcome
            .map_err(|_| FetchError::Build(format!("rendering {url} timed out")))?
            .map_err(browser_error)?;
        Ok(FetchResult {
            html,
            document_bytes: None,
            status: 200,
            url: final_url,
            headers: http::HeaderMap::new(),
            elapsed: start.elapsed(),
        })
    }

    /// Shut down a browser this renderer launched and wait for it to exit.
    /// A browser reached through [`Renderer::connect`] keeps running.
    pub async fn close(mut self) {
        if let Some(profile) = self.profile.take() {
            let _ = self.browser.close().await;
            let _ = self.browser.wait().await;
            let _ = std::fs::remove_dir_all(profile);
        }
        self.events.abort();
    }
}

const INSTALL_PATHS: &[&str] = &[
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
    "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
    "/usr/bin/google-chrome",
    "/usr/bin/google-chrome-stable",
    "/usr/bin/chromium",
    "/usr/bin/chromium-browser",
    r"C:\Program Files\Google\Chrome\Application\chrome.exe",
];

fn response_headers(headers: &http::HeaderMap) -> Vec<HeaderEntry> {
    let mut entries: Vec<HeaderEntry> = headers
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "content-encoding" | "content-length" | "transfer-encoding" | "connection"
            )
        })
        .filter_map(|(name, value)| Some(HeaderEntry::new(name.as_str(), value.to_str().ok()?)))
        .collect();
    if !headers.contains_key(http::header::CONTENT_TYPE) {
        entries.push(HeaderEntry::new("content-type", "text/html; charset=utf-8"));
    }
    entries
}

fn merged_headers(original: &serde_json::Value, extra: &[(String, String)]) -> Vec<HeaderEntry> {
    let mut entries: Vec<HeaderEntry> = original
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(name, _)| !extra.iter().any(|(e, _)| e.eq_ignore_ascii_case(name)))
        .filter_map(|(name, value)| Some(HeaderEntry::new(name.as_str(), value.as_str()?)))
        .collect();
    entries.extend(
        extra
            .iter()
            .map(|(n, v)| HeaderEntry::new(n.as_str(), v.as_str())),
    );
    entries
}

fn percent_decode(text: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", text.replace('+', "%2B")).as_bytes())
        .next()
        .map_or_else(|| text.to_string(), |(_, v)| v.into_owned())
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n =
            chunk.iter().fold(0u32, |acc, b| (acc << 8) | u32::from(*b)) << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn browser_error(e: impl std::fmt::Display) -> FetchError {
    FetchError::Build(format!("browser: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{Prefetched, RenderOptions, Renderer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn a_rendered_page_cannot_reach_loopback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        tokio::spawn(async move {
            while listener.accept().await.is_ok() {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        let html = format!(
            r#"<html><body><p id="msg">rendered</p><img src="http://127.0.0.1:{port}/img"><script>fetch("http://127.0.0.1:{port}/xhr").catch(() => {{}}); document.getElementById("msg").textContent = "scripts ran";</script></body></html>"#
        );
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let rendered = renderer
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: &html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        let rendered = rendered.expect("render succeeds");
        assert!(rendered.html.contains("scripts ran"), "{}", rendered.html);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the page reached loopback");
    }

    #[test]
    fn base64_matches_the_rfc_vectors() {
        for (input, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(super::base64(input.as_bytes()), encoded);
        }
    }

    #[test]
    fn prefetched_headers_keep_cookies_and_drop_transfer_encoding() {
        let mut headers = http::HeaderMap::new();
        headers.append("set-cookie", "a=1".parse().unwrap());
        headers.append("set-cookie", "b=2".parse().unwrap());
        headers.insert("content-encoding", "br".parse().unwrap());
        let entries = super::response_headers(&headers);
        let cookies: Vec<_> = entries.iter().filter(|e| e.name == "set-cookie").collect();
        assert_eq!(cookies.len(), 2);
        assert!(!entries.iter().any(|e| e.name == "content-encoding"));
        assert!(entries.iter().any(|e| e.name == "content-type"));
    }

    #[test]
    fn extra_headers_override_the_originals() {
        let original = serde_json::json!({"accept-language": "fr", "user-agent": "x"});
        let merged = super::merged_headers(
            &original,
            &[
                ("Accept-Language".into(), "it-IT".into()),
                ("Authorization".into(), "Bearer t".into()),
            ],
        );
        assert!(
            merged
                .iter()
                .any(|e| e.name == "Accept-Language" && e.value == "it-IT")
        );
        assert!(!merged.iter().any(|e| e.value == "fr"));
        assert!(merged.iter().any(|e| e.name == "user-agent"));
        assert!(merged.iter().any(|e| e.name == "Authorization"));
    }

    #[test]
    fn proxy_credentials_are_percent_decoded() {
        assert_eq!(super::percent_decode("p%40ss%3Aword"), "p@ss:word");
        assert_eq!(super::percent_decode("plain"), "plain");
    }
}
