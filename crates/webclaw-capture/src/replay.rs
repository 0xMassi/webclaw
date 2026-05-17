use std::collections::BTreeSet;

use reqwest::{
    Client, Method, RequestBuilder,
    header::{HeaderName, HeaderValue},
};
use serde_json::{Map, Value};
use url::{Url, form_urlencoded::byte_serialize};

use crate::types::{CaptureError, EndpointDefinition, HeaderMap, ReplayOptions, ReplayResult};

const MAX_BODY_SAMPLE_BYTES: usize = 64 * 1024;

pub async fn replay_endpoint(
    endpoint: &EndpointDefinition,
    options: ReplayOptions,
) -> Result<ReplayResult, CaptureError> {
    if unsafe_replay_requires_confirmation(endpoint, &options) {
        return Ok(ReplayResult::Blocked {
            reason: format!(
                "{} replay requires --confirm-unsafe unless --dry-run is used",
                endpoint.method.to_ascii_uppercase()
            ),
        });
    }

    let spec = replay_spec(endpoint, &options)?;
    if options.dry_run {
        return Ok(ReplayResult::Preview {
            method: spec.method.as_str().to_owned(),
            url: spec.url.to_string(),
            headers: spec.headers,
            body_sample: spec.body_sample,
        });
    }

    let response = request_builder_from_spec(spec)?.send().await?;
    let status = response.status().as_u16();
    let headers = response_headers_to_json(response.headers());
    let body = response.bytes().await?;
    let body_sample = body_sample_from_bytes(&body);

    Ok(ReplayResult::Executed {
        status,
        headers,
        body_sample,
    })
}

pub fn build_replay_request(
    endpoint: &EndpointDefinition,
    options: &ReplayOptions,
) -> Result<RequestBuilder, CaptureError> {
    if unsafe_replay_requires_confirmation(endpoint, options) {
        return Err(CaptureError::Replay(format!(
            "{} replay requires confirmation",
            endpoint.method.to_ascii_uppercase()
        )));
    }

    request_builder_from_spec(replay_spec(endpoint, options)?)
}

#[derive(Debug, Clone)]
struct ReplaySpec {
    method: Method,
    url: Url,
    headers: HeaderMap,
    body_sample: Option<String>,
}

fn replay_spec(
    endpoint: &EndpointDefinition,
    options: &ReplayOptions,
) -> Result<ReplaySpec, CaptureError> {
    let method = Method::from_bytes(endpoint.method.as_bytes()).map_err(|error| {
        CaptureError::Replay(format!(
            "invalid replay method {:?}: {error}",
            endpoint.method
        ))
    })?;

    let (path, consumed_params) = interpolate_path_template(&endpoint.path_template, options)?;
    let mut url = Url::parse(&format!(
        "{}{}",
        endpoint.origin.trim_end_matches('/'),
        ensure_leading_slash(&path)
    ))
    .map_err(|error| CaptureError::InvalidUrl(error.to_string()))?;

    apply_query_params(&mut url, endpoint, options, &consumed_params);

    let mut headers = HeaderMap::new();
    if let Some(example) = endpoint.examples.first() {
        merge_safe_headers(&mut headers, &example.request_headers);
    }
    merge_safe_headers(&mut headers, &options.headers);

    let body_sample = replay_body_sample(endpoint, options)?;

    Ok(ReplaySpec {
        method,
        url,
        headers,
        body_sample,
    })
}

fn request_builder_from_spec(spec: ReplaySpec) -> Result<RequestBuilder, CaptureError> {
    let client = Client::new();
    let mut builder = client.request(spec.method, spec.url);

    for (name, value) in spec.headers {
        let Some(value) = header_value_to_string(&value) else {
            continue;
        };

        let Ok(name) = HeaderName::from_bytes(name.as_bytes()) else {
            continue;
        };
        let Ok(value) = HeaderValue::from_str(&value) else {
            continue;
        };

        builder = builder.header(name, value);
    }

    if let Some(body_sample) = spec.body_sample
        && !contains_redacted_material(&body_sample)
    {
        builder = builder.body(body_sample);
    }

    Ok(builder)
}

fn unsafe_replay_requires_confirmation(
    endpoint: &EndpointDefinition,
    options: &ReplayOptions,
) -> bool {
    is_unsafe_endpoint(endpoint) && !options.dry_run && !options.confirm_unsafe
}

fn is_unsafe_endpoint(endpoint: &EndpointDefinition) -> bool {
    endpoint.safety.requires_confirmation
        || !endpoint.safety.safe_to_replay
        || !matches!(
            endpoint.method.to_ascii_uppercase().as_str(),
            "GET" | "HEAD" | "OPTIONS"
        )
}

