/// Extraction errors — kept minimal since this crate does no I/O.
/// Most failures come from malformed HTML or invalid URLs.
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ExtractError {
    #[error("failed to parse HTML")]
    ParseError,

    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    #[error("no content found")]
    NoContent,
}
