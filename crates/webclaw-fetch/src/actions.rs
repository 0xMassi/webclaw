//! Steps run on a rendered page after it loads and before it is read.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
use chromiumoxide::cdp::browser_protocol::network::{
    EventLoadingFailed, EventLoadingFinished, EventRequestWillBeSent, RequestId, ResourceType,
};
use chromiumoxide::cdp::browser_protocol::page::{
    EventJavascriptDialogOpening, HandleJavaScriptDialogParams,
};
use chromiumoxide::cdp::browser_protocol::target::{
    CloseTargetParams, EventTargetCreated, GetTargetsParams, TargetId,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallFunctionOnParams, EvaluateParams, ReleaseObjectParams,
};
use chromiumoxide::element::Element;
use chromiumoxide::error::CdpError;
use chromiumoxide::{Browser, Page};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::Value;

use crate::error::FetchError;

/// One step run on a rendered page after it loads and before webclaw reads
/// or captures it, in the shape the hosted API's `actions` take. Parse a
/// list with [`parse_actions`].
///
/// Every step that needs an element waits up to 10 seconds for it. After a
/// click, a key press or a scroll, webclaw waits until the page stops
/// changing, up to 5 seconds. Alerts, confirms and prompts that steps
/// open are dismissed.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum Action {
    /// Click the first element matching `selector`, the match at index `nth`
    /// (from 0), or with `all` every match. With `repeat`, click the first
    /// visible match again and again, up to that many times, and stop early
    /// once none is left or two clicks in a row change nothing, as for a
    /// "load more" button. Running out of time before that fails the step.
    Click {
        selector: String,
        #[serde(default)]
        all: bool,
        nth: Option<usize>,
        repeat: Option<u32>,
    },
    /// Focus the element matching `selector` and type `value` into it, as
    /// an input method would, so any text works.
    Type { selector: String, value: String },
    /// Press a key such as `Enter`, `Tab` or `ArrowDown` in the focused element.
    Press { key: String },
    /// Scroll the page, or the element matching `selector`, `amount` pixels
    /// in `direction` (`down` by default, or `up`), or without `amount` all
    /// the way to the end. With `repeat`, scroll again and again, up to that
    /// many times, and stop early once two scrolls in a row add nothing, as
    /// for infinite scroll. Running out of time before that fails the step.
    Scroll {
        #[serde(default)]
        direction: Direction,
        amount: Option<u32>,
        selector: Option<String>,
        repeat: Option<u32>,
    },
    /// Pause for `milliseconds`.
    Wait { milliseconds: u64 },
    /// Wait until an element matching `selector` exists, up to `timeout`
    /// milliseconds (10000 by default).
    WaitForSelector {
        selector: String,
        timeout: Option<u64>,
    },
    /// Run JavaScript, an expression, statements whose last one gives the
    /// value, or a function body with `return`, with `await` allowed
    /// anywhere, and keep that value, awaited when it is a promise, as JSON.
    /// A later step may repeat the same top-level `let`, `const` or `class`
    /// declaration, but not switch its kind or reuse a name the page's own
    /// scripts declare.
    /// The page turns the value into JSON itself, and a value longer than
    /// what is left of the 1 MiB all such steps share fails the step there.
    ExecuteJavascript { code: String },
}

/// Which way a [`Action::Scroll`] goes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    #[default]
    Down,
    Up,
}

impl Action {
    fn name(&self) -> &'static str {
        match self {
            Self::Click { .. } => "click",
            Self::Type { .. } => "type",
            Self::Press { .. } => "press",
            Self::Scroll { .. } => "scroll",
            Self::Wait { .. } => "wait",
            Self::WaitForSelector { .. } => "waitForSelector",
            Self::ExecuteJavascript { .. } => "executeJavascript",
        }
    }

    fn selector(&self) -> Option<&str> {
        match self {
            Self::Click { selector, .. }
            | Self::Type { selector, .. }
            | Self::WaitForSelector { selector, .. } => Some(selector),
            Self::Scroll { selector, .. } => selector.as_deref(),
            _ => None,
        }
    }
}

/// Parse a JSON array of [`Action`]s, such as
/// `[{"type": "click", "selector": "button.more", "repeat": 10}]`.
///
/// ```
/// let actions = webclaw_fetch::render::parse_actions(
///     r#"[{"type": "scroll", "repeat": 20}, {"type": "executeJavascript", "code": "document.title"}]"#,
/// )
/// .unwrap();
/// assert_eq!(actions.len(), 2);
/// assert!(webclaw_fetch::render::parse_actions(r#"[{"type": "hover"}]"#).is_err());
/// ```
pub fn parse_actions(json: &str) -> Result<Vec<Action>, String> {
    let actions: Vec<Action> =
        serde_json::from_str(json).map_err(|e| format!("invalid actions: {e}"))?;
    for (i, action) in actions.iter().enumerate() {
        if let Action::Click {
            all, nth, repeat, ..
        } = action
            && u8::from(*all) + u8::from(nth.is_some()) + u8::from(repeat.is_some()) > 1
        {
            return Err(format!(
                "action {}: click takes one of all, nth and repeat",
                i + 1
            ));
        }
    }
    Ok(actions)
}

