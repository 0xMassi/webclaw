//! PDF text extraction for webclaw.
//!
//! Uses pdf-extract (backed by lopdf) to pull text from PDF bytes.
//! No OCR -- text-based PDFs only. Scanned PDFs return EmptyPdf in Auto mode.
#![forbid(unsafe_code)]

pub mod error;

pub use error::PdfError;

// pdf-extract re-exports all of lopdf via `pub use lopdf::*`
use pdf_extract::{Dictionary, Document, Object};
use tracing::debug;

/// Controls how strictly we treat empty/sparse PDFs.
#[derive(Debug, Clone, Default)]
pub enum PdfMode {
    /// Try text extraction; error if no text found (catches scanned PDFs early).
    #[default]
    Auto,
    /// Return whatever text is found, even if empty. Caller decides what to do.
    Fast,
}

/// Successful PDF extraction output.
#[derive(Debug, Clone)]
pub struct PdfResult {
    pub text: String,
    pub page_count: usize,
    pub metadata: PdfMetadata,
}

/// PDF document metadata from the info dictionary.
#[derive(Debug, Clone, Default)]
pub struct PdfMetadata {
    pub title: Option<String>,
    pub author: Option<String>,
    pub subject: Option<String>,
    pub creator: Option<String>,
}

const MAX_PDF_SIZE: usize = 50 * 1024 * 1024; // 50MB

/// Extract text content from raw PDF bytes.
///
/// Uses pdf-extract for text extraction and lopdf (transitive dep) for
/// metadata and page count. In `Auto` mode, returns `PdfError::EmptyPdf`
/// if no text is found (likely a scanned/image-only PDF).
pub fn extract_pdf(bytes: &[u8], mode: PdfMode) -> Result<PdfResult, PdfError> {
    if bytes.len() > MAX_PDF_SIZE {
        return Err(PdfError::InvalidPdf(format!(
            "PDF too large ({} bytes, max {})",
            bytes.len(),
            MAX_PDF_SIZE
        )));
    }

    if bytes.len() < 5 || &bytes[..5] != b"%PDF-" {
        return Err(PdfError::InvalidPdf("missing PDF header".into()));
    }

    let doc = Document::load_mem(bytes).map_err(|e| PdfError::InvalidPdf(e.to_string()))?;

    let page_count = doc.get_pages().len();
    let metadata = read_metadata(&doc);

    debug!(pages = page_count, "PDF document loaded");

    // Extract text via pdf-extract (higher-level API over lopdf).
    // pdf-extract has bare `panic!`/`unreachable!` sites on malformed input,
    // so we isolate it in catch_unwind: a caught panic becomes a normal
    // ExtractionFailed error instead of unwinding through our callers.
    // AssertUnwindSafe is sound here: the closure only borrows `bytes` (a
    // read-only slice) and we discard all closure state on a caught panic.
    let extracted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        pdf_extract::extract_text_from_mem(bytes)
    }))
    .map_err(|_| PdfError::ExtractionFailed("pdf-extract panicked on malformed input".into()))?;

    let text = extracted.map_err(|e| PdfError::ExtractionFailed(e.to_string()))?;

    let text = normalize_text(&text);

    if text.is_empty() {
        if matches!(mode, PdfMode::Auto) {
            return Err(PdfError::EmptyPdf);
        }
        debug!("PDF text extraction returned empty (Fast mode, returning as-is)");
    }

    debug!(chars = text.len(), "PDF text extracted");

    Ok(PdfResult {
        text,
        page_count,
        metadata,
    })
}

/// Format a PdfResult as markdown for downstream consumers.
///
/// Adds title as a heading if available, followed by the extracted text body.
pub fn to_markdown(result: &PdfResult) -> String {
    let mut out = String::new();

    if let Some(ref title) = result.metadata.title
        && !title.is_empty()
    {
        out.push_str("# ");
        out.push_str(title);
        out.push_str("\n\n");
    }

    out.push_str(&result.text);
    out
}

/// Read metadata from the PDF info dictionary.
/// Gracefully returns defaults for any missing or unreadable fields.
fn read_metadata(doc: &Document) -> PdfMetadata {
    let info = match doc.trailer.get(b"Info") {
        Ok(obj) => match doc.dereference(obj) {
            Ok((_, Object::Dictionary(dict))) => Some(dict),
            _ => None,
        },
        Err(_) => None,
    };

    let Some(info) = info else {
        return PdfMetadata::default();
    };

    PdfMetadata {
        title: info_string(info, b"Title"),
        author: info_string(info, b"Author"),
        subject: info_string(info, b"Subject"),
        creator: info_string(info, b"Creator"),
    }
}

