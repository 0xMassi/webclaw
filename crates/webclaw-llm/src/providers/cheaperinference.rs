/// Cheaper Inference provider — OpenAI-compatible chat completions with Cheaper Inference defaults.
use async_trait::async_trait;

use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

use super::openai::OpenAiProvider;

const DEFAULT_BASE_URL: &str = "https://api.cheaperinference.com/v1";
const DEFAULT_MODEL: &str = "gpt-5.4-mini";

pub struct CheaperInferenceProvider {
    inner: OpenAiProvider,
}

impl CheaperInferenceProvider {
    /// Returns `None` if no Cheaper Inference API key is available (param or env).
    pub fn new(
        key_override: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Option<Self> {
        Self::with_env(
            key_override,
            base_url,
            model,
            std::env::var("CHEAPER_INFERENCE_BASE_URL").ok(),
            std::env::var("CHEAPER_INFERENCE_MODEL").ok(),
        )
    }

    /// Builds the provider from values the caller already read, so tests can
    /// check the defaults and the env fallback without touching process env.
    /// Precedence for base URL and model: explicit param, then env, then default.
    fn with_env(
        key_override: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
        env_base_url: Option<String>,
        env_model: Option<String>,
    ) -> Option<Self> {
        let key = super::load_api_key(key_override, "CHEAPER_INFERENCE_API_KEY")?;
        let base_url = base_url
            .or(env_base_url)
            .unwrap_or_else(|| DEFAULT_BASE_URL.into());
        let model = model.or(env_model).unwrap_or_else(|| DEFAULT_MODEL.into());
        let inner = OpenAiProvider::new(Some(key), Some(base_url), Some(model))?;
        Some(Self { inner })
    }

    /// The model used when a request does not name one.
    pub fn default_model(&self) -> &str {
        self.inner.default_model()
    }
}

#[async_trait]
impl LlmProvider for CheaperInferenceProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        self.inner.complete(request).await
    }

    async fn is_available(&self) -> bool {
        self.inner.is_available().await
    }

    fn name(&self) -> &str {
        "cheaperinference"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_key_returns_none() {
        assert!(CheaperInferenceProvider::new(Some(String::new()), None, None).is_none());
    }

    #[test]
    fn explicit_key_constructs_with_cheaperinference_defaults() {
        // No env values are passed in, so CHEAPER_INFERENCE_MODEL in the
        // developer's shell cannot change the result.
        let provider =
            CheaperInferenceProvider::with_env(Some("test-key".into()), None, None, None, None)
                .expect("should construct");
        assert_eq!(provider.name(), "cheaperinference");
        assert_eq!(provider.default_model(), DEFAULT_MODEL);
    }

    #[test]
    fn env_model_overrides_default() {
        let provider = CheaperInferenceProvider::with_env(
            Some("test-key".into()),
            None,
            None,
            None,
            Some("env-model".into()),
        )
        .expect("should construct");
        assert_eq!(provider.default_model(), "env-model");
    }

    #[test]
    fn explicit_model_overrides_env() {
        let provider = CheaperInferenceProvider::with_env(
            Some("test-key".into()),
            None,
            Some("explicit-model".into()),
            None,
            Some("env-model".into()),
        )
        .expect("should construct");
        assert_eq!(provider.default_model(), "explicit-model");
    }

    #[test]
    fn explicit_model_override() {
        let provider = CheaperInferenceProvider::new(
            Some("test-key".into()),
            Some("https://proxy.example.com/v1".into()),
            Some("some-model".into()),
        )
        .expect("should construct");
        assert_eq!(provider.default_model(), "some-model");
    }

    // Reads the real process env, so it races with parallel tests. Same
    // convention as the other providers. Run in isolation if needed:
    //   cargo test -p webclaw-llm cheaperinference::tests::env_var -- --ignored --test-threads=1
    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn env_var_model_fallback() {
        unsafe { std::env::set_var("CHEAPER_INFERENCE_MODEL", "env-model") };
        let provider = CheaperInferenceProvider::new(Some("test-key".into()), None, None)
            .expect("should construct");
        assert_eq!(provider.default_model(), "env-model");
        unsafe { std::env::remove_var("CHEAPER_INFERENCE_MODEL") };
    }
}