const FIND: Duration = Duration::from_secs(10);
const PATIENCE: Duration = Duration::from_millis(1500);
const QUIET: Duration = Duration::from_millis(400);
const SETTLE_LIMIT: Duration = Duration::from_secs(5);
const MAX_RESULT_CHARS: usize = 1 << 20;
const LONG_POLL: Duration = Duration::from_secs(5);

#[derive(Clone, Default)]
struct Requests(Arc<Mutex<HashMap<RequestId, Instant>>>);

impl Requests {
    async fn watch(page: &Page) -> Result<(Self, tokio::task::JoinHandle<()>), CdpError> {
        let site = page
            .url()
            .await?
            .and_then(|u| Some(url::Url::parse(&u).ok()?.host_str()?.to_string()))
            .unwrap_or_default();
        let mut sent = page.event_listener::<EventRequestWillBeSent>().await?;
        let mut finished = page.event_listener::<EventLoadingFinished>().await?;
        let mut failed = page.event_listener::<EventLoadingFailed>().await?;
        let requests = Self::default();
        let open = requests.0.clone();
        let task = tokio::spawn(async move {
            loop {
                let (id, start) = tokio::select! {
                    Some(e) = sent.next() => (e.request_id.clone(), match e.r#type {
                        Some(ResourceType::Document) => true,
                        Some(ResourceType::Xhr | ResourceType::Fetch) => same_site(&e.request.url, &site),
                        _ => false,
                    }),
                    Some(e) = finished.next() => (e.request_id.clone(), false),
                    Some(e) = failed.next() => (e.request_id.clone(), false),
                    else => break,
                };
                if let Ok(mut open) = open.lock() {
                    if start {
                        open.insert(id, Instant::now());
                    } else {
                        open.remove(&id);
                    }
                }
            }
        });
        Ok((requests, task))
    }

    fn busy(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|open| open.values().any(|at| at.elapsed() < LONG_POLL))
    }
}

pub(crate) struct Popups {
    opener: TargetId,
    opened: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Popups {
    pub(crate) async fn watch(browser: &Arc<Browser>, opener: TargetId) -> Result<Self, CdpError> {
        let mut created = browser.event_listener::<EventTargetCreated>().await?;
        let weak = Arc::downgrade(browser);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let log = opened.clone();
        let parent = opener.clone();
        let task = tokio::spawn(async move {
            while let Some(event) = created.next().await {
                let info = &event.target_info;
                if info.opener_id.as_ref() != Some(&parent) {
                    continue;
                }
                if let Ok(mut log) = log.lock() {
                    log.push(info.url.clone());
                }
                let Some(browser) = weak.upgrade() else {
                    break;
                };
                let _ = browser
                    .execute(CloseTargetParams::new(info.target_id.clone()))
                    .await;
            }
        });
        Ok(Self {
            opener,
            opened,
            task,
        })
    }

    pub(crate) async fn close(self, browser: &Browser) -> Result<(), FetchError> {
        self.task.abort();
        if let Ok(targets) = browser.execute(GetTargetsParams::default()).await {
            for target in &targets.result.target_infos {
                if target.opener_id.as_ref() == Some(&self.opener) {
                    if let Ok(mut log) = self.opened.lock() {
                        log.push(target.url.clone());
                    }
                    let _ = browser
                        .execute(CloseTargetParams::new(target.target_id.clone()))
                        .await;
                }
            }
        }
        let opened = self.opened.lock().map(|l| l.clone()).unwrap_or_default();
        for url in opened.iter().filter(|u| u.starts_with("http")) {
            if let Err(e) = crate::url_security::validate_public_http_url(url).await {
                return Err(FetchError::Build(format!(
                    "the actions opened a window on {url}: {e}"
                )));
            }
        }
        Ok(())
    }
}

pub(crate) async fn run(
    page: &Page,
    actions: &[Action],
    budget: Duration,
) -> Result<Vec<Value>, FetchError> {
    if actions.is_empty() {
        return Ok(Vec::new());
    }
    let deadline = later(budget);
    let (requests, watching) = Requests::watch(page)
        .await
        .map_err(|e| FetchError::Build(format!("browser: {e}")))?;
    let mut dialogs = page
        .event_listener::<EventJavascriptDialogOpening>()
        .await
        .map_err(|e| FetchError::Build(format!("browser: {e}")))?;
    let answering = page.clone();
    let dismissing = tokio::spawn(async move {
        while dialogs.next().await.is_some() {
            let _ = answering
                .execute(HandleJavaScriptDialogParams::new(false))
                .await;
        }
    });
    let ran = run_all(page, &requests, actions, budget, deadline).await;
    watching.abort();
    dismissing.abort();
    ran
}

async fn run_all(
    page: &Page,
    requests: &Requests,
    actions: &[Action],
    budget: Duration,
    deadline: Instant,
) -> Result<Vec<Value>, FetchError> {
    let mut results = Vec::new();
    let mut used = 0;
    for (i, action) in actions.iter().enumerate() {
        let fail =
            |e: String| FetchError::Build(format!("action {} ({}): {e}", i + 1, action.name()));
        let out_of_time = || fail(format!("ran out of time; the actions get {budget:?}"));
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(out_of_time());
        }
        let room = MAX_RESULT_CHARS.saturating_sub(used);
        let value = tokio::time::timeout(left, step(page, requests, action, deadline, room))
            .await
            .map_err(|_| out_of_time())?
            .map_err(fail)?;
        if let Some(value) = value {
            used += value.to_string().len();
            results.push(value);
        }
    }
    Ok(results)
}

