//! Local JavaScript rendering through Chrome's DevTools protocol.

use std::time::{Duration, Instant};

use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams,
    FulfillRequestParams, HeaderEntry,
};
use chromiumoxide::cdp::browser_protocol::network::ErrorReason;
use chromiumoxide::cdp::browser_protocol::network::ResourceType;
use futures_util::StreamExt;

use crate::client::FetchResult;
use crate::error::FetchError;

/// How to render one page.
#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Also wait until an element matching this CSS selector exists.
    pub wait_for: Option<String>,
    /// Upper bound for navigating, waiting and reading the page.
    pub timeout: Duration,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            wait_for: None,
            timeout: Duration::from_secs(30),
        }
    }
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
}

impl Renderer {
    /// Launch a local headless Chrome with a throwaway profile, so several
    /// webclaw processes can render at once. `WEBCLAW_CHROME` points at the
    /// executable; without it the standard install locations are tried, then
    /// the browsers on `PATH`.
    pub async fn launch() -> Result<Self, FetchError> {
        let profile = std::env::temp_dir().join(format!(
            "webclaw-chrome-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let mut config = BrowserConfig::builder()
            .headless_mode(HeadlessMode::New)
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
        let config = config.build().map_err(browser_error)?;
        let (browser, handler) = match Browser::launch(config).await {
            Ok(launched) => launched,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(browser_error(e));
            }
        };
        Ok(Self::start(browser, handler, Some(profile)))
    }

    /// Use a browser that is already running with its DevTools endpoint at
    /// `cdp_url` (`ws://...` or `http://host:port`).
    pub async fn connect(cdp_url: &str) -> Result<Self, FetchError> {
        let (browser, handler) = Browser::connect(cdp_url).await.map_err(browser_error)?;
        Ok(Self::start(browser, handler, None))
    }

    fn start(
        browser: Browser,
        mut handler: chromiumoxide::Handler,
        profile: Option<std::path::PathBuf>,
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
        }
    }

    /// Render `url` and return the DOM after its scripts ran. The result has
    /// status 200 and no headers; extract it with [`crate::FetchClient::extract`].
    ///
    /// With `html`, the browser gets that body for the page itself instead of
    /// requesting it again, so a page already fetched over HTTP costs only
    /// its scripts and other subresources.
    pub async fn render(
        &self,
        url: &str,
        html: Option<&str>,
        options: &RenderOptions,
    ) -> Result<FetchResult, FetchError> {
        let start = Instant::now();
        let url = crate::url_security::validate_public_http_url(url)
            .await?
            .to_string();
        let page = self
            .browser
            .new_page("about:blank")
            .await
            .map_err(browser_error)?;
        let mut paused = page
            .event_listener::<EventRequestPaused>()
            .await
            .map_err(browser_error)?;
        page.execute(EnableParams::default())
            .await
            .map_err(browser_error)?;
        let guard_page = page.clone();
        let mut prefetched = html.map(|body| (url.clone(), base64(body.as_bytes())));
        let guard = tokio::spawn(async move {
            while let Some(event) = paused.next().await {
                let target = event.request.url.as_str();
                if event.resource_type == ResourceType::Document
                    && prefetched.as_ref().is_some_and(|(main, _)| main == target)
                    && let Some((_, body)) = prefetched.take()
                    && let Ok(fulfill) = FulfillRequestParams::builder()
                        .request_id(event.request_id.clone())
                        .response_code(200)
                        .response_header(HeaderEntry::new(
                            "content-type",
                            "text/html; charset=utf-8",
                        ))
                        .body(body)
                        .build()
                {
                    let _ = guard_page.execute(fulfill).await;
                    continue;
                }
                let allowed = !target.starts_with("http")
                    || crate::url_security::validate_public_http_url(target)
                        .await
                        .is_ok();
                let id = event.request_id.clone();
                let _ = if allowed {
                    guard_page
                        .execute(ContinueRequestParams::new(id))
                        .await
                        .map(drop)
                } else {
                    guard_page
                        .execute(FailRequestParams::new(id, ErrorReason::BlockedByClient))
                        .await
                        .map(drop)
                };
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
}
