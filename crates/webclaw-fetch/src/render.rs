//! Local JavaScript rendering through Chrome's DevTools protocol.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::auth::Credentials;
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, FailRequestParams, FulfillRequestParams, HeaderEntry,
};
use chromiumoxide::cdp::browser_protocol::network::{
    ErrorReason, EventResponseReceived, ResourceType, SetCookieParams,
};
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams,
};
use futures_util::StreamExt;
use rand::seq::SliceRandom;
use tokio::sync::{RwLock, RwLockReadGuard, Semaphore};
use tracing::warn;
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
        static LAUNCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let profile = std::env::temp_dir().join(format!(
            "webclaw-chrome-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
            LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
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
        Self::start(browser, handler, Some(profile), proxy_login).await
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
        Self::start(browser, handler, None, None).await
    }

    async fn start(
        browser: Browser,
        mut handler: chromiumoxide::Handler,
        profile: Option<std::path::PathBuf>,
        proxy_login: Option<(String, String)>,
    ) -> Result<Self, FetchError> {
        let events = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        let mut renderer = Self {
            browser,
            events,
            profile,
            proxy_login,
        };
        if let Err(e) = renderer.browser.start_incognito_context().await {
            renderer.close().await;
            return Err(browser_error(e));
        }
        Ok(renderer)
    }

    /// Render `url` and return the DOM after its scripts ran. The result has
    /// status 200 and no headers; extract it with [`crate::FetchClient::extract`].
    ///
    /// With `prefetched`, the browser gets that response for the page itself
    /// (body and headers, `Set-Cookie` included) instead of requesting it
    /// again, so a page already fetched over HTTP costs only its scripts and
    /// other subresources.
    ///
    /// A render that sends a `Cookie` header runs in its own browser context,
    /// so those cookies, and any the site sets in reply, never reach another
    /// render. Other renders share one context of their own, with its cache and
    /// cookies, apart from the browser's default profile.
    pub async fn render(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
    ) -> Result<FetchResult, FetchError> {
        let private = options
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("cookie"));
        if !private {
            return self.render_in(None, url, prefetched, options).await;
        }
        let context = self
            .browser
            .execute(CreateBrowserContextParams::default())
            .await
            .map_err(browser_error)?
            .result
            .browser_context_id;
        let rendered = self
            .render_in(Some(context.clone()), url, prefetched, options)
            .await;
        let _ = self.browser.dispose_browser_context(context).await;
        rendered
    }

    async fn render_in(
        &self,
        context: Option<BrowserContextId>,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
    ) -> Result<FetchResult, FetchError> {
        let start = Instant::now();
        let target = crate::url_security::validate_public_http_url(url).await?;
        let url = target.to_string();
        let mut blank = CreateTargetParams::new("about:blank");
        blank.browser_context_id = context;
        let page = self.browser.new_page(blank).await.map_err(browser_error)?;
        let main_frame = page.mainframe().await.map_err(browser_error)?;
        let mut responses = page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(browser_error)?;
        let document_status = tokio::spawn(async move {
            while let Some(event) = responses.next().await {
                if event.r#type == ResourceType::Document
                    && (main_frame.is_none() || event.frame_id == main_frame)
                {
                    return Some(event.response.status);
                }
            }
            None
        });

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

        let settled_words = settle_floor(prefetched.as_ref().map(|p| p.html));
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
            tokio::select! {
                _ = page.wait_for_network_almost_idle_with_timeout(options.timeout / 3) => {}
                _ = text_settled(&page, settled_words) => {}
            }
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
        let status = if document_status.is_finished() {
            document_status.await.ok().flatten()
        } else {
            document_status.abort();
            None
        };
        let _ = page.close().await;

        let (html, final_url) = outcome
            .map_err(|_| FetchError::Build(format!("rendering {url} timed out")))?
            .map_err(browser_error)?;
        Ok(FetchResult {
            html,
            document_bytes: None,
            status: status
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(200),
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

/// Where a [`RenderPool`] gets its browser.
#[derive(Debug, Clone)]
pub enum BrowserSource {
    /// Launch a local headless Chrome; see [`Renderer::launch`].
    Launch { proxy: Option<String> },
    /// Use a browser already running at this DevTools endpoint; see [`Renderer::connect`].
    Connect(String),
}

impl BrowserSource {
    /// Launch a local Chrome behind `config.proxy`, or behind one proxy
    /// picked at random from `config.proxy_pool`.
    pub fn launch_for(config: &crate::FetchConfig) -> Self {
        let proxy = config
            .proxy
            .clone()
            .or_else(|| config.proxy_pool.choose(&mut rand::thread_rng()).cloned());
        Self::Launch { proxy }
    }
}

/// One browser shared by every fetch that may need rendering.
///
/// The browser starts on the first render, so a crawl or batch that never
/// meets a script-built page never launches it. At most four pages render at
/// once. Per host, the pool remembers whether rendering found more text:
/// after three renders it renders every scripted page of a host where
/// renders keep helping, and stops rendering on a host where they do not.
pub struct RenderPool {
    source: BrowserSource,
    timeout: Duration,
    browser: RwLock<Option<Result<Renderer, String>>>,
    tabs: Semaphore,
    hosts: Mutex<HashMap<String, HostRenders>>,
}

#[derive(Debug, Default, Clone, Copy)]
struct HostRenders {
    renders: u32,
    helped: u32,
}

const MAX_TABS: usize = 4;

/// How a [`crate::FetchClient`] uses a [`RenderPool`]; see [`crate::FetchConfig::render`].
#[derive(Debug, Clone)]
pub struct RenderConfig {
    pub pool: Arc<RenderPool>,
    /// Render every page that runs scripts, not only the ones that seem to need it.
    pub always: bool,
    /// Also wait until an element matching this CSS selector exists.
    pub wait_for: Option<String>,
}

impl std::fmt::Debug for RenderPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RenderPool")
            .field("source", &self.source)
            .field("timeout", &self.timeout)
            .finish_non_exhaustive()
    }
}

impl RenderPool {
    /// A pool that gets its browser from `source` when it first renders.
    /// `timeout` bounds each render.
    pub fn new(source: BrowserSource, timeout: Duration) -> Self {
        Self {
            source,
            timeout,
            browser: RwLock::new(None),
            tabs: Semaphore::new(MAX_TABS),
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Render `url`; see [`Renderer::render`]. A failed browser launch is
    /// remembered, so later renders fail at once instead of retrying it.
    pub async fn render(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        wait_for: Option<&str>,
        headers: &[(String, String)],
    ) -> Result<FetchResult, FetchError> {
        let _tab = self
            .tabs
            .acquire()
            .await
            .map_err(|_| FetchError::Build("browser closed".into()))?;
        let renderer = self.renderer().await?;
        let options = RenderOptions {
            wait_for: wait_for.map(str::to_string),
            timeout: self.timeout,
            headers: headers.to_vec(),
        };
        renderer.render(url, prefetched, &options).await
    }

    async fn renderer(&self) -> Result<RwLockReadGuard<'_, Renderer>, FetchError> {
        if self.browser.read().await.is_none() {
            let mut slot = self.browser.write().await;
            if slot.is_none() {
                let started = match &self.source {
                    BrowserSource::Launch { proxy } => Renderer::launch(proxy.as_deref()).await,
                    BrowserSource::Connect(endpoint) => Renderer::connect(endpoint).await,
                };
                if let Err(e) = &started {
                    warn!("no browser for rendering: {e}");
                }
                *slot = Some(started.map_err(|e| e.to_string()));
            }
        }
        RwLockReadGuard::try_map(self.browser.read().await, |slot| {
            slot.as_ref().and_then(|started| started.as_ref().ok())
        })
        .map_err(|slot| {
            FetchError::Build(match slot.as_ref() {
                Some(Err(e)) => e.clone(),
                _ => "browser closed".into(),
            })
        })
    }

    /// Render `fetched` when `config` asks for it and the page seems to need
    /// a browser, handing the browser the HTTP response so the document is
    /// not requested twice. `http_words` is the word count extracted from it
    /// over HTTP. A failed render is logged and gives `None`.
    pub async fn render_if_needed(
        &self,
        fetched: &FetchResult,
        http_words: usize,
        config: &RenderConfig,
        headers: &[(String, String)],
    ) -> Option<FetchResult> {
        if !self.wants(fetched, http_words, config.always) {
            return None;
        }
        let prefetched = Prefetched {
            html: &fetched.html,
            headers: &fetched.headers,
        };
        let rendered = self
            .render(
                &fetched.url,
                Some(prefetched),
                config.wait_for.as_deref(),
                headers,
            )
            .await;
        match rendered {
            Ok(rendered) => {
                let words = webclaw_core::extract(&rendered.html, Some(&rendered.url))
                    .map_or(0, |e| e.metadata.word_count);
                self.record(&fetched.url, http_words, words);
                Some(rendered)
            }
            Err(e) => {
                warn!(url = %fetched.url, "rendering failed, keeping the HTTP result: {e}");
                None
            }
        }
    }

    fn wants(&self, fetched: &FetchResult, http_words: usize, always: bool) -> bool {
        if fetched.status >= 400
            || fetched.document_bytes.is_some()
            || !crate::cloud::contains_ignore_case(&fetched.html, "<script")
        {
            return false;
        }
        if always {
            return true;
        }
        let known = host(&fetched.url).and_then(|host| {
            let hosts = self.hosts.lock().ok()?;
            let seen = hosts.get(&host)?;
            verdict(seen.renders, seen.helped)
        });
        known.unwrap_or_else(|| crate::cloud::needs_js_rendering(http_words, &fetched.html))
    }

    fn record(&self, url: &str, http_words: usize, rendered_words: usize) {
        let (Some(host), Ok(mut hosts)) = (host(url), self.hosts.lock()) else {
            return;
        };
        let seen = hosts.entry(host).or_default();
        seen.renders += 1;
        seen.helped += u32::from(rendered_words >= (2 * http_words).max(http_words + 50));
    }

    /// Shut down the browser if this pool launched it. A later render starts a new one.
    pub async fn close(&self) {
        if let Some(Ok(renderer)) = self.browser.write().await.take() {
            renderer.close().await;
        }
    }
}

fn host(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()?
        .host_str()
        .map(str::to_ascii_lowercase)
}

fn verdict(renders: u32, helped: u32) -> Option<bool> {
    if renders < 3 {
        None
    } else if helped * 5 <= renders {
        Some(false)
    } else if helped * 5 >= renders * 4 {
        Some(true)
    } else {
        None
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

fn settle_floor(http_html: Option<&str>) -> usize {
    http_html.map_or(0, webclaw_core::quality::visible_word_count) + 50
}

async fn text_settled(page: &chromiumoxide::Page, min_words: usize) {
    const PROBE: &str = "(() => { const t = document.body ? document.body.innerText : ''; \
                         return [t.length, t.split(/\\s+/).filter(Boolean).length]; })()";
    let mut last = None;
    let mut steady = 0;
    loop {
        let probe = page.evaluate(PROBE).await.ok();
        let sample = probe.and_then(|r| r.into_value::<(usize, usize)>().ok());
        steady = match (sample, last) {
            (Some((chars, words)), Some(before)) if chars == before && words >= min_words => {
                steady + 1
            }
            _ => 0,
        };
        if steady >= 2 {
            return;
        }
        last = sample.map(|(chars, _)| chars);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn response_headers(headers: &http::HeaderMap) -> Vec<HeaderEntry> {
    let mut entries: Vec<HeaderEntry> = headers
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.as_str(),
                "content-encoding"
                    | "content-length"
                    | "transfer-encoding"
                    | "connection"
                    | "content-type"
            )
        })
        .filter_map(|(name, value)| Some(HeaderEntry::new(name.as_str(), value.to_str().ok()?)))
        .collect();
    let mime = headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("text/html");
    entries.push(HeaderEntry::new(
        "content-type",
        format!("{mime}; charset=utf-8"),
    ));
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

    #[tokio::test]
    #[ignore = "needs a local Chrome"]
    async fn cookies_from_one_render_never_reach_the_next() {
        let mut first_headers = http::HeaderMap::new();
        first_headers.insert("set-cookie", "served=1; Path=/".parse().unwrap());
        let reader = r#"<html><body><p id="c"></p><script>document.getElementById("c").textContent = "cookies:" + document.cookie + ":end";</script></body></html>"#;
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let first = renderer
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: reader,
                    headers: &first_headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    headers: vec![("Cookie".into(), "session=abc".into())],
                    ..Default::default()
                },
            )
            .await;
        let empty = http::HeaderMap::new();
        let second = renderer
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: reader,
                    headers: &empty,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        let first = first.expect("first render succeeds").html;
        let second = second.expect("second render succeeds").html;
        assert!(first.contains("session=abc"), "{first}");
        assert!(second.contains("cookies::end"), "{second}");
    }

    #[test]
    fn the_text_must_grow_past_the_http_page_before_it_counts_as_settled() {
        let menu = format!(
            "<html><body><nav>{}</nav><div id=\"root\"></div></body></html>",
            "Menu ".repeat(60)
        );
        assert_eq!(super::settle_floor(Some(&menu)), 110);
        assert_eq!(super::settle_floor(None), 50);
    }

    #[tokio::test]
    #[ignore = "needs a local Chrome and network access"]
    async fn a_rendered_error_page_keeps_its_status() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let rendered = renderer
            .render(
                "https://books.toscrape.com/no-such-page",
                None,
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        assert_eq!(rendered.expect("render succeeds").status, 404);
    }

    fn page(url: &str, html: &str, status: u16) -> crate::client::FetchResult {
        crate::client::FetchResult {
            html: html.into(),
            document_bytes: None,
            status,
            url: url.into(),
            headers: http::HeaderMap::new(),
            elapsed: std::time::Duration::ZERO,
        }
    }

    fn pool() -> super::RenderPool {
        super::RenderPool::new(
            super::BrowserSource::Launch { proxy: None },
            std::time::Duration::from_secs(1),
        )
    }

    #[test]
    fn renders_only_script_pages_with_too_little_text() {
        let pool = pool();
        let shell =
            r#"<html><body><div id="root"></div><script src="/app.js"></script></body></html>"#;
        assert!(pool.wants(&page("https://a.test/", shell, 200), 0, false));
        assert!(!pool.wants(&page("https://a.test/", shell, 404), 0, false));
        let article = format!(
            "<html><body><article>{}</article><script src=\"/app.js\"></script></body></html>",
            "word ".repeat(300)
        );
        assert!(!pool.wants(&page("https://a.test/", &article, 200), 300, false));
        assert!(pool.wants(&page("https://a.test/", &article, 200), 300, true));
        let no_scripts = "<html><body><p>short</p></body></html>";
        assert!(!pool.wants(&page("https://a.test/", no_scripts, 200), 1, true));
    }

    #[test]
    fn a_host_where_renders_help_renders_every_scripted_page() {
        let pool = pool();
        let thick = format!(
            "<html><body><p>{}</p><script src=\"/app.js\"></script></body></html>",
            "word ".repeat(120)
        );
        let spa = page("https://spa.test/a", &thick, 200);
        assert!(!pool.wants(&spa, 120, false));
        for _ in 0..3 {
            pool.record("https://spa.test/x", 5, 400);
        }
        assert!(pool.wants(&spa, 120, false));
        assert!(!pool.wants(&page("https://other.test/a", &thick, 200), 120, false));
    }

    #[test]
    fn a_host_where_renders_do_not_help_stops_rendering() {
        let pool = pool();
        let shell =
            r#"<html><body><div id="root"></div><script src="/app.js"></script></body></html>"#;
        let ssr = page("https://ssr.test/a", shell, 200);
        assert!(pool.wants(&ssr, 0, false));
        for _ in 0..3 {
            pool.record("https://ssr.test/x", 30, 40);
        }
        assert!(!pool.wants(&ssr, 0, false));
        assert!(pool.wants(&ssr, 0, true));
    }

    #[test]
    fn the_verdict_needs_three_renders_and_a_clear_majority() {
        assert_eq!(super::verdict(2, 0), None);
        assert_eq!(super::verdict(3, 0), Some(false));
        assert_eq!(super::verdict(3, 3), Some(true));
        assert_eq!(super::verdict(3, 2), None);
        assert_eq!(super::verdict(10, 2), Some(false));
        assert_eq!(super::verdict(10, 8), Some(true));
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
        headers.insert(
            "content-type",
            "text/html; charset=iso-8859-1".parse().unwrap(),
        );
        let entries = super::response_headers(&headers);
        let types: Vec<_> = entries
            .iter()
            .filter(|e| e.name == "content-type")
            .collect();
        assert_eq!(types.len(), 1);
        assert_eq!(types[0].value, "text/html; charset=utf-8");
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
    fn the_browser_uses_the_proxy_pool_when_no_single_proxy_is_set() {
        let config = crate::FetchConfig {
            proxy_pool: vec!["http://pool.example:8080".into()],
            ..Default::default()
        };
        assert!(matches!(
            super::BrowserSource::launch_for(&config),
            super::BrowserSource::Launch { proxy: Some(p) } if p == "http://pool.example:8080"
        ));
    }

    #[test]
    fn proxy_credentials_are_percent_decoded() {
        assert_eq!(super::percent_decode("p%40ss%3Aword"), "p@ss:word");
        assert_eq!(super::percent_decode("plain"), "plain");
    }
}