async fn step(
    page: &Page,
    requests: &Requests,
    action: &Action,
    deadline: Instant,
    room: usize,
) -> Result<Option<Value>, String> {
    if !matches!(action, Action::Wait { .. }) {
        ready(page).await?;
    }
    if let Some(selector) = action.selector() {
        check_selector(page, selector).await?;
    }
    match action {
        Action::Click {
            selector,
            all,
            nth,
            repeat,
        } => {
            let found = find(page, selector).await?;
            if let Some(times) = repeat {
                let mut target = first_visible(page, selector)
                    .await
                    .or_else(|| found.into_iter().next());
                let mut misses = 0;
                for done in 1..=*times {
                    let Some(element) = target.take() else { break };
                    let before = signature(page).await;
                    click(page, &element).await?;
                    misses = if settle(page, requests, before, PATIENCE, false).await {
                        0
                    } else {
                        misses + 1
                    };
                    if misses == 2 || done == *times {
                        break;
                    }
                    if Instant::now() + PATIENCE > deadline {
                        return Err(format!(
                            "ran out of time after {done} clicks; raise the timeout or lower repeat"
                        ));
                    }
                    target = first_visible(page, selector).await;
                }
                settle(page, requests, signature(page).await, QUIET, true).await;
            } else if *all {
                let before = signature(page).await;
                for element in &found {
                    click(page, element).await?;
                }
                settle(page, requests, before, PATIENCE, true).await;
            } else {
                let index = nth.unwrap_or(0);
                let element = found.get(index).ok_or_else(|| {
                    format!(
                        "{selector:?} matches {} elements, no index {index}",
                        found.len()
                    )
                })?;
                let before = signature(page).await;
                click(page, element).await?;
                settle(page, requests, before, PATIENCE, true).await;
            }
        }
        Action::Type { selector, value } => {
            let element = find(page, selector).await?.swap_remove(0);
            element.focus().await.map_err(js_error)?;
            page.execute(InsertTextParams::new(value.clone()))
                .await
                .map_err(js_error)?;
        }
        Action::Press { key } => {
            let before = signature(page).await;
            page.press_key(key).await.map_err(js_error)?;
            settle(page, requests, before, PATIENCE, true).await;
        }
        Action::Scroll {
            direction,
            amount,
            selector,
            repeat,
        } => {
            let sign = if *direction == Direction::Up { -1 } else { 1 };
            let script = format!(
                "(() => {{ const sel = {}; const by = {}; const sign = {sign}; \
                 const el = sel === null ? document.scrollingElement || document.documentElement : document.querySelector(sel); \
                 if (!el) return -1; const top = el.scrollTop; \
                 if (by === null) el.scrollTop = sign > 0 ? el.scrollHeight : 0; else el.scrollTop += sign * by; \
                 return Math.abs(el.scrollTop - top); }})()",
                serde_json::to_string(selector).unwrap_or_default(),
                serde_json::to_string(amount).unwrap_or_default(),
            );
            let mut misses = 0;
            let times = repeat.unwrap_or(1);
            for done in 1..=times {
                let before = signature(page).await;
                let moved = page
                    .evaluate_expression(script.as_str())
                    .await
                    .map_err(js_error)?
                    .into_value::<f64>()
                    .unwrap_or(0.);
                if moved < 0. {
                    return Err(format!(
                        "no element matches {:?}",
                        selector.as_deref().unwrap_or_default()
                    ));
                }
                let stepping = amount.is_some() && moved > 0.;
                let grew = settle(
                    page,
                    requests,
                    before,
                    if stepping { QUIET } else { PATIENCE },
                    false,
                )
                .await;
                misses = if grew || stepping { 0 } else { misses + 1 };
                if misses == 2 || done == times {
                    break;
                }
                if Instant::now() + PATIENCE > deadline {
                    return Err(format!(
                        "ran out of time after {done} scrolls; raise the timeout or lower repeat"
                    ));
                }
            }
            settle(page, requests, signature(page).await, QUIET, true).await;
        }
        Action::Wait { milliseconds } => {
            let until = later(Duration::from_millis(*milliseconds)).min(deadline);
            tokio::time::sleep_until(until.into()).await;
        }
        Action::WaitForSelector { selector, timeout } => {
            let limit = timeout.map_or(FIND, Duration::from_millis);
            page.wait_for_selector(selector.as_str(), Some(limit))
                .await
                .map_err(|_| format!("no element matches {selector:?} after {limit:?}"))?;
        }
        Action::ExecuteJavascript { code } => return run_script(page, code, room).await.map(Some),
    }
    Ok(None)
}

