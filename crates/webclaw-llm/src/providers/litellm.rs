/// LiteLLM provider — OpenAI-compatible chat completions against a LiteLLM proxy.
///
/// A LiteLLM proxy speaks the OpenAI wire format, so this provider reuses the
/// `OpenAiProvider` transport unchanged. Pointing it at a LiteLLM gateway lets
/// webclaw reach 100+ upstream providers (OpenAI, Anthropic, Bedrock, Vertex
/// AI, Azure, and more) through a single endpoint with centralized keys.
use async_trait::async_trait;

use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

use super::openai::OpenAiProvider;

pub struct LiteLlmProvider {
    inner: OpenAiProvider,
}

impl LiteLlmProvider {
    /// Returns `None` if no LiteLLM API key is available (param or env).
    pub fn new(
        key_override: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Option<Self> {
        let key = super::load_api_key(key_override, "LITELLM_API_KEY")?;
        let base_url = base_url
            .or_else(|| std::env::var("LITELLM_BASE_URL").ok())
            .unwrap_or_else(|| "http://localhost:4000/v1".into());
        let model = model
            .or_else(|| std::env::var("LITELLM_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o-mini".into());
        let inner = OpenAiProvider::new(Some(key), Some(base_url), Some(model))?;
        Some(Self { inner })
    }

    pub fn default_model(&self) -> &str {
        self.inner.default_model()
    }
}

#[async_trait]
impl LlmProvider for LiteLlmProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        self.inner.complete(request).await
    }

    async fn is_available(&self) -> bool {
        self.inner.is_available().await
    }

    fn name(&self) -> &str {
        "litellm"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_key_returns_none() {
        assert!(LiteLlmProvider::new(Some(String::new()), None, None).is_none());
    }

    #[test]
    #[ignore = "reads LITELLM_MODEL from the process env; run with --test-threads=1"]
    fn explicit_key_constructs_with_litellm_defaults() {
        let provider =
            LiteLlmProvider::new(Some("test-key".into()), None, None).expect("should construct");
        assert_eq!(provider.name(), "litellm");
        assert_eq!(provider.default_model(), "gpt-4o-mini");
    }

    #[test]
    fn explicit_model_override() {
        let provider = LiteLlmProvider::new(
            Some("test-key".into()),
            Some("http://proxy.example.com:4000/v1".into()),
            Some("claude-sonnet-4-6".into()),
        )
        .expect("should construct");
        assert_eq!(provider.default_model(), "claude-sonnet-4-6");
    }
}
