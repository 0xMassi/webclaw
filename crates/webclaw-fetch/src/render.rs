//! Local JavaScript rendering through Chrome's DevTools protocol.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::auth::Credentials;
use chromiumoxide::browser::{Browser, BrowserConfig, HeadlessMode};
use chromiumoxide::cdp::browser_protocol::browser::BrowserContextId;
use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EnableParams, EventRequestPaused, FailRequestParams,
    FulfillRequestParams, HeaderEntry,
};
use chromiumoxide::cdp::browser_protocol::network::{
    ErrorReason, EventResponseReceived, GetCookiesParams, ResourceType, SetCookieParams,
};
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams,
};
use chromiumoxide::page::ScreenshotParams;
use futures_util::StreamExt;
use rand::seq::SliceRandom;
use tokio::sync::{RwLock, RwLockReadGuard, Semaphore};
use tracing::warn;
use url::Url;

mod solve;

use crate::Block;
pub use crate::actions::{Action, Direction, parse_actions};
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
    /// Resource types whose requests fail; see [`Block`] for what it misses.
    pub block: Block,
    /// Get past a Cloudflare challenge page: wait for one that clears on its
    /// own, click the checkbox of one that asks for it, then read the page it
    /// leads to. A Turnstile widget embedded in the page is clicked as well.
    /// The render fails when the challenge is still shown after three
    /// quarters of `timeout`, leaving the rest for the page it leads to.
    /// `headers` then go on the page's other requests to its origin but not
    /// on its documents, since rewriting a document request fails the
    /// challenge. Cloudflare rejects a browser that calls itself
    /// `HeadlessChrome`, so launch it with [`Renderer::launch_as_chrome`].
    pub solve: bool,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            wait_for: None,
            timeout: Duration::from_secs(30),
            headers: Vec::new(),
            block: Block::default(),
            solve: false,
        }
    }
}

impl Block {
    fn blocks(self, kind: &ResourceType) -> bool {
        match kind {
            ResourceType::Image => self.images,
            ResourceType::Font => self.fonts,
            ResourceType::Media | ResourceType::TextTrack => self.media,
            _ => false,
        }
    }
}

/// What a screenshot captures.
#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenshotOptions {
    /// The whole scrollable page instead of the 1280x720 viewport.
    pub full_page: bool,
    /// JPEG at quality 80 instead of PNG.
    pub jpeg: bool,
}

/// A screenshot of a rendered page.
#[derive(Debug, Clone)]
pub struct Screenshot {
    /// The encoded PNG or JPEG image.
    pub image: Vec<u8>,
    /// HTTP status of the page's document.
    pub status: u16,
    /// The page's URL after redirects.
    pub url: String,
}

impl Screenshot {
    /// The image in standard base64, as MCP image content carries it.
    pub fn base64(&self) -> String {
        base64(&self.image)
    }
}

const SCREENSHOT_VIEWPORT: (i64, i64) = (1280, 720);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

struct Visited<T> {
    value: T,
    status: u16,
    url: String,
    scripts: Vec<serde_json::Value>,
}

/// A page already fetched over HTTP, handed to the browser so it does not
/// request the document a second time.
pub struct Prefetched<'a> {
    pub html: &'a str,
    pub headers: &'a http::HeaderMap,
}

/// A Chrome that renders pages so their JavaScript-built content can be extracted.
///
/// In a browser this renderer launches:
///
/// - every HTTP request from the page, its frames and its workers, each
///   redirect included, goes through the same public-address check as
///   webclaw's own HTTP client, and fails when the host resolves to a
///   private, loopback, link-local or reserved address;
/// - Chrome's local network checks refuse a public page's WebSockets to
///   those addresses, after the TCP connection opens but before anything
///   is sent. Behind a proxy, WebSockets to anything but loopback go
///   through the proxy, so they are only as safe as the proxy;
/// - speculation rules, prefetching and preconnects are off;
/// - WebRTC sends no UDP, though a TURN server over TCP or TLS can still
///   open a TCP connection to a local port;
/// - without a proxy, the render fails when a document of the page was
///   served from a blocked address, as after DNS rebinding. The request
///   itself still goes out, and a popup the page opens is not checked.
///
/// Behind a proxy, the proxy resolves host names itself and the document
/// check is off, so HTTP too is only as safe as the proxy. Service workers
/// are untested. A browser reached through [`Renderer::connect`] is checked
/// page by page instead, so its other tabs are left alone; the frames,
/// workers, WebSockets, WebRTC and speculation rules of a page it renders
/// are not covered there.
pub struct Renderer {
    browser: Arc<Browser>,
    guard: Option<tokio::task::JoinHandle<()>>,
    events: tokio::task::JoinHandle<()>,
    profile: Option<std::path::PathBuf>,
    proxy_login: Option<(String, String)>,
    check_documents: bool,
    agent: String,
    clearances: Mutex<Vec<(String, String, String)>>,
}

impl Renderer {
    /// Launch a local headless Chrome with a throwaway profile, so several
    /// webclaw processes can render at once. `proxy` is an `http://` or
    /// `https://` proxy URL, credentials included. `WEBCLAW_CHROME` points at
    /// the executable; without it the standard install locations are tried,
    /// then the browsers on `PATH`.
    pub async fn launch(proxy: Option<&str>) -> Result<Self, FetchError> {
        Self::launch_with(proxy, &[]).await
    }

    /// [`Renderer::launch`], with the browser calling itself Chrome instead
    /// of `HeadlessChrome` in its User-Agent, in every page and worker, so
    /// that it can get past Cloudflare challenges. Chrome tells its version
    /// only once it runs, so this launches it twice, about half a second more.
    pub async fn launch_as_chrome(proxy: Option<&str>) -> Result<Self, FetchError> {
        Self::launch_as_chrome_timed(proxy, COMMAND_TIMEOUT).await
    }

    async fn launch_as_chrome_timed(
        proxy: Option<&str>,
        command_timeout: Duration,
    ) -> Result<Self, FetchError> {
        let renderer = Self::launch_timed(proxy, &[], command_timeout).await?;
        if !renderer.agent.contains("HeadlessChrome") {
            return Ok(renderer);
        }
        let agent = format!(
            "--user-agent={}",
            renderer.agent.replace("HeadlessChrome", "Chrome")
        );
        renderer.close().await;
        Self::launch_timed(proxy, &[agent.as_str()], command_timeout).await
    }

    async fn launch_with(proxy: Option<&str>, args: &[&str]) -> Result<Self, FetchError> {
        Self::launch_timed(proxy, args, COMMAND_TIMEOUT).await
    }

