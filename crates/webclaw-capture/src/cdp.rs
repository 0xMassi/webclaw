use std::collections::HashMap;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::network::{
    EnableParams, EventLoadingFinished, EventRequestWillBeSent, EventResponseReceived,
    GetResponseBodyParams, Headers, RequestId, ResourceType, TimeSinceEpoch,
};
use chromiumoxide::{Browser, BrowserConfig, Page};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use tokio::sync::oneshot;
use url::Url;

use crate::infer::infer_endpoints;
use crate::store::{capture_id_for, save_capture};
use crate::types::{CaptureArtifact, CaptureError, CapturedExchange, HeaderMap, SavedCapture};

const BODY_SAMPLE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaptureOptions {
    pub url: String,
    pub intent: Option<String>,
    pub wait_ms: u64,
    pub headed: bool,
}

pub async fn capture_network(options: CaptureOptions) -> Result<SavedCapture, CaptureError> {
    let source_url =
        Url::parse(&options.url).map_err(|error| CaptureError::InvalidUrl(error.to_string()))?;
    let started_at = Utc::now();
    let capture_id = capture_id_for(&source_url, started_at);

    let (mut browser, mut handler) = launch_browser(options.headed).await?;
    let handler_task = tokio::spawn(async move {
        while let Some(event) = handler.next().await {
            if let Err(error) = event {
                tracing::debug!(error = %error, "chromiumoxide browser handler stopped");
                break;
            }
        }
    });

    let capture_result = async {
        let page = browser
            .new_page("about:blank")
            .await
            .map_err(|error| CaptureError::Capture(format!("could not create page: {error}")))?;

        enable_network_capture(&page).await?;
        let request_events = page
            .event_listener::<EventRequestWillBeSent>()
            .await
            .map_err(|error| {
                CaptureError::Capture(format!("could not listen for network requests: {error}"))
            })?;
        let response_events = page
            .event_listener::<EventResponseReceived>()
            .await
            .map_err(|error| {
                CaptureError::Capture(format!("could not listen for network responses: {error}"))
            })?;
        let finished_events = page
            .event_listener::<EventLoadingFinished>()
            .await
            .map_err(|error| {
                CaptureError::Capture(format!("could not listen for completed requests: {error}"))
            })?;

        let (stop_tx, stop_rx) = oneshot::channel();
        let collector_page = page.clone();
        let collector_task = tokio::spawn(async move {
            collect_exchanges(
                collector_page,
                request_events,
                response_events,
                finished_events,
                stop_rx,
                started_at,
            )
            .await
        });

        page.goto(options.url.clone()).await.map_err(|error| {
            CaptureError::Capture(format!("could not navigate to {}: {error}", options.url))
        })?;

        tokio::time::sleep(Duration::from_millis(options.wait_ms)).await;
        let _ = stop_tx.send(());

        let exchanges = collector_task
            .await
            .map_err(|error| CaptureError::Capture(format!("capture collector failed: {error}")))?
            .map_err(|error| CaptureError::Capture(format!("capture collector failed: {error}")))?;
        let completed_at = Utc::now();
        let endpoints = infer_endpoints(&exchanges);
        let exchange_count = exchanges.len();
        let endpoint_count = endpoints.len();

        let mut metadata = Map::new();
        metadata.insert("wait_ms".to_owned(), json!(options.wait_ms));
        metadata.insert("headed".to_owned(), json!(options.headed));
        metadata.insert("exchange_count".to_owned(), json!(exchange_count));
        metadata.insert("endpoint_count".to_owned(), json!(endpoint_count));

        let artifact = CaptureArtifact {
            id: capture_id,
            source_url: options.url,
            intent: options.intent,
            started_at,
            completed_at: Some(completed_at),
            exchanges,
            endpoints,
            metadata,
        };

        save_capture(&artifact)
    }
    .await;

    if let Err(error) = browser.close().await {
        tracing::debug!(error = %error, "failed to close browser after capture");
    }
    if let Err(error) = handler_task.await {
        tracing::debug!(error = %error, "failed to join browser handler after capture");
    }

    capture_result
}

