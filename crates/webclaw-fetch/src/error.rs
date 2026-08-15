/// Fetch-layer errors. Wraps HTTP/network failures into a single type
/// that callers can match on without leaking transport details.
use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error)]
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

    #[error("PDF artifact is {actual_bytes} bytes, exceeding caller limit {max_bytes} bytes")]
    PdfArtifactTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },

    #[error("client build failed: {0}")]
    Build(String),
}
