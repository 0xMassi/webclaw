use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::{Map, Value, json};
use url::Url;

use crate::redact::{redact_headers, redact_json};
use crate::store::{capture_root, load_endpoints};
use crate::types::{CaptureError, EndpointDefinition, EndpointExample};

const OPENAPI_FILE: &str = "openapi.json";
const REDACTED: &str = "[REDACTED]";

pub fn export_openapi(endpoints: &[EndpointDefinition]) -> Value {
    let mut paths = Map::new();

    for endpoint in endpoints {
        let path = normalize_openapi_path(&endpoint.path_template);
        let method = endpoint.method.to_ascii_lowercase();
        let operation = operation_for(endpoint);

        let path_item = paths
            .entry(path)
            .or_insert_with(|| Value::Object(Map::new()));
        if let Value::Object(path_item) = path_item {
            path_item.insert(method, operation);
        }
    }

    json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Webclaw Learned API",
            "version": "1.0.0"
        },
        "paths": paths
    })
}

pub fn write_openapi(capture_id: &str) -> Result<PathBuf, CaptureError> {
    let endpoints = load_endpoints(capture_id)?;
    let document = export_openapi(&endpoints);
    let capture_dir = capture_dir_for_id(&capture_root(), capture_id)?;
    fs::create_dir_all(&capture_dir)?;

    let path = capture_dir.join(OPENAPI_FILE);
    fs::write(&path, serde_json::to_string_pretty(&document)?)?;

    Ok(path)
}

fn operation_for(endpoint: &EndpointDefinition) -> Value {
    let mut operation = Map::new();
    let method = endpoint.method.to_ascii_uppercase();

    operation.insert(
        "operationId".to_owned(),
        Value::String(operation_id(endpoint)),
    );
    operation.insert(
        "summary".to_owned(),
        Value::String(format!("{method} {}", endpoint.path_template)),
    );
    operation.insert(
        "x-webclaw-endpoint-id".to_owned(),
        Value::String(endpoint.id.clone()),
    );
    operation.insert(
        "x-webclaw-origin".to_owned(),
        Value::String(endpoint.origin.clone()),
    );

    if !endpoint.auth_evidence.is_empty() {
        operation.insert(
            "x-webclaw-auth-evidence".to_owned(),
            json!(endpoint.auth_evidence),
        );
    }

    if endpoint.safety.requires_confirmation || !endpoint.safety.safe_to_replay {
        operation.insert("x-webclaw-requires-confirmation".to_owned(), json!(true));
    }

    let parameters = parameters_for(endpoint);
    if !parameters.is_empty() {
        operation.insert("parameters".to_owned(), Value::Array(parameters));
    }

    if let Some(request_body) = request_body_for(endpoint) {
        operation.insert("requestBody".to_owned(), request_body);
    }

    operation.insert("responses".to_owned(), responses_for(endpoint));

    let examples = examples_for(endpoint);
    if !examples.is_empty() {
        operation.insert("x-webclaw-examples".to_owned(), Value::Array(examples));
    }

    Value::Object(operation)
}

fn parameters_for(endpoint: &EndpointDefinition) -> Vec<Value> {
    let mut parameters = path_parameters(&endpoint.path_template);

    for (name, values) in &endpoint.query_params {
        let examples = examples_object(
            values
                .iter()
                .map(|value| Value::String(redacted_parameter_value(name, value))),
        );
        let mut parameter = Map::new();

        parameter.insert("name".to_owned(), Value::String(name.clone()));
        parameter.insert("in".to_owned(), Value::String("query".to_owned()));
        parameter.insert("required".to_owned(), Value::Bool(false));
        parameter.insert("schema".to_owned(), json!({ "type": "string" }));

        if !examples.is_empty() {
            parameter.insert("examples".to_owned(), Value::Object(examples));
        }

        parameters.push(Value::Object(parameter));
    }

    parameters
}

fn path_parameters(path_template: &str) -> Vec<Value> {
    let mut parameters = Vec::new();
    let mut cursor = path_template;

    while let Some(start) = cursor.find('{') {
        let after_start = &cursor[start + 1..];
        let Some(end) = after_start.find('}') else {
            break;
        };

        let name = &after_start[..end];
        if !name.is_empty()
            && !parameters
                .iter()
                .any(|parameter| parameter_name(parameter) == name)
        {
            parameters.push(json!({
                "name": name,
                "in": "path",
                "required": true,
                "schema": { "type": "string" }
            }));
        }

        cursor = &after_start[end + 1..];
    }

    parameters
}

fn request_body_for(endpoint: &EndpointDefinition) -> Option<Value> {
    let examples = body_examples(endpoint.examples.iter().filter_map(|example| {
        example
            .request_body_sample
            .as_deref()
            .map(redacted_body_sample)
    }));

    if endpoint.request_schema.is_none() && examples.is_empty() {
        return None;
    }

    Some(json!({
        "required": false,
        "content": {
            "application/json": media_type_object(endpoint.request_schema.clone(), examples)
        }
    }))
}

fn responses_for(endpoint: &EndpointDefinition) -> Value {
    let mut responses = Map::new();
    let mut statuses = endpoint
        .examples
        .iter()
        .map(|example| example.response_status)
        .collect::<Vec<_>>();

    statuses.sort_unstable();
    statuses.dedup();

    if statuses.is_empty() {
        statuses.push(200);
    }

    for status in statuses {
        let examples = body_examples(
            endpoint
                .examples
                .iter()
                .filter(move |example| example.response_status == status)
                .filter_map(|example| {
                    example
                        .response_body_sample
                        .as_deref()
                        .map(redacted_body_sample)
                }),
        );

        responses.insert(
            status.to_string(),
            json!({
                "description": format!("Captured HTTP {status} response"),
                "content": {
                    "application/json": media_type_object(endpoint.response_schema.clone(), examples)
                }
            }),
        );
    }

    Value::Object(responses)
}