const CAP: &str = "let text; \
                   try { text = JSON.stringify(value === undefined ? null : value); } \
                   catch (e) { throw new Error('the value is not JSON: ' + e.message); } \
                   if (text === undefined) text = 'null'; \
                   if (text.length > ROOM) throw new Error('the value is ' + text.length + ' characters long, with ROOM characters left of the 1 MiB all script values share'); \
                   return text;";

async fn run_script(page: &Page, code: &str, room: usize) -> Result<Value, String> {
    let cap = CAP.replace("ROOM", &room.to_string());
    let expression = format!("(async () => (\n{code}\n))");
    let capped = async |function: String| {
        let text: String = page
            .evaluate_expression(format!(
                "(async () => {{ const value = await {function}(); {cap} }})()"
            ))
            .await
            .map_err(js_error)?
            .into_value()
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    };
    if page
        .evaluate_expression(format!("typeof {expression}"))
        .await
        .is_ok()
    {
        return capped(expression).await;
    }
    let as_is = EvaluateParams::builder()
        .expression(code)
        .await_promise(true)
        .repl_mode(true)
        .return_by_value(false)
        .build()
        .map_err(|e| e.to_string())?;
    let object = match page.evaluate_expression(as_is).await {
        Err(CdpError::JavascriptException(e))
            if exception_text(&e).contains("Illegal return statement") =>
        {
            return capped(format!("(async () => {{\n{code}\n}})")).await;
        }
        other => other.map_err(js_error)?.object().clone(),
    };
    let Some(id) = object.object_id else {
        let value = object.value.unwrap_or(Value::Null);
        let length = value.to_string().len();
        if length > room {
            return Err(format!(
                "the value is {length} characters long, with {room} characters left of the 1 MiB all script values share"
            ));
        }
        return Ok(value);
    };
    let call = CallFunctionOnParams::builder()
        .function_declaration(format!(
            "async function() {{ const value = await this; {cap} }}"
        ))
        .object_id(id.clone())
        .await_promise(true)
        .return_by_value(true)
        .build()
        .map_err(|e| e.to_string())?;
    let text = page.execute(call).await;
    let _ = page.execute(ReleaseObjectParams::new(id)).await;
    let text = text.map_err(js_error)?.result;
    if let Some(e) = text.exception_details {
        return Err(exception_text(&e));
    }
    let text = text
        .result
        .value
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "null".into());
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

fn later(wait: Duration) -> Instant {
    let now = Instant::now();
    now.checked_add(wait)
        .unwrap_or_else(|| now + Duration::from_secs(1 << 30))
}

