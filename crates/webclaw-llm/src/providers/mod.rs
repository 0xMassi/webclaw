pub mod anthropic;
pub mod ollama;
pub mod openai;

use std::time::Duration;

use crate::error::LlmError;

/// Connect timeout shared by every provider. A dead or wrong host should fail
/// fast (so the chain can move to the next provider) rather than hang on the
/// OS default connect timeout.
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// Build the HTTP client for a provider with a fixed connect timeout and a
/// caller-chosen overall request timeout. Falls back to `reqwest::Client::new()`
/// only if the builder somehow fails, so construction stays infallible.
pub(crate) fn build_http_client(request_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(request_timeout)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// Load an API key from an explicit override or an environment variable.
/// Returns `None` if neither is set or the value is empty.
pub(crate) fn load_api_key(override_key: Option<String>, env_var: &str) -> Option<String> {
    let key = override_key.or_else(|| std::env::var(env_var).ok())?;
    if key.is_empty() { None } else { Some(key) }
}

/// Maximum bytes we'll pull from an LLM provider response. 5 MB is already
/// ~5× the largest real payload any of these providers emits for normal
/// completions; anything bigger is either a streaming bug on their end or
/// an adversarial response aimed at exhausting our memory.
pub(crate) const MAX_RESPONSE_BYTES: u64 = 5 * 1024 * 1024;

/// Read a provider response as JSON, capping total bytes at
/// [`MAX_RESPONSE_BYTES`]. Rejects via Content-Length if the server is
/// honest about size; otherwise reads to completion and checks the actual
/// byte length so an unbounded body still can't swallow unbounded memory.
pub(crate) async fn response_json_capped(
    resp: reqwest::Response,
) -> Result<serde_json::Value, LlmError> {
    if let Some(len) = resp.content_length()
        && len > MAX_RESPONSE_BYTES
    {
        return Err(LlmError::ProviderError(format!(
            "response body {len} bytes exceeds cap {MAX_RESPONSE_BYTES}"
        )));
    }
    let bytes = resp.bytes().await?;
    if bytes.len() as u64 > MAX_RESPONSE_BYTES {
        return Err(LlmError::ProviderError(format!(
            "response body {} bytes exceeds cap {MAX_RESPONSE_BYTES}",
            bytes.len()
        )));
    }
    serde_json::from_slice(&bytes).map_err(|e| LlmError::InvalidJson(format!("response body: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_key_takes_precedence() {
        assert_eq!(
            load_api_key(Some("explicit".into()), "NONEXISTENT_VAR"),
            Some("explicit".into())
        );
    }

    #[test]
    fn empty_override_returns_none() {
        assert_eq!(load_api_key(Some(String::new()), "NONEXISTENT_VAR"), None);
    }

    #[test]
    fn none_override_with_no_env_returns_none() {
        assert_eq!(
            load_api_key(None, "WEBCLAW_TEST_NONEXISTENT_KEY_12345"),
            None
        );
    }
}