fn media_type_object(schema: Option<Value>, examples: Map<String, Value>) -> Value {
    let mut media_type = Map::new();

    if let Some(schema) = schema {
        media_type.insert("schema".to_owned(), redact_json(&schema));
    }

    if !examples.is_empty() {
        media_type.insert("examples".to_owned(), Value::Object(examples));
    }

    Value::Object(media_type)
}

fn examples_for(endpoint: &EndpointDefinition) -> Vec<Value> {
    endpoint.examples.iter().map(redacted_example).collect()
}

fn redacted_example(example: &EndpointExample) -> Value {
    json!({
        "url": redacted_example_url(&example.url),
        "request_headers": redact_headers(&example.request_headers),
        "request_body": example.request_body_sample.as_deref().map(redacted_body_sample),
        "response_status": example.response_status,
        "response_headers": redact_headers(&example.response_headers),
        "response_body": example.response_body_sample.as_deref().map(redacted_body_sample),
        "captured_at": example.captured_at
    })
}

fn redacted_example_url(url: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_owned();
    };

    let pairs: Vec<(String, String)> = parsed.query_pairs().into_owned().collect();
    if pairs.is_empty() {
        return parsed.to_string();
    }

    parsed.set_query(None);
    {
        let mut query = parsed.query_pairs_mut();
        for (name, value) in pairs {
            query.append_pair(&name, &redacted_parameter_value(&name, &value));
        }
    }

    parsed.to_string()
}

fn body_examples(values: impl Iterator<Item = Value>) -> Map<String, Value> {
    examples_object(values)
}

fn examples_object(values: impl Iterator<Item = Value>) -> Map<String, Value> {
    let mut examples = Map::new();

    for (index, value) in values.enumerate() {
        examples.insert(format!("captured-{}", index + 1), json!({ "value": value }));
    }

    examples
}

fn redacted_body_sample(sample: &str) -> Value {
    match serde_json::from_str::<Value>(sample) {
        Ok(value) => redact_json(&value),
        Err(_) if contains_obvious_secret(sample) => Value::String(REDACTED.to_owned()),
        Err(_) => Value::String(sample.to_owned()),
    }
}

fn contains_obvious_secret(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("bearer ")
        || lower.contains("authorization")
        || lower.contains("api_key")
        || lower.contains("api-key")
        || lower.contains("csrf")
        || lower.contains("token")
        || lower.contains("session")
        || lower.contains("password")
        || lower.contains("cookie")
        || contains_email_like_value(value)
}

fn redacted_parameter_value(name: &str, value: &str) -> String {
    if is_sensitive_name(name) || contains_obvious_secret(value) {
        REDACTED.to_owned()
    } else {
        value.to_owned()
    }
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

fn contains_email_like_value(value: &str) -> bool {
    let Some(at_index) = value.find('@') else {
        return false;
    };

    let before = &value[..at_index];
    let after = &value[at_index + 1..];

    before
        .chars()
        .rev()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '%' | '+' | '-')
        })
        .count()
        > 0
        && after
            .chars()
            .take_while(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '-')
            })
            .any(|character| character == '.')
}

fn operation_id(endpoint: &EndpointDefinition) -> String {
    format!(
        "{}_{}",
        endpoint.method.to_ascii_lowercase(),
        endpoint
            .path_template
            .trim_matches('/')
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character.to_ascii_lowercase()
                } else {
                    '_'
                }
            })
            .collect::<String>()
    )
    .trim_matches('_')
    .to_owned()
}

fn normalize_openapi_path(path_template: &str) -> String {
    if path_template.starts_with('/') {
        path_template.to_owned()
    } else {
        format!("/{path_template}")
    }
}

fn parameter_name(parameter: &Value) -> &str {
    parameter
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
}

fn capture_dir_for_id(root: &Path, capture_id: &str) -> Result<PathBuf, CaptureError> {
    let mut capture_dir = root.to_path_buf();
    let parts = capture_id
        .split(['/', '\\'])
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if parts.is_empty() {
        return Err(CaptureError::Storage(
            "capture id cannot be empty".to_owned(),
        ));
    }

    for part in parts {
        if !is_safe_path_segment(part) {
            return Err(CaptureError::Storage(format!(
                "capture id contains unsafe path segment: {capture_id}"
            )));
        }
        capture_dir.push(part);
    }

    ensure_within_root(root, &capture_dir)?;

    Ok(capture_dir)
}

fn ensure_within_root(root: &Path, path: &Path) -> Result<(), CaptureError> {
    if relative_components(path).starts_with(&relative_components(root)) {
        Ok(())
    } else {
        Err(CaptureError::Storage(format!(
            "capture path escapes capture root: {}",
            path.display()
        )))
    }
}

fn relative_components(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Prefix(prefix) => Some(prefix.as_os_str().to_string_lossy().to_string()),
            Component::RootDir => Some(String::from("\\")),
            Component::Normal(value) => Some(value.to_string_lossy().to_string()),
            Component::CurDir => None,
            Component::ParentDir => Some(String::from("..")),
        })
        .collect()
}

fn is_safe_path_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.contains(':')
        && !segment.contains('/')
        && !segment.contains('\\')
}