async fn ready(page: &Page) -> Result<(), String> {
    let start = Instant::now();
    loop {
        let parsed = page
            .evaluate_expression("document.readyState !== 'loading' && !!document.body")
            .await
            .ok()
            .and_then(|r| r.into_value::<bool>().ok());
        if parsed == Some(true) {
            return Ok(());
        }
        if start.elapsed() >= FIND {
            return Err(format!("the page was still loading after {FIND:?}"));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn check_selector(page: &Page, selector: &str) -> Result<(), String> {
    let check = format!(
        "(() => {{ try {{ document.createDocumentFragment().querySelector({}); return true; }} catch (e) {{ return false; }} }})()",
        serde_json::to_string(selector).unwrap_or_default()
    );
    let valid = page
        .evaluate_expression(check)
        .await
        .map_err(js_error)?
        .into_value::<bool>()
        .unwrap_or(false);
    if valid {
        Ok(())
    } else {
        Err(format!("{selector:?} is not a valid CSS selector"))
    }
}

async fn find(page: &Page, selector: &str) -> Result<Vec<Element>, String> {
    let start = Instant::now();
    loop {
        match page.find_elements(selector).await {
            Ok(found) if !found.is_empty() => return Ok(found),
            _ if start.elapsed() >= FIND => {
                return Err(format!("no element matches {selector:?} after {FIND:?}"));
            }
            _ => tokio::time::sleep(Duration::from_millis(100)).await,
        }
    }
}

async fn first_visible(page: &Page, selector: &str) -> Option<Element> {
    let start = Instant::now();
    loop {
        if let Some(element) = visible(page, selector).await {
            return Some(element);
        }
        if start.elapsed() >= PATIENCE {
            return None;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn visible(page: &Page, selector: &str) -> Option<Element> {
    for element in page.find_elements(selector).await.ok()? {
        let shown = element
            .call_js_fn(
                "function() { return !!(this.offsetWidth || this.offsetHeight || this.getClientRects().length); }",
                false,
            )
            .await
            .ok()
            .and_then(|r| r.result.value)
            .and_then(|v| v.as_bool());
        if shown == Some(true) {
            return Some(element);
        }
    }
    None
}

async fn click(page: &Page, element: &Element) -> Result<(), String> {
    const CENTER: &str = "function() { this.scrollIntoView({block: 'center', inline: 'center', behavior: 'instant'}); \
                          const r = this.getBoundingClientRect(); const x = r.left + r.width / 2, y = r.top + r.height / 2; \
                          const hit = document.elementFromPoint(x, y); \
                          return hit && (hit === this || this.contains(hit)) ? [x, y] : null; }";
    let center = element
        .call_js_fn(CENTER, false)
        .await
        .map_err(js_error)?
        .result
        .value
        .and_then(|v| serde_json::from_value::<(f64, f64)>(v).ok());
    if let Some((x, y)) = center
        && page
            .click(chromiumoxide::layout::Point { x, y })
            .await
            .is_ok()
    {
        return Ok(());
    }
    element
        .call_js_fn("function() { this.click(); }", false)
        .await
        .map(|_| ())
        .map_err(js_error)
}

async fn signature(page: &Page) -> Option<String> {
    const SIGNATURE: &str = "(() => { const t = document.body ? document.body.innerText : ''; \
                             let h = 0; for (let i = 0; i < t.length; i++) h = (h * 31 + t.charCodeAt(i)) | 0; \
                             return [document.documentElement.scrollHeight, document.getElementsByTagName('*').length, t.length, h].join(); })()";
    page.evaluate_expression(SIGNATURE)
        .await
        .ok()
        .and_then(|r| r.into_value::<String>().ok())
}

async fn settle(
    page: &Page,
    requests: &Requests,
    before: Option<String>,
    patience: Duration,
    until_quiet: bool,
) -> bool {
    let start = Instant::now();
    let mut last = before.clone();
    let mut changed = false;
    let mut calm_since = start;
    loop {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let now = signature(page).await;
        let busy = requests.busy();
        if now != last {
            changed = true;
            last = now;
            calm_since = Instant::now();
            if !until_quiet && !busy {
                return true;
            }
        } else if busy {
            calm_since = Instant::now();
        }
        let calm = !busy && calm_since.elapsed() >= QUIET;
        if calm && (changed || start.elapsed() >= patience) {
            return changed;
        }
        if start.elapsed() >= SETTLE_LIMIT {
            return last != before;
        }
    }
}

fn same_site(url: &str, page_host: &str) -> bool {
    let site = page_host.trim_start_matches("www.");
    url::Url::parse(url)
        .ok()
        .and_then(|u| {
            u.host_str()
                .map(|h| h == site || h.ends_with(&format!(".{site}")))
        })
        .unwrap_or(false)
}

fn exception_text(e: &chromiumoxide::cdp::js_protocol::runtime::ExceptionDetails) -> String {
    e.exception
        .as_ref()
        .and_then(|x| x.description.clone())
        .unwrap_or_else(|| e.text.clone())
}

fn js_error(e: CdpError) -> String {
    match e {
        CdpError::JavascriptException(e) => exception_text(&e),
        CdpError::Timeout => "the browser did not answer within its command time limit, the render timeout plus 30 s when rendering through a pool and 30 s otherwise".into(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, parse_actions};
    use crate::render::{Prefetched, RenderOptions, Renderer};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn only_the_sites_own_requests_keep_a_step_waiting() {
        assert!(super::same_site(
            "https://www.npr.org/get/1001",
            "www.npr.org"
        ));
        assert!(super::same_site("https://api.github.com/x", "github.com"));
        assert!(!super::same_site(
            "https://ads.example.net/x",
            "www.npr.org"
        ));
        assert!(!super::same_site("https://evilnpr.org/x", "www.npr.org"));
    }

    #[test]
    fn actions_parse_from_json_and_reject_mistakes() {
        let actions = parse_actions(
            r##"[{"type": "click", "selector": ".more", "repeat": 5},
                {"type": "type", "selector": "#q", "value": "rust"},
                {"type": "press", "key": "Enter"},
                {"type": "scroll", "direction": "down", "amount": 800, "repeat": 3},
                {"type": "wait", "milliseconds": 200},
                {"type": "waitForSelector", "selector": ".done", "timeout": 5000},
                {"type": "executeJavascript", "code": "document.title"}]"##,
        )
        .unwrap();
        assert_eq!(actions.len(), 7);
        assert_eq!(
            actions[0],
            Action::Click {
                selector: ".more".into(),
                all: false,
                nth: None,
                repeat: Some(5)
            }
        );
        for bad in [
            r##"[{"type": "hover", "selector": "a"}]"##,
            r##"[{"type": "click", "selecter": "a"}]"##,
            r##"[{"type": "click"}]"##,
            r##"[{"type": "click", "selector": "a", "all": true, "nth": 1}]"##,
            r##"[{"type": "click", "selector": "a", "repeat": 2, "nth": 1}]"##,
            r##"[{"type": "scroll", "direction": "left"}]"##,
            r##"[{"type": "type", "selector": "a", "text": "x"}]"##,
            r##"{"type": "wait", "milliseconds": 1}"##,
        ] {
            assert!(parse_actions(bad).is_err(), "{bad}");
        }
        assert!(
            parse_actions(
                r##"[{"type": "wait"}, {"type": "click", "selector": "a", "all": true, "nth": 0}]"##
            )
            .is_err()
        );
        assert!(
            parse_actions(r##"[{"type": "wait", "milliseconds": 1}, {"type": "click", "selector": "a", "all": true, "nth": 0}]"##)
                .unwrap_err()
                .starts_with("action 2")
        );
    }

    const FEED: &str = r##"<html><body><div id="feed"></div><button id="more">More</button><input id="q"><p id="echo"></p><script>
let n = 0, pages = 1, clicks = 0;
const feed = document.getElementById("feed");
function add() { for (let i = 0; i < 5; i++) { const p = document.createElement("p"); p.className = "item"; p.style.height = "300px"; p.textContent = "item " + (++n); feed.appendChild(p); } }
add();
addEventListener("scroll", () => { if (pages < 4 && innerHeight + scrollY >= document.documentElement.scrollHeight - 10) { pages++; setTimeout(add, 300); } });
document.getElementById("more").addEventListener("click", () => { clicks++; setTimeout(() => { const p = document.createElement("p"); p.textContent = "more " + clicks; document.body.appendChild(p); if (clicks === 3) document.getElementById("more").style.display = "none"; }, 200); });
document.getElementById("q").addEventListener("keydown", e => { if (e.key === "Enter") document.getElementById("echo").textContent = "searched " + e.target.value; });
</script></body></html>"##;

    async fn render_feed(
        renderer: &Renderer,
        html: &str,
        actions: &str,
        timeout: Duration,
    ) -> Result<(crate::FetchResult, Vec<serde_json::Value>), crate::FetchError> {
        let headers = http::HeaderMap::new();
        renderer
            .render_actions(
                "https://example.com/feed",
                Some(Prefetched {
                    html,
                    headers: &headers,
                }),
                &RenderOptions {
                    timeout,
                    ..Default::default()
                },
                &parse_actions(actions).unwrap(),
            )
            .await
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn actions_scroll_click_type_and_return_eval_values() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let done = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "scroll", "repeat": 10},
                {"type": "click", "selector": "#more", "repeat": 10},
                {"type": "type", "selector": "#q", "value": "rust"},
                {"type": "press", "key": "Enter"},
                {"type": "executeJavascript", "code": "document.querySelectorAll('.item').length"},
                {"type": "executeJavascript", "code": "const x = 1; return x + 1;"},
                {"type": "executeJavascript", "code": "new Promise(r => setTimeout(() => r({ok: true}), 50))"}]"##,
            Duration::from_secs(30),
        )
        .await;
        let missing = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "wait", "milliseconds": 10}, {"type": "click", "selector": "#nope"}]"##,
            Duration::from_secs(30),
        )
        .await;
        let slow = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "wait", "milliseconds": 60000}, {"type": "executeJavascript", "code": "1"}]"##,
            Duration::from_secs(2),
        )
        .await;
        let thrown = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "executeJavascript", "code": "null.x"}]"##,
            Duration::from_secs(30),
        )
        .await;
        renderer.close().await;
        let (done, js) = done.expect("actions run");
        assert!(done.html.contains("item 20") && !done.html.contains("item 21"));
        assert!(done.html.contains("more 3") && !done.html.contains("more 4"));
        assert!(done.html.contains("searched rust"), "{}", done.html);
        assert_eq!(
            js,
            vec![
                serde_json::json!(20),
                serde_json::json!(2),
                serde_json::json!({"ok": true})
            ]
        );
        let missing = missing.unwrap_err().to_string();
        assert!(
            missing.contains("action 2 (click)") && missing.contains("#nope"),
            "{missing}"
        );
        let slow = slow.unwrap_err().to_string();
        assert!(
            slow.contains("action 2 (executeJavascript): ran out of time"),
            "{slow}"
        );
        let thrown = thrown.unwrap_err().to_string();
        assert!(
            thrown.contains("action 1 (executeJavascript)") && thrown.contains("TypeError"),
            "{thrown}"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn actions_cannot_reach_private_addresses() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let loopback = format!("http://{}", listener.local_addr().unwrap());
        let hits = Arc::new(AtomicUsize::new(0));
        let counted = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let counted = counted.clone();
                tokio::spawn(async move {
                    use tokio::io::AsyncReadExt;
                    let mut buffer = [0; 512];
                    let n = socket.read(&mut buffer).await.unwrap_or(0);
                    if n > 0 {
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });
        let page = format!(
            r##"<html><body><p>start</p><a id="go" href="{loopback}/nav">go</a><form id="f" method="post" action="{loopback}/post"><input name="a"></form></body></html>"##
        );
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let fetched = render_feed(
            &renderer,
            &page,
            &format!(
                r##"[{{"type": "executeJavascript", "code": "fetch('{loopback}/fetch').then(() => 'reached', () => 'blocked')"}},
                    {{"type": "executeJavascript", "code": "new Promise(r => {{ const x = new XMLHttpRequest(); x.onload = () => r('reached'); x.onerror = () => r('blocked'); x.open('GET', '{loopback}/xhr'); x.send(); }})"}},
                    {{"type": "executeJavascript", "code": "location.href = 'file:///etc/hosts'; 1"}},
                    {{"type": "wait", "milliseconds": 500}},
                    {{"type": "executeJavascript", "code": "document.body.innerText"}}]"##
            ),
            Duration::from_secs(20),
        )
        .await;
        let clicked = render_feed(
            &renderer,
            &page,
            r##"[{"type": "click", "selector": "#go"}]"##,
            Duration::from_secs(20),
        )
        .await;
        let submitted = render_feed(
            &renderer,
            &page,
            r##"[{"type": "executeJavascript", "code": "document.getElementById('f').submit(); 1"}, {"type": "wait", "milliseconds": 1000}]"##,
            Duration::from_secs(20),
        )
        .await;
        renderer.close().await;
        let (_, fetched) = fetched.expect("render succeeds");
        assert_eq!(
            fetched[..2],
            [serde_json::json!("blocked"), serde_json::json!("blocked")]
        );
        assert!(fetched[3].as_str().unwrap().contains("start"));
        assert!(clicked.is_err(), "{:?}", clicked.map(|(r, _)| r.url));
        assert!(submitted.is_err(), "{:?}", submitted.map(|(r, _)| r.url));
        assert_eq!(
            hits.load(Ordering::SeqCst),
            0,
            "an action sent a request to loopback"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn type_enters_text_outside_ascii() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let done = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "type", "selector": "#q", "value": "città 東京 😀"},
                {"type": "executeJavascript", "code": "document.getElementById('q').value"}]"##,
            Duration::from_secs(20),
        )
        .await;
        renderer.close().await;
        assert_eq!(
            done.expect("typed").1,
            vec![serde_json::json!("città 東京 😀")]
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn script_values_over_the_cap_fail_inside_the_page() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let big = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "executeJavascript", "code": "'x'.repeat(600000)"},
                {"type": "executeJavascript", "code": "'y'.repeat(600000)"}]"##,
            Duration::from_secs(20),
        )
        .await;
        renderer.close().await;
        let error = big.unwrap_err().to_string();
        assert!(
            error.contains("action 2 (executeJavascript)") && error.contains("characters left"),
            "{error}"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_script_may_run_longer_than_thirty_seconds_within_the_budget() {
        let pool = crate::render::RenderPool::new(
            crate::render::BrowserSource::Launch { proxy: None },
            Duration::from_secs(60),
        );
        let headers = http::HeaderMap::new();
        let done = pool
            .render_actions(
                "https://example.com/feed",
                Some(Prefetched {
                    html: FEED,
                    headers: &headers,
                }),
                None,
                &[],
                crate::Block::default(),
                &parse_actions(
                    r##"[{"type": "executeJavascript", "code": "new Promise(r => setTimeout(() => r('late'), 35000))"}]"##,
                )
                .unwrap(),
            )
            .await;
        pool.close().await;
        assert_eq!(
            done.expect("the script finishes").1,
            vec![serde_json::json!("late")]
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn dialogs_opened_by_a_step_are_dismissed() {
        let page = r#"<html><body><button id="b" onclick="alert('hi'); document.getElementById('out').textContent = confirm('sure?') ? 'yes' : 'no';">b</button><p id="out"></p></body></html>"#;
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let started = std::time::Instant::now();
        let done = render_feed(
            &renderer,
            page,
            r##"[{"type": "click", "selector": "#b"},
                {"type": "executeJavascript", "code": "document.getElementById('out').textContent"}]"##,
            Duration::from_secs(15),
        )
        .await;
        renderer.close().await;
        assert_eq!(
            done.expect("dialogs dismissed").1,
            vec![serde_json::json!("no")]
        );
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_huge_timeout_does_not_overflow() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let done = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "wait", "milliseconds": 18446744073709551615},
                {"type": "executeJavascript", "code": "1"}]"##,
            Duration::MAX,
        );
        let done = tokio::time::timeout(Duration::from_secs(5), done).await;
        renderer.close().await;
        assert!(done.is_err(), "the wait should still be running: {done:?}");
        let pool = crate::render::RenderPool::new(
            crate::render::BrowserSource::Launch { proxy: None },
            Duration::MAX,
        );
        let headers = http::HeaderMap::new();
        let pooled = pool
            .render_actions(
                "https://example.com/feed",
                Some(Prefetched {
                    html: FEED,
                    headers: &headers,
                }),
                None,
                &[],
                crate::Block::default(),
                &parse_actions(r##"[{"type": "executeJavascript", "code": "1"}]"##).unwrap(),
            )
            .await;
        pool.close().await;
        assert_eq!(
            pooled.expect("the pool renders").1,
            vec![serde_json::json!(1)]
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_bad_selector_fails_at_once() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let started = std::time::Instant::now();
        let bad = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "click", "selector": "a["}]"##,
            Duration::from_secs(20),
        )
        .await;
        let elapsed = started.elapsed();
        renderer.close().await;
        let error = bad.unwrap_err().to_string();
        assert!(error.contains("not a valid CSS selector"), "{error}");
        assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_click_that_changes_the_page_at_once_counts_as_a_change() {
        let page = r#"<html><body><button id="more" onclick="const p = document.createElement('p'); p.textContent = 'row ' + document.querySelectorAll('p').length; document.body.appendChild(p);">more</button></body></html>"#;
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let done = render_feed(
            &renderer,
            page,
            r##"[{"type": "click", "selector": "#more", "repeat": 5}]"##,
            Duration::from_secs(20),
        )
        .await;
        renderer.close().await;
        let html = done.expect("clicks run").0.html;
        assert!(html.contains("row 4") && !html.contains("row 5"), "{html}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn a_repeat_cut_short_by_the_budget_fails() {
        let page = r#"<html><body><button id="more" onclick="const p = document.createElement('p'); p.textContent = 'row ' + document.querySelectorAll('p').length; document.body.appendChild(p);">more</button></body></html>"#;
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let endless = render_feed(
            &renderer,
            page,
            r##"[{"type": "click", "selector": "#more", "repeat": 100000}]"##,
            Duration::from_secs(4),
        )
        .await;
        renderer.close().await;
        let error = endless.unwrap_err().to_string();
        assert!(
            error.contains("action 1 (click)") && error.contains("ran out of time"),
            "{error}"
        );
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn script_statements_give_the_value_of_the_last_one() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let done = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "executeJavascript", "code": "document.title = 'feed'; document.title;"},
                {"type": "executeJavascript", "code": "x = 1; 1"},
                {"type": "executeJavascript", "code": "const f = () => 2; new Promise(r => setTimeout(() => r(f()), 50))"},
                {"type": "executeJavascript", "code": "const o = {n: [1, 2]}; o"},
                {"type": "executeJavascript", "code": "const y = 3; return y;"},
                {"type": "executeJavascript", "code": "let u = 1; undefined"}]"##,
            Duration::from_secs(20),
        )
        .await;
        let big = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "executeJavascript", "code": "const big = {s: 'x'.repeat(2000000)}; big"}]"##,
            Duration::from_secs(20),
        )
        .await;
        renderer.close().await;
        assert_eq!(
            done.expect("scripts run").1,
            vec![
                serde_json::json!("feed"),
                serde_json::json!(1),
                serde_json::json!(2),
                serde_json::json!({"n": [1, 2]}),
                serde_json::json!(3),
                serde_json::Value::Null
            ]
        );
        let error = big.unwrap_err().to_string();
        assert!(error.contains("characters left"), "{error}");
    }

    #[tokio::test]
    #[ignore = "launches Chrome and needs public DNS"]
    async fn script_bodies_may_await_before_they_return() {
        let renderer = Renderer::launch(None).await.expect("Chrome launches");
        let done = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "executeJavascript", "code": "await new Promise(r => setTimeout(r, 10)); return 3"},
                {"type": "executeJavascript", "code": "const x = await Promise.resolve(5); return x"},
                {"type": "executeJavascript", "code": "await 0\nreturn 8"},
                {"type": "executeJavascript", "code": "return 1"},
                {"type": "executeJavascript", "code": "await Promise.resolve(2)"},
                {"type": "executeJavascript", "code": "return await Promise.resolve(4)"},
                {"type": "executeJavascript", "code": "if (location.href) { return 6 } await 0"},
                {"type": "executeJavascript", "code": "(async () => { await 0; return 7 })()"},
                {"type": "executeJavascript", "code": "const f = async () => { await 0; return 9 }; return f()"},
                {"type": "executeJavascript", "code": "const z = await Promise.resolve(10); z"},
                {"type": "executeJavascript", "code": "document.title = 'feed'; document.title;"},
                {"type": "executeJavascript", "code": "x = 1; 1"},
                {"type": "executeJavascript", "code": "let a = 11; a"},
                {"type": "executeJavascript", "code": "let a = 12; a"}]"##,
            Duration::from_secs(20),
        )
        .await;
        let big = render_feed(
            &renderer,
            FEED,
            r##"[{"type": "executeJavascript", "code": "const big = await Promise.resolve({s: 'x'.repeat(2000000)}); big"}]"##,
            Duration::from_secs(20),
        )
        .await;
        renderer.close().await;
        assert_eq!(
            done.expect("scripts run").1,
            [3, 5, 8, 1, 2, 4, 6, 7, 9, 10]
                .map(serde_json::Value::from)
                .into_iter()
                .chain([
                    serde_json::json!("feed"),
                    serde_json::json!(1),
                    serde_json::json!(11),
                    serde_json::json!(12),
                ])
                .collect::<Vec<_>>()
        );
        let error = big.unwrap_err().to_string();
        assert!(error.contains("characters left"), "{error}");
    }
}