    async fn launch_timed(
        proxy: Option<&str>,
        args: &[&str],
        command_timeout: Duration,
    ) -> Result<Self, FetchError> {
        static LAUNCHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let profile = std::env::temp_dir().join(format!(
            "webclaw-chrome-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos()),
            LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(profile.join("Default"))
            .and_then(|()| {
                std::fs::write(
                    profile.join("Default").join("Preferences"),
                    r#"{"net":{"network_prediction_options":2}}"#,
                )
            })
            .map_err(|e| FetchError::Build(format!("browser profile: {e}")))?;
        let mut config = BrowserConfig::builder()
            .headless_mode(HeadlessMode::New)
            .enable_request_intercept()
            .arg("--disable-background-networking")
            .arg("--disable-component-update")
            .arg("--enable-features=NetworkService,NetworkServiceInProcess,LocalNetworkAccessChecks:LocalNetworkAccessChecksWarn/false,LocalNetworkAccessChecksWebSockets")
            .arg("--webrtc-ip-handling-policy=disable_non_proxied_udp")
            .args(args.iter().copied())
            .request_timeout(command_timeout)
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
            let public: Vec<String> = tokio::net::lookup_host(format!("{host}:{port}"))
                .await
                .map(|addresses| addresses.map(|a| format!("{a}=public")).collect())
                .unwrap_or_default();
            if !public.is_empty() {
                config = config.arg(format!("--ip-address-space-overrides={}", public.join(",")));
            }
            if !parsed.username().is_empty() {
                proxy_login = Some((
                    percent_decode(parsed.username()),
                    percent_decode(parsed.password().unwrap_or_default()),
                ));
            }
        } else {
            config = config.arg("--no-proxy-server");
        }
        let config = config.build().map_err(browser_error)?;
        let (browser, handler) = match Browser::launch(config).await {
            Ok(launched) => launched,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&profile);
                return Err(browser_error(e));
            }
        };
        let mut renderer = Self::start(browser, handler, Some(profile), proxy_login, true).await?;
        renderer.check_documents = proxy.is_none();
        Ok(renderer)
    }

    /// Use a browser that is already running with its DevTools endpoint at
    /// `cdp_url` (`ws://...` or `http://host:port`). Its own proxy settings apply.
    pub async fn connect(cdp_url: &str) -> Result<Self, FetchError> {
        Self::connect_timed(cdp_url, COMMAND_TIMEOUT).await
    }

    async fn connect_timed(cdp_url: &str, command_timeout: Duration) -> Result<Self, FetchError> {
        let config = chromiumoxide::handler::HandlerConfig {
            request_intercept: true,
            request_timeout: command_timeout,
            ..Default::default()
        };
        let (browser, handler) = Browser::connect_with_config(cdp_url, config)
            .await
            .map_err(browser_error)?;
        Self::start(browser, handler, None, None, false).await
    }

    async fn start(
        mut browser: Browser,
        mut handler: chromiumoxide::Handler,
        profile: Option<std::path::PathBuf>,
        proxy_login: Option<(String, String)>,
        browser_wide: bool,
    ) -> Result<Self, FetchError> {
        let events = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        let ready = async {
            browser.start_incognito_context().await?;
            if !browser_wide {
                return Ok(None);
            }
            let paused = browser.event_listener::<EventRequestPaused>().await?;
            browser.execute(EnableParams::default()).await?;
            Ok::<_, chromiumoxide::error::CdpError>(Some(paused))
        }
        .await;
        let agent = browser.user_agent().await.unwrap_or_default();
        let mut renderer = Self {
            browser: Arc::new(browser),
            guard: None,
            events,
            profile,
            proxy_login,
            check_documents: false,
            agent,
            clearances: Mutex::new(Vec::new()),
        };
        match ready {
            Ok(paused) => {
                let browser = Arc::downgrade(&renderer.browser);
                renderer.guard = paused.map(|paused| tokio::spawn(keep_public(browser, paused)));
                Ok(renderer)
            }
            Err(e) => {
                renderer.close().await;
                Err(browser_error(e))
            }
        }
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
        Ok(self.render_actions(url, prefetched, options, &[]).await?.0)
    }

    /// [`Renderer::render`], running `actions` in order once the page has
    /// loaded and before it is read. They get their own time budget, equal to
    /// `options.timeout`, and the first failing step fails the render with
    /// its number in the error. Returns the page and the values of its
    /// `executeJavascript` steps, the hosted API's `js_results`.
    pub async fn render_actions(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        actions: &[Action],
    ) -> Result<(FetchResult, Vec<serde_json::Value>), FetchError> {
        let start = Instant::now();
        let visited = self
            .visit(url, prefetched, options, actions, None, async |page| {
                page.content().await
            })
            .await?;
        let page = FetchResult {
            html: visited.value,
            document_bytes: None,
            status: visited.status,
            url: visited.url,
            headers: http::HeaderMap::new(),
            elapsed: start.elapsed(),
        };
        Ok((page, visited.scripts))
    }

    /// Render `url` in a 1280x720 viewport and capture it once its content
    /// has settled, under the same rules as [`Renderer::render`].
    /// `options.block` is ignored: a screenshot loads everything.
    pub async fn screenshot(
        &self,
        url: &str,
        options: &RenderOptions,
        shot: ScreenshotOptions,
    ) -> Result<Screenshot, FetchError> {
        Ok(self.screenshot_actions(url, options, shot, &[]).await?.0)
    }

    /// [`Renderer::screenshot`] after `actions`, as [`Renderer::render_actions`] runs them.
    pub async fn screenshot_actions(
        &self,
        url: &str,
        options: &RenderOptions,
        shot: ScreenshotOptions,
        actions: &[Action],
    ) -> Result<(Screenshot, Vec<serde_json::Value>), FetchError> {
        let mut params = ScreenshotParams::builder().full_page(shot.full_page);
        params = if shot.jpeg {
            params.format(CaptureScreenshotFormat::Jpeg).quality(80)
        } else {
            params.format(CaptureScreenshotFormat::Png)
        };
        let params = params.build();
        let visited = self
            .visit(
                url,
                None,
                &RenderOptions {
                    block: Block::default(),
                    ..options.clone()
                },
                actions,
                Some(SCREENSHOT_VIEWPORT),
                async move |page| page.screenshot(params).await,
            )
            .await?;
        let shot = Screenshot {
            image: visited.value,
            status: visited.status,
            url: visited.url,
        };
        Ok((shot, visited.scripts))
    }

    async fn visit<T>(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        actions: &[Action],
        viewport: Option<(i64, i64)>,
        read: impl AsyncFnOnce(&chromiumoxide::Page) -> Result<T, chromiumoxide::error::CdpError>,
    ) -> Result<Visited<T>, FetchError> {
        let private = options
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("cookie"));
        if !private {
            return self
                .visit_in(None, url, prefetched, options, actions, viewport, read)
                .await;
        }
        let context = self
            .browser
            .execute(CreateBrowserContextParams::default())
            .await
            .map_err(browser_error)?
            .result
            .browser_context_id;
        let visited = self
            .visit_in(
                Some(context.clone()),
                url,
                prefetched,
                options,
                actions,
                viewport,
                read,
            )
            .await;
        let _ = self.browser.dispose_browser_context(context).await;
        visited
    }

    #[allow(clippy::too_many_arguments)]
    async fn visit_in<T>(
        &self,
        context: Option<BrowserContextId>,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        actions: &[Action],
        viewport: Option<(i64, i64)>,
        read: impl AsyncFnOnce(&chromiumoxide::Page) -> Result<T, chromiumoxide::error::CdpError>,
    ) -> Result<Visited<T>, FetchError> {
        let target = crate::url_security::validate_public_http_url(url).await?;
        let url = target.to_string();
        let mut blank = CreateTargetParams::new("about:blank");
        blank.browser_context_id = context;
        let page = self.browser.new_page(blank).await.map_err(browser_error)?;
        if let Some((width, height)) = viewport {
            page.execute(SetDeviceMetricsOverrideParams::new(
                width, height, 1., false,
            ))
            .await
            .map_err(browser_error)?;
        }
        let main_frame = page.mainframe().await.map_err(browser_error)?;
        let mut responses = page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(browser_error)?;
        let documents = Arc::new(Mutex::new((None, None)));
        let seen = documents.clone();
        let check_documents = self.check_documents;
        let solve = options.solve;
        let document_watch = tokio::spawn(async move {
            while let Some(event) = responses.next().await {
                if event.r#type != ResourceType::Document {
                    continue;
                }
                let Ok(mut seen) = seen.lock() else {
                    break;
                };
                if check_documents && seen.1.is_none() {
                    seen.1 = private_address(&event.response);
                }
                if (solve || seen.0.is_none())
                    && (main_frame.is_none() || event.frame_id == main_frame)
                {
                    seen.0 = Some(event.response.status);
                }
            }
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
        let block = options.block;
        let check_here = self.guard.is_none();
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
                if block.blocks(&event.resource_type)
                    || (check_here && !is_public(request_url).await)
                {
                    let _ = guard_page
                        .execute(FailRequestParams::new(id, ErrorReason::BlockedByClient))
                        .await;
                    continue;
                }
                let mut resume = ContinueRequestParams::new(id);
                let untouched = solve && event.resource_type == ResourceType::Document;
                if !extra.is_empty()
                    && !untouched
                    && Url::parse(request_url).is_ok_and(|u| u.origin() == origin)
                {
                    resume.headers = Some(merged_headers(event.request.headers.inner(), &extra));
                }
                let _ = guard_page.execute(resume).await;
            }
        });
        let popups = match actions {
            [] => None,
            _ => Some(
                crate::actions::Popups::watch(&self.browser, page.target_id().clone())
                    .await
                    .map_err(browser_error)?,
            ),
        };
        let limit = options
            .timeout
            .saturating_mul(if actions.is_empty() { 1 } else { 2 });
        let deadline = Instant::now() + options.timeout * 3 / 4;
        let solved = async |page: &chromiumoxide::Page| {
            if !options.solve {
                return Ok(());
            }
            solve::solve(page, deadline)
                .await
                .map_err(|e| FetchError::Build(format!("rendering {url}: {e}")))
        };
        let outcome = tokio::time::timeout(limit, async {
            page.goto(url.as_str()).await.map_err(browser_error)?;
            solved(&page).await?;
            tokio::select! {
                _ = page.wait_for_network_almost_idle_with_timeout(options.timeout / 3) => {}
                _ = text_settled(&page, settled_words) => {}
            }
            solved(&page).await?;
            if let Some(selector) = &options.wait_for {
                page.wait_for_selector(selector.as_str(), Some(options.timeout))
                    .await
                    .map_err(browser_error)?;
            }
            let scripts = crate::actions::run(&page, actions, options.timeout).await?;
            let value = read(&page).await.map_err(browser_error)?;
            let final_url = page
                .url()
                .await
                .map_err(browser_error)?
                .unwrap_or_else(|| url.clone());
            if !actions.is_empty() {
                if final_url.starts_with("chrome-error:") {
                    return Err(FetchError::Build(
                        "the actions navigated to a page the browser refused or could not load"
                            .into(),
                    ));
                }
                if final_url.starts_with("http") {
                    crate::url_security::validate_public_http_url(&final_url)
                        .await
                        .map_err(|e| {
                            FetchError::Build(format!("the actions led to {final_url}: {e}"))
                        })?;
                }
            }
            if options.solve {
                self.keep_clearance(&page, &final_url).await;
            }
            Ok::<_, FetchError>((value, final_url, scripts))
        })
        .await;
        guard.abort();
        document_watch.abort();
        let (status, private) = documents.lock().map_or((None, None), |seen| *seen);
        let popped = match popups {
            Some(popups) => popups.close(&self.browser).await,
            None => Ok(()),
        };
        let _ = page.close().await;

        if let Some(address) = private {
            return Err(FetchError::Build(format!(
                "rendering {url} loaded a document from the private address {address}; \
                 a system proxy or a host that resolves differently in the browser can cause this"
            )));
        }
        popped?;
        let (value, final_url, scripts) =
            outcome.map_err(|_| FetchError::Build(format!("rendering {url} timed out")))??;
        Ok(Visited {
            value,
            status: status
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(200),
            url: final_url,
            scripts,
        })
    }

    /// The cookies for `url` that renders with [`RenderOptions::solve`] kept
    /// once they got past a Cloudflare challenge, as a `Cookie` header value,
    /// and the User-Agent the browser sent. Cloudflare accepts them from
    /// another client only with that User-Agent and from the same IP address.
    pub fn clearance(&self, url: &str) -> Option<(String, String)> {
        let host = host(url)?;
        let jar = self.clearances.lock().ok()?;
        let cookies: Vec<String> = jar
            .iter()
            .filter(|(domain, _, _)| cookie_domain_matches(domain, &host))
            .map(|(_, name, value)| format!("{name}={value}"))
            .collect();
        cookies
            .iter()
            .any(|c| c.starts_with("cf_clearance="))
            .then(|| (cookies.join("; "), self.agent.clone()))
    }

    async fn keep_clearance(&self, page: &chromiumoxide::Page, url: &str) {
        let params = GetCookiesParams {
            urls: Some(vec![url.to_string()]),
        };
        let Ok(cookies) = page.execute(params).await else {
            return;
        };
        let cookies = cookies.result.cookies;
        if !cookies.iter().any(|c| c.name == "cf_clearance") {
            return;
        }
        if let Ok(mut jar) = self.clearances.lock() {
            for cookie in cookies {
                jar.retain(|(domain, name, _)| *domain != cookie.domain || *name != cookie.name);
                jar.push((cookie.domain, cookie.name, cookie.value));
            }
        }
    }

    /// Shut down a browser this renderer launched and wait for it to exit.
    /// A browser reached through [`Renderer::connect`] keeps running.
    pub async fn close(mut self) {
        if let Some(guard) = self.guard.take() {
            guard.abort();
            let _ = guard.await;
        }
        if let Some(profile) = self.profile.take() {
            let _ = self.browser.close().await;
            if let Some(browser) = Arc::get_mut(&mut self.browser) {
                let _ = browser.wait().await;
            }
            let _ = std::fs::remove_dir_all(profile);
        }
        self.events.abort();
    }
}

