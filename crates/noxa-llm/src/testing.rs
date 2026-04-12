/// Shared test utilities for noxa-llm.
///
/// Provides a configurable mock LLM provider for unit tests across
/// extract, chain, and other modules that need a fake LLM backend.
#[cfg(test)]
pub(crate) mod mock {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use async_trait::async_trait;

    use crate::error::LlmError;
    use crate::provider::{CompletionRequest, LlmProvider};

    /// A mock LLM provider that returns a canned response or error.
    /// Covers the common test cases: success, failure, and availability.
    pub struct MockProvider {
        pub name: &'static str,
        pub response: Result<String, String>,
        pub available: bool,
    }

    impl MockProvider {
        /// Shorthand for a mock that always succeeds with the given response.
        pub fn ok(response: &str) -> Self {
            Self {
                name: "mock",
                response: Ok(response.to_string()),
                available: true,
            }
        }
    }

    #[async_trait]
    impl LlmProvider for MockProvider {
        async fn complete(&self, _request: &CompletionRequest) -> Result<String, LlmError> {
            match &self.response {
                Ok(text) => Ok(text.clone()),
                Err(msg) => Err(LlmError::ProviderError(msg.clone())),
            }
        }

        async fn is_available(&self) -> bool {
            self.available
        }

        fn name(&self) -> &str {
            self.name
        }
    }

    /// A mock provider that returns responses from a sequence.
    /// Call N → returns responses[N], wrapping at the end.
    /// Useful for testing first-failure / second-success retry paths.
    pub struct SequenceMockProvider {
        pub name: &'static str,
        pub responses: Vec<Result<String, String>>,
        pub available: bool,
        call_count: Arc<AtomicUsize>,
    }

    impl SequenceMockProvider {
        pub fn new(
            name: &'static str,
            responses: Vec<Result<String, String>>,
        ) -> Self {
            Self {
                name,
                responses,
                available: true,
                call_count: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl LlmProvider for SequenceMockProvider {
        async fn complete(&self, _request: &CompletionRequest) -> Result<String, LlmError> {
            let idx = self.call_count.fetch_add(1, Ordering::SeqCst);
            let response = &self.responses[idx.min(self.responses.len() - 1)];
            match response {
                Ok(text) => Ok(text.clone()),
                Err(msg) => Err(LlmError::ProviderError(msg.clone())),
            }
        }

        async fn is_available(&self) -> bool {
            self.available
        }

        fn name(&self) -> &str {
            self.name
        }
    }
}