async fn launch_browser(headed: bool) -> Result<(Browser, chromiumoxide::Handler), CaptureError> {
    let mut config = BrowserConfig::builder()
        .request_timeout(Duration::from_secs(15))
        .no_sandbox()
        .disable_cache()
        .disable_https_first();

    if headed {
        config = config.with_head();
    }

    let config = config.build().map_err(|error| {
        CaptureError::Capture(format!("could not build browser config: {error}"))
    })?;

    Browser::launch(config)
        .await
        .map_err(|error| CaptureError::Capture(format!("could not launch Chromium: {error}")))
}

async fn enable_network_capture(page: &Page) -> Result<(), CaptureError> {
    let params = EnableParams::builder()
        .max_total_buffer_size(16 * 1024 * 1024)
        .max_resource_buffer_size(2 * 1024 * 1024)
        .max_post_data_size(BODY_SAMPLE_LIMIT as i64)
        .build();

    page.execute(params).await.map_err(|error| {
        CaptureError::Capture(format!("could not enable CDP network capture: {error}"))
    })?;

    Ok(())
}

async fn collect_exchanges(
    page: Page,
    mut request_events: chromiumoxide::listeners::EventStream<EventRequestWillBeSent>,
    mut response_events: chromiumoxide::listeners::EventStream<EventResponseReceived>,
    mut finished_events: chromiumoxide::listeners::EventStream<EventLoadingFinished>,
    mut stop_rx: oneshot::Receiver<()>,
    fallback_started_at: DateTime<Utc>,
) -> Result<Vec<CapturedExchange>, CaptureError> {
    let mut pending = HashMap::<RequestId, PendingExchange>::new();
    let mut exchanges = Vec::<CapturedExchange>::new();

    loop {
        tokio::select! {
            _ = &mut stop_rx => break,
            event = request_events.next() => {
                if let Some(event) = event {
                    record_request(&mut pending, &event, fallback_started_at);
                }
            }
            event = response_events.next() => {
                if let Some(event) = event {
                    record_response(&mut pending, &event);
                }
            }
            event = finished_events.next() => {
                if let Some(event) = event
                    && let Some(exchange) = finish_request(&page, &mut pending, &event).await?
                {
                    exchanges.push(exchange);
                }
            }
        }
    }

    for (_request_id, pending_exchange) in pending {
        if let Some(exchange) = pending_exchange.into_exchange() {
            exchanges.push(exchange);
        }
    }

    exchanges.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.url.cmp(&right.url))
    });

    Ok(exchanges)
}

fn record_request(
    pending: &mut HashMap<RequestId, PendingExchange>,
    event: &EventRequestWillBeSent,
    fallback_started_at: DateTime<Utc>,
) {
    let request_id = event.request_id.clone();
    let mut current = pending.remove(&request_id).unwrap_or_default();

    if let Some(redirect_response) = &event.redirect_response {
        if !current.url.is_empty() {
            current.redirect_chain.push(current.url.clone());
        }
        current.redirect_chain.push(redirect_response.url.clone());
    }

    current.method = event.request.method.clone();
    current.url = event.request.url.clone();
    current.request_headers = headers_to_map(&event.request.headers);
    current.request_body_sample = request_body_sample(event);
    current.resource_type = event.r#type.as_ref().map(resource_type_name);
    current.started_at = wall_time_to_utc(&event.wall_time, fallback_started_at);
    current.started_monotonic = Some(*event.timestamp.inner());

    pending.insert(request_id, current);
}

