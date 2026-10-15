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
use chromiumoxide::cdp::browser_protocol::io::{CloseParams, ReadParams};
use chromiumoxide::cdp::browser_protocol::network::{
    CookieParam, ErrorReason, EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent,
    EventResponseReceived, GetCookiesParams, GetResponseBodyParams, ResourceType, SetCookieParams,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, PrintToPdfParams, PrintToPdfTransferMode,
};
use chromiumoxide::cdp::browser_protocol::target::{
    CreateBrowserContextParams, CreateTargetParams,
};
use chromiumoxide::page::ScreenshotParams;
use futures_util::{FutureExt, StreamExt};
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
    /// cookies for the page's URL instead, except on a persistent profile,
    /// where it is ignored.
    pub headers: Vec<(String, String)>,
    /// Resource types whose requests fail; see [`Block`] for what it misses.
    pub block: Block,
    /// Get past a Cloudflare challenge page: wait for one that clears on its
    /// own, click the checkbox of one that asks for it, then read the page it
    /// leads to. The render fails when the challenge is still shown after
    /// three quarters of `timeout`, leaving the rest for the page it leads
    /// to. A Turnstile widget embedded in any page rendered this way is
    /// clicked as well, and the page is read as it is when the widget shows
    /// no token within 10 seconds.
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

/// A response a rendered page received, as [`Renderer::capture`] keeps it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct NetworkResponse {
    /// The response's URL, after redirects.
    pub url: String,
    pub method: String,
    pub status: u16,
    /// The browser's resource type in lowercase, such as `xhr`, `fetch` or `script`.
    pub resource_type: String,
    pub mime_type: String,
    /// The headers the page set on the request. Cookies are added by the
    /// browser afterwards and are never here; headers that carry credentials
    /// are left out unless the capture asked for them.
    pub request_headers: std::collections::BTreeMap<String, String>,
    /// The body of a text response (JSON, HTML, JavaScript, XML, plain text),
    /// cut to the capture's size limits. `None` for binary responses and for
    /// bodies the browser no longer holds.
    pub body: Option<String>,
    /// Whether the size limits cut `body` short.
    pub truncated: bool,
}

/// Which of a rendered page's network responses to keep.
///
/// At most 100 responses are kept. Each body is cut to 1 MB and all bodies
/// together to 5 MB, unless [`NetworkCapture::limit_bytes`] sets other limits.
#[derive(Debug, Clone)]
pub struct NetworkCapture {
    pattern: Option<regex::Regex>,
    types: Vec<String>,
    credentials: bool,
    max_body: usize,
    max_total: usize,
}

const RESOURCE_TYPES: &[&str] = &[
    "xhr",
    "fetch",
    "document",
    "script",
    "stylesheet",
    "image",
    "font",
    "media",
    "texttrack",
    "eventsource",
    "websocket",
    "manifest",
    "prefetch",
    "preflight",
    "ping",
    "signedexchange",
    "cspviolationreport",
    "fedcm",
    "other",
];
const MAX_RESPONSES: usize = 100;
const QUIET: Duration = Duration::from_secs(1);
const MAX_BODY_BYTES: usize = 1_000_000;
const MAX_TOTAL_BYTES: usize = 5_000_000;

impl NetworkCapture {
    /// Keep the responses whose URL matches the regular expression `pattern`
    /// (every URL when it is `None` or empty) and whose resource type is in
    /// the comma list `types` (`xhr,fetch` when `None`, `all` for every type).
    /// Without `credentials`, request headers whose name looks like a
    /// credential, such as `Authorization` or `X-Api-Key`, are left out and
    /// URL query parameters named that way read `REDACTED`.
    pub fn new(
        pattern: Option<&str>,
        types: Option<&str>,
        credentials: bool,
    ) -> Result<Self, String> {
        let pattern = pattern
            .filter(|p| !p.is_empty())
            .map(regex::Regex::new)
            .transpose()
            .map_err(|e| format!("invalid capture pattern: {e}"))?;
        let mut types: Vec<String> = types
            .unwrap_or("xhr,fetch")
            .split(',')
            .map(|t| t.trim().to_ascii_lowercase())
            .filter(|t| !t.is_empty())
            .collect();
        if types.is_empty() {
            return Err(format!(
                "no resource types to capture: use all or a comma list of {}",
                RESOURCE_TYPES.join(", ")
            ));
        }
        if let Some(unknown) = types
            .iter()
            .find(|t| *t != "all" && !RESOURCE_TYPES.contains(&t.as_str()))
        {
            return Err(format!(
                "unknown resource type \"{unknown}\": use all or a comma list of {}",
                RESOURCE_TYPES.join(", ")
            ));
        }
        if types.iter().any(|t| t == "all") {
            types.clear();
        }
        Ok(Self {
            pattern,
            types,
            credentials,
            max_body: MAX_BODY_BYTES,
            max_total: MAX_TOTAL_BYTES,
        })
    }

    /// Cut each body to `body` bytes and all bodies together to `total`
    /// bytes, for output that has to fit in a smaller space, such as a
    /// model's context.
    pub fn limit_bytes(mut self, body: usize, total: usize) -> Self {
        self.max_body = body;
        self.max_total = total;
        self
    }

    fn cut(&self, bodies: Vec<Option<String>>) -> Vec<(Option<String>, bool)> {
        let mut budget = self.max_total;
        bodies
            .into_iter()
            .map(|body| {
                let limit = self.max_body.min(budget);
                let truncated = body.as_ref().is_some_and(|text| text.len() > limit);
                let body = body.map(|mut text| {
                    text.truncate(char_floor(&text, limit));
                    text
                });
                budget -= body.as_ref().map_or(0, String::len);
                (body, truncated)
            })
            .collect()
    }

    fn wants(&self, kind: &ResourceType, url: &str) -> bool {
        (self.types.is_empty()
            || self
                .types
                .iter()
                .any(|t| t.eq_ignore_ascii_case(kind.as_ref())))
            && self.pattern.as_ref().is_none_or(|p| p.is_match(url))
    }