/// Where a [`RenderPool`] gets its browser.
#[derive(Debug, Clone)]
pub enum BrowserSource {
    /// Launch a local headless Chrome; see [`Renderer::launch`]. With
    /// `solve`, launch it with [`Renderer::launch_as_chrome`], which
    /// [`RenderOptions::solve`] needs.
    Launch { proxy: Option<String>, solve: bool },
    /// Use a browser already running at this DevTools endpoint; see [`Renderer::connect`].
    Connect(String),
}

impl BrowserSource {
    /// Launch a local Chrome behind `config.proxy`, or behind one proxy
    /// picked at random from `config.proxy_pool`, ready to get past
    /// Cloudflare challenges when `solve` is set.
    pub fn launch_for(config: &crate::FetchConfig, solve: bool) -> Self {
        let proxy = config
            .proxy
            .clone()
            .or_else(|| config.proxy_pool.choose(&mut rand::thread_rng()).cloned());
        Self::Launch { proxy, solve }
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
    /// Resource types whose requests fail while rendering; see [`Block`].
    /// Screenshots load everything.
    pub block: Block,
    /// Render a page that comes back as a Cloudflare challenge and get past
    /// it; see [`RenderOptions::solve`]. Later requests to that host over
    /// HTTP then send the cookies the browser got, with its User-Agent.
    pub solve: bool,
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
        block: Block,
        solve: bool,
    ) -> Result<FetchResult, FetchError> {
        Ok(self
            .render_actions(url, prefetched, wait_for, headers, block, solve, &[])
            .await?
            .0)
    }

