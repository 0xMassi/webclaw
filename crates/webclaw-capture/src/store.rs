use std::env;
use std::fs;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use url::Url;

use crate::redact::redact_artifact;
use crate::types::{CaptureArtifact, CaptureError, EndpointDefinition, SavedCapture};

const CAPTURE_DIR_ENV: &str = "WEBCLAW_CAPTURE_DIR";
const RAW_CAPTURE_FILE: &str = "raw-capture.json";
const REDACTED_CAPTURE_FILE: &str = "redacted-capture.json";
const ENDPOINTS_FILE: &str = "endpoints.json";
const METADATA_FILE: &str = "metadata.json";

pub fn capture_root() -> PathBuf {
    env::var_os(CAPTURE_DIR_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".webclaw").join("api-captures"))
}

pub fn capture_id_for(url: &Url, started_at: DateTime<Utc>) -> String {
    let host = url.host_str().unwrap_or("unknown-host");
    let host = match url.port() {
        Some(port) => format!("{host}-{port}"),
        None => host.to_owned(),
    };
    let timestamp = started_at.format("%Y-%m-%dT%H-%M-%SZ");

    format!("{}/{timestamp}", sanitize_id_segment(&host))
}

pub fn save_capture(artifact: &CaptureArtifact) -> Result<SavedCapture, CaptureError> {
    let root = capture_root();
    let capture_dir = capture_dir_for_id(&root, &artifact.id)?;

    fs::create_dir_all(&capture_dir)?;

    let raw_capture_path = capture_dir.join(RAW_CAPTURE_FILE);
    let redacted_capture_path = capture_dir.join(REDACTED_CAPTURE_FILE);
    let endpoints_path = capture_dir.join(ENDPOINTS_FILE);
    let metadata_path = capture_dir.join(METADATA_FILE);
    let redacted_artifact = redact_artifact(artifact);

    write_json(&raw_capture_path, artifact)?;
    write_json(&redacted_capture_path, &redacted_artifact)?;
    write_json(&endpoints_path, &redacted_artifact.endpoints)?;
    write_json(&metadata_path, &metadata_for(&redacted_artifact))?;

    Ok(SavedCapture {
        id: artifact.id.clone(),
        root,
        capture_dir,
        raw_capture_path,
        redacted_capture_path,
        endpoints_path,
        metadata_path,
    })
}

pub fn load_endpoints(capture_id: &str) -> Result<Vec<EndpointDefinition>, CaptureError> {
    let endpoints_path = capture_dir_for_id(&capture_root(), capture_id)?.join(ENDPOINTS_FILE);
    let contents = fs::read_to_string(&endpoints_path).map_err(|error| {
        CaptureError::Storage(format!(
            "could not read endpoints for capture id {capture_id}: {error}"
        ))
    })?;

    serde_json::from_str(&contents).map_err(CaptureError::from)
}

pub fn find_endpoint(endpoint_id: &str) -> Result<EndpointDefinition, CaptureError> {
    let root = capture_root();
    if !root.exists() {
        return Err(CaptureError::EndpointNotFound(endpoint_id.to_owned()));
    }

    let mut stack = vec![root];
    while let Some(path) = stack.pop() {
        let entries = match fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }

            if path.file_name().and_then(|name| name.to_str()) != Some(ENDPOINTS_FILE) {
                continue;
            }

            let contents = match fs::read_to_string(&path) {
                Ok(contents) => contents,
                Err(_) => continue,
            };
            let endpoints: Vec<EndpointDefinition> = match serde_json::from_str(&contents) {
                Ok(endpoints) => endpoints,
                Err(_) => continue,
            };

            if let Some(endpoint) = endpoints
                .into_iter()
                .find(|endpoint| endpoint.id == endpoint_id)
            {
                return Ok(endpoint);
            }
        }
    }

    Err(CaptureError::EndpointNotFound(endpoint_id.to_owned()))
}

fn home_dir() -> PathBuf {
    env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
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

fn sanitize_id_segment(segment: &str) -> String {
    let sanitized = segment
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_') {
                character
            } else {
                '-'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

fn write_json<T: serde::Serialize>(path: &PathBuf, value: &T) -> Result<(), CaptureError> {
    let contents = serde_json::to_string_pretty(value)?;
    fs::write(path, contents)?;
    Ok(())
}

fn metadata_for(artifact: &CaptureArtifact) -> Map<String, Value> {
    let mut metadata = artifact.metadata.clone();
    metadata.insert("id".to_owned(), json!(artifact.id));
    metadata.insert("source_url".to_owned(), json!(artifact.source_url));
    metadata.insert("intent".to_owned(), json!(artifact.intent));
    metadata.insert("started_at".to_owned(), json!(artifact.started_at));
    metadata.insert("completed_at".to_owned(), json!(artifact.completed_at));
    metadata.insert("exchange_count".to_owned(), json!(artifact.exchanges.len()));
    metadata.insert("endpoint_count".to_owned(), json!(artifact.endpoints.len()));
    metadata
}