    fn keeps(&self, event: &EventResponseReceived, check_addresses: bool) -> bool {
        self.wants(&event.r#type, &event.response.url)
            && !(check_addresses && private_address(&event.response).is_some())
    }

    async fn collect(
        &self,
        page: &chromiumoxide::Page,
        mut sent: impl futures_util::Stream<Item = Arc<EventRequestWillBeSent>> + Unpin,
        mut received: impl futures_util::Stream<Item = Arc<EventResponseReceived>> + Unpin,
        check_addresses: bool,
    ) -> Vec<NetworkResponse> {
        let (requests, kept) = tokio::task::unconstrained(async {
            let mut requests = HashMap::new();
            while let Some(Some(event)) = sent.next().now_or_never() {
                requests.insert(event.request_id.clone(), event);
            }
            let mut kept = Vec::new();
            while let Some(Some(event)) = received.next().now_or_never() {
                if kept.len() < MAX_RESPONSES && self.keeps(&event, check_addresses) {
                    kept.push(event);
                }
            }
            (requests, kept)
        })
        .await;
        let bodies =
            futures_util::future::join_all(kept.iter().map(|event| response_body(page, event)))
                .await;
        kept.iter()
            .zip(self.cut(bodies))
            .map(|(event, (body, truncated))| {
                let request = requests.get(&event.request_id).map(|sent| &sent.request);
                NetworkResponse {
                    url: if self.credentials {
                        event.response.url.clone()
                    } else {
                        redact_query(&event.response.url)
                    },
                    method: request.map(|r| r.method.clone()).unwrap_or_default(),
                    status: u16::try_from(event.response.status).unwrap_or_default(),
                    resource_type: event.r#type.as_ref().to_ascii_lowercase(),
                    mime_type: event.response.mime_type.clone(),
                    request_headers: request
                        .map(|r| self.headers(r.headers.inner()))
                        .unwrap_or_default(),
                    body,
                    truncated,
                }
            })
            .collect()
    }

    fn headers(&self, sent: &serde_json::Value) -> std::collections::BTreeMap<String, String> {
        sent.as_object()
            .into_iter()
            .flatten()
            .filter(|(name, _)| self.credentials || !is_credential(name))
            .filter_map(|(name, value)| {
                let value = value.as_str()?;
                let value = if !self.credentials && name.eq_ignore_ascii_case("referer") {
                    redact_query(value)
                } else {
                    value.to_string()
                };
                Some((name.clone(), value))
            })
            .collect()
    }
}

async fn response_body(
    page: &chromiumoxide::Page,
    event: &EventResponseReceived,
) -> Option<String> {
    let mime = event.response.mime_type.to_ascii_lowercase();
    if ["image/", "audio/", "video/", "font/"]
        .iter()
        .any(|kind| mime.starts_with(kind))
    {
        return None;
    }
    let body = page
        .execute(GetResponseBodyParams::new(event.request_id.clone()))
        .await
        .ok()?
        .result;
    (!body.base64_encoded).then_some(body.body)
}

async fn network_quiet(
    capture: &NetworkCapture,
    mut sent: impl futures_util::Stream<Item = Arc<EventRequestWillBeSent>> + Unpin,
    finished: impl futures_util::Stream<Item = Arc<EventLoadingFinished>> + Unpin,
    failed: impl futures_util::Stream<Item = Arc<EventLoadingFailed>> + Unpin,
) {
    let mut done = futures_util::stream::select(
        finished.map(|event| event.request_id.clone()),
        failed.map(|event| event.request_id.clone()),
    );
    let mut in_flight = std::collections::HashSet::new();
    let quiet = tokio::time::sleep(QUIET);
    tokio::pin!(quiet);
    loop {
        tokio::select! {
            biased;
            Some(event) = sent.next() => {
                let kept = event
                    .r#type
                    .as_ref()
                    .is_some_and(|kind| capture.wants(kind, &event.request.url));
                if kept && in_flight.insert(event.request_id.clone()) {
                    quiet.as_mut().reset(tokio::time::Instant::now() + QUIET);
                }
            }
            Some(id) = done.next() => {
                if in_flight.remove(&id) {
                    quiet.as_mut().reset(tokio::time::Instant::now() + QUIET);
                }
            }
            _ = &mut quiet, if in_flight.is_empty() => return,
            else => return,
        }
    }
}

fn is_credential(name: &str) -> bool {
    const WORDS: &[&str] = &[
        "access",
        "apikey",
        "appcheck",
        "auth",
        "authorization",
        "bearer",
        "cookie",
        "credential",
        "credentials",
        "csrf",
        "jwt",
        "key",
        "passwd",
        "password",
        "secret",
        "session",
        "sessionid",
        "sig",
        "signature",
        "token",
        "xsrf",
    ];
    let name = name.to_ascii_lowercase();
    if name == "x-client-data" {
        return true;
    }
    if name.starts_with("access-control-") {
        return false;
    }
    name.split(['-', '_', '.']).any(|word| {
        WORDS.contains(&word)
            || ["token", "secret", "password", "apikey"]
                .iter()
                .any(|end| word.ends_with(end))
    })
}

fn redact_query(url: &str) -> String {
    let (rest, fragment) = match url.split_once('#') {
        Some((rest, fragment)) => (rest, Some(fragment)),
        None => (url, None),
    };
    let (base, query) = match rest.split_once('?') {
        Some((base, query)) => (base, Some(query)),
        None => (rest, None),
    };
    let mut out = base.to_string();
    if let Some(query) = query {
        out.push('?');
        out.push_str(&redact_pairs(query));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(&redact_pairs(fragment));
    }
    out
}

fn redact_pairs(pairs: &str) -> String {
    pairs
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some((name, _)) if is_credential(&percent_decode(name)) => format!("{name}=REDACTED"),
            _ => pair.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

fn char_floor(text: &str, at: usize) -> usize {
    (0..=at.min(text.len()))
        .rev()
        .find(|&i| text.is_char_boundary(i))
        .unwrap_or(0)
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

/// A render that kept its network responses; see [`Renderer::capture`].
#[derive(Debug, Clone)]
pub struct Captured {
    pub page: FetchResult,
    /// The values of the `executeJavascript` steps, the hosted API's `js_results`.
    pub js_results: Vec<serde_json::Value>,
    /// The kept responses, in the order they arrived.
    pub network: Vec<NetworkResponse>,
}

/// Paper size of a PDF.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Paper {
    /// US Letter, 8.5x11 inches.
    #[default]
    Letter,
    /// A4, 210x297 mm.
    A4,
}

/// How a page is printed to PDF. Start from `PdfOptions::default()` and
/// set the fields you need.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct PdfOptions {
    /// Paper size, US Letter by default.
    pub paper: Paper,
    /// Turn the paper sideways.
    pub landscape: bool,
    /// Print background colors and images, which browsers leave out by default.
    pub print_background: bool,
    /// Zoom of the page content, from 0.1 to 2.
    pub scale: f64,
}

impl PdfOptions {
    fn check(&self) -> Result<(), FetchError> {
        if (0.1..=2.0).contains(&self.scale) {
            Ok(())
        } else {
            Err(FetchError::Build(format!(
                "PDF scale must be between 0.1 and 2, got {}",
                self.scale
            )))
        }
    }
}

impl Default for PdfOptions {
    fn default() -> Self {
        Self {
            paper: Paper::default(),
            landscape: false,
            print_background: false,
            scale: 1.0,
        }
    }
}

/// A rendered page printed to PDF. The file itself went to the writer the
/// print was given.
#[derive(Debug, Clone)]
pub struct Pdf {
    /// Size of the PDF in bytes.
    pub bytes: u64,
    /// HTTP status of the page's document.
    pub status: u16,
    /// The page's URL after redirects.
    pub url: String,
}

const SCREENSHOT_VIEWPORT: (i64, i64) = (1280, 720);
const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

type Stopped = Arc<Mutex<Option<String>>>;

struct Visited<T> {
    value: T,
    status: u16,
    url: String,
    scripts: Vec<serde_json::Value>,
    network: Vec<NetworkResponse>,
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
    persistent: bool,
    proxy_login: Option<(String, String)>,
    check_documents: bool,
    agent: String,
    clearances: Arc<Mutex<Jar>>,
}

#[derive(Debug, Default)]
struct Jar {
    agent: String,
    cookies: Vec<Clearance>,
}

impl Jar {
    fn clearance(&self, url: &str) -> Option<(String, String)> {
        let url = Url::parse(url).ok()?;
        let now = unix_now();
        let cookies: Vec<String> = self
            .cookies
            .iter()
            .filter(|c| c.applies_to(&url, now))
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        (!cookies.is_empty()).then(|| (cookies.join("; "), self.agent.clone()))
    }
}

const CLEARANCE_COOKIES: &[&str] = &["cf_clearance"];

#[derive(Debug, Clone, PartialEq)]
struct Clearance {
    domain: String,
    path: String,
    secure: bool,
    expires: Option<f64>,
    name: String,
    value: String,
}

impl Clearance {
    fn applies_to(&self, url: &Url, now: f64) -> bool {
        let Some(host) = url.host_str() else {
            return false;
        };
        let path = url.path();
        cookie_domain_matches(&self.domain, host)
            && (path == self.path
                || (path.starts_with(&self.path)
                    && (self.path.ends_with('/') || path[self.path.len()..].starts_with('/'))))
            && (!self.secure || url.scheme() == "https")
            && self.expires.is_none_or(|at| at > now)
    }
}

fn unix_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
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
        Self::launch_as_chrome_timed(proxy, None, COMMAND_TIMEOUT).await
    }

    async fn launch_as_chrome_timed(
        proxy: Option<&str>,
        profile: Option<&std::path::Path>,
        command_timeout: Duration,
    ) -> Result<Self, FetchError> {
        let launch = async |args: &[&str]| match profile {
            Some(dir) => {
                Self::launch_in(proxy, args, dir.to_path_buf(), true, command_timeout).await
            }
            None => Self::launch_timed(proxy, args, command_timeout).await,
        };
        let renderer = launch(&[]).await?;
        if renderer.agent.is_empty() {
            renderer.close().await;
            return Err(FetchError::Build(
                "the browser did not tell its User-Agent, so it cannot present itself as Chrome"
                    .into(),
            ));
        }
        if !renderer.agent.contains("HeadlessChrome") {
            return Ok(renderer);
        }
        let agent = format!(
            "--user-agent={}",
            renderer.agent.replace("HeadlessChrome", "Chrome")
        );
        renderer.close().await;
        launch(&[agent.as_str()]).await
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
        Self::launch_in(proxy, args, profile, false, command_timeout).await
    }

    /// Launch a local headless Chrome on the profile in `dir`, so cookies,
    /// session cookies included, local storage and IndexedDB carry over to
    /// later launches. Every render shares the profile's cookies. A `Cookie`
    /// header in [`RenderOptions::headers`] is neither sent nor stored there;
    /// put cookies in the profile with [`Renderer::save_cookies`]. One
    /// browser can use a profile at a time: a second launch on it fails
    /// while the first runs.
    ///
    /// `dir` is created when missing, readable by its owner only on Unix,
    /// and marked as made by webclaw. A directory that is neither empty nor
    /// marked, such as the profile of a Chrome you browse with, is refused,
    /// as are a symbolic link and, on Unix, a directory other users can
    /// write to. Tabs left open by an earlier launch and the service workers
    /// sites registered are dropped, and the profile's preferences turn off
    /// prefetching. Otherwise as [`Renderer::launch`].
    pub async fn launch_with_profile(
        proxy: Option<&str>,
        dir: &std::path::Path,
    ) -> Result<Self, FetchError> {
        Self::launch_in(proxy, &[], dir.to_path_buf(), true, COMMAND_TIMEOUT).await
    }

    async fn launch_in(
        proxy: Option<&str>,
        args: &[&str],
        profile: std::path::PathBuf,
        persistent: bool,
        command_timeout: Duration,
    ) -> Result<Self, FetchError> {
        prepare_profile(&profile, persistent).map_err(|e| {
            FetchError::Build(format!("browser profile {}: {e}", profile.display()))
        })?;
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
        if persistent {
            config = config
                .arg("--restore-last-session")
                .arg("--profile-directory=Default");
        }
        let config = config.build().map_err(browser_error)?;
        let (browser, handler) = match Browser::launch(config).await {
            Ok(launched) => launched,
            Err(e) => return Err(launch_error(&profile, persistent, e.to_string())),
        };
        let mut renderer = Self::start(
            browser,
            handler,
            Some(profile),
            proxy_login,
            true,
            persistent,
        )
        .await?;
        renderer.check_documents = proxy.is_none();
        if persistent && let Some(dir) = &renderer.profile {
            OPEN_PROFILES
                .lock()
                .map(|mut open| open.push((Arc::downgrade(&renderer.browser), dir.clone())))
                .ok();
        }
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
        Self::start(browser, handler, None, None, false, false).await
    }

    async fn start(
        mut browser: Browser,
        mut handler: chromiumoxide::Handler,
        profile: Option<std::path::PathBuf>,
        proxy_login: Option<(String, String)>,
        browser_wide: bool,
        persistent: bool,
    ) -> Result<Self, FetchError> {
        let events = tokio::spawn(async move {
            while let Some(event) = handler.next().await {
                if event.is_err() {
                    break;
                }
            }
        });
        let ready = async {
            if !persistent {
                browser.start_incognito_context().await?;
            }
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
            persistent,
            proxy_login,
            check_documents: false,
            agent,
            clearances: Arc::default(),
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
    /// cookies, apart from the browser's default profile. On a profile from
    /// [`Renderer::launch_with_profile`], every render uses the profile.
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
        let captured = self
            .rendered(url, prefetched, options, actions, None)
            .await?;
        Ok((captured.page, captured.js_results))
    }

    /// [`Renderer::render_actions`], also keeping the network responses
    /// `capture` asks for. Returns the page, the `js_results` and the
    /// responses in the order they arrived. A capturing render waits until
    /// no request is in flight and none has started for 1 second, up to a
    /// third of `options.timeout`, instead of stopping once the text settles.
    /// Requests from dedicated workers are not seen either. A failed render
    /// returns only the error. Only requests the browser was
    /// allowed to make get a response. The requests a cross-site frame makes
    /// itself are not seen, since Chrome runs it in its own process.
    pub async fn capture(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        actions: &[Action],
        capture: &NetworkCapture,
    ) -> Result<Captured, FetchError> {
        self.rendered(url, prefetched, options, actions, Some(capture))
            .await
    }

    async fn rendered(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        actions: &[Action],
        capture: Option<&NetworkCapture>,
    ) -> Result<Captured, FetchError> {
        let start = Instant::now();
        let visited = self
            .visit(
                url,
                prefetched,
                options,
                actions,
                capture,
                None,
                None,
                async |page| page.content().await,
            )
            .await?;
        let page = FetchResult {
            html: visited.value,
            document_bytes: None,
            status: visited.status,
            url: visited.url,
            headers: http::HeaderMap::new(),
            elapsed: start.elapsed(),
        };
        Ok(Captured {
            page,
            js_results: visited.scripts,
            network: visited.network,
        })
    }

    async fn render_unfollowed(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
    ) -> Result<FetchResult, FetchError> {
        let start = Instant::now();
        let stop: Stopped = Arc::default();
        let visited = self
            .visit(
                url,
                prefetched,
                options,
                &[],
                None,
                None,
                Some(stop.clone()),
                async |page| page.content().await,
            )
            .await;
        let location = stop.lock().ok().and_then(|stop| stop.clone());
        if let Some(location) = location {
            let mut headers = http::HeaderMap::new();
            if let Ok(value) = http::HeaderValue::from_str(&location) {
                headers.insert(http::header::LOCATION, value);
            }
            return Ok(FetchResult {
                html: String::new(),
                document_bytes: None,
                status: 302,
                url: url.to_string(),
                headers,
                elapsed: start.elapsed(),
            });
        }
        let visited = visited?;
        Ok(FetchResult {
            html: visited.value,
            document_bytes: None,
            status: visited.status,
            url: visited.url,
            headers: http::HeaderMap::new(),
            elapsed: start.elapsed(),
        })
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
                None,
                Some(SCREENSHOT_VIEWPORT),
                None,
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

    /// Render `url` like [`Renderer::screenshot`] and print it to PDF with
    /// the page's print styles, every page of it, into `out`. Chrome hands
    /// the file over in chunks, so a writer that fails, say past a size
    /// limit, stops the copy there and fails the print.
    pub async fn pdf(
        &self,
        url: &str,
        options: &RenderOptions,
        pdf: PdfOptions,
        out: &mut (dyn std::io::Write + Send),
    ) -> Result<Pdf, FetchError> {
        Ok(self.pdf_actions(url, options, pdf, &[], out).await?.0)
    }

    /// [`Renderer::pdf`] after `actions`, as [`Renderer::render_actions`] runs them.
    pub async fn pdf_actions(
        &self,
        url: &str,
        options: &RenderOptions,
        pdf: PdfOptions,
        actions: &[Action],
        out: &mut (dyn std::io::Write + Send),
    ) -> Result<(Pdf, Vec<serde_json::Value>), FetchError> {
        self.print(url, None, options, pdf, actions, out).await
    }

    async fn print(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        pdf: PdfOptions,
        actions: &[Action],
        out: &mut (dyn std::io::Write + Send),
    ) -> Result<(Pdf, Vec<serde_json::Value>), FetchError> {
        pdf.check()?;
        let (width, height) = match pdf.paper {
            Paper::Letter => (8.5, 11.0),
            Paper::A4 => (210.0 / 25.4, 297.0 / 25.4),
        };
        let params = PrintToPdfParams::builder()
            .paper_width(width)
            .paper_height(height)
            .landscape(pdf.landscape)
            .print_background(pdf.print_background)
            .scale(pdf.scale)
            .transfer_mode(PrintToPdfTransferMode::ReturnAsStream)
            .generate_tagged_pdf(false)
            .build();
        let visited = self
            .visit(
                url,
                prefetched,
                &RenderOptions {
                    block: Block::default(),
                    ..options.clone()
                },
                actions,
                None,
                Some(SCREENSHOT_VIEWPORT),
                None,
                async move |page| Ok(copy_pdf(page, params, out).await),
            )
            .await
            .map_err(|e| match e {
                FetchError::Build(message) if message.ends_with(" timed out") => {
                    FetchError::Build(format!(
                        "{message}: the render timeout covers loading the page, its actions and printing it"
                    ))
                }
                e => e,
            })?;
        let printed = Pdf {
            bytes: visited.value.map_err(FetchError::Build)?,
            status: visited.status,
            url: visited.url,
        };
        Ok((printed, visited.scripts))
    }

    #[allow(clippy::too_many_arguments)]
    async fn visit<T>(
        &self,
        url: &str,
        prefetched: Option<Prefetched<'_>>,
        options: &RenderOptions,
        actions: &[Action],
        capture: Option<&NetworkCapture>,
        viewport: Option<(i64, i64)>,
        stop: Option<Stopped>,
        read: impl AsyncFnOnce(&chromiumoxide::Page) -> Result<T, chromiumoxide::error::CdpError>,
    ) -> Result<Visited<T>, FetchError> {
        let private = !self.persistent
            && options
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("cookie"));
        if !private {
            return self
                .visit_in(
                    None, url, prefetched, options, actions, capture, viewport, stop, read,
                )
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
                capture,
                viewport,
                stop,
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
        capture: Option<&NetworkCapture>,
        viewport: Option<(i64, i64)>,
        stop: Option<Stopped>,
        read: impl AsyncFnOnce(&chromiumoxide::Page) -> Result<T, chromiumoxide::error::CdpError>,
    ) -> Result<Visited<T>, FetchError> {
        let target = crate::url_security::validate_public_http_url(url).await?;
        let url = target.to_string();
        let private = context.is_some();
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
        let navigating = main_frame.clone();
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
                if self.persistent {
                    continue;
                }
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
        let traffic = match capture {
            Some(_) => Some((
                page.event_listener::<EventRequestWillBeSent>()
                    .await
                    .map_err(browser_error)?,
                page.event_listener::<EventResponseReceived>()
                    .await
                    .map_err(browser_error)?,
            )),
            None => None,
        };
        let mut quiet = match capture {
            Some(_) => Some((
                page.event_listener::<EventRequestWillBeSent>()
                    .await
                    .map_err(browser_error)?,
                page.event_listener::<EventLoadingFinished>()
                    .await
                    .map_err(browser_error)?,
                page.event_listener::<EventLoadingFailed>()
                    .await
                    .map_err(browser_error)?,
            )),
            None => None,
        };
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
                if let Some(stop) = &stop
                    && event.resource_type == ResourceType::Document
                    && navigating.as_ref() == Some(&event.frame_id)
                    && !same_document(request_url, &target)
                {
                    if let Ok(mut stop) = stop.lock() {
                        stop.get_or_insert_with(|| request_url.to_string());
                    }
                    let _ = guard_page
                        .execute(FailRequestParams::new(id, ErrorReason::BlockedByClient))
                        .await;
                    continue;
                }
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
        let deadline = Instant::now()
            + (options.timeout - options.timeout / 4).min(Duration::from_secs(86_400));
        let mut solver = options.solve.then(|| solve::Solver::new(deadline));
        let mut solved = async |page: &chromiumoxide::Page| match solver.as_mut() {
            Some(solver) => solver
                .run(page)
                .await
                .map_err(|e| FetchError::Build(format!("rendering {url}: {e}"))),
            None => Ok(()),
        };
        let outcome = tokio::time::timeout(limit, async {
            page.goto(url.as_str()).await.map_err(browser_error)?;
            solved(&page).await?;
            match capture.zip(quiet.take()) {
                Some((capture, (sent, finished, failed))) => {
                    let _ = tokio::time::timeout(
                        options.timeout / 3,
                        network_quiet(capture, sent, finished, failed),
                    )
                    .await;
                }
                None => tokio::select! {
                    _ = page.wait_for_network_almost_idle_with_timeout(options.timeout / 3) => {}
                    _ = text_settled(&page, settled_words) => {}
                },
            }
            solved(&page).await?;
            if let Some(selector) = &options.wait_for {
                page.wait_for_selector(selector.as_str(), Some(options.timeout))
                    .await
                    .map_err(browser_error)?;
            }
            let scripts = crate::actions::run(&page, actions, options.timeout).await?;
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
            if options.solve && !private {
                self.keep_clearance(&page, &final_url).await;
            }
            let value = read(&page).await.map_err(browser_error)?;
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
        let network = match (capture, traffic, &outcome) {
            (Some(capture), Some((sent, received)), Ok(Ok(_))) => tokio::time::timeout(
                options.timeout,
                capture.collect(&page, sent, received, self.check_documents),
            )
            .await
            .ok(),
            _ => Some(Vec::new()),
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
        let network = network.ok_or_else(|| {
            FetchError::Build(format!("reading the captured responses of {url} timed out"))
        })?;
        Ok(Visited {
            value,
            status: status
                .and_then(|code| u16::try_from(code).ok())
                .unwrap_or(200),
            url: final_url,
            scripts,
            network,
        })
    }

    /// Store the cookies of `cookie`, a `Cookie` header value such as
    /// `"a=1; b=2"`, in the browser for `url`. On a profile from
    /// [`Renderer::launch_with_profile`] they are kept for later launches.
    pub async fn save_cookies(&self, url: &str, cookie: &str) -> Result<(), FetchError> {
        let cookies = cookie
            .split(';')
            .filter_map(|pair| {
                let (name, value) = pair.trim().split_once('=')?;
                let mut param = CookieParam::new(name.trim(), value.trim());
                param.url = Some(url.to_string());
                Some(param)
            })
            .collect();
        self.browser
            .set_cookies(cookies)
            .await
            .map(|_| ())
            .map_err(browser_error)
    }

    /// The Cloudflare clearance cookie for `url` that a render with
    /// [`RenderOptions::solve`] got from getting past a challenge, as a
    /// `Cookie` header value, and the User-Agent the browser sent.
    /// Cloudflare accepts it from another client only with that User-Agent
    /// and from the same IP address. Only `cf_clearance` is kept, under its
    /// domain, path, `Secure` flag and expiry, and never from a render that
    /// sent its own `Cookie` header.
    pub fn clearance(&self, url: &str) -> Option<(String, String)> {
        self.clearances.lock().ok()?.clearance(url)
    }

    async fn keep_clearance(&self, page: &chromiumoxide::Page, url: &str) {
        let params = GetCookiesParams {
            urls: Some(vec![url.to_string()]),
        };
        let Ok(cookies) = page.execute(params).await else {
            return;
        };
        let now = unix_now();
        let Ok(mut jar) = self.clearances.lock() else {
            return;
        };
        jar.agent.clone_from(&self.agent);
        jar.cookies.retain(|c| c.expires.is_none_or(|at| at > now));
        for cookie in cookies.result.cookies {
            if !CLEARANCE_COOKIES.contains(&cookie.name.as_str()) {
                continue;
            }
            jar.cookies.retain(|c| {
                c.domain != cookie.domain || c.path != cookie.path || c.name != cookie.name
            });
            jar.cookies.push(Clearance {
                domain: cookie.domain,
                path: cookie.path,
                secure: cookie.secure,
                expires: (!cookie.session && cookie.expires > 0.0).then_some(cookie.expires),
                name: cookie.name,
                value: cookie.value,
            });
        }
    }

    /// Shut down a browser this renderer launched and wait for it to exit,
    /// which also writes a persistent profile's cookies and storage to disk.
    /// A browser reached through [`Renderer::connect`] keeps running.
    pub async fn close(mut self) {
        if let Some(guard) = self.guard.take() {
            guard.abort();
            let _ = guard.await;
        }
        if let Some(profile) = self.profile.take() {
            if let Ok(mut open) = OPEN_PROFILES.lock() {
                open.retain(|(browser, _)| browser.as_ptr() != Arc::as_ptr(&self.browser));
            }
            let _ = self.browser.close().await;
            if let Some(browser) = Arc::get_mut(&mut self.browser) {
                let _ = browser.wait().await;
            }
            if !self.persistent {
                let _ = std::fs::remove_dir_all(profile);
            }
        }
        self.events.abort();
    }
}

type OpenProfile = (std::sync::Weak<Browser>, std::path::PathBuf);

static OPEN_PROFILES: Mutex<Vec<OpenProfile>> = Mutex::new(Vec::new());

/// On Unix, when the process gets SIGTERM, shut down the browsers it
/// launched on persistent profiles and exit with status 143, so a stopped
/// process leaves no Chrome holding its profile locked. Call it once, from
/// inside a Tokio runtime.
#[cfg(unix)]
pub fn close_profiles_on_terminate() {
    use tokio::signal::unix::{SignalKind, signal};
    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        return;
    };
    tokio::spawn(async move {
        terminate.recv().await;
        close_profiles().await;
        std::process::exit(143);
    });
}

/// Shut down every browser this process launched on a persistent profile
/// and wait up to five seconds for each to release its profile, so a
/// process that is about to exit leaves no Chrome holding a profile locked.
pub async fn close_profiles() {
    let open = OPEN_PROFILES
        .lock()
        .map(|mut open| std::mem::take(&mut *open))
        .unwrap_or_default();
    for (browser, dir) in open {
        shut_down(browser, &dir).await;
    }
}

async fn shut_down(browser: std::sync::Weak<Browser>, dir: &std::path::Path) {
    let Some(browser) = browser.upgrade() else {
        return;
    };
    let _ = browser.close().await;
    let lock = dir.join("SingletonLock");
    for _ in 0..50 {
        if std::fs::symlink_metadata(&lock).is_err() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Where a [`RenderPool`] gets its browser.
#[derive(Debug, Clone)]
pub enum BrowserSource {
    /// Launch a local headless Chrome; see [`Renderer::launch`]. With
    /// `solve`, launch it with [`Renderer::launch_as_chrome`], which
    /// [`RenderOptions::solve`] needs.
    Launch { proxy: Option<String>, solve: bool },
    /// [`BrowserSource::Launch`] on the persistent profile in `dir`; see
    /// [`Renderer::launch_with_profile`].
    Profile {
        proxy: Option<String>,
        dir: std::path::PathBuf,
        solve: bool,
    },
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

    /// The same launch on the persistent profile in `dir`, when there is
    /// one. A browser to connect to is left as it is.
    pub fn with_profile(self, dir: Option<std::path::PathBuf>) -> Self {
        match (self, dir) {
            (Self::Launch { proxy, solve }, Some(dir)) => Self::Profile { proxy, dir, solve },
            (source, _) => source,
        }
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
    cookies: Mutex<Vec<(String, String)>>,
    clearances: Arc<Mutex<Jar>>,
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
    /// Screenshots and PDFs load everything.
    pub block: Block,
    /// Render a page that comes back as a Cloudflare challenge and get past
    /// it; see [`RenderOptions::solve`]. Later requests to that site over
    /// HTTP then send the clearance cookie the browser got, with its
    /// User-Agent; see [`Renderer::clearance`].
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
            cookies: Mutex::new(Vec::new()),
            clearances: Arc::default(),
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

    /// Render `url` running `actions` and keep the network responses
    /// `capture` asks for; see [`Renderer::capture`].
    #[allow(clippy::too_many_arguments)]
    pub async fn capture(
        &self,
        url: &str,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        block: Block,
        solve: bool,
        actions: &[Action],
        capture: &NetworkCapture,
    ) -> Result<Captured, FetchError> {
        let _tab = self
            .tabs
            .acquire()
            .await
            .map_err(|_| FetchError::Build("browser closed".into()))?;
        let renderer = self.renderer().await?;
        renderer
            .capture(
                url,
                None,
                &RenderOptions {
                    solve,
                    ..self.options(wait_for, headers, block)
                },
                actions,
                capture,
            )
            .await
    }

    /// [`Renderer::clearance`] for `url`, from any browser this pool has
    /// started. It never waits for a browser to launch.
    pub fn clearance(&self, url: &str) -> Option<(String, String)> {
        self.clearances.lock().ok()?.clearance(url)
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

    /// Print `url` to PDF; see [`Renderer::pdf`].
    pub async fn pdf(
        &self,
        url: &str,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        pdf: PdfOptions,
        out: &mut (dyn std::io::Write + Send),
    ) -> Result<Pdf, FetchError> {
        Ok(self
            .pdf_actions(url, wait_for, headers, pdf, &[], out)
            .await?
            .0)
    }

    /// Print `url` to PDF after `actions`; see [`Renderer::pdf_actions`].
    pub async fn pdf_actions(
        &self,
        url: &str,
        wait_for: Option<&str>,
        headers: &[(String, String)],
        pdf: PdfOptions,
        actions: &[Action],
        out: &mut (dyn std::io::Write + Send),
    ) -> Result<(Pdf, Vec<serde_json::Value>), FetchError> {
        pdf.check()?;
        let _tab = self
            .tabs
            .acquire()
            .await
            .map_err(|_| FetchError::Build("browser closed".into()))?;
        let renderer = self.renderer().await?;
        renderer
            .pdf_actions(
                url,
                &self.options(wait_for, headers, Block::default()),
                pdf,
                actions,
                out,
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
                        Renderer::launch_as_chrome_timed(proxy.as_deref(), None, commands).await
                    }
                    BrowserSource::Profile {
                        proxy,
                        dir,
                        solve: false,
                    } => {
                        Renderer::launch_in(proxy.as_deref(), &[], dir.clone(), true, commands)
                            .await
                    }
                    BrowserSource::Profile {
                        proxy,
                        dir,
                        solve: true,
                    } => {
                        Renderer::launch_as_chrome_timed(proxy.as_deref(), Some(dir), commands)
                            .await
                    }
                    BrowserSource::Connect(endpoint) => {
                        Renderer::connect_timed(endpoint, commands).await
                    }
                };
                if let Err(e) = &started {
                    warn!("no browser for rendering: {e}");
                }
                *slot = Some(
                    started
                        .map(|mut renderer| {
                            renderer.clearances = self.clearances.clone();
                            renderer
                        })
                        .map_err(|e| e.to_string()),
                );
            }
        }
        let renderer = RwLockReadGuard::try_map(self.browser.read().await, |slot| {
            slot.as_ref().and_then(|started| started.as_ref().ok())
        })
        .map_err(|slot| {
            FetchError::Build(match slot.as_ref() {
                Some(Err(e)) => e.clone(),
                _ => "browser closed".into(),
            })
        })?;
        let pending = self
            .cookies
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default();
        for (url, cookie) in pending {
            if let Err(e) = renderer.save_cookies(&url, &cookie).await {
                warn!(url, "cookies not saved in the browser profile: {e}");
            }
        }
        Ok(renderer)
    }

    /// Render `fetched`, a Cloudflare challenge page, from a fresh request
    /// and get past the challenge; see [`RenderOptions::solve`]. The error
    /// says why when the challenge is still there.
    pub async fn solve(
        &self,
        fetched: &FetchResult,
        config: &RenderConfig,
        headers: &[(String, String)],
    ) -> Result<FetchResult, FetchError> {
        self.render(
            &fetched.url,
            None,
            config.wait_for.as_deref(),
            headers,
            config.block,
            true,
        )
        .await
    }

    /// Render `fetched` when `config` asks for it and the page seems to need
    /// a browser, handing the browser the HTTP response so the document is
    /// not requested twice. `http_words` is the word count extracted from it
    /// over HTTP. A failed render is logged and gives `None`. Only renders
    /// that block nothing count toward the per-host verdict, since a blocked
    /// render can come back with less text than the site really has.
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
        if self.persistent()
            || fetched.status >= 400
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

    /// On a persistent profile, save the cookies of `cookie`, a `Cookie`
    /// header value, in the profile for `url` before the next render; see
    /// [`Renderer::save_cookies`]. Other pools ignore it.
    pub fn save_cookies(&self, url: &str, cookie: &str) {
        if self.persistent()
            && let Ok(mut pending) = self.cookies.lock()
        {
            pending.push((url.to_string(), cookie.to_string()));
        }
    }

    /// Whether this pool launches Chrome on a persistent profile.
    pub fn persistent(&self) -> bool {
        matches!(self.source, BrowserSource::Profile { .. })
    }

    /// With a persistent profile, load `url` in the browser, which sends the
    /// profile's cookies, when `fetched`, its HTTP response, is an HTML page,
    /// and return the rendered page in its place. Otherwise return `fetched`.
    /// A profile that cannot be opened fails, as [`RenderPool::open`] does;
    /// a page that fails in an opened profile is logged and `fetched` kept.
    ///
    /// Without `follow_redirects`, the browser stops at the page's first
    /// redirect or navigation, which comes back as a 302 with its target in
    /// `Location`, the way an HTTP client that does not follow redirects
    /// answers, so a crawl can check the target before it is loaded.
    pub async fn load_in_profile(
        &self,
        url: &str,
        fetched: FetchResult,
        config: &RenderConfig,
        headers: &[(String, String)],
        follow_redirects: bool,
    ) -> Result<FetchResult, FetchError> {
        let html = fetched.document_bytes.is_none()
            && fetched
                .headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.contains("html"));
        if !self.persistent() || !html {
            return Ok(fetched);
        }
        self.open().await?;
        let rendered = if follow_redirects {
            self.render(
                url,
                None,
                config.wait_for.as_deref(),
                headers,
                config.block,
                config.solve,
            )
            .await
        } else {
            self.render_unfollowed(url, config, headers).await
        };
        Ok(match rendered {
            Ok(rendered) => rendered,
            Err(e) => {
                warn!(
                    url,
                    "rendering in the profile failed, keeping the HTTP result: {e}"
                );
                fetched
            }
        })
    }

    /// Start the browser if it has not started, and save the cookies queued
    /// with [`RenderPool::save_cookies`] in it, so a browser or a profile
    /// that cannot be opened (in use, refused, unreadable) fails here, before
    /// any page is fetched.
    pub async fn open(&self) -> Result<(), FetchError> {
        self.renderer().await.map(|_| ())
    }

    async fn render_unfollowed(
        &self,
        url: &str,
        config: &RenderConfig,
        headers: &[(String, String)],
    ) -> Result<FetchResult, FetchError> {
        let _tab = self
            .tabs
            .acquire()
            .await
            .map_err(|_| FetchError::Build("browser closed".into()))?;
        let renderer = self.renderer().await?;
        let options = RenderOptions {
            solve: config.solve,
            ..self.options(config.wait_for.as_deref(), headers, config.block)
        };
        renderer.render_unfollowed(url, None, &options).await
    }

    /// Shut down the browser if this pool launched it. A later render starts a new one.
    pub async fn close(&self) {
        if let Some(Ok(renderer)) = self.browser.write().await.take() {
            renderer.close().await;
        }
    }
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

fn same_document(request: &str, target: &Url) -> bool {
    let Ok(mut request) = Url::parse(request) else {
        return false;
    };
    let mut target = target.clone();
    request.set_fragment(None);
    target.set_fragment(None);
    request == target
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

/// `bytes` in standard base64, as MCP image and resource content carries them.
pub fn base64(bytes: &[u8]) -> String {
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

async fn copy_pdf(
    page: &chromiumoxide::Page,
    params: PrintToPdfParams,
    out: &mut (dyn std::io::Write + Send),
) -> Result<u64, String> {
    let printed = page.execute(params).await.map_err(|e| match e {
        chromiumoxide::error::CdpError::Chrome(e) if e.message.contains("Printing failed") => {
            "Chrome could not print the page: it navigated away while printing, or it is too large to print".to_string()
        }
        e => e.to_string(),
    })?;
    let handle = printed
        .result
        .stream
        .ok_or("Chrome printed the PDF without a stream")?;
    let mut bytes = 0;
    let copied = loop {
        let mut read = ReadParams::new(handle.clone());
        read.size = Some(1 << 20);
        let chunk = match page.execute(read).await {
            Ok(chunk) => chunk.result,
            Err(e) => break Err(e.to_string()),
        };
        let data = if chunk.base64_encoded == Some(true) {
            match chromiumoxide::utils::base64::decode(&chunk.data) {
                Ok(data) => data,
                Err(e) => break Err(e.to_string()),
            }
        } else {
            chunk.data.into_bytes()
        };
        if let Err(e) = out.write_all(&data) {
            break Err(format!("writing the PDF failed: {e}"));
        }
        bytes += data.len() as u64;
        if chunk.eof {
            break Ok(bytes);
        }
    };
    let _ = page.execute(CloseParams::new(handle)).await;
    copied
}

fn browser_error(e: impl std::fmt::Display) -> FetchError {
    FetchError::Build(format!("browser: {e}"))
}

const PROFILE_MARKER: &str = ".webclaw-profile";

fn prepare_profile(dir: &std::path::Path, persistent: bool) -> Result<(), String> {
    if persistent {
        claim_profile(dir)?;
    }
    let default = dir.join("Default");
    std::fs::create_dir_all(&default).map_err(|e| e.to_string())?;
    let path = default.join("Preferences");
    let existing = match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("cannot read {}: {e}", path.display())),
    };
    let preferences = without_prediction(existing.as_deref())
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let staged = default.join("Preferences.webclaw");
    std::fs::write(&staged, preferences)
        .and_then(|()| std::fs::rename(&staged, &path))
        .map_err(|e| e.to_string())?;
    if persistent {
        let _ = std::fs::remove_dir_all(default.join("Sessions"));
        let _ = std::fs::remove_dir_all(default.join("Service Worker"));
    }
    Ok(())
}

fn claim_profile(dir: &std::path::Path) -> Result<(), String> {
    let marker = dir.join(PROFILE_MARKER);
    let found = match std::fs::symlink_metadata(dir) {
        Ok(found) => found,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(dir)
                    .map_err(|e| e.to_string())?;
            }
            #[cfg(not(unix))]
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            return std::fs::write(&marker, b"").map_err(|e| e.to_string());
        }
        Err(e) => return Err(e.to_string()),
    };
    if found.file_type().is_symlink() {
        return Err("it is a symbolic link; give the directory itself".into());
    }
    if !found.is_dir() {
        return Err("it is not a directory".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if found.permissions().mode() & 0o022 != 0 {
            return Err("other users can write to it; make it private with chmod 700".into());
        }
    }
    if marker.exists() {
        return Ok(());
    }
    let empty = std::fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .next()
        .is_none();
    if !empty {
        return Err(
            "it is not empty and webclaw did not make it; give webclaw a new or empty directory, \
             not the profile of a Chrome you browse with"
                .into(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    std::fs::write(&marker, b"").map_err(|e| e.to_string())
}

fn without_prediction(existing: Option<&[u8]>) -> Result<Vec<u8>, String> {
    let mut preferences = match existing {
        Some(bytes) => serde_json::from_slice(bytes).map_err(|e| e.to_string())?,
        None => serde_json::json!({}),
    };
    let net = preferences
        .as_object_mut()
        .ok_or("not a JSON object")?
        .entry("net")
        .or_insert_with(|| serde_json::json!({}));
    net.as_object_mut()
        .ok_or("\"net\" is not a JSON object")?
        .insert("network_prediction_options".into(), 2.into());
    serde_json::to_vec(&preferences).map_err(|e| e.to_string())
}

fn launch_error(profile: &std::path::Path, persistent: bool, error: String) -> FetchError {
    if !persistent {
        let _ = std::fs::remove_dir_all(profile);
    }
    if !persistent || !error.contains("ProcessSingleton") {
        return browser_error(error);
    }
    let owner = std::fs::read_link(profile.join("SingletonLock"))
        .map(|owner| format!(" (its lock names {})", owner.display()))
        .unwrap_or_default();
    FetchError::Build(format!(
        "the browser profile {} is in use by another browser{owner}, and a profile takes one browser at a time",
        profile.display()
    ))
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
        let profile = tempfile::tempdir().unwrap();
        for renderer in [
            Renderer::launch(None).await,
            Renderer::launch_with_profile(None, profile.path()).await,
        ] {
            let renderer = renderer.expect("Chrome launches");
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
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_profile_keeps_cookies_and_storage_between_launches() {
        let profile = tempfile::tempdir().unwrap();
        let mut served = http::HeaderMap::new();
        served.insert("set-cookie", "served=1; Path=/".parse().unwrap());
        let writer = r#"<html><body><script>localStorage.setItem("kept", "local"); indexedDB.open("wc", 1).onupgradeneeded = (e) => e.target.result.createObjectStore("s").put("indexed", "k");</script></body></html>"#;
        let reader = r#"<html><body><p id="c"></p><script>const out = document.getElementById("c"); const r = indexedDB.open("wc", 1); r.onupgradeneeded = () => {}; r.onsuccess = () => { const db = r.result; if (!db.objectStoreNames.contains("s")) { out.textContent = document.cookie + "|" + localStorage.getItem("kept") + "|none"; out.id = "done"; return; } db.transaction("s").objectStore("s").get("k").onsuccess = (e) => { out.textContent = document.cookie + "|" + localStorage.getItem("kept") + "|" + e.target.result; out.id = "done"; }; };</script></body></html>"#;
        let options = RenderOptions {
            timeout: std::time::Duration::from_secs(20),
            wait_for: Some("#done".into()),
            ..Default::default()
        };
        let first = Renderer::launch_with_profile(None, profile.path())
            .await
            .expect("Chrome launches");
        first
            .save_cookies("https://example.com/", "imported=2")
            .await
            .expect("cookies are saved");
        let wrote = first
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: writer,
                    headers: &served,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    headers: vec![("Cookie".into(), "header=3".into())],
                    ..Default::default()
                },
            )
            .await;
        first.close().await;
        wrote.expect("first render succeeds");
        let empty = http::HeaderMap::new();
        let second = Renderer::launch_with_profile(None, profile.path())
            .await
            .expect("Chrome launches again on the profile");
        let read = second
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: reader,
                    headers: &empty,
                }),
                &options,
            )
            .await;
        second.close().await;
        let read = read.expect("second render succeeds").html;
        assert!(read.contains("served=1"), "{read}");
        assert!(read.contains("imported=2"), "{read}");
        assert!(!read.contains("header=3"), "{read}");
        assert!(read.contains("|local|indexed"), "{read}");

        let fresh = Renderer::launch(None).await.expect("Chrome launches");
        let unrelated = fresh
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: reader,
                    headers: &empty,
                }),
                &options,
            )
            .await;
        fresh.close().await;
        let unrelated = unrelated.expect("fresh render succeeds").html;
        assert!(unrelated.contains(">|null|none<"), "{unrelated}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_profile_pool_saves_cookies_only_for_the_url_given() {
        let profile = tempfile::tempdir().unwrap();
        let pool = super::RenderPool::new(
            super::BrowserSource::Profile {
                proxy: None,
                dir: profile.path().into(),
                solve: false,
            },
            std::time::Duration::from_secs(20),
        );
        pool.save_cookies("https://example.com/", "given=4");
        let reader = r#"<html><body><p id="c"></p><script>document.getElementById("c").textContent = "cookies:" + document.cookie + ":end";</script></body></html>"#;
        let headers = http::HeaderMap::new();
        let mut seen = Vec::new();
        for url in ["https://example.com/", "https://example.org/"] {
            let page = pool
                .render(
                    url,
                    Some(Prefetched {
                        html: reader,
                        headers: &headers,
                    }),
                    None,
                    &[],
                    crate::Block::default(),
                    false,
                )
                .await;
            seen.push(page.expect("render succeeds").html);
        }
        pool.close().await;
        assert!(seen[0].contains("cookies:given=4:end"), "{}", seen[0]);
        assert!(seen[1].contains("cookies::end"), "{}", seen[1]);
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn cookies_are_saved_when_the_profile_opens() {
        let profile = tempfile::tempdir().unwrap();
        let pool = super::RenderPool::new(
            super::BrowserSource::Profile {
                proxy: None,
                dir: profile.path().into(),
                solve: false,
            },
            std::time::Duration::from_secs(20),
        );
        pool.save_cookies("https://example.com/", "opened=5");
        pool.open().await.expect("the profile opens");
        pool.close().await;
        let reader = r#"<html><body><p id="c"></p><script>document.getElementById("c").textContent = "cookies:" + document.cookie + ":end";</script></body></html>"#;
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch_with_profile(None, profile.path())
            .await
            .expect("Chrome launches");
        let read = renderer
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: reader,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        let read = read.expect("render succeeds").html;
        assert!(read.contains("cookies:opened=5:end"), "{read}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_profile_always_opens_its_default_profile() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("profile");
        super::prepare_profile(&dir, true).unwrap();
        std::fs::write(
            dir.join("Local State"),
            br#"{"profile": {"last_used": "Profile 1"}}"#,
        )
        .unwrap();
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch_with_profile(None, &dir)
            .await
            .expect("Chrome launches");
        let rendered = renderer
            .render(
                "https://example.com/",
                Some(Prefetched {
                    html: "<p>rendered</p>",
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
            )
            .await;
        renderer.close().await;
        rendered.expect("render succeeds");
        assert!(!dir.join("Profile 1").exists(), "Chrome opened Profile 1");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn an_unfollowed_render_stops_at_the_first_navigation() {
        let headers = http::HeaderMap::new();
        let options = RenderOptions {
            timeout: std::time::Duration::from_secs(20),
            ..Default::default()
        };
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let moved = renderer
            .render_unfollowed(
                "https://example.com/start",
                Some(Prefetched {
                    html: r#"<html><body><script>location.hash = "top"; location.href = "https://example.org/elsewhere";</script></body></html>"#,
                    headers: &headers,
                }),
                &options,
            )
            .await;
        let stayed = renderer
            .render_unfollowed(
                "https://example.com/start",
                Some(Prefetched {
                    html: r#"<html><body><p id="here">stayed</p><script>location.hash = "top";</script></body></html>"#,
                    headers: &headers,
                }),
                &options,
            )
            .await;
        renderer.close().await;
        let moved = moved.expect("a stopped navigation is not an error");
        assert_eq!(moved.status, 302);
        assert_eq!(
            moved.headers.get(http::header::LOCATION).unwrap(),
            "https://example.org/elsewhere"
        );
        let stayed = stayed.expect("render succeeds");
        assert_eq!(stayed.status, 200);
        assert!(stayed.html.contains("stayed"), "{}", stayed.html);
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_page_that_fails_in_an_open_profile_keeps_its_http_page() {
        let profile = tempfile::tempdir().unwrap();
        let config = super::RenderConfig {
            pool: Arc::new(super::RenderPool::new(
                super::BrowserSource::Profile {
                    proxy: None,
                    dir: profile.path().into(),
                    solve: false,
                },
                std::time::Duration::from_secs(3),
            )),
            always: false,
            wait_for: Some("#never-there".into()),
            block: crate::Block::default(),
            solve: false,
        };
        let mut http = page("https://example.com/", "<p>from http</p>", 200);
        http.headers
            .insert("content-type", "text/html".parse().unwrap());
        let kept = config
            .pool
            .load_in_profile("https://example.com/", http, &config, &[], true)
            .await;
        config.pool.close().await;
        assert_eq!(kept.expect("the page falls back").html, "<p>from http</p>");
    }

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn shutting_down_a_profile_browser_frees_its_profile() {
        let profile = tempfile::tempdir().unwrap();
        let first = Renderer::launch_with_profile(None, profile.path())
            .await
            .expect("Chrome launches");
        super::shut_down(Arc::downgrade(&first.browser), profile.path()).await;
        let second = Renderer::launch_with_profile(None, profile.path()).await;
        drop(first);
        second
            .expect("the profile is free once its browser shut down")
            .close()
            .await;
    }

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn a_profile_takes_one_browser_at_a_time() {
        let profile = tempfile::tempdir().unwrap();
        let first = Renderer::launch_with_profile(None, profile.path())
            .await
            .expect("Chrome launches");
        let second = Renderer::launch_with_profile(None, profile.path()).await;
        first.close().await;
        let error = second.err().expect("a second browser on the profile fails");
        assert!(
            error.to_string().contains("in use by another browser"),
            "{error}"
        );
        #[cfg(unix)]
        assert!(error.to_string().contains("its lock names"), "{error}");
        let again = Renderer::launch_with_profile(None, profile.path())
            .await
            .expect("the profile is free once the first browser closed");
        again.close().await;
    }

    #[test]
    fn a_profile_is_prepared_without_losing_the_users_preferences() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("new");
        super::prepare_profile(&dir, true).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }
        let default = dir.join("Default");
        std::fs::write(
            default.join("Preferences"),
            br#"{"homepage": "https://example.com"}"#,
        )
        .unwrap();
        std::fs::create_dir_all(default.join("Sessions")).unwrap();
        std::fs::create_dir_all(default.join("Service Worker")).unwrap();
        super::prepare_profile(&dir, true).unwrap();
        let kept: serde_json::Value =
            serde_json::from_slice(&std::fs::read(default.join("Preferences")).unwrap()).unwrap();
        assert_eq!(kept["homepage"], "https://example.com");
        assert_eq!(kept["net"]["network_prediction_options"], 2);
        assert!(!default.join("Sessions").exists());
        assert!(!default.join("Service Worker").exists());
        std::fs::write(default.join("Preferences"), b"{broken").unwrap();
        assert!(super::prepare_profile(&dir, true).is_err());
        assert_eq!(
            std::fs::read(default.join("Preferences")).unwrap(),
            b"{broken"
        );
    }

    #[test]
    #[cfg(unix)]
    fn only_chromes_own_refusal_reads_as_a_profile_in_use() {
        let profile = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("otherhost-4242", profile.path().join("SingletonLock")).unwrap();
        let other = super::launch_error(profile.path(), true, "no executable found".into());
        assert!(!other.to_string().contains("in use"), "{other}");
        let refused = super::launch_error(
            profile.path(),
            true,
            "Failed to create a ProcessSingleton for your profile directory".into(),
        )
        .to_string();
        assert!(refused.contains("in use by another browser"), "{refused}");
        assert!(refused.contains("otherhost-4242"), "{refused}");
    }

    #[test]
    fn only_a_private_directory_webclaw_made_or_an_empty_one_becomes_a_profile() {
        let root = tempfile::tempdir().unwrap();
        let made = root.path().join("made");
        super::prepare_profile(&made, true).unwrap();
        assert!(made.join(super::PROFILE_MARKER).exists());
        super::prepare_profile(&made, true).expect("its own profile again");

        let empty = root.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&empty, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        super::prepare_profile(&empty, true).expect("an empty directory");
        assert!(empty.join(super::PROFILE_MARKER).exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&empty).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700);
        }

        let chrome = root.path().join("chrome");
        std::fs::create_dir(&chrome).unwrap();
        std::fs::write(chrome.join("Local State"), b"{}").unwrap();
        let refused = super::prepare_profile(&chrome, true).unwrap_err();
        assert!(refused.contains("did not make it"), "{refused}");
        assert!(!chrome.join("Default").exists());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let open = root.path().join("open");
            std::fs::create_dir(&open).unwrap();
            std::fs::set_permissions(&open, std::fs::Permissions::from_mode(0o777)).unwrap();
            let refused = super::prepare_profile(&open, true).unwrap_err();
            assert!(refused.contains("other users can write"), "{refused}");

            let link = root.path().join("link");
            std::os::unix::fs::symlink(&made, &link).unwrap();
            let refused = super::prepare_profile(&link, true).unwrap_err();
            assert!(refused.contains("symbolic link"), "{refused}");
        }
    }

    #[test]
    fn profile_preferences_keep_the_users_own() {
        let merged = super::without_prediction(Some(
            br#"{"homepage": "https://example.com", "net": {"other": 1, "network_prediction_options": 0}}"#,
        ))
        .unwrap();
        let merged: serde_json::Value = serde_json::from_slice(&merged).unwrap();
        assert_eq!(merged["homepage"], "https://example.com");
        assert_eq!(merged["net"]["other"], 1);
        assert_eq!(merged["net"]["network_prediction_options"], 2);
        let fresh: serde_json::Value =
            serde_json::from_slice(&super::without_prediction(None).unwrap()).unwrap();
        assert_eq!(
            fresh,
            serde_json::json!({"net": {"network_prediction_options": 2}})
        );
        assert!(super::without_prediction(Some(b"{broken")).is_err());
        assert!(super::without_prediction(Some(b"[]")).is_err());
        assert!(super::without_prediction(Some(br#"{"net": 3}"#)).is_err());
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

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn a_rendered_page_prints_to_a_pdf_with_its_text() {
        let html = r#"<html><body><h1>Quarterly harvest report</h1><p id="late"></p><button id="more" onclick="document.getElementById('extra').textContent = 'Turnips added by a click'">more</button><p id="extra"></p><h2 style="break-before: page">Second sheet</h2><script>document.getElementById("late").textContent = "Pumpkins grown by script";</script></body></html>"#;
        let actions = super::parse_actions(
            r##"[{"type": "click", "selector": "#more"}, {"type": "executeJavascript", "code": "document.title = 'harvest'"}]"##,
        )
        .unwrap();
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let options = RenderOptions {
            timeout: std::time::Duration::from_secs(20),
            ..Default::default()
        };
        let print = async |out: &mut (dyn std::io::Write + Send)| {
            renderer
                .print(
                    "https://example.com/",
                    Some(Prefetched {
                        html,
                        headers: &headers,
                    }),
                    &options,
                    super::PdfOptions {
                        paper: super::Paper::A4,
                        print_background: true,
                        ..Default::default()
                    },
                    &actions,
                    out,
                )
                .await
        };
        let mut data = Vec::new();
        let printed = print(&mut data).await;
        let mut full = std::io::Cursor::new([0u8; 100]);
        let refused = print(&mut full).await;
        let slow = renderer
            .print(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(1),
                    wait_for: Some("#never".into()),
                    ..Default::default()
                },
                super::PdfOptions::default(),
                &[],
                &mut Vec::new(),
            )
            .await;
        let bad_scale = renderer
            .pdf(
                "https://example.com/",
                &options,
                super::PdfOptions {
                    scale: 3.0,
                    ..Default::default()
                },
                &mut Vec::new(),
            )
            .await;
        renderer.close().await;
        let (printed, js_results) = printed.expect("PDF prints");
        assert_eq!(js_results, [serde_json::json!("harvest")]);
        assert_eq!(printed.bytes, data.len() as u64);
        assert_eq!(&data[..5], b"%PDF-");
        let refused = refused.expect_err("a full writer stops the print");
        assert!(
            refused.to_string().contains("writing the PDF failed"),
            "{refused}"
        );
        let read = webclaw_pdf::extract_pdf(&data, webclaw_pdf::PdfMode::Auto)
            .expect("webclaw-pdf reads the PDF");
        assert_eq!(read.page_count, 2);
        for text in [
            "Quarterly harvest report",
            "Pumpkins grown by script",
            "Turnips added by a click",
            "Second sheet",
        ] {
            assert!(
                read.text.contains(text),
                "{text} missing from {}",
                read.text
            );
        }
        assert!(bad_scale.is_err());
        let slow = slow.expect_err("a page that never settles times out");
        assert!(
            slow.to_string()
                .contains("the render timeout covers loading the page"),
            "{slow}"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome"]
    async fn a_pdf_reports_the_url_it_printed_not_where_the_page_went_after() {
        let html = r#"<html><body><p>printed here</p><script>addEventListener("afterprint", () => history.pushState(null, "", "/after"))</script></body></html>"#;
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let printed = renderer
            .print(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
                super::PdfOptions::default(),
                &[],
                &mut Vec::new(),
            )
            .await;
        renderer.close().await;
        let (printed, _) = printed.expect("PDF prints");
        assert_eq!(printed.url, "https://example.com/");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn a_page_that_navigates_while_printing_fails_in_plain_words() {
        let html = r#"<html><body><p>leaving</p><script>addEventListener("beforeprint", () => { location.href = "https://example.com/elsewhere"; })</script></body></html>"#;
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let printed = renderer
            .print(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
                super::PdfOptions::default(),
                &[],
                &mut Vec::new(),
            )
            .await;
        renderer.close().await;
        let error = printed.expect_err("printing fails").to_string();
        assert!(error.contains("navigated away while printing"), "{error}");
    }

    #[tokio::test]
    async fn a_bad_pdf_scale_fails_before_the_browser_starts() {
        let pool = super::RenderPool::new(
            super::BrowserSource::Connect("ws://127.0.0.1:9/never".into()),
            std::time::Duration::from_secs(5),
        );
        let options = super::PdfOptions {
            scale: 0.0,
            ..Default::default()
        };
        let error = pool
            .pdf("https://example.com/", None, &[], options, &mut Vec::new())
            .await
            .expect_err("the scale is refused");
        assert!(error.to_string().contains("PDF scale"), "{error}");
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
        assert!(matches!(
            super::BrowserSource::launch_for(&config, true).with_profile(Some("/p".into())),
            super::BrowserSource::Profile { proxy: Some(p), dir, solve: true } if p == "http://pool.example:8080" && dir == std::path::Path::new("/p")
        ));
        assert!(matches!(
            super::BrowserSource::Connect("ws://x".into()).with_profile(Some("/p".into())),
            super::BrowserSource::Connect(_)
        ));
    }

    #[tokio::test]
    async fn a_profile_pool_passes_through_what_is_not_a_web_page() {
        let profile = super::RenderPool::new(
            super::BrowserSource::Profile {
                proxy: None,
                dir: "/nonexistent/webclaw-profile".into(),
                solve: false,
            },
            std::time::Duration::from_secs(1),
        );
        let config = super::RenderConfig {
            pool: Arc::new(pool()),
            always: false,
            wait_for: None,
            block: crate::Block::default(),
            solve: false,
        };
        let mut json = page("https://example.com/api", "{}", 200);
        json.headers
            .insert("content-type", "application/json".parse().unwrap());
        let kept = profile
            .load_in_profile("https://example.com/api", json, &config, &[], true)
            .await
            .expect("no browser is needed");
        assert_eq!(kept.html, "{}");
        let mut html = page("https://example.com/", "<script></script>", 200);
        html.headers
            .insert("content-type", "text/html".parse().unwrap());
        assert!(profile.persistent() && !pool().persistent());
        assert!(pool().wants(&html, 0, true) && !profile.wants(&html, 0, true));
        let kept = pool()
            .load_in_profile("https://example.com/", html.clone(), &config, &[], true)
            .await
            .expect("no browser is needed");
        assert_eq!(kept.html, "<script></script>");
        let failed = profile
            .load_in_profile("https://example.com/", html, &config, &[], true)
            .await
            .expect_err("a profile that cannot be opened fails the page")
            .to_string();
        assert!(failed.contains("/nonexistent/webclaw-profile"), "{failed}");
    }

    #[test]
    fn proxy_credentials_are_percent_decoded() {
        assert_eq!(super::percent_decode("p%40ss%3Aword"), "p@ss:word");
        assert_eq!(super::percent_decode("plain"), "plain");
    }

    #[test]
    fn a_capture_keeps_xhr_and_fetch_unless_told_otherwise() {
        use chromiumoxide::cdp::browser_protocol::network::ResourceType;
        let default = super::NetworkCapture::new(None, None, false).unwrap();
        assert!(default.wants(&ResourceType::Xhr, "https://a.example/api"));
        assert!(default.wants(&ResourceType::Fetch, "https://a.example/api"));
        assert!(!default.wants(&ResourceType::Script, "https://a.example/app.js"));
        let all = super::NetworkCapture::new(Some(""), Some("all"), false).unwrap();
        assert!(all.wants(&ResourceType::Image, "https://a.example/a.png"));
        let scripts =
            super::NetworkCapture::new(Some(r"/api/\d+"), Some(" Script, XHR "), false).unwrap();
        assert!(scripts.wants(&ResourceType::Script, "https://a.example/api/12"));
        assert!(scripts.wants(&ResourceType::Xhr, "https://a.example/api/12"));
        assert!(!scripts.wants(&ResourceType::Xhr, "https://a.example/api/x"));
        assert!(!scripts.wants(&ResourceType::Fetch, "https://a.example/api/12"));
        let unknown = super::NetworkCapture::new(None, Some("xhr,json"), false).unwrap_err();
        assert!(unknown.contains("\"json\""), "{unknown}");
        assert!(super::NetworkCapture::new(Some("("), None, false).is_err());
        for empty in [",", " , ", ""] {
            assert!(
                super::NetworkCapture::new(None, Some(empty), false).is_err(),
                "{empty:?} captured something"
            );
        }
    }

    #[test]
    fn credential_headers_are_left_out_unless_asked_for() {
        let sent = serde_json::json!({
            "Accept": "application/json",
            "Authorization": "Bearer secret",
            "X-Api-Key": "k",
            "X-CSRF-Token": "t",
            "Cookie": "a=b",
            "X-Requested-With": "XMLHttpRequest",
        });
        let kept = super::NetworkCapture::new(None, None, false)
            .unwrap()
            .headers(&sent);
        assert_eq!(
            kept.keys().collect::<Vec<_>>(),
            ["Accept", "X-Requested-With"]
        );
        let all = super::NetworkCapture::new(None, None, true)
            .unwrap()
            .headers(&sent);
        assert_eq!(all.len(), 6);
    }

    #[test]
    fn responses_from_private_addresses_are_not_kept() {
        let response = |ip: &str| {
            serde_json::from_value::<
                chromiumoxide::cdp::browser_protocol::network::EventResponseReceived,
            >(serde_json::json!({
                "requestId": "1",
                "loaderId": "1",
                "timestamp": 1.0,
                "type": "XHR",
                "hasExtraInfo": false,
                "response": {
                    "url": "https://a.example/api",
                    "status": 200,
                    "statusText": "",
                    "headers": {},
                    "mimeType": "application/json",
                    "charset": "",
                    "connectionReused": false,
                    "connectionId": 1.0,
                    "remoteIPAddress": ip,
                    "encodedDataLength": 10.0,
                    "securityState": "secure"
                }
            }))
            .unwrap()
        };
        let capture = super::NetworkCapture::new(None, None, false).unwrap();
        assert!(capture.keeps(&response("93.184.215.14"), true));
        assert!(!capture.keeps(&response("127.0.0.1"), true));
        assert!(!capture.keeps(&response("[::1]"), true));
        assert!(!capture.keeps(&response("10.1.2.3"), true));
        assert!(capture.keeps(&response("127.0.0.1"), false));
    }

    #[test]
    fn credential_headers_are_told_apart_by_their_name_parts() {
        for name in [
            "Authorization",
            "Proxy-Authorization",
            "Cookie",
            "X-Api-Key",
            "X-Firebase-AppCheck",
            "Bearer",
            "X-Access",
            "X-Sig",
            "X-Client-Data",
            "X-Amz-Security-Token",
            "Private-Token",
            "Ocp-Apim-Subscription-Key",
            "X-Auth-Hash",
            "X-CSRFToken",
            "x_session_id",
        ] {
            assert!(super::is_credential(name), "{name} kept");
        }
        for name in [
            "Accept",
            "Content-Type",
            "X-Author",
            "X-Monkey",
            "X-Requested-With",
            "Access-Control-Request-Headers",
            "Referer",
        ] {
            assert!(!super::is_credential(name), "{name} dropped");
        }
    }

    #[test]
    fn credentials_in_query_strings_are_redacted() {
        assert_eq!(
            super::redact_query("https://a.example/api?access_token=S1&q=rust&key=AIzaS2"),
            "https://a.example/api?access_token=REDACTED&q=rust&key=REDACTED"
        );
        assert_eq!(
            super::redact_query(
                "https://x-dsn.algolia.net/1/indexes/i?query=a&x-algolia-api-key=S3&x-algolia-application-id=X"
            ),
            "https://x-dsn.algolia.net/1/indexes/i?query=a&x-algolia-api-key=REDACTED&x-algolia-application-id=X"
        );
        let untouched = "https://a.example/search?keyword=monkey%20key&page=2";
        assert_eq!(super::redact_query(untouched), untouched);
    }

    #[test]
    fn bodies_are_cut_to_their_own_limit_and_to_the_total() {
        let capture = super::NetworkCapture::new(None, None, false)
            .unwrap()
            .limit_bytes(4, 6);
        let bodies = ["abcdef", "xyz", "q"]
            .map(|body| Some(body.to_string()))
            .into_iter()
            .chain([None])
            .collect();
        assert_eq!(
            capture.cut(bodies),
            [
                (Some("abcd".to_string()), true),
                (Some("xy".to_string()), true),
                (Some(String::new()), true),
                (None, false),
            ]
        );
    }

    #[test]
    fn bodies_are_cut_on_a_character_boundary() {
        assert_eq!(super::char_floor("héllo", 2), 1);
        assert_eq!(super::char_floor("héllo", 3), 3);
        assert_eq!(super::char_floor("abc", 10), 3);
        assert_eq!(super::char_floor("abc", 0), 0);
    }

    #[tokio::test]
    #[ignore = "needs a local Chrome and network access"]
    async fn a_render_captures_the_json_a_page_loads() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let rendered = renderer
            .capture(
                "https://quotes.toscrape.com/scroll",
                None,
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(30),
                    ..Default::default()
                },
                &[],
                &super::NetworkCapture::new(None, None, false).unwrap(),
            )
            .await;
        renderer.close().await;
        let network = rendered.expect("render succeeds").network;
        let api = network
            .iter()
            .find(|r| r.url.contains("/api/quotes?page=1"))
            .unwrap_or_else(|| panic!("no API response in {network:?}"));
        assert_eq!(
            (api.method.as_str(), api.status, api.resource_type.as_str()),
            ("GET", 200, "xhr")
        );
        assert!(api.mime_type.contains("json"), "{}", api.mime_type);
        let body: serde_json::Value = serde_json::from_str(api.body.as_deref().unwrap()).unwrap();
        assert!(body["quotes"].as_array().is_some_and(|q| !q.is_empty()));
        assert!(!api.truncated);
        assert!(
            network
                .iter()
                .all(|r| r.resource_type == "xhr" || r.resource_type == "fetch")
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn a_capture_keeps_responses_up_to_its_limit_and_none_that_were_blocked() {
        let html = r#"<html><body><p id="msg">rendered</p><img src="/blocked.png"><script>
            fetch("/?keyed=1&access_token=S1", {headers: {"Authorization": "Bearer secret", "X-Api-Key": "k", "X-Custom": "v"}})
                .then(() => Promise.allSettled([...Array(140).keys()].map(i => fetch("/?n=" + i))))
                .then(() => {
                    const done = document.createElement("div"); done.id = "done"; document.body.appendChild(done);
                });
            </script></body></html>"#;
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let rendered = renderer
            .capture(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(30),
                    wait_for: Some("#done".into()),
                    block: "images".parse().unwrap(),
                    ..Default::default()
                },
                &[],
                &super::NetworkCapture::new(None, Some("fetch,image"), false).unwrap(),
            )
            .await;
        renderer.close().await;
        let network = rendered.expect("render succeeds").network;
        assert_eq!(network.len(), super::MAX_RESPONSES, "{network:?}");
        assert!(network.iter().all(|r| r.resource_type == "fetch"));
        let keyed = network
            .iter()
            .find(|r| r.url.ends_with("?keyed=1&access_token=REDACTED"))
            .expect("the keyed request was captured");
        assert_eq!(
            keyed.request_headers.get("X-Custom").map(String::as_str),
            Some("v")
        );
        assert!(!keyed.request_headers.contains_key("Authorization"));
        assert!(!keyed.request_headers.contains_key("X-Api-Key"));
        assert!(
            keyed
                .body
                .as_deref()
                .is_some_and(|b| b.contains("Example Domain"))
        );
    }

    async fn capture_on_example(
        html: &str,
        pattern: &str,
        wait_for: Option<&str>,
    ) -> Vec<super::NetworkResponse> {
        let headers = http::HeaderMap::new();
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let rendered = renderer
            .capture(
                "https://example.com/",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(30),
                    wait_for: wait_for.map(str::to_string),
                    ..Default::default()
                },
                &[],
                &super::NetworkCapture::new(Some(pattern), None, false).unwrap(),
            )
            .await;
        renderer.close().await;
        rendered.expect("render succeeds").network
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn a_capture_waits_for_a_request_that_follows_a_pause() {
        let html = r#"<html><body><p>paused</p><script>
            fetch("/?first=1")
                .then(() => new Promise(done => setTimeout(done, 800)))
                .then(() => fetch("/?second=1"));
            </script></body></html>"#;
        let network = capture_on_example(html, r"\?(first|second)=", None).await;
        let urls: Vec<&str> = network.iter().map(|r| r.url.as_str()).collect();
        assert!(urls.iter().any(|u| u.ends_with("?second=1")), "{urls:?}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn a_late_request_is_caught_by_waiting_for_what_it_builds() {
        let html = r#"<html><body><p>timer</p><script>
            setTimeout(() => fetch("/?late=1").then(() => {
                const done = document.createElement("div"); done.id = "done"; document.body.appendChild(done);
            }), 1500);
            </script></body></html>"#;
        assert!(capture_on_example(html, r"\?late=", None).await.is_empty());
        let network = capture_on_example(html, r"\?late=", Some("#done")).await;
        assert_eq!(network.len(), 1, "{network:?}");
    }

    #[test]
    fn solving_needs_a_chrome_profile() {
        let config = |browser| crate::FetchConfig {
            browser,
            render: Some(super::RenderConfig {
                pool: Arc::new(pool()),
                always: false,
                wait_for: None,
                block: crate::Block::default(),
                solve: true,
            }),
            ..Default::default()
        };
        use crate::BrowserProfile;
        for ok in [
            BrowserProfile::Chrome,
            BrowserProfile::ChromeMacos,
            BrowserProfile::Edge,
        ] {
            assert!(crate::FetchClient::new(config(ok)).is_ok());
        }
        for refused in [
            BrowserProfile::Firefox,
            BrowserProfile::Safari,
            BrowserProfile::SafariIos,
            BrowserProfile::Random,
        ] {
            assert!(crate::FetchClient::new(config(refused)).is_err());
        }
    }

    #[test]
    fn a_clearance_goes_only_where_its_cookie_would() {
        let kept = super::Clearance {
            domain: ".example.com".into(),
            path: "/shop".into(),
            secure: true,
            expires: Some(2_000.0),
            name: "cf_clearance".into(),
            value: "x".into(),
        };
        let url = |u: &str| url::Url::parse(u).unwrap();
        assert!(kept.applies_to(&url("https://www.example.com/shop"), 1_000.0));
        assert!(kept.applies_to(&url("https://example.com/shop/item"), 1_000.0));
        assert!(!kept.applies_to(&url("https://example.com/shopping"), 1_000.0));
        assert!(!kept.applies_to(&url("https://example.com/"), 1_000.0));
        assert!(!kept.applies_to(&url("http://example.com/shop"), 1_000.0));
        assert!(!kept.applies_to(&url("https://example.com/shop"), 3_000.0));
        assert!(!kept.applies_to(&url("https://example.org/shop"), 1_000.0));
    }

    #[tokio::test]
    async fn the_clearance_is_read_while_a_browser_launches() {
        let pool = pool();
        pool.clearances
            .lock()
            .unwrap()
            .cookies
            .push(super::Clearance {
                domain: "example.com".into(),
                path: "/".into(),
                secure: false,
                expires: None,
                name: "cf_clearance".into(),
                value: "x".into(),
            });
        let launching = pool.browser.write().await;
        assert_eq!(
            pool.clearance("https://example.com/a").map(|c| c.0),
            Some("cf_clearance=x".to_string())
        );
        drop(launching);
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
                    let set_cookie = if passed && !target.contains("example.net") {
                        "Set-Cookie: visitor=anon; Path=/\r\n"
                    } else {
                        ""
                    };
                    let (status, body) = if target.contains("challenge-platform") {
                        ("200 OK", CHECKBOX.to_string())
                    } else if target.contains("example.net") {
                        ("404 Not Found", String::new())
                    } else if target.contains("stuck") {
                        (
                            "403 Forbidden",
                            CHALLENGE.replace("challenge-platform/fixture", "gone"),
                        )
                    } else if passed {
                        (
                            "200 OK",
                            "<html><body><p>the real page</p></body></html>".to_string(),
                        )
                    } else {
                        ("403 Forbidden", CHALLENGE.to_string())
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/html\r\n{set_cookie}Cache-Control: no-store\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
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
    async fn a_callers_own_cookies_never_reach_the_shared_clearance() {
        let proxy = challenge_proxy().await;
        let renderer = Renderer::launch_as_chrome(Some(&proxy))
            .await
            .expect("Chrome launches");
        let solved = renderer
            .render(
                "http://example.com:8080/page",
                None,
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(30),
                    solve: true,
                    headers: vec![("Cookie".into(), "session=secretA".into())],
                    ..Default::default()
                },
            )
            .await;
        let clearance = renderer.clearance("http://example.com:8080/other");
        renderer.close().await;
        let solved = solved.expect("render succeeds");
        assert!(solved.html.contains("the real page"), "{}", solved.html);
        assert_eq!(clearance, None);
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_widget_that_never_gives_a_token_costs_one_wait_per_render() {
        let proxy = challenge_proxy().await;
        let renderer = Renderer::launch_as_chrome(Some(&proxy))
            .await
            .expect("Chrome launches");
        let widget = r#"<html><body><p>Sign up for the newsletter.</p><div class="cf-turnstile"><iframe src="http://example.net:8080/cdn-cgi/challenge-platform/fixture" width="300" height="65" style="border: 0"></iframe></div><script src="https://challenges.cloudflare.com/turnstile/v0/api.js"></script></body></html>"#;
        let started = std::time::Instant::now();
        let rendered = renderer
            .render(
                "http://example.com:8080/signup",
                Some(Prefetched {
                    html: widget,
                    headers: &http::HeaderMap::new(),
                }),
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(40),
                    solve: true,
                    ..Default::default()
                },
            )
            .await;
        let took = started.elapsed();
        renderer.close().await;
        assert!(
            rendered
                .expect("the page is returned")
                .html
                .contains("newsletter")
        );
        assert!(took < std::time::Duration::from_secs(14), "{took:?}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_failed_solve_is_the_error_a_fetch_reports() {
        let proxy = challenge_proxy().await;
        let config = crate::FetchConfig {
            proxy: Some(proxy.clone()),
            render: Some(super::RenderConfig {
                pool: Arc::new(super::RenderPool::new(
                    super::BrowserSource::Launch {
                        proxy: Some(proxy),
                        solve: true,
                    },
                    std::time::Duration::from_secs(8),
                )),
                always: false,
                wait_for: None,
                block: crate::Block::default(),
                solve: true,
            }),
            ..Default::default()
        };
        let client = crate::FetchClient::new(config.clone()).expect("client");
        let fetched = client
            .fetch("http://example.com:8080/stuck")
            .await
            .expect("the challenge comes back");
        assert_eq!(fetched.status, 403);
        let (_, extracted) = client
            .extract_page(fetched, &webclaw_core::ExtractionOptions::default())
            .await;
        if let Some(render) = &config.render {
            render.pool.close().await;
        }
        let error = extracted.expect_err("the solve fails").to_string();
        assert!(error.contains("challenge was still shown"), "{error}");
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

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_capture_through_a_local_proxy_keeps_its_responses() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let proxy = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_url = format!("http://localhost:{}", proxy.local_addr().unwrap().port());
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = proxy.accept().await {
                tokio::spawn(async move {
                    let mut buffer = [0; 4096];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    let request = String::from_utf8_lossy(&buffer[..n]).to_string();
                    let (kind, body) = if request.contains("/api ") {
                        ("application/json", r#"{"ok":true}"#)
                    } else {
                        (
                            "text/html",
                            r#"<html><body><p>proxied</p><script>fetch("/api");</script></body></html>"#,
                        )
                    };
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = socket.write_all(response.as_bytes()).await;
                });
            }
        });
        let renderer = Renderer::launch(Some(&proxy_url))
            .await
            .expect("Chrome launches");
        let rendered = renderer
            .capture(
                "http://example.com/page",
                None,
                &RenderOptions {
                    timeout: std::time::Duration::from_secs(20),
                    ..Default::default()
                },
                &[],
                &super::NetworkCapture::new(None, None, false).unwrap(),
            )
            .await;
        renderer.close().await;
        let network = rendered.expect("render succeeds").network;
        assert!(
            network
                .iter()
                .any(|r| r.url.ends_with("/api") && r.body.as_deref() == Some(r#"{"ok":true}"#)),
            "{network:?}"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs network access"]
    async fn requests_a_capture_drops_do_not_hold_it_open() {
        let html = r#"<html><body><p>beacons</p><script>
            fetch("/?data=1");
            setInterval(() => { new Image().src = "/beacon.gif?" + Date.now(); }, 700);
            </script></body></html>"#;
        let start = std::time::Instant::now();
        let network = capture_on_example(html, r"\?data=", None).await;
        let took = start.elapsed();
        assert_eq!(network.len(), 1, "{network:?}");
        assert!(
            took < std::time::Duration::from_secs(7),
            "the capture took {took:?}"
        );
    }

    #[test]
    fn the_page_url_in_a_referer_is_redacted_too() {
        let sent = serde_json::json!({
            "Referer": "https://a.example/page?access_token=PAGE_SECRET&q=1",
        });
        let kept = super::NetworkCapture::new(None, None, false)
            .unwrap()
            .headers(&sent);
        assert_eq!(
            kept["Referer"],
            "https://a.example/page?access_token=REDACTED&q=1"
        );
        let all = super::NetworkCapture::new(None, None, true)
            .unwrap()
            .headers(&sent);
        assert_eq!(
            all["Referer"],
            "https://a.example/page?access_token=PAGE_SECRET&q=1"
        );
    }

    #[test]
    fn redaction_leaves_the_rest_of_the_url_as_it_was() {
        assert_eq!(
            super::redact_query("https://a.example/s?q=a%20b&token=T1&x=%7E~#frag"),
            "https://a.example/s?q=a%20b&token=REDACTED&x=%7E~#frag"
        );
        assert_eq!(
            super::redact_query("https://a.example/cb#access_token=T2&state=s%20t&expires_in=3600"),
            "https://a.example/cb#access_token=REDACTED&state=s%20t&expires_in=3600"
        );
        assert_eq!(
            super::redact_query("https://a.example/s?Access%5FToken=T3"),
            "https://a.example/s?Access%5FToken=REDACTED"
        );
    }
}