    /// Render `url` running `actions`; see [`Renderer::render_actions`].
    #[allow(clippy::too_many_arguments)]
    pub async fn render_actions(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        block: Block,
        solve: bool,
        actions: &[Action],
    ) -> Result<(FetchResult, Vec<serde_json::Value>), FetchError> {
        let _tab = self
            .tabs
            .acquire()
            .await
            .map_err(|_| FetchError::Build("browser closed".into()))?;
        let renderer = self.renderer().await?;
        renderer
            .render_actions(
                url,
                prefetched,
                &RenderOptions {
                    solve,
                    ..self.options(wait_for, headers, block)
                },
                actions,
            )
            .await
    }

    /// [`Renderer::clearance`] for `url`, once the browser has started.
    pub async fn clearance(&self, url: &str) -> Option<(String, String)> {
        match self.browser.read().await.as_ref() {
            Some(Ok(renderer)) => renderer.clearance(url),
            _ => None,
        }
    }

    /// Screenshot `url`; see [`Renderer::screenshot`].
    pub async fn screenshot(
        &self,
        url: &str,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        shot: ScreenshotOptions,
    ) -> Result<Screenshot, FetchError> {
        Ok(self
            .screenshot_actions(url, wait_for, headers, shot, &[])
            .await?
            .0)
    }

    /// Screenshot `url` after `actions`; see [`Renderer::screenshot_actions`].
    pub async fn screenshot_actions(
        &self,
        url: &str,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        shot: ScreenshotOptions,
        actions: &[Action],
    ) -> Result<(Screenshot, Vec<serde_json::Value>), FetchError> {
        let _tab = self
            .tabs
            .acquire()
            .await
            .map_err(|_| FetchError::Build("browser closed".into()))?;
        let renderer = self.renderer().await?;
        renderer
            .screenshot_actions(
                url,
                &self.options(wait_for, headers, Block::default()),
                shot,
                actions,
            )
            .await
    }

    fn options(
        &self,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        block: Block,
    ) -> RenderOptions {
        RenderOptions {
            wait_for: wait_for.map(str::to_string),
            timeout: self.timeout,
            headers: headers.to_vec(),
            block,
            solve: false,
        }
    }

