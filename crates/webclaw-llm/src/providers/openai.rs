/// OpenAI provider — works with api.openai.com and any OpenAI-compatible endpoint.
use async_trait::async_trait;
use serde_json::json;

use crate::clean::strip_thinking_tags;
use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

use super::load_api_key;

pub struct OpenAiProvider {
    client: reqwest::Client,
    key: String,
    base_url: String,
    default_model: String,
    response_format: OpenAiResponseFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenAiResponseFormat {
    JsonObject,
    JsonSchema,
    Text,
}

impl OpenAiResponseFormat {
    fn from_env() -> Self {
        std::env::var("OPENAI_RESPONSE_FORMAT_TYPE")
            .ok()
            .and_then(|value| Self::parse(&value))
            .unwrap_or(Self::JsonObject)
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "json_object" => Some(Self::JsonObject),
            "json_schema" => Some(Self::JsonSchema),
            "text" => Some(Self::Text),
            _ => None,
        }
    }

    fn as_response_format(self) -> serde_json::Value {
        match self {
            Self::JsonObject => json!({ "type": "json_object" }),
            Self::JsonSchema => json!({
                "type": "json_schema",
                "json_schema": {
                    "name": "webclaw_response",
                    "schema": {
                        "type": "object",
                        "additionalProperties": true
                    },
                    "strict": false
                }
            }),
            Self::Text => json!({ "type": "text" }),
        }
    }
}

impl OpenAiProvider {
    /// Returns `None` if no API key is available (param or env).
    pub fn new(
        key_override: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Option<Self> {
        let key = load_api_key(key_override, "OPENAI_API_KEY")?;

        Some(Self {
            client: reqwest::Client::new(),
            key,
            base_url: base_url
                .or_else(|| std::env::var("OPENAI_BASE_URL").ok())
                .unwrap_or_else(|| "https://api.openai.com/v1".into()),
            default_model: model.unwrap_or_else(|| "gpt-4o-mini".into()),
            response_format: OpenAiResponseFormat::from_env(),
        })
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    fn request_body(&self, request: &CompletionRequest, model: &str) -> serde_json::Value {
        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect();

        let mut body = json!({
            "model": model,
            "messages": messages,
        });

        if request.json_mode {
            body["response_format"] = self.response_format.as_response_format();
        }
        if let Some(temp) = request.temperature {
            body["temperature"] = json!(temp);
        }
        if let Some(max) = request.max_tokens {
            body["max_tokens"] = json!(max);
        }

        body
    }
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let body = self.request_body(request, model);

        let url = format!("{}/chat/completions", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.key))
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let safe_text = if text.len() > 500 {
                &text[..500]
            } else {
                &text
            };
            return Err(LlmError::ProviderError(format!(
                "openai returned {status}: {safe_text}"
            )));
        }

        // Cap response body size to defend against adversarial payloads.
        let json = super::response_json_capped(resp).await?;

        let raw = json["choices"][0]["message"]["content"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                LlmError::InvalidJson(
                    "missing choices[0].message.content in openai response".into(),
                )
            })?;

        Ok(strip_thinking_tags(&raw))
    }

    async fn is_available(&self) -> bool {
        !self.key.is_empty()
    }

    fn name(&self) -> &str {
        "openai"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_key_returns_none() {
        assert!(OpenAiProvider::new(Some(String::new()), None, None).is_none());
    }

    #[test]
    fn explicit_key_constructs() {
        let provider = OpenAiProvider::new(
            Some("test-key-123".into()),
            Some("https://api.openai.com/v1".into()),
            Some("gpt-4o-mini".into()),
        )
        .expect("should construct");
        assert_eq!(provider.name(), "openai");
        assert_eq!(provider.default_model, "gpt-4o-mini");
        assert_eq!(provider.base_url, "https://api.openai.com/v1");
        assert_eq!(provider.key, "test-key-123");
        assert_eq!(provider.response_format, OpenAiResponseFormat::JsonObject);
    }

    #[test]
    fn custom_base_url_and_model() {
        let provider = OpenAiProvider::new(
            Some("test-key".into()),
            Some("http://localhost:8080/v1".into()),
            Some("gpt-3.5-turbo".into()),
        )
        .unwrap();
        assert_eq!(provider.base_url, "http://localhost:8080/v1");
        assert_eq!(provider.default_model, "gpt-3.5-turbo");
    }

    #[test]
    fn default_model_accessor() {
        let provider = OpenAiProvider::new(
            Some("test-key".into()),
            Some("https://api.openai.com/v1".into()),
            None,
        )
        .unwrap();
        assert_eq!(provider.default_model(), "gpt-4o-mini");
    }

    #[test]
    fn json_mode_defaults_to_openai_json_object() {
        let provider = OpenAiProvider::new(
            Some("test-key".into()),
            Some("https://api.openai.com/v1".into()),
            None,
        )
        .unwrap();
        let req = CompletionRequest {
            model: String::new(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            json_mode: true,
        };
        let body = provider.request_body(&req, provider.default_model());
        assert_eq!(body["response_format"], json!({ "type": "json_object" }));
    }

    #[test]
    fn json_schema_response_format_for_compatible_backends() {
        let mut provider = OpenAiProvider::new(
            Some("test-key".into()),
            Some("http://localhost:1234/v1".into()),
            Some("local-model".into()),
        )
        .unwrap();
        provider.response_format = OpenAiResponseFormat::JsonSchema;
        let req = CompletionRequest {
            model: String::new(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            json_mode: true,
        };
        let body = provider.request_body(&req, provider.default_model());
        assert_eq!(body["response_format"]["type"], "json_schema");
        assert_eq!(
            body["response_format"]["json_schema"]["schema"]["type"],
            "object"
        );
    }

    #[test]
    fn text_response_format_for_lm_studio() {
        let mut provider = OpenAiProvider::new(
            Some("test-key".into()),
            Some("http://localhost:1234/v1".into()),
            Some("local-model".into()),
        )
        .unwrap();
        provider.response_format = OpenAiResponseFormat::Text;
        let req = CompletionRequest {
            model: String::new(),
            messages: vec![],
            temperature: None,
            max_tokens: None,
            json_mode: true,
        };
        let body = provider.request_body(&req, provider.default_model());
        assert_eq!(body["response_format"], json!({ "type": "text" }));
    }

    // Env var fallback tests mutate process-global state and race with parallel tests.
    // The code path is trivial (load_api_key -> env::var().ok()). Run in isolation if needed:
    //   cargo test -p webclaw-llm env_var -- --ignored --test-threads=1
    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn env_var_key_fallback() {
        unsafe { std::env::set_var("OPENAI_API_KEY", "sk-env-key") };
        let provider = OpenAiProvider::new(None, None, None).expect("should construct from env");
        assert_eq!(provider.key, "sk-env-key");
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
    }

    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn no_key_returns_none() {
        unsafe { std::env::remove_var("OPENAI_API_KEY") };
        assert!(OpenAiProvider::new(None, None, None).is_none());
    }
}
