use std::collections::BTreeMap;

use serde_json::{Map, Value};
use url::Url;

use crate::types::{
    CaptureArtifact, CapturedExchange, EndpointDefinition, EndpointExample, HeaderMap,
};

const REDACTED: &str = "[REDACTED]";

const SENSITIVE_NAMES: &[&str] = &[
    "authorization",
    "cookie",
    "set-cookie",
    "api-key",
    "csrf",
    "token",
    "session",
    "password",
    "email",
];

pub fn redact_headers(headers: &HeaderMap) -> HeaderMap {
    headers
        .iter()
        .map(|(name, value)| {
            let value = if is_sensitive_name(name) {
                Value::String(REDACTED.to_owned())
            } else {
                value.clone()
            };
            (name.clone(), value)
        })
        .collect()
}

pub fn redact_url(url: &str) -> String {
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
            let value = if is_sensitive_name(&name) {
                REDACTED.to_owned()
            } else {
                value
            };
            query.append_pair(&name, &value);
        }
    }

    parsed.to_string()
}

pub fn redact_json(value: &Value) -> Value {
    match value {
        Value::Object(object) => Value::Object(redact_json_object(object)),
        Value::Array(items) => Value::Array(items.iter().map(redact_json).collect()),
        _ => value.clone(),
    }
}

pub fn redact_artifact(artifact: &CaptureArtifact) -> CaptureArtifact {
    let metadata = match redact_json(&Value::Object(artifact.metadata.clone())) {
        Value::Object(metadata) => metadata,
        _ => Map::new(),
    };

    CaptureArtifact {
        id: artifact.id.clone(),
        source_url: redact_url(&artifact.source_url),
        intent: artifact.intent.clone(),
        started_at: artifact.started_at,
        completed_at: artifact.completed_at,
        exchanges: artifact.exchanges.iter().map(redact_exchange).collect(),
        endpoints: artifact.endpoints.iter().map(redact_endpoint).collect(),
        metadata,
    }
}

fn redact_exchange(exchange: &CapturedExchange) -> CapturedExchange {
    CapturedExchange {
        method: exchange.method.clone(),
        url: redact_url(&exchange.url),
        request_headers: redact_headers(&exchange.request_headers),
        request_body_sample: redact_body_sample(exchange.request_body_sample.as_deref()),
        resource_type: exchange.resource_type.clone(),
        status: exchange.status,
        response_headers: redact_headers(&exchange.response_headers),
        response_body_sample: redact_body_sample(exchange.response_body_sample.as_deref()),
        started_at: exchange.started_at,
        duration_ms: exchange.duration_ms,
        redirect_chain: exchange
            .redirect_chain
            .iter()
            .map(|redirect| redact_url(redirect))
            .collect(),
    }
}

fn redact_endpoint(endpoint: &EndpointDefinition) -> EndpointDefinition {
    EndpointDefinition {
        id: endpoint.id.clone(),
        method: endpoint.method.clone(),
        origin: endpoint.origin.clone(),
        path_template: endpoint.path_template.clone(),
        query_params: redact_query_params(&endpoint.query_params),
        request_schema: endpoint.request_schema.as_ref().map(redact_json),
        response_schema: endpoint.response_schema.as_ref().map(redact_json),
        auth_evidence: endpoint.auth_evidence.clone(),
        safety: endpoint.safety.clone(),
        examples: endpoint
            .examples
            .iter()
            .map(redact_endpoint_example)
            .collect(),
    }
}

fn redact_endpoint_example(example: &EndpointExample) -> EndpointExample {
    EndpointExample {
        url: redact_url(&example.url),
        request_headers: redact_headers(&example.request_headers),
        request_body_sample: redact_body_sample(example.request_body_sample.as_deref()),
        response_status: example.response_status,
        response_headers: redact_headers(&example.response_headers),
        response_body_sample: redact_body_sample(example.response_body_sample.as_deref()),
        captured_at: example.captured_at,
    }
}

fn redact_query_params(params: &BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    params
        .iter()
        .map(|(name, values)| {
            let values = if is_sensitive_name(name) {
                vec![REDACTED.to_owned()]
            } else {
                values.clone()
            };
            (name.clone(), values)
        })
        .collect()
}

fn redact_json_object(object: &Map<String, Value>) -> Map<String, Value> {
    object
        .iter()
        .map(|(key, value)| {
            let value = if is_sensitive_name(key) {
                Value::String(REDACTED.to_owned())
            } else {
                redact_json(value)
            };
            (key.clone(), value)
        })
        .collect()
}

fn redact_body_sample(sample: Option<&str>) -> Option<String> {
    sample.map(|body| match serde_json::from_str::<Value>(body) {
        Ok(value) => redact_json(&value).to_string(),
        Err(_) => redact_text_body(body),
    })
}

fn is_sensitive_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    let compact: String = lower
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect();

    SENSITIVE_NAMES.iter().any(|sensitive| {
        let sensitive_compact: String = sensitive
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric())
            .collect();

        lower.contains(sensitive) || compact.contains(&sensitive_compact)
    })
}

fn redact_text_body(body: &str) -> String {
    body.lines()
        .map(|line| {
            if is_sensitive_text_line(line) {
                REDACTED.to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_sensitive_text_line(line: &str) -> bool {
    is_sensitive_name(line) || contains_bearer_token(line) || contains_email_like_value(line)
}

fn contains_bearer_token(line: &str) -> bool {
    line.to_ascii_lowercase().contains("bearer ")
}

fn contains_email_like_value(line: &str) -> bool {
    let Some(at_index) = line.find('@') else {
        return false;
    };

    let before = &line[..at_index];
    let after = &line[at_index + 1..];

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
