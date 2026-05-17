use std::collections::BTreeMap;

use axum::{Json, extract::Path};
use serde::Deserialize;
use serde_json::{Value, json};
use webclaw_capture::cdp::{CaptureOptions, capture_network as run_network_capture};
use webclaw_capture::openapi::write_openapi;
use webclaw_capture::replay::replay_endpoint as run_endpoint_replay;
use webclaw_capture::store::{find_endpoint, load_endpoints};
use webclaw_capture::types::{
    CaptureError, EndpointDefinition, HeaderMap, ReplayOptions, ReplayResult,
};

use crate::error::ApiError;

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct CaptureNetworkRequest {
    pub url: String,
    pub intent: Option<String>,
    pub wait_ms: Option<u64>,
    pub headed: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
pub struct ReplayEndpointRequest {
    pub endpoint_id: String,
    pub params_json: Option<Value>,
    pub dry_run: Option<bool>,
    pub confirm_unsafe: Option<bool>,
    pub headers: Option<BTreeMap<String, String>>,
    pub body_json: Option<Value>,
}

pub async fn capture_network(
    Json(request): Json<CaptureNetworkRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.url.trim().is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }

    let url = normalize_capture_url(&request.url)?;
    webclaw_fetch::url_security::validate_public_http_url(&url).await?;

    let saved = run_network_capture(CaptureOptions {
        url,
        intent: request.intent,
        wait_ms: request.wait_ms.unwrap_or(3000),
        headed: request.headed.unwrap_or(false),
    })
    .await
    .map_err(|error| capture_error("capture-network failed", error))?;

    Ok(Json(json!(saved)))
}

pub async fn endpoints(
    Path((domain, timestamp)): Path<(String, String)>,
) -> Result<Json<Vec<EndpointDefinition>>, ApiError> {
    let capture_id = capture_id_from_path(&domain, &timestamp)?;
    let endpoints = load_endpoints(&capture_id).map_err(|error| {
        capture_error(
            format!("could not load endpoints for capture id {capture_id}"),
            error,
        )
    })?;

    Ok(Json(endpoints))
}

pub async fn replay_endpoint(
    Json(request): Json<ReplayEndpointRequest>,
) -> Result<Json<ReplayResult>, ApiError> {
    if request.endpoint_id.trim().is_empty() {
        return Err(ApiError::bad_request("`endpoint_id` is required"));
    }

    let endpoint = find_endpoint(&request.endpoint_id).map_err(|error| {
        capture_error(
            format!("could not find endpoint id {}", request.endpoint_id),
            error,
        )
    })?;
    let options = replay_options_from_request(&endpoint, &request)?;
    let result = run_endpoint_replay(&endpoint, options)
        .await
        .map_err(|error| capture_error("replay-endpoint failed", error))?;

    Ok(Json(result))
}

pub async fn export_openapi(
    Path((domain, timestamp)): Path<(String, String)>,
) -> Result<Json<Value>, ApiError> {
    let capture_id = capture_id_from_path(&domain, &timestamp)?;
    let path = write_openapi(&capture_id).map_err(|error| {
        capture_error(
            format!("could not export OpenAPI for capture id {capture_id}"),
            error,
        )
    })?;

    Ok(Json(json!({ "path": path.display().to_string() })))
}

fn normalize_capture_url(url: &str) -> Result<String, ApiError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(ApiError::bad_request("`url` is required"));
    }

    let normalized = if let Some((scheme, _rest)) = trimmed.split_once("://") {
        if !matches!(scheme, "http" | "https") {
            return Err(ApiError::bad_request(format!(
                "capture-network only supports http and https URLs, got {scheme:?}"
            )));
        }
        trimmed.to_owned()
    } else {
        format!("https://{trimmed}")
    };

    Ok(normalized)
}

fn capture_id_from_path(domain: &str, timestamp: &str) -> Result<String, ApiError> {
    if !is_safe_capture_segment(domain) || !is_safe_capture_segment(timestamp) {
        return Err(ApiError::bad_request(
            "capture id contains an unsafe path segment",
        ));
    }

    Ok(format!("{domain}/{timestamp}"))
}

fn replay_options_from_request(
    endpoint: &EndpointDefinition,
    request: &ReplayEndpointRequest,
) -> Result<ReplayOptions, ApiError> {
    if let Some(value) = &request.params_json
        && !value.is_object()
    {
        return Err(ApiError::bad_request("`params_json` must be a JSON object"));
    }

    let confirm_unsafe = request.confirm_unsafe.unwrap_or(false);
    let default_dry_run = endpoint_defaults_to_dry_run(endpoint) && !confirm_unsafe;

    Ok(ReplayOptions {
        dry_run: request.dry_run.unwrap_or(false) || default_dry_run,
        confirm_unsafe,
        params_json: request.params_json.clone(),
        headers: header_map_from_strings(request.headers.as_ref()),
        body_json: request.body_json.clone(),
    })
}