fn record_response(
    pending: &mut HashMap<RequestId, PendingExchange>,
    event: &EventResponseReceived,
) {
    let current = pending.entry(event.request_id.clone()).or_default();

    if current.url.is_empty() {
        current.url = event.response.url.clone();
    }
    current.status = u16::try_from(event.response.status).unwrap_or_default();
    current.response_headers = headers_to_map(&event.response.headers);
    current.response_mime_type = Some(event.response.mime_type.clone());
    current.resource_type = Some(resource_type_name(&event.r#type));
}

async fn finish_request(
    page: &Page,
    pending: &mut HashMap<RequestId, PendingExchange>,
    event: &EventLoadingFinished,
) -> Result<Option<CapturedExchange>, CaptureError> {
    let Some(mut current) = pending.remove(&event.request_id) else {
        return Ok(None);
    };

    if let Some(started) = current.started_monotonic {
        let elapsed = ((*event.timestamp.inner() - started) * 1_000.0).max(0.0);
        current.duration_ms = elapsed.round() as u64;
    }

    current.response_body_sample = response_body_sample(page, event.request_id.clone()).await;

    Ok(current.into_exchange())
}

async fn response_body_sample(page: &Page, request_id: RequestId) -> Option<String> {
    let response = page
        .execute(GetResponseBodyParams::new(request_id))
        .await
        .ok()?;
    Some(truncate_sample(response.result.body))
}

fn headers_to_map(headers: &Headers) -> HeaderMap {
    match headers.inner() {
        Value::Object(headers) => headers.clone(),
        _ => HeaderMap::new(),
    }
}

fn request_body_sample(event: &EventRequestWillBeSent) -> Option<String> {
    let entries = event.request.post_data_entries.as_ref()?;
    let mut body = String::new();

    for entry in entries {
        if let Some(bytes) = &entry.bytes {
            body.push_str(bytes.as_ref());
        }
    }

    if body.is_empty() {
        None
    } else {
        Some(truncate_sample(body))
    }
}

fn resource_type_name(resource_type: &ResourceType) -> String {
    resource_type.as_ref().to_owned()
}

fn wall_time_to_utc(wall_time: &TimeSinceEpoch, fallback: DateTime<Utc>) -> DateTime<Utc> {
    let seconds = *wall_time.inner();
    if !seconds.is_finite() || seconds < 0.0 {
        return fallback;
    }

    let whole_seconds = seconds.trunc() as i64;
    let nanos = ((seconds.fract() * 1_000_000_000.0).round() as u32).min(999_999_999);

    DateTime::<Utc>::from_timestamp(whole_seconds, nanos).unwrap_or(fallback)
}

fn truncate_sample(sample: String) -> String {
    if sample.len() <= BODY_SAMPLE_LIMIT {
        return sample;
    }

    let end = sample
        .char_indices()
        .take_while(|(index, _)| *index <= BODY_SAMPLE_LIMIT)
        .map(|(index, character)| index + character.len_utf8())
        .last()
        .unwrap_or(0)
        .min(sample.len());

    sample[..end].to_owned()
}

#[derive(Debug, Clone)]
struct PendingExchange {
    method: String,
    url: String,
    request_headers: HeaderMap,
    request_body_sample: Option<String>,
    resource_type: Option<String>,
    status: u16,
    response_headers: HeaderMap,
    response_body_sample: Option<String>,
    response_mime_type: Option<String>,
    started_at: DateTime<Utc>,
    started_monotonic: Option<f64>,
    duration_ms: u64,
    redirect_chain: Vec<String>,
}

impl Default for PendingExchange {
    fn default() -> Self {
        Self {
            method: String::new(),
            url: String::new(),
            request_headers: HeaderMap::new(),
            request_body_sample: None,
            resource_type: None,
            status: 0,
            response_headers: HeaderMap::new(),
            response_body_sample: None,
            response_mime_type: None,
            started_at: Utc::now(),
            started_monotonic: None,
            duration_ms: 0,
            redirect_chain: Vec::new(),
        }
    }
}

impl PendingExchange {
    fn into_exchange(mut self) -> Option<CapturedExchange> {
        if self.method.is_empty() || self.url.is_empty() {
            return None;
        }

        if !self.response_headers.contains_key("content-type")
            && let Some(mime_type) = self.response_mime_type.take()
        {
            self.response_headers
                .insert("content-type".to_owned(), Value::String(mime_type));
        }

        Some(CapturedExchange {
            method: self.method,
            url: self.url,
            request_headers: self.request_headers,
            request_body_sample: self.request_body_sample,
            resource_type: self.resource_type,
            status: self.status,
            response_headers: self.response_headers,
            response_body_sample: self.response_body_sample,
            started_at: self.started_at,
            duration_ms: self.duration_ms,
            redirect_chain: self.redirect_chain,
        })
    }
}