    async fn renderer(&self) -> Result<RwLockReadGuard<'_, Renderer>, FetchError> {
        if self.browser.read().await.is_none() {
            let mut slot = self.browser.write().await;
            if slot.is_none() {
                let commands = self
                    .timeout
                    .saturating_add(COMMAND_TIMEOUT)
                    .min(Duration::from_secs(86_400));
                let started = match &self.source {
                    BrowserSource::Launch {
                        proxy,
                        solve: false,
                    } => Renderer::launch_timed(proxy.as_deref(), &[], commands).await,
                    BrowserSource::Launch { proxy, solve: true } => {
                        Renderer::launch_as_chrome_timed(proxy.as_deref(), commands).await
                    }
                    BrowserSource::Connect(endpoint) => {
                        Renderer::connect_timed(endpoint, commands).await
                    }
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
    /// not requested twice. With `config.solve`, a Cloudflare challenge page
    /// is rendered too, from a fresh request. `http_words` is the word count
    /// extracted from it over HTTP. A failed render is logged and gives `None`. Only renders
    /// that block nothing count toward the per-host verdict, since a blocked
    /// render can come back with less text than the site really has.
    pub async fn render_if_needed(
        &self,
        fetched: &FetchResult,
        http_words: usize,
        config: &RenderConfig,
        headers: &[(String, String)],
    ) -> Option<FetchResult> {
        let challenged = config.solve && is_challenge(fetched);
        if !challenged && !self.wants(fetched, http_words, config.always) {
            return None;
        }
        let prefetched = Prefetched {
            html: &fetched.html,
            headers: &fetched.headers,
        };
        let rendered = self
            .render(
                &fetched.url,
                (!challenged).then_some(prefetched),
                config.wait_for.as_deref(),
                headers,
                config.block,
                config.solve,
            )
            .await;
        match rendered {
            Ok(rendered) => {
                let words = webclaw_core::extract(&rendered.html, Some(&rendered.url))
                    .map_or(0, |e| e.metadata.word_count);
                if config.block == Block::default() {
                    self.record(&fetched.url, http_words, words);
                }
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

/// Whether `fetched` is a Cloudflare challenge page, which
/// [`RenderConfig::solve`] renders whatever its status.
pub fn is_challenge(fetched: &FetchResult) -> bool {
    fetched.document_bytes.is_none()
        && crate::cloud::is_cloudflare_challenge(&fetched.html, &fetched.headers)
}

async fn keep_public(
    browser: std::sync::Weak<Browser>,
    paused: impl futures_util::Stream<Item = Arc<EventRequestPaused>>,
) {
    let browser = &browser;
    paused
        .for_each_concurrent(None, |event| async move {
            let Some(browser) = browser.upgrade() else {
                return;
            };
            let id = event.request_id.clone();
            if is_public(event.request.url.as_str()).await {
                let _ = browser.execute(ContinueRequestParams::new(id)).await;
            } else {
                let _ = browser
                    .execute(FailRequestParams::new(id, ErrorReason::BlockedByClient))
                    .await;
            }
        })
        .await;
}

fn private_address(
    response: &chromiumoxide::cdp::browser_protocol::network::Response,
) -> Option<std::net::IpAddr> {
    response
        .remote_ip_address
        .as_deref()
        .and_then(|ip| ip.trim_matches(['[', ']']).parse().ok())
        .filter(|ip| crate::url_security::is_blocked_ip(*ip))
}

async fn is_public(url: &str) -> bool {
    !url.starts_with("http")
        || crate::url_security::validate_public_http_url(url)
            .await
            .is_ok()
}

fn cookie_domain_matches(domain: &str, host: &str) -> bool {
    let (domain, host) = (domain.to_ascii_lowercase(), host.to_ascii_lowercase());
    let bare = domain.trim_start_matches('.');
    host == bare || (domain.starts_with('.') && host.ends_with(&format!(".{bare}")))
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

    async fn loopback_listener() -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let reached = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = reached.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buffer = [0; 1024];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n > 0 {
                        let request = String::from_utf8_lossy(&buffer[..n]);
                        log.lock()
                            .unwrap()
                            .push(request.lines().next().unwrap_or_default().to_string());
                        let body = "<html><body><p>loopback secret</p></body></html>";
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = socket.write_all(response.as_bytes()).await;
                    }
                });
            }
        });
        (address, reached)
    }

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn a_connected_browser_checks_its_renders_and_leaves_other_tabs_alone() {
        let (loopback, reached) = loopback_listener().await;
        let profile =
            std::env::temp_dir().join(format!("webclaw-connect-test-{}", std::process::id()));
        let chrome = std::env::var("WEBCLAW_CHROME")
            .ok()
            .or_else(|| {
                super::INSTALL_PATHS
                    .iter()
                    .find(|path| std::path::Path::new(path).exists())
                    .map(|path| path.to_string())
            })
            .expect("a Chrome install");
        let mut chrome = std::process::Command::new(chrome)
            .args([
                "--headless=new",
                "--remote-debugging-port=0",
                "--no-first-run",
            ])
            .arg(format!("--user-data-dir={}", profile.display()))
            .arg("about:blank")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("Chrome starts");
        let mut port = None;
        for _ in 0..100 {
            port = std::fs::read_to_string(profile.join("DevToolsActivePort"))
                .ok()
                .and_then(|text| text.lines().next().map(str::to_string))
                .filter(|line| !line.is_empty());
            if port.is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let renderer = Renderer::connect(&format!(
            "http://127.0.0.1:{}",
            port.expect("DevTools port")
        ))
        .await
        .expect("connects");
        let html = format!(
            r#"<html><body><p id="msg">rendered</p><img src="{loopback}/render-img"><script>document.getElementById("msg").textContent = "scripts ran";</script></body></html>"#
        );
        let headers = http::HeaderMap::new();
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
        let tab = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            renderer
                .browser
                .new_page(format!("{loopback}/user-tab").as_str()),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let reached = reached.lock().unwrap().clone();
        renderer.close().await;
        let _ = chrome.kill();
        let _ = chrome.wait();
        let _ = std::fs::remove_dir_all(&profile);
        assert!(
            rendered
                .expect("render succeeds")
                .html
                .contains("scripts ran")
        );
        assert!(tab.is_ok(), "the other tab stalled");
        assert!(
            reached.iter().any(|r| r.contains("/user-tab")),
            "the other tab was blocked: {reached:?}"
        );
        assert!(
            !reached.iter().any(|r| r.contains("/render-img")),
            "the render reached loopback: {reached:?}"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_document_that_resolves_to_loopback_fails_the_render() {
        let (loopback, reached) = loopback_listener().await;
        let port = loopback.rsplit(':').next().unwrap().to_string();
        let rules = format!("--host-resolver-rules=MAP example.net 127.0.0.1:{port}");
        let renderer = Renderer::launch_with(None, &[rules.as_str()])
            .await
            .expect("Chrome launches");
        let html = r#"<html><body><div id="msg">rendered</div><script>location.href = "http://example.net:81/rebound";</script></body></html>"#;
        let headers = http::HeaderMap::new();
        let rendered = renderer
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    wait_for: Some("p".into()),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        let reached = reached.lock().unwrap().clone();
        assert!(
            reached.iter().any(|r| r.contains("/rebound")),
            "the navigation never happened: {reached:?}"
        );
        match rendered {
            Ok(page) => assert!(
                !page.html.contains("loopback secret"),
                "the render returned the loopback page"
            ),
            Err(e) => assert!(e.to_string().contains("private address 127.0.0.1"), "{e}"),
        }
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn an_action_that_navigates_to_a_rebound_name_fails_the_render() {
        let (loopback, reached) = loopback_listener().await;
        let port = loopback.rsplit(':').next().unwrap().to_string();
        let rules = format!("--host-resolver-rules=MAP example.net 127.0.0.1:{port}");
        let renderer = Renderer::launch_with(None, &[rules.as_str()])
            .await
            .expect("Chrome launches");
        let html =
            r#"<html><body><a id="go" href="http://example.net:81/rebound">go</a></body></html>"#;
        let headers = http::HeaderMap::new();
        let rendered = renderer
            .render_actions(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
                &super::parse_actions(r##"[{"type": "click", "selector": "#go"}]"##).unwrap(),
            )
            .await;
        renderer.close().await;
        let reached = reached.lock().unwrap().clone();
        assert!(
            reached.iter().any(|r| r.contains("/rebound")),
            "the click never navigated: {reached:?}"
        );
        let error = rendered.expect_err("the render must fail").to_string();
        assert!(error.contains("private address 127.0.0.1"), "{error}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn windows_opened_by_actions_are_closed() {
        let (loopback, reached) = loopback_listener().await;
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let pages = async || {
            renderer
                .browser
                .execute(chromiumoxide::cdp::browser_protocol::target::GetTargetsParams::default())
                .await
                .map(|r| {
                    r.result
                        .target_infos
                        .iter()
                        .filter(|t| t.r#type == "page")
                        .count()
                })
                .unwrap_or(0)
        };
        let before = pages().await;
        let html = format!(
            r#"<html><body><a id="tab" target="_blank" href="https://example.org/">tab</a><button id="open" onclick="window.open('https://example.org/x')">open</button><a id="private" target="_blank" href="{loopback}/popup">private</a></body></html>"#
        );
        let headers = http::HeaderMap::new();
        let options = RenderOptions {
            timeout: std::time::Duration::from_secs(20),
            ..Default::default()
        };
        let render = async |steps: &str| {
            renderer
                .render_actions(
                    "https://example.com/",
                    Some(Prefetched {
                        html: &html,
                        headers: &headers,
                    }),
                    &options,
                    &super::parse_actions(steps).unwrap(),
                )
                .await
        };
        let public = render(
            r##"[{"type": "click", "selector": "#tab"}, {"type": "click", "selector": "#open"}, {"type": "wait", "milliseconds": 1500}]"##,
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let after = pages().await;
        let private = render(
            r##"[{"type": "click", "selector": "#private"}, {"type": "wait", "milliseconds": 1500}]"##,
        )
        .await;
        renderer.close().await;
        assert!(public.is_ok(), "{:?}", public.err());
        assert_eq!(after, before, "popups were left open");
        if let Err(e) = private {
            assert!(e.to_string().contains("private"), "{e}");
        }
        let reached = reached.lock().unwrap().clone();
        assert!(reached.is_empty(), "a popup reached loopback: {reached:?}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn a_dropped_renderer_leaves_no_chrome_running() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let profile = renderer.profile.clone().expect("a launched profile");
        let running = || {
            let ps = std::process::Command::new("/bin/ps")
                .args(["-axo", "command="])
                .output()
                .expect("ps runs");
            String::from_utf8_lossy(&ps.stdout)
                .lines()
                .filter(|line| line.contains(profile.to_str().unwrap()))
                .count()
        };
        assert!(running() > 0, "no Chrome found for {}", profile.display());
        drop(renderer);
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        let left = running();
        let _ = std::fs::remove_dir_all(&profile);
        assert_eq!(left, 0, "Chrome processes still running after drop");
    }

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn webrtc_sends_no_udp_to_loopback() {
        let udp = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let port = udp.local_addr().unwrap().port();
        let packets = Arc::new(AtomicUsize::new(0));
        let counted = packets.clone();
        tokio::spawn(async move {
            let mut buffer = [0; 2048];
            while udp.recv_from(&mut buffer).await.is_ok() {
                counted.fetch_add(1, Ordering::SeqCst);
            }
        });
        let html = format!(
            r#"<html><body><p id="msg">rendered</p><script>const pc = new RTCPeerConnection({{iceServers: [{{urls: "stun:127.0.0.1:{port}"}}]}}); pc.createDataChannel("x"); pc.createOffer().then(o => pc.setLocalDescription(o)).then(() => {{ document.getElementById("msg").textContent = "offer made"; }}); setTimeout(() => {{ const done = document.createElement("div"); done.id = "done"; document.body.appendChild(done); }}, 3000);</script></body></html>"#
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
                    wait_for: Some("#done".into()),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        assert!(
            rendered
                .expect("render succeeds")
                .html
                .contains("offer made")
        );
        assert_eq!(
            packets.load(Ordering::SeqCst),
            0,
            "STUN packets reached loopback"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn a_system_proxy_does_not_take_over_a_render_without_one() {
        let (system_proxy, reached) = loopback_listener().await;
        let flag = format!("--proxy-server={system_proxy}");
        let renderer = Renderer::launch_with(None, &[flag.as_str()])
            .await
            .expect("Chrome launches");
        let shot = renderer
            .screenshot(
                "https://example.com/",
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
                super::ScreenshotOptions::default(),
            )
            .await;
        renderer.close().await;
        let reached = reached.lock().unwrap().clone();
        assert!(
            reached.is_empty(),
            "the render used the system proxy: {reached:?}"
        );
        shot.expect("screenshot succeeds");
    }

    const CROSS_SITE_FRAME: &str = r#"<html><body><p>framed</p><img src="LOOP/frame-img"><img src="NAMED/frame-named"><img src="http://example.org/redirect"><iframe src="LOOP/frame-nested"></iframe><iframe src="http://example.org/nav"></iframe><script>fetch("LOOP/frame-fetch").catch(() => {}); const xhr = new XMLHttpRequest(); xhr.open("GET", "LOOP/frame-xhr"); xhr.send(); new Worker("/worker.js"); new SharedWorker("/shared.js"); new WebSocket("WSLOOP/frame-ws");</script></body></html>"#;

    fn cross_site_fixture(path: &str, loopback: &str) -> (&'static str, &'static str, String) {
        let ws = loopback.replace("http://", "ws://");
        let named = loopback.replace("127.0.0.1", "localhost");
        let fill = |body: &str| {
            body.replace("WSLOOP", &ws)
                .replace("NAMED", &named)
                .replace("LOOP", loopback)
        };
        match path {
            "/page" => (
                "200 OK",
                "text/html",
                fill(
                    r#"<html><body><p id="msg">rendered</p><iframe src="http://example.org/frame"></iframe><script type="speculationrules">{"prefetch": [{"source": "list", "urls": ["LOOP/spec-prefetch"]}], "prerender": [{"source": "list", "urls": ["LOOP/spec-prerender"]}]}</script><script>new WebSocket("WSLOOP/main-ws"); document.getElementById("msg").textContent = "scripts ran";</script></body></html>"#,
                ),
            ),
            "/frame" => ("200 OK", "text/html", fill(CROSS_SITE_FRAME)),
            "/nav" => (
                "200 OK",
                "text/html",
                fill(r#"<script>location.href = "LOOP/frame-nav";</script>"#),
            ),
            "/worker.js" => (
                "200 OK",
                "text/javascript",
                fill(r#"fetch("LOOP/worker-fetch").catch(() => {});"#),
            ),
            "/shared.js" => (
                "200 OK",
                "text/javascript",
                fill(r#"fetch("LOOP/shared-fetch").catch(() => {});"#),
            ),
            "/redirect" => ("302 Found", "text/plain", String::new()),
            _ => ("404 Not Found", "text/plain", String::new()),
        }
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn no_frame_or_worker_of_a_rendered_page_reaches_loopback() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (loopback, reached) = loopback_listener().await;
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_url = format!("http://localhost:{}", proxy.local_addr().unwrap().port());
        let served = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let served_log = served.clone();
        let fixture_loopback = loopback.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = proxy.accept().await {
                let log = served_log.clone();
                let loopback = fixture_loopback.clone();
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let target = request.split_whitespace().nth(1).unwrap_or_default();
                    let path = url::Url::parse(target)
                        .map(|u| u.path().to_string())
                        .unwrap_or_default();
                    log.lock().unwrap().push(target.to_string());
                    let (status, kind, body) = cross_site_fixture(&path, &loopback);
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nLocation: {loopback}/redirected\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        let renderer = Renderer::launch(Some(&proxy_url))
            .await
            .expect("Chrome launches");
        let options = RenderOptions {
            timeout: std::time::Duration::from_secs(20),
            ..Default::default()
        };
        let (_, _, page) = cross_site_fixture("/page", &loopback);
        let headers = http::HeaderMap::new();
        let rendered = renderer
            .render(
                "http://example.com/page",
                Some(Prefetched {
                    html: &page,
                    headers: &headers,
                }),
                &options,
            )
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let after_render = std::mem::take(&mut *reached.lock().unwrap());
        let shot = renderer
            .screenshot(
                "http://example.com/page",
                &options,
                super::ScreenshotOptions::default(),
            )
            .await;
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        renderer.close().await;
        let after_shot = std::mem::take(&mut *reached.lock().unwrap());
        let served = served.lock().unwrap().clone();
        assert!(
            rendered
                .expect("render succeeds")
                .html
                .contains("scripts ran")
        );
        shot.expect("screenshot succeeds");
        for loaded in ["/frame", "/worker.js", "/shared.js", "/nav", "/redirect"] {
            assert!(
                served.iter().any(|t| t.ends_with(loaded)),
                "{loaded} never loaded: {served:?}"
            );
        }
        assert!(
            after_render.is_empty(),
            "a render reached loopback: {after_render:?}"
        );
        assert!(
            after_shot.is_empty(),
            "a screenshot reached loopback: {after_shot:?}"
        );
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

    #[tokio::test]
    #[ignore = "needs a local Chrome and network access"]
    async fn screenshots_capture_the_viewport_or_the_whole_page() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let options = RenderOptions {
            timeout: std::time::Duration::from_secs(20),
            ..Default::default()
        };
        let viewport = renderer
            .screenshot(
                "https://quotes.toscrape.com/js/",
                &options,
                super::ScreenshotOptions::default(),
            )
            .await;
        let full = renderer
            .screenshot(
                "https://quotes.toscrape.com/js/",
                &options,
                super::ScreenshotOptions {
                    full_page: true,
                    jpeg: false,
                },
            )
            .await;
        let jpeg = renderer
            .screenshot(
                "https://quotes.toscrape.com/js/",
                &options,
                super::ScreenshotOptions {
                    full_page: false,
                    jpeg: true,
                },
            )
            .await;
        renderer.close().await;
        let viewport = viewport.expect("viewport screenshot").image;
        assert_eq!(&viewport[..8], b"\x89PNG\r\n\x1a\n");
        assert_eq!(png_size(&viewport), (1280, 720));
        let (width, height) = png_size(&full.expect("full-page screenshot").image);
        assert_eq!(width, 1280);
        assert!(height > 1000, "full page is only {height}px tall");
        assert_eq!(&jpeg.expect("jpeg screenshot").image[..3], b"\xff\xd8\xff");
    }

    fn png_size(png: &[u8]) -> (u32, u32) {
        let be = |at: usize| u32::from_be_bytes(png[at..at + 4].try_into().unwrap());
        (be(16), be(20))
    }

    #[test]
    fn block_lists_parse_into_resource_types() {
        use chromiumoxide::cdp::browser_protocol::network::ResourceType;
        let block: super::Block = " Images, fonts,media ".parse().unwrap();
        assert!(block.blocks(&ResourceType::Image));
        assert!(block.blocks(&ResourceType::Font));
        assert!(block.blocks(&ResourceType::Media));
        assert!(block.blocks(&ResourceType::TextTrack));
        assert!(!block.blocks(&ResourceType::Stylesheet));
        for kind in [
            ResourceType::Document,
            ResourceType::Script,
            ResourceType::Xhr,
            ResourceType::Fetch,
        ] {
            assert!(!block.blocks(&kind));
        }
        assert!("stylesheets".parse::<super::Block>().is_err());
        assert_eq!("none".parse::<super::Block>(), Ok(super::Block::default()));
        assert_eq!("".parse::<super::Block>(), Ok(super::Block::default()));
        assert!("scripts".parse::<super::Block>().is_err());
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn blocked_resources_never_reach_the_network() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy = format!("http://{}", listener.local_addr().unwrap());
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let log = log.clone();
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                    if let Some(target) = request.split_whitespace().nth(1) {
                        log.lock().unwrap().push(target.to_string());
                    }
                    let _ = socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\nCache-Control: no-store\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                });
            }
        });
        let html = r#"<html><head><link rel="stylesheet" href="/style.css"><style>@font-face { font-family: F; src: url(/font.woff2); } p { font-family: F; }</style></head><body><p id="msg">rendered</p><img src="/pic.png"><video src="/clip.mp4" preload="auto"></video><script>document.getElementById("msg").textContent = "scripts ran";</script></body></html>"#;
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(Some(&proxy))
            .await
            .expect("Chrome launches");
        let mut results = Vec::new();
        for block in ["images,fonts,media", "none"] {
            let rendered = renderer
                .render(
                    "http://example.com/page",
                    Some(Prefetched {
                        html,
                        headers: &headers,
                    }),
                    &RenderOptions {
                        timeout: std::time::Duration::from_secs(20),
                        block: block.parse().unwrap(),
                        ..Default::default()
                    },
                )
                .await;
            results.push((rendered, std::mem::take(&mut *seen.lock().unwrap())));
        }
        renderer.close().await;
        let paths = ["/font.woff2", "/pic.png", "/clip.mp4"];
        let reached = |log: &[String], path: &str| log.iter().any(|t| t.ends_with(path));
        assert!(reached(&results[0].1, "/style.css"), "{:?}", results[0].1);
        let (blocked, blocked_log) = &results[0];
        assert!(
            blocked
                .as_ref()
                .expect("blocked render")
                .html
                .contains("scripts ran")
        );
        for path in paths {
            assert!(
                !reached(blocked_log, path),
                "{path} loaded: {blocked_log:?}"
            );
        }
        let (open, open_log) = &results[1];
        assert!(
            open.as_ref()
                .expect("open render")
                .html
                .contains("scripts ran")
        );
        for path in paths {
            assert!(reached(open_log, path), "{path} not loaded: {open_log:?}");
        }
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
            super::BrowserSource::Launch {
                proxy: None,
                solve: false,
            },
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

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn blocked_renders_leave_the_host_verdict_alone() {
        let config = super::RenderConfig {
            pool: Arc::new(super::RenderPool::new(
                super::BrowserSource::Launch {
                    proxy: None,
                    solve: false,
                },
                std::time::Duration::from_secs(20),
            )),
            always: false,
            wait_for: None,
            block: "images".parse().unwrap(),
            solve: false,
        };
        let shell = page(
            "https://example.com/",
            r#"<html><body><div id="root"></div><script>1</script></body></html>"#,
            200,
        );
        let pool = &config.pool;
        for _ in 0..3 {
            assert!(
                pool.render_if_needed(&shell, 0, &config, &[])
                    .await
                    .is_some()
            );
        }
        pool.close().await;
        assert!(pool.wants(&shell, 0, false));
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
            super::BrowserSource::launch_for(&config, true),
            super::BrowserSource::Launch { proxy: Some(p), solve: true } if p == "http://pool.example:8080"
        ));
    }

    #[test]
    fn proxy_credentials_are_percent_decoded() {
        assert_eq!(super::percent_decode("p%40ss%3Aword"), "p@ss:word");
        assert_eq!(super::percent_decode("plain"), "plain");
    }

    #[test]
    fn solving_renders_challenge_pages_whatever_their_status() {
        let challenge = r#"<html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt = {cType: 'managed'};</script></body></html>"#;
        assert!(super::is_challenge(&page(
            "https://a.test/",
            challenge,
            403
        )));
        let mut mitigated = page("https://a.test/", "<html></html>", 403);
        mitigated
            .headers
            .insert("cf-mitigated", "challenge".parse().unwrap());
        assert!(super::is_challenge(&mitigated));
        assert!(!super::is_challenge(&page(
            "https://a.test/",
            "<html><body><p>Forbidden</p></body></html>",
            403
        )));
    }

    #[test]
    fn clearance_cookies_go_only_to_hosts_their_domain_covers() {
        use super::cookie_domain_matches;
        assert!(cookie_domain_matches(".example.com", "example.com"));
        assert!(cookie_domain_matches(".example.com", "www.example.com"));
        assert!(cookie_domain_matches("example.com", "EXAMPLE.com"));
        assert!(!cookie_domain_matches("example.com", "www.example.com"));
        assert!(!cookie_domain_matches(".example.com", "badexample.com"));
        assert!(!cookie_domain_matches(".example.com", "example.org"));
    }

    const CHALLENGE: &str = r#"<html><head><title>Just a moment...</title></head><body><script>window._cf_chl_opt = {cType: 'managed'};</script><div style="margin: 100px 0 0 40px"><iframe src="http://example.net:8080/cdn-cgi/challenge-platform/fixture" width="300" height="65" style="border: 0"></iframe></div><script>addEventListener("message", (e) => { if (e.data === "passed") { document.cookie = "cf_clearance=fixture; path=/"; location.reload(); } });</script></body></html>"#;

    const CHECKBOX: &str = r#"<html><body style="margin: 0"><input type="checkbox" id="box" style="position: absolute; left: 16px; top: 16px; width: 32px; height: 32px; margin: 0"><script>document.getElementById("box").addEventListener("click", (e) => { if (e.isTrusted) parent.postMessage("passed", "*"); });</script></body></html>"#;

    async fn challenge_proxy() -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://localhost:{}", proxy.local_addr().unwrap().port());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = proxy.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 8192];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let target = request.split_whitespace().nth(1).unwrap_or_default();
                    let passed = request.lines().any(|l| {
                        l.to_ascii_lowercase().starts_with("cookie:")
                            && l.contains("cf_clearance=fixture")
                    });
                    let (status, body) = if target.contains("challenge-platform") {
                        ("200 OK", CHECKBOX.to_string())
                    } else if target.contains("example.net") {
                        ("404 Not Found", String::new())
                    } else if passed {
                        (
                            "200 OK",
                            "<html><body><p>the real page</p></body></html>".to_string(),
                        )
                    } else {
                        ("403 Forbidden", CHALLENGE.to_string())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\nCache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        url
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_render_clicks_through_a_challenge_and_keeps_its_cookies() {
        let proxy = challenge_proxy().await;
        let renderer = Renderer::launch_as_chrome(Some(&proxy))
            .await
            .expect("Chrome launches");
        assert!(
            !renderer.agent.contains("HeadlessChrome"),
            "{}",
            renderer.agent
        );
        let solved = renderer
            .render(
                "http://example.com:8080/page",
                None,
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(30),
                    solve: true,
                    ..Default::default()
                },
            )
            .await;
        let unsolved = renderer
            .render(
                "http://example.org:8080/page",
                None,
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
            )
            .await;
        let clearance = renderer.clearance("http://example.com:8080/other");
        let elsewhere = renderer.clearance("http://example.org:8080/other");
        renderer.close().await;
        let solved = solved.expect("render succeeds");
        assert!(solved.html.contains("the real page"), "{}", solved.html);
        assert_eq!(solved.status, 200);
        let (cookies, agent) = clearance.expect("the clearance is kept");
        assert_eq!(cookies, "cf_clearance=fixture");
        assert!(agent.contains("Chrome/") && !agent.contains("Headless"));
        assert!(elsewhere.is_none());
        let unsolved = unsolved.expect("a render without solve still returns the page");
        assert_eq!(unsolved.status, 403);
        assert!(unsolved.html.contains("Just a moment"));
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_challenge_that_never_clears_fails_the_render_in_time() {
        let proxy = challenge_proxy().await;
        let renderer = Renderer::launch_as_chrome(Some(&proxy))
            .await
            .expect("Chrome launches");
        let started = std::time::Instant::now();
        let rendered = renderer
            .render(
                "http://example.com:8080/page",
                Some(Prefetched {
                    html: &CHALLENGE.replace("challenge-platform/fixture", "gone"),
                    headers: &http::HeaderMap::new(),
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(5),
                    solve: true,
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        let error = rendered
            .expect_err("the challenge never clears")
            .to_string();
        assert!(error.contains("challenge was still shown"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(8));
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn real_cloudflare_challenges_get_solved() {
        let renderer = Renderer::launch_as_chrome(None)
            .await
            .expect("Chrome launches");
        let mut failed = Vec::new();
        for (url, marker) in [
            (
                "https://www.scrapingcourse.com/cloudflare-challenge",
                "You bypassed the Cloudflare challenge",
            ),
            ("https://www.bscscan.com/", "BNB Smart Chain"),
            ("https://gitlab.com/users/sign_in", "Remember me"),
        ] {
            let rendered = renderer
                .render(
                    url,
                    None,
                    &RenderOptions {
                        timeout: std::time::Duration::from_secs(45),
                        solve: true,
                        ..Default::default()
                    },
                )
                .await;
            if !rendered.as_ref().is_ok_and(|r| r.html.contains(marker)) {
                failed.push((url, rendered.map(|r| r.status)));
            }
        }
        renderer.close().await;
        assert!(failed.is_empty(), "{failed:?}");
    }
}