fn endpoint_defaults_to_dry_run(endpoint: &EndpointDefinition) -> bool {
    endpoint.safety.requires_confirmation
        || !endpoint.safety.safe_to_replay
        || !matches!(
            endpoint.method.to_ascii_uppercase().as_str(),
            "GET" | "HEAD" | "OPTIONS"
        )
}

fn header_map_from_strings(headers: Option<&BTreeMap<String, String>>) -> HeaderMap {
    headers
        .into_iter()
        .flat_map(|headers| headers.iter())
        .map(|(name, value)| (name.clone(), Value::String(value.clone())))
        .collect()
}

fn is_safe_capture_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains(':')
        && !segment.contains('/')
        && !segment.contains('\\')
}

fn capture_error(context: impl Into<String>, error: CaptureError) -> ApiError {
    let context = context.into();
    match error {
        CaptureError::InvalidUrl(_) | CaptureError::Replay(_) | CaptureError::Storage(_) => {
            ApiError::bad_request(format!("{context}: {error}"))
        }
        CaptureError::EndpointNotFound(_) => ApiError::NotFound,
        CaptureError::Request(_) | CaptureError::Capture(_) => ApiError::Fetch(error.to_string()),
        CaptureError::Io(_) | CaptureError::Json(_) => ApiError::Internal(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;
    use webclaw_capture::types::{EndpointDefinition, EndpointSafety};

    use super::*;

    fn endpoint(
        method: &str,
        safe_to_replay: bool,
        requires_confirmation: bool,
    ) -> EndpointDefinition {
        EndpointDefinition {
            id: format!("{}_example", method.to_ascii_lowercase()),
            method: method.to_owned(),
            origin: "https://example.test".to_owned(),
            path_template: "/api/items".to_owned(),
            query_params: BTreeMap::new(),
            request_schema: None,
            response_schema: None,
            auth_evidence: Vec::new(),
            safety: EndpointSafety {
                safe_to_replay,
                requires_confirmation,
                reason: "test".to_owned(),
            },
            examples: Vec::new(),
        }
    }

    #[test]
    fn capture_id_from_path_joins_domain_timestamp_and_rejects_unsafe_segments() {
        assert_eq!(
            capture_id_from_path("example.test", "2026-05-16T12-00-00Z").unwrap(),
            "example.test/2026-05-16T12-00-00Z"
        );

        assert!(capture_id_from_path("..", "2026-05-16T12-00-00Z").is_err());
        assert!(capture_id_from_path("example.test", "..").is_err());
    }

    #[test]
    fn replay_request_defaults_unsafe_methods_to_dry_run_unless_confirmed() {
        let unsafe_endpoint = endpoint("POST", false, true);
        let request = ReplayEndpointRequest {
            endpoint_id: unsafe_endpoint.id.clone(),
            params_json: Some(json!({"id": "123"})),
            dry_run: None,
            confirm_unsafe: None,
            headers: Some(BTreeMap::from([("X-Test".to_owned(), "ok".to_owned())])),
            body_json: Some(json!({"name": "tool"})),
        };

        let options = replay_options_from_request(&unsafe_endpoint, &request).unwrap();
        assert!(options.dry_run);
        assert!(!options.confirm_unsafe);
        assert_eq!(options.params_json, Some(json!({"id": "123"})));
        assert_eq!(options.headers.get("X-Test"), Some(&json!("ok")));
        assert_eq!(options.body_json, Some(json!({"name": "tool"})));

        let confirmed = ReplayEndpointRequest {
            confirm_unsafe: Some(true),
            ..request
        };
        let options = replay_options_from_request(&unsafe_endpoint, &confirmed).unwrap();
        assert!(!options.dry_run);
        assert!(options.confirm_unsafe);
    }

    #[test]
    fn replay_request_rejects_non_object_params_json() {
        let safe_endpoint = endpoint("GET", true, false);
        let request = ReplayEndpointRequest {
            endpoint_id: safe_endpoint.id.clone(),
            params_json: Some(json!(["not", "an", "object"])),
            dry_run: None,
            confirm_unsafe: None,
            headers: None,
            body_json: None,
        };

        let error = replay_options_from_request(&safe_endpoint, &request).unwrap_err();
        assert!(error.to_string().contains("params_json"));
    }
}
