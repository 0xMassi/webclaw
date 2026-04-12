/// LLM-specific errors. Kept flat — one enum covers transport, provider, and parsing failures.
#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("subprocess error: {0}")]
    Subprocess(#[from] std::io::Error),

    #[error("subprocess timed out")]
    Timeout,

    #[error("no providers available")]
    NoProviders,

    #[error("all providers failed: {0}")]
    AllProvidersFailed(String),

    #[error("invalid JSON response: {0}")]
    InvalidJson(String),

    #[error("provider error: {0}")]
    ProviderError(String),
}
