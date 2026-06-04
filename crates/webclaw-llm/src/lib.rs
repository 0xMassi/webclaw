//! webclaw-llm: LLM integration with local-first hybrid architecture.
//!
//! Provider chain tries Ollama (local) first, falls back to OpenAI, then Anthropic.
//! Provides schema-based extraction, prompt extraction, and summarization
//! on top of webclaw-core's content pipeline.
//!
//! ```no_run
//! use webclaw_llm::{ProviderChain, LlmProvider, CompletionRequest, Message};
//!
//! # async fn run() -> Result<(), webclaw_llm::LlmError> {
//! // Builds Ollama -> OpenAI -> Anthropic, including only configured providers.
//! let chain = ProviderChain::default().await;
//!
//! let request = CompletionRequest {
//!     model: String::new(), // empty = each provider's default model
//!     messages: vec![Message { role: "user".into(), content: "Hello".into() }],
//!     temperature: None,
//!     max_tokens: None,
//!     json_mode: false,
//! };
//!
//! let answer = chain.complete(&request).await?;
//! println!("{answer}");
//! # Ok(())
//! # }
//! ```
#![deny(unsafe_code)]

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
