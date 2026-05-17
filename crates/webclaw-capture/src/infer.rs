use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};
use url::Url;

use crate::classify::filter_api_exchanges;
use crate::redact::{redact_headers, redact_url};
use crate::types::{
    CapturedExchange, EndpointDefinition, EndpointExample, EndpointSafety, HeaderMap,
};

pub fn infer_endpoints(exchanges: &[CapturedExchange]) -> Vec<EndpointDefinition> {
    let mut groups = BTreeMap::<EndpointKey, EndpointBuilder>::new();

    for exchange in filter_api_exchanges(exchanges) {
        let Ok(url) = Url::parse(&exchange.url) else {
            continue;
        };

        let method = exchange.method.to_ascii_uppercase();
        let origin = url.origin().ascii_serialization();
        let path_template = normalize_path_template(url.path());
        let key = EndpointKey {
            method: method.clone(),
            origin: origin.clone(),
            path_template: path_template.clone(),
        };

        groups
            .entry(key)
            .or_insert_with(|| EndpointBuilder::new(method, origin, path_template))
            .add_exchange(&exchange, &url);
    }

    groups
        .into_values()
        .map(EndpointBuilder::into_endpoint)
        .collect()
}

pub fn normalize_path_template(path: &str) -> String {
    let normalized = if path.is_empty() { "/" } else { path };
    let trailing_slash = normalized.len() > 1 && normalized.ends_with('/');

    let mut segments = normalized
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(|segment| {
            if is_identifier_segment(segment) {
                "{id}".to_owned()
            } else {
                segment.to_owned()
            }
        })
        .collect::<Vec<_>>();

    if segments.is_empty() {
        return "/".to_owned();
    }

    let mut path_template = format!("/{}", segments.join("/"));
    if trailing_slash {
        path_template.push('/');
    }
    segments.clear();
    path_template
}

pub fn infer_json_schema(value: &Value) -> Value {
    match value {
        Value::Null => json!({ "type": "null" }),
        Value::Bool(_) => json!({ "type": "boolean" }),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            json!({ "type": "integer" })
        }
        Value::Number(_) => json!({ "type": "number" }),
        Value::String(_) => json!({ "type": "string" }),
        Value::Array(items) => {
            let item_schema = items
                .iter()
                .map(infer_json_schema)
                .reduce(|left, right| merge_json_schemas(&left, &right))
                .unwrap_or_else(|| json!({}));

            json!({
                "type": "array",
                "items": item_schema
            })
        }
        Value::Object(object) => {
            let properties = object
                .iter()
                .map(|(key, value)| (key.clone(), infer_json_schema(value)))
                .collect::<Map<_, _>>();

            json!({
                "type": "object",
                "properties": properties
            })
        }
    }
}

