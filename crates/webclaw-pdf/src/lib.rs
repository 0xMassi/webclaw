/// PDF text extraction for webclaw.
///
/// Uses pdf-extract (backed by lopdf) to pull text from PDF bytes.
/// No OCR -- text-based PDFs only. In Auto mode, PDFs with no extracted
/// non-whitespace text return EmptyPdf.
pub mod error;

pub use error::PdfError;

// pdf-extract re-exports all of lopdf via `pub use lopdf::*`
use pdf_extract::{Dictionary, Document, Object};
use std::collections::HashSet;
use std::ops::Range;
use tracing::debug;

/// Controls how strictly we treat empty/sparse PDFs.
#[derive(Debug, Clone, Default)]
pub enum PdfMode {
    /// Try text extraction; error if the result has no non-whitespace text.
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
const MAX_PDF_PAGES: usize = 10_000;
const PDF_HEADER_WINDOW: usize = 1_024;
const PDF_WHITESPACE: &[u8] = b" \t\n\r\0\x0C";

/// Extract text content from raw PDF bytes.
///
/// Uses pdf-extract for text extraction and lopdf (transitive dep) for
/// metadata and page count. In `Auto` mode, returns `PdfError::EmptyPdf`
/// when the normalized extraction contains no non-whitespace text. This is
/// narrower than detecting every scanned or image-only document: any text layer,
/// page number, footer, or OCR residue that survives normalization returns `Ok`.
pub fn extract_pdf(bytes: &[u8], mode: PdfMode) -> Result<PdfResult, PdfError> {
    if bytes.len() > MAX_PDF_SIZE {
        return Err(PdfError::InvalidPdf(format!(
            "PDF too large ({} bytes, max {})",
            bytes.len(),
            MAX_PDF_SIZE
        )));
    }

    let header_window = &bytes[..bytes.len().min(PDF_HEADER_WINDOW + 4)];
    if !header_window.windows(5).any(|window| window == b"%PDF-") {
        return Err(PdfError::InvalidPdf("missing PDF header".into()));
    }

    let repaired;
    let (bytes, doc) = match Document::load_mem(bytes) {
        Ok(doc) => (bytes, doc),
        Err(e) => {
            let invalid = || PdfError::InvalidPdf(e.to_string());
            repaired = repair_xref_offsets(bytes).ok_or_else(invalid)?;
            let doc = Document::load_mem(&repaired).map_err(|_| invalid())?;
            (repaired.as_slice(), doc)
        }
    };

    let page_count = doc.page_iter().take(MAX_PDF_PAGES + 1).count();
    if page_count > MAX_PDF_PAGES {
        return Err(PdfError::InvalidPdf(format!(
            "PDF has too many pages ({page_count}, max {MAX_PDF_PAGES})"
        )));
    }
    let metadata = read_metadata(&doc);

    debug!(pages = page_count, "PDF document loaded");

    let text = match std::panic::catch_unwind(|| pdf_extract::extract_text_from_mem(bytes)) {
        Ok(Ok(text)) => text,
        _ => {
            tracing::warn!("PDF layout extraction failed; trying text-only extraction");
            let pages: Vec<_> = doc.get_pages().into_keys().collect();
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                doc.extract_text(&pages)
            }))
            .map_err(|_| PdfError::ExtractionFailed("text parser panicked".into()))?
            .map_err(|e| PdfError::ExtractionFailed(e.to_string()))?
        }
    };

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

fn repair_xref_offsets(bytes: &[u8]) -> Option<Vec<u8>> {
    let bytes = &bytes[find(bytes, b"%PDF-")?..];
    let tail = bytes.len().saturating_sub(512);
    let at = tail + rfind(&bytes[tail..], b"startxref")? + b"startxref".len();
    let (startxref, first) = xref_offset(bytes, at)?;
    let mut repaired = bytes.to_vec();
    let mut xref = first;
    let mut seen = HashSet::new();
    while seen.insert(xref) {
        let Some(trailer) = find(&bytes[xref..], b"trailer").map(|pos| xref + pos) else {
            break;
        };
        let dict = &bytes[trailer..bytes.len().min(trailer + 1_024)];
        let dict = &dict[..find(dict, b"startxref").unwrap_or(dict.len())];
        let Some((digits, prev)) =
            find(dict, b"/Prev").and_then(|pos| xref_offset(bytes, trailer + pos + b"/Prev".len()))
        else {
            break;
        };
        let fixed = prev.to_string();
        if fixed.len() == digits.len() {
            repaired[digits].copy_from_slice(fixed.as_bytes());
        }
        xref = prev;
    }
    repaired.splice(startxref, first.to_string().into_bytes());
    (repaired != bytes).then_some(repaired)
}

