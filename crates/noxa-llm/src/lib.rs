/// noxa-llm: LLM integration with Gemini-CLI-first hybrid architecture.
///
/// Provider chain: Gemini CLI (primary) → OpenAI → Ollama → Anthropic.
/// Gemini CLI requires the `gemini` binary on PATH; GEMINI_MODEL env var sets the model.
/// Provides schema-validated extraction (with one retry on parse failure),
/// prompt extraction, and summarization on top of noxa-core's content pipeline.
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
