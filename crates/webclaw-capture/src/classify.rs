use serde::{Deserialize, Serialize};
use serde_json::Value;
use url::Url;

use crate::types::CapturedExchange;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ApiClassification {
    pub include: bool,
    pub confidence: f32,
    pub reasons: Vec<String>,
}

pub fn classify_exchange(exchange: &CapturedExchange) -> ApiClassification {
    let url = match Url::parse(&exchange.url) {
        Ok(url) => url,
        Err(error) => {
            return ApiClassification {
                include: false,
                confidence: 0.0,
                reasons: vec![format!("invalid URL: {error}")],
            };
        }
    };

    let mut exclusion_reasons = Vec::new();

    if is_browser_extension_url(&url) {
        exclusion_reasons.push("browser extension URL".to_owned());
    }

    if is_tracking_host(url.host_str()) {
        exclusion_reasons.push("tracking, ad, or telemetry host".to_owned());
    }

    if has_static_asset_extension(url.path()) {
        exclusion_reasons.push("static asset extension".to_owned());
    }

    if is_static_resource_type(exchange.resource_type.as_deref()) {
        exclusion_reasons.push("static browser resource type".to_owned());
    }

    if !exclusion_reasons.is_empty() {
        return ApiClassification {
            include: false,
            confidence: 0.0,
            reasons: exclusion_reasons,
        };
    }

    let mut confidence = 0.0_f32;
    let mut reasons = Vec::new();

    if matches_resource_type(exchange.resource_type.as_deref(), &["fetch", "xhr"]) {
        confidence += 0.65;
        reasons.push("browser resource type is fetch/xhr".to_owned());
    }

    if response_is_json(exchange) {
        confidence += 0.55;
        reasons.push("response content type is JSON".to_owned());
    }

    let path = url.path();

    if has_api_path(path) {
        confidence += 0.55;
        reasons.push("URL path contains an API prefix".to_owned());
    }

    if has_versioned_path(path) {
        confidence += 0.55;
        reasons.push("URL path starts with a versioned API prefix".to_owned());
    }

    if has_graphql_path(path) {
        confidence += 0.55;
        reasons.push("URL path is GraphQL-like".to_owned());
    }

    if has_graphql_body(exchange.request_body_sample.as_deref()) {
        confidence += 0.55;
        reasons.push("request body is GraphQL-like".to_owned());
    }

    let confidence = confidence.min(1.0);

    if reasons.is_empty() {
        reasons.push("no API traffic signals found".to_owned());
    }

    ApiClassification {
        include: confidence >= 0.5,
        confidence,
        reasons,
    }
}

pub fn filter_api_exchanges(exchanges: &[CapturedExchange]) -> Vec<CapturedExchange> {
    exchanges
        .iter()
        .filter(|exchange| classify_exchange(exchange).include)
        .cloned()
        .collect()
}

fn is_browser_extension_url(url: &Url) -> bool {
    matches!(
        url.scheme().to_ascii_lowercase().as_str(),
        "chrome-extension" | "moz-extension" | "edge-extension" | "safari-extension"
    )
}

fn is_tracking_host(host: Option<&str>) -> bool {
    let Some(host) = host else {
        return false;
    };
    let host = host.to_ascii_lowercase();

    [
        "google-analytics",
        "googletagmanager",
        "googlesyndication",
        "doubleclick",
        "adservice",
        "ads.",
        ".ads.",
        "analytics.",
        ".analytics.",
        "telemetry",
        "segment.",
        "segment.io",
        "amplitude",
        "mixpanel",
        "hotjar",
        "sentry.io",
        "datadog",
        "newrelic",
    ]
    .iter()
    .any(|needle| host.contains(needle))
}

fn has_static_asset_extension(path: &str) -> bool {
    let path = path.to_ascii_lowercase();

    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".avif", ".svg", ".ico", ".css", ".js", ".mjs",
        ".woff", ".woff2", ".ttf", ".otf", ".eot", ".map", ".mp4", ".webm", ".mp3", ".wav",
    ]
    .iter()
    .any(|extension| path.ends_with(extension))
}

fn is_static_resource_type(resource_type: Option<&str>) -> bool {
    matches_resource_type(
        resource_type,
        &[
            "image",
            "stylesheet",
            "script",
            "font",
            "media",
            "manifest",
            "ping",
            "cspviolationreport",
        ],
    )
}

fn matches_resource_type(resource_type: Option<&str>, candidates: &[&str]) -> bool {
    let Some(resource_type) = resource_type else {
        return false;
    };
    candidates
        .iter()
        .any(|candidate| resource_type.eq_ignore_ascii_case(candidate))
}

fn response_is_json(exchange: &CapturedExchange) -> bool {
    exchange.response_headers.iter().any(|(name, value)| {
        name.eq_ignore_ascii_case("content-type")
            && header_value_as_str(value)
                .map(|value| value.to_ascii_lowercase().contains("json"))
                .unwrap_or(false)
    })
}

fn header_value_as_str(value: &Value) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        _ => None,
    }
}

fn has_api_path(path: &str) -> bool {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .any(|segment| segment.eq_ignore_ascii_case("api"))
}

fn has_versioned_path(path: &str) -> bool {
    path.split('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| {
            let segment = segment.to_ascii_lowercase();
            segment.len() > 1
                && segment.starts_with('v')
                && segment[1..]
                    .chars()
                    .all(|character| character.is_ascii_digit())
        })
        .unwrap_or(false)
}

fn has_graphql_path(path: &str) -> bool {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .any(|segment| segment.eq_ignore_ascii_case("graphql"))
}

fn has_graphql_body(body: Option<&str>) -> bool {
    let Some(body) = body else {
        return false;
    };

    if let Ok(value) = serde_json::from_str::<Value>(body) {
        return value
            .as_object()
            .map(|object| {
                object.contains_key("operationName")
                    || object
                        .get("query")
                        .and_then(Value::as_str)
                        .map(is_graphql_query_text)
                        .unwrap_or(false)
            })
            .unwrap_or(false);
    }

    is_graphql_query_text(body)
}

fn is_graphql_query_text(text: &str) -> bool {
    let text = text.trim_start();
    text.starts_with("query ")
        || text.starts_with("query{")
        || text.starts_with("mutation ")
        || text.starts_with("mutation{")
        || text.starts_with("subscription ")
        || text.starts_with("subscription{")
}
