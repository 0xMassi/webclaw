use std::collections::BTreeMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

pub type HeaderMap = Map<String, Value>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapturedRequest {
    pub method: String,
    pub url: String,
    pub headers: HeaderMap,
    pub body_sample: Option<String>,
    pub resource_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapturedResponse {
    pub status: u16,
    pub headers: HeaderMap,
    pub body_sample: Option<String>,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CapturedExchange {
    pub method: String,
    pub url: String,
    pub request_headers: HeaderMap,
    pub request_body_sample: Option<String>,
    pub resource_type: Option<String>,
    pub status: u16,
    pub response_headers: HeaderMap,
    pub response_body_sample: Option<String>,
    pub started_at: DateTime<Utc>,
    pub duration_ms: u64,
    pub redirect_chain: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CaptureArtifact {
    pub id: String,
    pub source_url: String,
    pub intent: Option<String>,
    pub started_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub exchanges: Vec<CapturedExchange>,
    pub endpoints: Vec<EndpointDefinition>,
    pub metadata: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndpointDefinition {
    pub id: String,
    pub method: String,
    pub origin: String,
    pub path_template: String,
    pub query_params: BTreeMap<String, Vec<String>>,
    pub request_schema: Option<Value>,
    pub response_schema: Option<Value>,
    pub auth_evidence: Vec<String>,
    pub safety: EndpointSafety,
    pub examples: Vec<EndpointExample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EndpointExample {
    pub url: String,
    pub request_headers: HeaderMap,
    pub request_body_sample: Option<String>,
    pub response_status: u16,
    pub response_headers: HeaderMap,
    pub response_body_sample: Option<String>,
    pub captured_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndpointSafety {
    pub safe_to_replay: bool,
    pub requires_confirmation: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReplayOptions {
    pub dry_run: bool,
    pub confirm_unsafe: bool,
    pub params_json: Option<Value>,
    pub headers: HeaderMap,
    pub body_json: Option<Value>,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        Self {
            dry_run: true,
            confirm_unsafe: false,
            params_json: None,
            headers: HeaderMap::new(),
            body_json: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReplayResult {
    Preview {
        method: String,
        url: String,
        headers: HeaderMap,
        body_sample: Option<String>,
    },
    Executed {
        status: u16,
        headers: HeaderMap,
        body_sample: Option<String>,
    },
    Blocked {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SavedCapture {
    pub id: String,
    pub root: PathBuf,
    pub capture_dir: PathBuf,
    pub raw_capture_path: PathBuf,
    pub redacted_capture_path: PathBuf,
    pub endpoints_path: PathBuf,
    pub metadata_path: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum CaptureError {
    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("capture failed: {0}")]
    Capture(String),

    #[error("storage failed: {0}")]
    Storage(String),

    #[error("replay failed: {0}")]
    Replay(String),

    #[error("endpoint not found: {0}")]
    EndpointNotFound(String),

    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),

    #[error("I/O failed: {0}")]
    Io(String),

    #[error("JSON failed: {0}")]
    Json(String),
}

impl From<std::io::Error> for CaptureError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}

impl From<serde_json::Error> for CaptureError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error.to_string())
    }
}
