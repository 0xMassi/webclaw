/// webclaw-llm: LLM integration with local-first hybrid architecture.
///
/// Provider chain tries Ollama (local) first, falls back to OpenAI, then Gemini, then Anthropic.
/// Provides schema-based extraction, prompt extraction, and summarization
/// on top of webclaw-core's content pipeline.
pub mod chain;
pub mod clean;
pub mod error;
pub mod extract;
pub mod provider;
pub mod providers;
pub mod summarize;
#[cfg(test)]
pub(crate) mod testing;

pub use chain::ProviderChain;
pub use clean::strip_thinking_tags;
pub use error::LlmError;
pub use provider::{CompletionRequest, LlmProvider, Message};
