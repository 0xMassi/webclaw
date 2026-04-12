/// Ollama provider — talks to a local Ollama instance (default localhost:11434).
/// First choice in the provider chain: free, private, fast on Apple Silicon.
use async_trait::async_trait;
use serde_json::json;
use std::time::Duration;

use crate::clean::strip_thinking_tags;
use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

pub struct OllamaProvider {
    client: reqwest::Client,
    base_url: String,
    default_model: String,
}

impl OllamaProvider {
    pub fn new(base_url: Option<String>, model: Option<String>) -> Self {
        let base_url = base_url
            .or_else(|| std::env::var("OLLAMA_HOST").ok())
            .unwrap_or_else(|| "http://localhost:11434".into());

        let default_model = model
            .or_else(|| std::env::var("OLLAMA_MODEL").ok())
            .unwrap_or_else(|| "qwen3:8b".into());

        Self {
            client: reqwest::Client::new(),
            base_url,
            default_model,
        }
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        let messages: Vec<serde_json::Value> = request
            .messages
            .iter()
            .map(|m| json!({ "role": m.role, "content": m.content }))
            .collect();

        let mut body = json!({
            "model": model,
            "messages": messages,
            "stream": false,
            "think": false,
        });

        if request.json_mode {
            body["format"] = json!("json");
        }
        if let Some(temp) = request.temperature {
            body["options"] = json!({ "temperature": temp });
        }

        let url = format!("{}/api/chat", self.base_url);
        let resp = self.client.post(&url).json(&body).send().await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let safe_text = if text.len() > 500 {
                &text[..500]
            } else {
                &text
            };
            return Err(LlmError::ProviderError(format!(
                "ollama returned {status}: {safe_text}"
            )));
        }

        let json: serde_json::Value = resp.json().await?;

        let raw = json["message"]["content"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                LlmError::InvalidJson(format!(
                    "missing message.content in ollama response: {json}"
                ))
            })?;

        Ok(strip_thinking_tags(&raw))
    }

    async fn is_available(&self) -> bool {
        let url = format!("{}/api/tags", self.base_url);
        matches!(
            tokio::time::timeout(Duration::from_millis(500), self.client.get(&url).send()).await,
            Ok(Ok(r)) if r.status().is_success()
        )
    }

    fn name(&self) -> &str {
        "ollama"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_params_used() {
        let provider = OllamaProvider::new(
            Some("http://gpu-box:11434".into()),
            Some("llama3:70b".into()),
        );
        assert_eq!(provider.base_url, "http://gpu-box:11434");
        assert_eq!(provider.default_model, "llama3:70b");
        assert_eq!(provider.name(), "ollama");
    }

    #[test]
    fn explicit_model_overrides_any_env() {
        // Passing Some(...) bypasses env vars entirely -- no race possible
        let provider = OllamaProvider::new(None, Some("mistral:7b".into()));
        assert_eq!(provider.default_model, "mistral:7b");
    }

    #[test]
    fn explicit_url_overrides_any_env() {
        let provider = OllamaProvider::new(Some("http://local:11434".into()), None);
        assert_eq!(provider.base_url, "http://local:11434");
    }

    #[test]
    fn default_model_accessor() {
        let provider = OllamaProvider::new(None, Some("phi3:mini".into()));
        assert_eq!(provider.default_model(), "phi3:mini");
    }

    // Env var fallback is a trivial `env::var().ok()` -- not worth the flakiness
    // of manipulating process-global state. Run in isolation if needed:
    //   cargo test -p noxa-llm env_var_fallback -- --ignored --test-threads=1
    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn env_var_fallback() {
        unsafe {
            std::env::set_var("OLLAMA_HOST", "http://remote:11434");
            std::env::set_var("OLLAMA_MODEL", "mistral:7b");
        }

        let provider = OllamaProvider::new(None, None);
        assert_eq!(provider.base_url, "http://remote:11434");
        assert_eq!(provider.default_model, "mistral:7b");

        unsafe {
            std::env::remove_var("OLLAMA_HOST");
            std::env::remove_var("OLLAMA_MODEL");
        }
    }
}
