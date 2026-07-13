/// Provider chain — tries providers in order until one succeeds.
/// Default order: Ollama (local, free) -> OpenAI -> Atlas Cloud -> Gemini -> Anthropic.
/// Only includes providers that are actually configured/available.
use async_trait::async_trait;
use tracing::{debug, warn};

use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};
use crate::providers::{
    anthropic::AnthropicProvider, atlascloud::AtlasCloudProvider, gemini::GeminiProvider,
    ollama::OllamaProvider, openai::OpenAiProvider,
};

pub struct ProviderChain {
    providers: Vec<Box<dyn LlmProvider>>,
}

impl ProviderChain {
    /// Build the default chain: Ollama -> OpenAI -> Atlas Cloud -> Gemini -> Anthropic.
    /// Ollama is always added (availability checked at call time).
    /// Cloud providers are only added if their API keys are configured.
    /// Gemini sits ahead of Anthropic so Google Cloud credits are preferred,
    /// with Anthropic as the last-resort fallback.
    pub async fn default() -> Self {
        let mut providers: Vec<Box<dyn LlmProvider>> = Vec::new();

        let ollama = OllamaProvider::new(None, None);
        if ollama.is_available().await {
            debug!("ollama is available, adding to chain");
            providers.push(Box::new(ollama));
        } else {
            debug!("ollama not available, skipping");
        }

        if let Some(openai) = OpenAiProvider::new(None, None, None) {
            debug!("openai configured, adding to chain");
            providers.push(Box::new(openai));
        }

        if let Some(atlascloud) = AtlasCloudProvider::new(None, None, None) {
            debug!("atlascloud configured, adding to chain");
            providers.push(Box::new(atlascloud));
        }

        if let Some(gemini) = GeminiProvider::new(None, None, None) {
            debug!("gemini configured, adding to chain");
            providers.push(Box::new(gemini));
        }

        if let Some(anthropic) = AnthropicProvider::with_base_url(None, None, None) {
            debug!("anthropic configured, adding to chain");
            providers.push(Box::new(anthropic));
        }

        Self { providers }
    }

    /// Build a chain with a single explicit provider.
    pub fn single(provider: Box<dyn LlmProvider>) -> Self {
        Self {
            providers: vec![provider],
        }
    }

    /// Build from an explicit list of providers.
    pub fn from_providers(providers: Vec<Box<dyn LlmProvider>>) -> Self {
        Self { providers }
    }

    /// How many providers are in the chain.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }
}

/// ProviderChain itself implements LlmProvider, so it can be used anywhere
/// a single provider is expected. This makes the CLI simple: build a chain
/// or a single provider, pass either as `Box<dyn LlmProvider>`.
#[async_trait]
impl LlmProvider for ProviderChain {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        if self.providers.is_empty() {
            return Err(LlmError::NoProviders);
        }

        let mut errors = Vec::new();

        for provider in &self.providers {
            debug!(provider = provider.name(), "attempting completion");

            match provider.complete(request).await {
                Ok(response) => {
                    debug!(provider = provider.name(), "completion succeeded");
                    return Ok(response);
                }
                Err(e) => {
                    warn!(provider = provider.name(), error = %e, "provider failed, trying next");
                    errors.push(format!("{}: {e}", provider.name()));
                }
            }
        }

        Err(LlmError::AllProvidersFailed(errors.join("; ")))
    }

    async fn is_available(&self) -> bool {
        !self.providers.is_empty()
    }

    fn name(&self) -> &str {
        "chain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;
    use crate::testing::mock::MockProvider;

    fn test_request() -> CompletionRequest {
        CompletionRequest {
            model: String::new(),
            messages: vec![Message {
                role: "user".into(),
                content: "test".into(),
            }],
            temperature: None,
            max_tokens: None,
            json_mode: false,
        }
    }

    #[tokio::test]
    async fn empty_chain_returns_no_providers() {
        let chain = ProviderChain::from_providers(vec![]);
        let result = chain.complete(&test_request()).await;
        assert!(matches!(result, Err(LlmError::NoProviders)));
    }

    #[tokio::test]
    async fn single_provider_success() {
        let chain = ProviderChain::from_providers(vec![Box::new(MockProvider {
            name: "mock",
            response: Ok("hello".into()),
            available: true,
        })]);

        let result = chain.complete(&test_request()).await.unwrap();
        assert_eq!(result, "hello");
    }

    #[tokio::test]
    async fn fallback_on_first_failure() {
        let chain = ProviderChain::from_providers(vec![
            Box::new(MockProvider {
                name: "failing",
                response: Err("connection refused".into()),
                available: true,
            }),
            Box::new(MockProvider {
                name: "backup",
                response: Ok("from backup".into()),
                available: true,
            }),
        ]);

        let result = chain.complete(&test_request()).await.unwrap();
        assert_eq!(result, "from backup");
    }

    #[tokio::test]
    async fn all_fail_collects_errors() {
        let chain = ProviderChain::from_providers(vec![
            Box::new(MockProvider {
                name: "a",
                response: Err("timeout".into()),
                available: true,
            }),
            Box::new(MockProvider {
                name: "b",
                response: Err("rate limited".into()),
                available: true,
            }),
        ]);

        let result = chain.complete(&test_request()).await;
        match result {
            Err(LlmError::AllProvidersFailed(msg)) => {
                assert!(msg.contains("timeout"));
                assert!(msg.contains("rate limited"));
            }
            other => panic!("expected AllProvidersFailed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn chain_length() {
        let chain = ProviderChain::from_providers(vec![
            Box::new(MockProvider {
                name: "a",
                response: Ok("ok".into()),
                available: true,
            }),
            Box::new(MockProvider {
                name: "b",
                response: Ok("ok".into()),
                available: true,
            }),
        ]);
        assert_eq!(chain.len(), 2);
        assert!(!chain.is_empty());
    }
}
