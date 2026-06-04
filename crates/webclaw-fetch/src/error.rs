/// Fetch-layer errors. Wraps HTTP/network failures into a single type
/// that callers can match on without leaking transport details.
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum FetchError {
    #[error("request failed: {0}")]
    Request(#[from] wreq::Error),

    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("response body decode failed: {0}")]
    BodyDecode(String),

    #[error("extraction failed: {0}")]
    Extraction(#[from] webclaw_core::ExtractError),

    #[error("PDF extraction failed: {0}")]
    Pdf(#[from] webclaw_pdf::PdfError),

    #[error("client build failed: {0}")]
    Build(String),
}
