/// Anthropic provider — Claude models via api.anthropic.com.
/// Anthropic's API differs from OpenAI: system message is a top-level param,
/// not part of the messages array.
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::clean::strip_thinking_tags;
use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

use super::load_api_key;

const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com/v1";
const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    client: reqwest::Client,
    key: String,
    base_url: String,
    default_model: String,
}

impl AnthropicProvider {
    /// Returns `None` if no API key is available (param or env).
    pub fn new(key_override: Option<String>, model: Option<String>) -> Option<Self> {
        Self::with_base_url(key_override, None, model)
    }

    /// Returns `None` if no API key is available (param or env).
    pub fn with_base_url(
        key_override: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Option<Self> {
        let key = load_api_key(key_override, "ANTHROPIC_API_KEY")?;

        Some(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            key,
            base_url: base_url
                .or_else(|| std::env::var("ANTHROPIC_BASE_URL").ok())
                .unwrap_or_else(|| DEFAULT_ANTHROPIC_BASE_URL.into())
                .trim_end_matches('/')
                .to_string(),
            default_model: model.unwrap_or_else(|| "claude-sonnet-4-20250514".into()),
        })
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    fn messages_url(&self) -> String {
        if self.base_url.ends_with("/messages") {
            self.base_url.clone()
        } else {
            format!("{}/messages", self.base_url)
        }
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        // Anthropic separates system from messages. Extract the system message if present.
        let system_content: Option<String> = request
            .messages
            .iter()
            .find(|m| m.role == "system")
            .map(|m| m.content.clone());

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect();

        let mut body = json!({
            "model": model,
            "messages": messages,
            "max_tokens": request.max_tokens.unwrap_or(4096),
        });

        if let Some(system) = &system_content {
            body["system"] = json!(system);
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }

        let resp = self
            .client
            .post(self.messages_url())
            .header("x-api-key", &self.key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let safe_text = text.chars().take(500).collect::<String>();
            return Err(LlmError::ProviderError(format!(
                "anthropic returned {status}: {safe_text}"
            )));
        }

        // Read body with a size cap so a malicious or misbehaving
        // endpoint can't allocate unbounded memory via resp.json().
        let json = super::response_json_capped(resp).await?;

        // Anthropic response: {"content": [{"type": "text", "text": "..."}]}
        let raw = json["content"][0]["text"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                LlmError::InvalidJson("missing content[0].text in anthropic response".into())
            })?;

        Ok(strip_thinking_tags(&raw))
    }

    async fn is_available(&self) -> bool {
        !self.key.is_empty()
    }

    fn name(&self) -> &str {
        "anthropic"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_key_returns_none() {
        assert!(AnthropicProvider::new(Some(String::new()), None).is_none());
    }

    #[test]
    fn explicit_key_constructs() {
        let provider =
            AnthropicProvider::new(Some("sk-ant-test".into()), None).expect("should construct");
        assert_eq!(provider.name(), "anthropic");
        assert_eq!(provider.default_model, "claude-sonnet-4-20250514");
        assert_eq!(provider.key, "sk-ant-test");
        assert_eq!(provider.base_url, "https://api.anthropic.com/v1");
        assert_eq!(
            provider.messages_url(),
            "https://api.anthropic.com/v1/messages"
        );
    }

    #[test]
    fn custom_model() {
        let provider =
            AnthropicProvider::new(Some("sk-ant-test".into()), Some("claude-3-haiku".into()))
                .unwrap();
        assert_eq!(provider.default_model, "claude-3-haiku");
    }

    #[test]
    fn default_model_accessor() {
        let provider = AnthropicProvider::new(Some("sk-ant-test".into()), None).unwrap();
        assert_eq!(provider.default_model(), "claude-sonnet-4-20250514");
    }

    #[test]
    fn custom_base_url_appends_messages_path() {
        let provider = AnthropicProvider::with_base_url(
            Some("sk-ant-test".into()),
            Some("https://proxy.example.test/anthropic/v1/".into()),
            None,
        )
        .unwrap();
        assert_eq!(provider.base_url, "https://proxy.example.test/anthropic/v1");
        assert_eq!(
            provider.messages_url(),
            "https://proxy.example.test/anthropic/v1/messages"
        );
    }

    #[test]
    fn custom_full_messages_url_is_not_doubled() {
        let provider = AnthropicProvider::with_base_url(
            Some("sk-ant-test".into()),
            Some("https://proxy.example.test/v1/messages".into()),
            None,
        )
        .unwrap();
        assert_eq!(
            provider.messages_url(),
            "https://proxy.example.test/v1/messages"
        );
    }

    // Env var fallback tests mutate process-global state and race with parallel tests.
    // The code path is trivial (load_api_key -> env::var().ok()). Run in isolation if needed:
    //   cargo test -p webclaw-llm env_var -- --ignored --test-threads=1
    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn env_var_key_fallback() {
        unsafe { std::env::set_var("ANTHROPIC_API_KEY", "sk-ant-env") };
        let provider = AnthropicProvider::new(None, None).expect("should construct from env");
        assert_eq!(provider.key, "sk-ant-env");
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
    }

    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn no_key_returns_none() {
        unsafe { std::env::remove_var("ANTHROPIC_API_KEY") };
        assert!(AnthropicProvider::new(None, None).is_none());
    }
}