pub fn endpoint_id(method: &str, origin: &str, path_template: &str) -> String {
    format!(
        "{} {}{}",
        method.to_ascii_uppercase(),
        origin.trim_end_matches('/'),
        ensure_leading_slash(path_template)
    )
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct EndpointKey {
    method: String,
    origin: String,
    path_template: String,
}

#[derive(Debug, Clone)]
struct EndpointBuilder {
    method: String,
    origin: String,
    path_template: String,
    query_params: BTreeMap<String, BTreeSet<String>>,
    request_schema: Option<Value>,
    response_schema: Option<Value>,
    auth_evidence: BTreeSet<String>,
    examples: Vec<EndpointExample>,
}

impl EndpointBuilder {
    fn new(method: String, origin: String, path_template: String) -> Self {
        Self {
            method,
            origin,
            path_template,
            query_params: BTreeMap::new(),
            request_schema: None,
            response_schema: None,
            auth_evidence: BTreeSet::new(),
            examples: Vec::new(),
        }
    }

    fn add_exchange(&mut self, exchange: &CapturedExchange, url: &Url) {
        for (name, value) in url.query_pairs() {
            self.query_params
                .entry(name.into_owned())
                .or_default()
                .insert(value.into_owned());
        }

        self.record_auth_evidence(&exchange.request_headers);
        self.record_auth_evidence(&exchange.response_headers);

        if let Some(schema) = infer_body_schema(exchange.request_body_sample.as_deref()) {
            self.request_schema = merge_optional_schema(self.request_schema.take(), schema);
        }

        if let Some(schema) = infer_body_schema(exchange.response_body_sample.as_deref()) {
            self.response_schema = merge_optional_schema(self.response_schema.take(), schema);
        }

        self.examples.push(EndpointExample {
            url: redact_url(&exchange.url),
            request_headers: redact_headers(&exchange.request_headers),
            request_body_sample: redact_body_sample(exchange.request_body_sample.as_deref()),
            response_status: exchange.status,
            response_headers: redact_headers(&exchange.response_headers),
            response_body_sample: redact_body_sample(exchange.response_body_sample.as_deref()),
            captured_at: exchange.started_at,
        });
    }

    fn into_endpoint(self) -> EndpointDefinition {
        let safety = endpoint_safety(&self.method);

        EndpointDefinition {
            id: endpoint_id(&self.method, &self.origin, &self.path_template),
            method: self.method,
            origin: self.origin,
            path_template: self.path_template,
            query_params: self
                .query_params
                .into_iter()
                .map(|(name, values)| (name, values.into_iter().collect()))
                .collect(),
            request_schema: self.request_schema,
            response_schema: self.response_schema,
            auth_evidence: self.auth_evidence.into_iter().collect(),
            safety,
            examples: self.examples,
        }
    }

    fn record_auth_evidence(&mut self, headers: &HeaderMap) {
        for name in headers.keys() {
            if is_auth_evidence_header(name) {
                self.auth_evidence.insert(format!("{name} header observed"));
            }
        }
    }
}

fn infer_body_schema(body: Option<&str>) -> Option<Value> {
    let body = body?.trim();
    if body.is_empty() {
        return None;
    }

    serde_json::from_str::<Value>(body)
        .ok()
        .map(|value| infer_json_schema(&value))
}

fn merge_optional_schema(current: Option<Value>, next: Value) -> Option<Value> {
    Some(match current {
        Some(current) => merge_json_schemas(&current, &next),
        None => next,
    })
}

fn merge_json_schemas(left: &Value, right: &Value) -> Value {
    if left == right {
        return left.clone();
    }

    let left_type = left.get("type").and_then(Value::as_str);
    let right_type = right.get("type").and_then(Value::as_str);

    match (left_type, right_type) {
        (Some("object"), Some("object")) => merge_object_schemas(left, right),
        (Some("array"), Some("array")) => {
            let left_items = left.get("items").cloned().unwrap_or_else(|| json!({}));
            let right_items = right.get("items").cloned().unwrap_or_else(|| json!({}));
            json!({
                "type": "array",
                "items": merge_json_schemas(&left_items, &right_items)
            })
        }
        (Some(_), Some(_)) => {
            let mut variants = Vec::new();
            push_unique_schema(&mut variants, left.clone());
            push_unique_schema(&mut variants, right.clone());
            json!({ "oneOf": variants })
        }
        _ => right.clone(),
    }
}

fn merge_object_schemas(left: &Value, right: &Value) -> Value {
    let mut properties = Map::new();

    if let Some(left_properties) = left.get("properties").and_then(Value::as_object) {
        for (name, schema) in left_properties {
            properties.insert(name.clone(), schema.clone());
        }
    }

    if let Some(right_properties) = right.get("properties").and_then(Value::as_object) {
        for (name, schema) in right_properties {
            let schema = properties
                .remove(name)
                .map(|existing| merge_json_schemas(&existing, schema))
                .unwrap_or_else(|| schema.clone());
            properties.insert(name.clone(), schema);
        }
    }

    json!({
        "type": "object",
        "properties": properties
    })
}

fn push_unique_schema(variants: &mut Vec<Value>, schema: Value) {
    if let Some(nested) = schema.get("oneOf").and_then(Value::as_array) {
        for item in nested {
            push_unique_schema(variants, item.clone());
        }
        return;
    }

    if !variants.iter().any(|existing| existing == &schema) {
        variants.push(schema);
    }
}

fn endpoint_safety(method: &str) -> EndpointSafety {
    if is_safe_method(method) {
        EndpointSafety {
            safe_to_replay: true,
            requires_confirmation: false,
            reason: format!(
                "{} is a read-oriented HTTP method",
                method.to_ascii_uppercase()
            ),
        }
    } else {
        EndpointSafety {
            safe_to_replay: false,
            requires_confirmation: true,
            reason: format!(
                "{} may mutate server state and requires confirmation",
                method.to_ascii_uppercase()
            ),
        }
    }
}

fn is_safe_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    )
}

fn redact_body_sample(sample: Option<&str>) -> Option<String> {
    sample.map(|body| match serde_json::from_str::<Value>(body) {
        Ok(value) => crate::redact::redact_json(&value).to_string(),
        Err(_) => body.to_owned(),
    })
}

fn is_auth_evidence_header(name: &str) -> bool {
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
    ]
    .iter()
    .any(|needle| {
        let compact_needle: String = needle
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect();

        lower.contains(needle) || compact.contains(&compact_needle)
    })
}

fn is_identifier_segment(segment: &str) -> bool {
    is_numeric_segment(segment) || is_uuid_like_segment(segment) || is_high_entropy_segment(segment)
}

fn is_numeric_segment(segment: &str) -> bool {
    !segment.is_empty() && segment.chars().all(|character| character.is_ascii_digit())
}

fn is_uuid_like_segment(segment: &str) -> bool {
    let parts = segment.split('-').map(str::len).collect::<Vec<_>>();
    parts == [8, 4, 4, 4, 12]
        && segment
            .chars()
            .all(|character| character == '-' || character.is_ascii_hexdigit())
}

fn is_high_entropy_segment(segment: &str) -> bool {
    segment.len() >= 16
        && segment.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '~')
        })
        && segment.chars().any(|character| character.is_ascii_digit())
        && segment
            .chars()
            .any(|character| character.is_ascii_alphabetic())
}

fn ensure_leading_slash(path: &str) -> String {
    if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}