fn xref_offset(bytes: &[u8], at: usize) -> Option<(Range<usize>, usize)> {
    let start = at
        + bytes
            .get(at..)?
            .iter()
            .take_while(|b| PDF_WHITESPACE.contains(b))
            .count();
    let len = bytes[start..]
        .iter()
        .take_while(|b| b.is_ascii_digit())
        .count();
    let offset: usize = std::str::from_utf8(&bytes[start..start + len])
        .ok()?
        .parse()
        .ok()?;
    let target = bytes.get(offset..)?;
    let skip = target
        .iter()
        .take_while(|b| PDF_WHITESPACE.contains(b))
        .count();
    target[skip..]
        .starts_with(b"xref")
        .then_some((start..start + len, offset + skip))
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
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
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_be_bytes)
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

    fn text_pdf(content: &[u8]) -> Vec<u8> {
        use pdf_extract::{Stream, dictionary};
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font = doc.add_object(
            dictionary! {"Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Helvetica"},
        );
        let resources = doc.add_object(dictionary! {"Font" => dictionary! {"F1" => font}});
        let content = doc.add_object(Stream::new(dictionary! {}, content.to_vec()));
        let page = doc.add_object(
            dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => content},
        );
        doc.objects.insert(pages_id, dictionary! {"Type" => "Pages", "Kids" => vec![Object::Reference(page)], "Count" => 1, "Resources" => resources, "MediaBox" => vec![0.into(), 0.into(), 600.into(), 800.into()]}.into());
        let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
        doc.trailer.set("Root", catalog);
        doc.reference_table.cross_reference_type = pdf_extract::xref::XrefType::CrossReferenceTable;
        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn malformed_drawing_operator_preserves_text_without_panicking() {
        let bytes = text_pdf(b"BT /F1 12 Tf 10 10 Td (Readable fallback text) Tj ET m");
        let result = extract_pdf(&bytes, PdfMode::Auto).unwrap();
        assert!(result.text.contains("Readable fallback text"));
        assert_eq!(result.page_count, 1);
    }

    fn startxref(bytes: &[u8]) -> (usize, usize, usize) {
        let tail = bytes.len() - 64;
        let at = tail + find(&bytes[tail..], b"startxref").unwrap() + b"startxref\n".len();
        let digits = bytes[at..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        let xref = std::str::from_utf8(&bytes[at..at + digits])
            .unwrap()
            .parse()
            .unwrap();
        (at, digits, xref)
    }

    fn point_startxref_at(bytes: &[u8], offset: usize) -> Vec<u8> {
        let (at, digits, _) = startxref(bytes);
        let mut drifted = bytes.to_vec();
        drifted.splice(at..at + digits, offset.to_string().into_bytes());
        drifted
    }

    #[test]
    fn xref_offset_pointing_at_preceding_whitespace_still_loads() {
        let bytes = text_pdf(b"BT /F1 12 Tf 10 10 Td (Offset drift text) Tj ET");
        let (_, _, xref) = startxref(&bytes);
        assert_eq!(&bytes[xref - 1..xref + 4], b"\nxref");
        let drifted = point_startxref_at(&bytes, xref - 1);
        assert!(Document::load_mem(&drifted).is_err());
        let result = extract_pdf(&drifted, PdfMode::Auto).unwrap();
        assert!(result.text.contains("Offset drift text"));
    }

    #[test]
    fn xref_repair_follows_prev_into_earlier_sections() {
        let bytes = text_pdf(b"BT /F1 12 Tf 10 10 Td (Earlier revision text) Tj ET");
        let (_, _, xref) = startxref(&bytes);
        let trailer = find(&bytes[xref..], b"<<").unwrap() + xref;
        let close = find(&bytes[trailer..], b">>").unwrap() + trailer;
        let dict = std::str::from_utf8(&bytes[trailer..close]).unwrap();
        let mut updated = bytes.clone();
        let section = updated.len() + 1;
        updated.extend_from_slice(
            format!(
                "\nxref\n0 1\n0000000000 65535 f \ntrailer\n{dict}/Prev {}>>\nstartxref\n{section}\n%%EOF\n",
                xref - 1
            )
            .as_bytes(),
        );
        assert!(Document::load_mem(&updated).is_err());
        let result = extract_pdf(&updated, PdfMode::Auto).unwrap();
        assert!(result.text.contains("Earlier revision text"));
    }

    #[test]
    fn xref_repair_leaves_stream_text_alone() {
        let content = |n: usize| format!("BT /F1 12 Tf 10 10 Td (See startxref {n}) Tj ET");
        let (_, _, xref) = startxref(&text_pdf(content(100).as_bytes()));
        let bytes = text_pdf(content(xref - 1).as_bytes());
        assert_eq!(startxref(&bytes).2, xref);
        let drifted = point_startxref_at(&bytes, xref - 1);
        let result = extract_pdf(&drifted, PdfMode::Auto).unwrap();
        assert!(result.text.contains(&format!("See startxref {}", xref - 1)));
    }

    #[test]
    fn xref_repair_counts_offsets_from_the_pdf_header() {
        let bytes = text_pdf(b"BT /F1 12 Tf 10 10 Td (Prefixed text) Tj ET");
        let (_, _, xref) = startxref(&bytes);
        let mut prefixed = b"junk\n".to_vec();
        prefixed.extend(point_startxref_at(&bytes, xref - 1));
        let result = extract_pdf(&prefixed, PdfMode::Auto).unwrap();
        assert!(result.text.contains("Prefixed text"));
    }

    #[test]
    fn xref_repair_skips_nul_padding() {
        let bytes = text_pdf(b"BT /F1 12 Tf 10 10 Td (Nul padded text) Tj ET");
        let (_, _, xref) = startxref(&bytes);
        let mut padded = point_startxref_at(&bytes, xref - 1);
        padded[xref - 1] = 0;
        let result = extract_pdf(&padded, PdfMode::Auto).unwrap();
        assert!(result.text.contains("Nul padded text"));
    }

    #[test]
    fn xref_repair_crosses_a_digit_boundary() {
        let content = |pad: usize| {
            format!(
                "BT /F1 12 Tf 10 10 Td (Boundary text) Tj ET{}",
                " ".repeat(pad)
            )
        };
        let bytes = (0..1000)
            .map(|pad| text_pdf(content(pad).as_bytes()))
            .find(|bytes| startxref(bytes).2 == 1000)
            .unwrap();
        let drifted = point_startxref_at(&bytes, 999);
        let result = extract_pdf(&drifted, PdfMode::Auto).unwrap();
        assert!(result.text.contains("Boundary text"));
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
    fn test_pdf_header_is_allowed_within_spec_window() {
        // ISO 32000 permits the header to appear within the first 1024 bytes.
        // This remains invalid as a whole document, but it must pass our
        // lightweight header precheck and reach lopdf's parser.
        let mut bytes = vec![b' '; 32];
        bytes.extend_from_slice(b"%PDF-1.7\n");
        let result = extract_pdf(&bytes, PdfMode::Auto);
        assert!(
            matches!(result, Err(PdfError::InvalidPdf(msg)) if !msg.contains("missing PDF header"))
        );
    }

    #[test]
    fn test_pdf_header_at_last_allowed_offset_reaches_parser() {
        let mut bytes = vec![b' '; PDF_HEADER_WINDOW - 1];
        bytes.extend_from_slice(b"%PDF-1.7\n");
        let result = extract_pdf(&bytes, PdfMode::Auto);
        assert!(
            matches!(result, Err(PdfError::InvalidPdf(msg)) if !msg.contains("missing PDF header"))
        );
    }

    #[test]
    fn test_pdf_header_immediately_after_spec_window_is_rejected() {
        let mut bytes = vec![b' '; PDF_HEADER_WINDOW];
        bytes.extend_from_slice(b"%PDF-1.7\n");
        let result = extract_pdf(&bytes, PdfMode::Auto);
        assert!(
            matches!(result, Err(PdfError::InvalidPdf(msg)) if msg.contains("missing PDF header"))
        );
    }

    #[test]
    fn test_pdf_header_after_spec_window_is_rejected() {
        let mut bytes = vec![b' '; PDF_HEADER_WINDOW + 1];
        bytes.extend_from_slice(b"%PDF-1.7\n");
        let result = extract_pdf(&bytes, PdfMode::Auto);
        assert!(
            matches!(result, Err(PdfError::InvalidPdf(msg)) if msg.contains("missing PDF header"))
        );
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