fn interpolate_path_template(
    path_template: &str,
    options: &ReplayOptions,
) -> Result<(String, BTreeSet<String>), CaptureError> {
    let params = params_object(options);
    let mut consumed = BTreeSet::new();
    let mut path = String::new();
    let mut rest = path_template;

    while let Some(start) = rest.find('{') {
        let (before, after_start) = rest.split_at(start);
        path.push_str(before);

        let Some(end) = after_start.find('}') else {
            path.push_str(after_start);
            return Ok((path, consumed));
        };

        let name = &after_start[1..end];
        if let Some(value) = params.and_then(|object| object.get(name)) {
            let value = scalar_param_to_string(value).ok_or_else(|| {
                CaptureError::Replay(format!("path parameter {name:?} must be scalar"))
            })?;
            path.push_str(&encode_path_segment(&value));
            consumed.insert(name.to_owned());
        } else {
            path.push_str(&after_start[..=end]);
        }

        rest = &after_start[end + 1..];
    }

    path.push_str(rest);
    Ok((path, consumed))
}

fn apply_query_params(
    url: &mut Url,
    endpoint: &EndpointDefinition,
    options: &ReplayOptions,
    consumed_params: &BTreeSet<String>,
) {
    url.set_query(None);
    let mut pairs = Vec::<(String, String)>::new();

    for (name, values) in &endpoint.query_params {
        if consumed_params.contains(name) || is_sensitive_name(name) {
            continue;
        }

        if let Some(value) = values
            .iter()
            .find(|value| !contains_redacted_material(value))
            .cloned()
        {
            pairs.push((name.clone(), value));
        }
    }

    if let Some(params) = params_object(options) {
        for (name, value) in params {
            if consumed_params.contains(name) || is_sensitive_name(name) {
                continue;
            }

            append_query_value(&mut pairs, name, value);
        }
    }

    if pairs.is_empty() {
        return;
    }

    let mut query = url.query_pairs_mut();
    for (name, value) in pairs {
        query.append_pair(&name, &value);
    }
}

fn append_query_value(pairs: &mut Vec<(String, String)>, name: &str, value: &Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                if let Some(value) = scalar_param_to_string(value)
                    && !contains_redacted_material(&value)
                {
                    pairs.push((name.to_owned(), value));
                }
            }
        }
        _ => {
            if let Some(value) = scalar_param_to_string(value)
                && !contains_redacted_material(&value)
            {
                pairs.retain(|(existing, _value)| existing != name);
                pairs.push((name.to_owned(), value));
            }
        }
    }
}

fn replay_body_sample(
    endpoint: &EndpointDefinition,
    options: &ReplayOptions,
) -> Result<Option<String>, CaptureError> {
    if let Some(body_json) = &options.body_json {
        return Ok(Some(serde_json::to_string(body_json)?));
    }

    let Some(example) = endpoint.examples.first() else {
        return Ok(None);
    };

    Ok(example
        .request_body_sample
        .as_ref()
        .filter(|sample| !contains_redacted_material(sample))
        .cloned())
}

fn merge_safe_headers(target: &mut HeaderMap, headers: &HeaderMap) {
    for (name, value) in headers {
        if should_skip_header(name, value) {
            continue;
        }

        target.insert(name.clone(), value.clone());
    }
}

fn should_skip_header(name: &str, value: &Value) -> bool {
    is_hop_by_hop_header(name)
        || header_value_to_string(value)
            .map(|value| value.trim().is_empty() || contains_redacted_material(&value))
            .unwrap_or(true)
}

fn is_hop_by_hop_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "host" | "connection" | "content-length" | "transfer-encoding" | "accept-encoding"
    )
}

fn header_value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn response_headers_to_json(headers: &reqwest::header::HeaderMap) -> HeaderMap {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), Value::String(value.to_owned())))
        })
        .collect()
}

fn body_sample_from_bytes(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }

    let capped = &bytes[..bytes.len().min(MAX_BODY_SAMPLE_BYTES)];
    Some(String::from_utf8_lossy(capped).into_owned())
}

fn params_object(options: &ReplayOptions) -> Option<&Map<String, Value>> {
    options.params_json.as_ref()?.as_object()
}

fn scalar_param_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

fn contains_redacted_material(value: &str) -> bool {
    value.to_ascii_lowercase().contains("[redacted]")
}

fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect();

    [
        "authorization",
        "cookie",
        "set-cookie",
        "api-key",
        "csrf",
        "token",
        "session",
        "password",
        "email",
    ]
    .iter()
    .any(|sensitive| {
        let sensitive_compact: String = sensitive
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect();

        lower.contains(sensitive) || compact.contains(&sensitive_compact)
    })
}

fn encode_path_segment(value: &str) -> String {
    byte_serialize(value.as_bytes()).collect()
}

fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}