/// Extract a string value from a PDF info dictionary entry.
/// Handles both String and Name object types.
fn info_string(dict: &Dictionary, key: &[u8]) -> Option<String> {
    let obj = dict.get(key).ok()?;
    let raw = match obj {
        Object::String(bytes, _) => bytes.clone(),
        Object::Name(bytes) => bytes.clone(),
        _ => return None,
    };

    // PDF strings can be UTF-16BE (BOM: FE FF) or PDFDocEncoding (~Latin-1)
    let text = if raw.len() >= 2 && raw[0] == 0xFE && raw[1] == 0xFF {
        // UTF-16BE: skip BOM, decode pairs
        let pairs: Vec<u16> = raw[2..]
            .chunks_exact(2)
            .map(|c| u16::from_be_bytes([c[0], c[1]]))
            .collect();
        String::from_utf16_lossy(&pairs)
    } else {
        // PDFDocEncoding -- first 128 chars match ASCII, rest is Latin-1-ish.
        // Good enough: lossy UTF-8 covers the common case.
        String::from_utf8_lossy(&raw).into_owned()
    };

    let trimmed = text.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Collapse excessive whitespace from pdf-extract output.
/// PDF text extraction often produces irregular spacing and blank lines.
fn normalize_text(raw: &str) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut prev_blank = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !prev_blank && !lines.is_empty() {
                lines.push("");
                prev_blank = true;
            }
        } else {
            lines.push(trimmed);
            prev_blank = false;
        }
    }

    // Strip trailing blank lines
    while lines.last() == Some(&"") {
        lines.pop();
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pdf_metadata_default() {
        let meta = PdfMetadata::default();
        assert!(meta.title.is_none());
        assert!(meta.author.is_none());
        assert!(meta.subject.is_none());
        assert!(meta.creator.is_none());
    }

    #[test]
    fn test_to_markdown_with_title() {
        let result = PdfResult {
            text: "Hello world.\n\nSecond paragraph.".into(),
            page_count: 1,
            metadata: PdfMetadata {
                title: Some("Test Document".into()),
                ..Default::default()
            },
        };

        let md = to_markdown(&result);
        assert!(md.starts_with("# Test Document\n\n"));
        assert!(md.contains("Hello world."));
        assert!(md.contains("Second paragraph."));
    }

    #[test]
    fn test_to_markdown_without_title() {
        let result = PdfResult {
            text: "Just text.".into(),
            page_count: 1,
            metadata: PdfMetadata::default(),
        };

        let md = to_markdown(&result);
        assert_eq!(md, "Just text.");
    }

    #[test]
    fn test_to_markdown_empty_title_skipped() {
        let result = PdfResult {
            text: "Body.".into(),
            page_count: 1,
            metadata: PdfMetadata {
                title: Some("".into()),
                ..Default::default()
            },
        };

        let md = to_markdown(&result);
        assert!(!md.starts_with('#'));
        assert_eq!(md, "Body.");
    }

    #[test]
    fn test_empty_bytes_returns_error() {
        let result = extract_pdf(&[], PdfMode::Auto);
        assert!(matches!(result, Err(PdfError::InvalidPdf(_))));
    }

    #[test]
    fn test_garbage_bytes_returns_error() {
        let result = extract_pdf(b"not a pdf at all", PdfMode::Auto);
        assert!(matches!(result, Err(PdfError::InvalidPdf(_))));
    }

    #[test]
    fn test_truncated_pdf_header_returns_error() {
        // Has the PDF magic but nothing else -- lopdf will reject it
        let result = extract_pdf(b"%PDF-1.4\n", PdfMode::Auto);
        assert!(result.is_err());
    }

    #[test]
    fn test_oversized_pdf_rejected() {
        let big = vec![0u8; MAX_PDF_SIZE + 1];
        let result = extract_pdf(&big, PdfMode::Auto);
        assert!(matches!(result, Err(PdfError::InvalidPdf(msg)) if msg.contains("too large")));
    }

    #[test]
    fn test_normalize_text_collapses_blanks() {
        let input = "Line one.\n\n\n\nLine two.\n\n\n";
        let output = normalize_text(input);
        assert_eq!(output, "Line one.\n\nLine two.");
    }

    #[test]
    fn test_normalize_text_trims_lines() {
        let input = "  hello  \n  world  ";
        let output = normalize_text(input);
        assert_eq!(output, "hello\nworld");
    }

    #[test]
    fn test_normalize_text_empty() {
        assert_eq!(normalize_text(""), "");
        assert_eq!(normalize_text("  \n  \n  "), "");
    }
}
