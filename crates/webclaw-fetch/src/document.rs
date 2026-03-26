/// Document extraction for DOCX, XLSX, XLS, and CSV files.
/// Auto-detects document type from Content-Type headers or URL extension,
/// then extracts text content as markdown — same pattern as PDF extraction.
use std::collections::HashMap;
use std::io::{Cursor, Read};

use tracing::debug;

use crate::error::FetchError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocType {
    Docx,
    Xlsx,
    Xls,
    Csv,
}

impl DocType {
    fn label(self) -> &'static str {
        match self {
            DocType::Docx => "DOCX",
            DocType::Xlsx => "XLSX",
            DocType::Xls => "XLS",
            DocType::Csv => "CSV",
        }
    }
}

/// Detect document type from response headers or URL extension.
/// Returns `None` for non-document responses (HTML, PDF, etc.).
pub fn is_document_content_type(headers: &HashMap<String, String>, url: &str) -> Option<DocType> {
    // Check Content-Type header first
    if let Some(ct) = headers.get("content-type") {
        let mime = ct.split(';').next().unwrap_or("").trim();

        if mime.eq_ignore_ascii_case(
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        ) {
            return Some(DocType::Docx);
        }
        if mime.eq_ignore_ascii_case(
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        ) {
            return Some(DocType::Xlsx);
        }
        if mime.eq_ignore_ascii_case("application/vnd.ms-excel") {
            return Some(DocType::Xls);
        }
        if mime.eq_ignore_ascii_case("text/csv") {
            return Some(DocType::Csv);
        }
    }

    // Fall back to URL extension
    let path = url.split('?').next().unwrap_or(url);
    let lower = path.to_ascii_lowercase();

    if lower.ends_with(".docx") {
        return Some(DocType::Docx);
    }
    if lower.ends_with(".xlsx") {
        return Some(DocType::Xlsx);
    }
    if lower.ends_with(".xls") {
        return Some(DocType::Xls);
    }
    if lower.ends_with(".csv") {
        return Some(DocType::Csv);
    }

    None
}

/// Extract text content from document bytes, returning an ExtractionResult.
pub fn extract_document(
    bytes: &[u8],
    doc_type: DocType,
) -> Result<webclaw_core::ExtractionResult, FetchError> {
    debug!(
        doc_type = doc_type.label(),
        bytes = bytes.len(),
        "extracting document"
    );

    let markdown = match doc_type {
        DocType::Docx => extract_docx(bytes)?,
        DocType::Xlsx => extract_xlsx(bytes)?,
        DocType::Xls => extract_xls(bytes)?,
        DocType::Csv => extract_csv(bytes)?,
    };

    let plain_text = strip_markdown_formatting(&markdown);
    let word_count = plain_text.split_whitespace().count();

    Ok(webclaw_core::ExtractionResult {
        metadata: webclaw_core::Metadata {
            title: None,
            description: None,
            author: None,
            published_date: None,
            language: None,
            url: None,
            site_name: None,
            image: None,
            favicon: None,
            word_count,
        },
        content: webclaw_core::Content {
            markdown,
            plain_text,
            links: Vec::new(),
            images: Vec::new(),
            code_blocks: Vec::new(),
            raw_html: None,
        },
        domain_data: None,
        structured_data: vec![],
    })
}

/// Extract text from a DOCX file (ZIP of XML).
/// Reads `word/document.xml`, extracts `<w:t>` text nodes, detects heading styles.
fn extract_docx(bytes: &[u8]) -> Result<String, FetchError> {
    let cursor = Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| FetchError::Build(format!("DOCX zip: {e}")))?;

    let xml = {
        let mut file = archive
            .by_name("word/document.xml")
            .map_err(|e| FetchError::Build(format!("DOCX missing document.xml: {e}")))?;
        let mut buf = String::new();
        file.read_to_string(&mut buf)
            .map_err(|e| FetchError::BodyDecode(format!("DOCX read: {e}")))?;
        buf
    };

    parse_docx_xml(&xml)
}

/// Parse DOCX XML (word/document.xml) into markdown.
///
/// Walks the XML looking for paragraph elements (`<w:p>`). Within each paragraph,
/// collects text from `<w:t>` tags and detects heading styles from `<w:pStyle>`.
fn parse_docx_xml(xml: &str) -> Result<String, FetchError> {
    use quick_xml::Reader;
    use quick_xml::events::Event;

    let mut reader = Reader::from_str(xml);
    let mut paragraphs: Vec<String> = Vec::new();

    // State tracking for the current paragraph
    let mut in_paragraph = false;
    let mut in_run = false; // inside <w:r> (run)
    let mut in_text = false; // inside <w:t>
    let mut current_text = String::new();
    let mut heading_level: Option<u8> = 0.into(); // None = normal paragraph
    let mut in_ppr = false; // inside <w:pPr> (paragraph properties)

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let local = local_name(&name_bytes);
                match local {
                    b"p" if is_w_namespace(&name_bytes) => {
                        in_paragraph = true;
                        current_text.clear();
                        heading_level = None;
                    }
                    b"pPr" if in_paragraph => in_ppr = true,
                    b"pStyle" if in_ppr => {
                        heading_level = extract_heading_level(e);
                    }
                    b"r" if in_paragraph => in_run = true,
                    b"t" if in_run => in_text = true,
                    b"br" if in_paragraph => {
                        current_text.push('\n');
                    }
                    b"tab" if in_paragraph => {
                        current_text.push('\t');
                    }
                    _ => {}
                }
            }
            Ok(Event::End(ref e)) => {
                let name_bytes = e.name().as_ref().to_vec();
                let local = local_name(&name_bytes);
                match local {
                    b"p" if in_paragraph => {
                        let text = current_text.trim().to_string();
                        if !text.is_empty() {
                            let formatted = match heading_level {
                                Some(1) => format!("# {text}"),
                                Some(2) => format!("## {text}"),
                                Some(3) => format!("### {text}"),
                                Some(4) => format!("#### {text}"),
                                Some(5) => format!("##### {text}"),
                                Some(6) => format!("###### {text}"),
                                _ => text,
                            };
                            paragraphs.push(formatted);
                        }
                        in_paragraph = false;
                    }
                    b"pPr" => in_ppr = false,
                    b"r" => {
                        in_run = false;
                        in_text = false;
                    }
                    b"t" => in_text = false,
                    _ => {}
                }
            }
            Ok(Event::Text(ref e)) if in_text => {
                if let Ok(text) = e.unescape() {
                    current_text.push_str(&text);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(FetchError::Build(format!("DOCX XML parse error: {e}")));
            }
            _ => {}
        }
    }

    Ok(paragraphs.join("\n\n"))
}

/// Check if a qualified name belongs to the `w:` (wordprocessingML) namespace.
/// Handles both `w:p` (prefixed) and just `p` (default namespace) forms.
fn is_w_namespace(name: &[u8]) -> bool {
    // quick-xml gives us the full name bytes. Accept both "w:p" and "p".
    name == b"w:p" || name == b"p"
}

/// Extract the local name from a possibly namespaced XML tag.
/// `w:p` -> `p`, `p` -> `p`
fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().position(|&b| b == b':') {
        Some(pos) => &name[pos + 1..],
        None => name,
    }
}

/// Extract heading level from a `<w:pStyle w:val="Heading1"/>` element.
fn extract_heading_level(e: &quick_xml::events::BytesStart) -> Option<u8> {
    for attr in e.attributes().flatten() {
        let local = local_name(attr.key.as_ref());
        if local == b"val" {
            let val = String::from_utf8_lossy(&attr.value);
            let lower = val.to_ascii_lowercase();

            // Match "heading1", "heading2", etc. and "title" -> h1
            if lower == "title" {
                return Some(1);
            }
            if let Some(rest) = lower.strip_prefix("heading")
                && let Ok(n) = rest.parse::<u8>()
            {
                return Some(n.min(6));
            }
        }
    }
    None
}

/// Extract spreadsheet content using calamine (XLSX format).
fn extract_xlsx(bytes: &[u8]) -> Result<String, FetchError> {
    extract_spreadsheet(bytes, "XLSX")
}

/// Extract spreadsheet content using calamine (XLS format).
fn extract_xls(bytes: &[u8]) -> Result<String, FetchError> {
    extract_spreadsheet(bytes, "XLS")
}

/// Shared spreadsheet extraction for both XLSX and XLS via calamine.
/// Reads all sheets and formats each as a markdown table.
fn extract_spreadsheet(bytes: &[u8], label: &str) -> Result<String, FetchError> {
    use calamine::Reader;

    let cursor = Cursor::new(bytes);
    let mut workbook: calamine::Sheets<_> = calamine::open_workbook_auto_from_rs(cursor)
        .map_err(|e| FetchError::Build(format!("{label} open: {e}")))?;

    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();
    let mut sections: Vec<String> = Vec::new();

    for name in &sheet_names {
        let range = workbook
            .worksheet_range(name)
            .map_err(|e| FetchError::Build(format!("{label} sheet '{name}': {e}")))?;

        let rows: Vec<Vec<String>> = range
            .rows()
            .map(|row| row.iter().map(cell_to_string).collect())
            .collect();

        if rows.is_empty() {
            continue;
        }

        let mut section = format!("## Sheet: {name}\n\n");
        section.push_str(&rows_to_markdown_table(&rows));
        sections.push(section);
    }

    if sections.is_empty() {
        return Ok("(empty spreadsheet)".to_string());
    }

    Ok(sections.join("\n\n"))
}

/// Convert a calamine cell value to a display string.
fn cell_to_string(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Empty => String::new(),
        Data::String(s) => s.clone(),
        Data::Int(n) => n.to_string(),
        Data::Float(f) => format_float(*f),
        Data::Bool(b) => b.to_string(),
        Data::Error(e) => format!("#{e:?}"),
        Data::DateTime(dt) => format!("{dt}"),
        Data::DateTimeIso(s) => s.clone(),
        Data::DurationIso(s) => s.clone(),
    }
}

/// Format a float, dropping trailing `.0` for clean integer display.
fn format_float(f: f64) -> String {
    if f.fract() == 0.0 && f.abs() < i64::MAX as f64 {
        format!("{}", f as i64)
    } else {
        format!("{f}")
    }
}

/// Extract CSV text and convert to markdown table.
fn extract_csv(bytes: &[u8]) -> Result<String, FetchError> {
    let text = String::from_utf8_lossy(bytes);
    let rows = parse_csv_rows(&text);

    if rows.is_empty() {
        return Ok("(empty CSV)".to_string());
    }

    Ok(rows_to_markdown_table(&rows))
}

/// Parse CSV text into rows of fields, handling quoted fields with commas/newlines.
fn parse_csv_rows(text: &str) -> Vec<Vec<String>> {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                // Escaped quote ("") or end of quoted field
                if chars.peek() == Some(&'"') {
                    chars.next();
                    current_field.push('"');
                } else {
                    in_quotes = false;
                }
            } else {
                current_field.push(ch);
            }
        } else {
            match ch {
                '"' => in_quotes = true,
                ',' => {
                    current_row.push(current_field.trim().to_string());
                    current_field = String::new();
                }
                '\n' => {
                    current_row.push(current_field.trim().to_string());
                    current_field = String::new();
                    if !current_row.iter().all(|f| f.is_empty()) {
                        rows.push(current_row);
                    }
                    current_row = Vec::new();
                }
                '\r' => {
                    // Skip carriage returns (handled with \n)
                }
                _ => current_field.push(ch),
            }
        }
    }

    // Flush last field/row
    if !current_field.is_empty() || !current_row.is_empty() {
        current_row.push(current_field.trim().to_string());
        if !current_row.iter().all(|f| f.is_empty()) {
            rows.push(current_row);
        }
    }

    rows
}

/// Convert rows (first row = header) into a markdown table.
fn rows_to_markdown_table(rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return String::new();
    }

    // Find the max column count across all rows
    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if col_count == 0 {
        return String::new();
    }

    let mut lines: Vec<String> = Vec::new();

    // Header row
    let header = &rows[0];
    let header_cells: Vec<&str> = (0..col_count)
        .map(|i| header.get(i).map(|s| s.as_str()).unwrap_or(""))
        .collect();
    lines.push(format!("| {} |", header_cells.join(" | ")));

    // Separator row
    let sep: Vec<&str> = vec!["---"; col_count];
    lines.push(format!("| {} |", sep.join(" | ")));

    // Data rows
    for row in &rows[1..] {
        let cells: Vec<&str> = (0..col_count)
            .map(|i| row.get(i).map(|s| s.as_str()).unwrap_or(""))
            .collect();
        lines.push(format!("| {} |", cells.join(" | ")));
    }

    lines.join("\n")
}

/// Strip markdown formatting to get plain text.
fn strip_markdown_formatting(markdown: &str) -> String {
    let mut plain = String::with_capacity(markdown.len());
    for line in markdown.lines() {
        let trimmed = line.trim_start_matches('#').trim();
        if trimmed.starts_with("| ---") || trimmed == "|---|" {
            continue; // Skip separator rows
        }
        if let Some(stripped) = trimmed.strip_prefix('|')
            && let Some(stripped) = stripped.strip_suffix('|')
        {
            // Table row: join cells with spaces
            let cells: Vec<&str> = stripped.split('|').map(|c| c.trim()).collect();
            plain.push_str(&cells.join(" "));
            plain.push('\n');
            continue;
        }
        plain.push_str(trimmed);
        plain.push('\n');
    }
    plain.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Content-type detection ---

    #[test]
    fn test_detect_docx_content_type() {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document".to_string(),
        );
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/file"),
            Some(DocType::Docx)
        );
    }

    #[test]
    fn test_detect_xlsx_content_type() {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet".to_string(),
        );
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/file"),
            Some(DocType::Xlsx)
        );
    }

    #[test]
    fn test_detect_xls_content_type() {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "application/vnd.ms-excel".to_string(),
        );
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/file"),
            Some(DocType::Xls)
        );
    }

    #[test]
    fn test_detect_csv_content_type() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/csv".to_string());
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/file"),
            Some(DocType::Csv)
        );
    }

    #[test]
    fn test_detect_csv_content_type_with_charset() {
        let mut headers = HashMap::new();
        headers.insert(
            "content-type".to_string(),
            "text/csv; charset=utf-8".to_string(),
        );
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/file"),
            Some(DocType::Csv)
        );
    }

    #[test]
    fn test_detect_by_url_extension() {
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            is_document_content_type(&empty, "https://example.com/report.docx"),
            Some(DocType::Docx)
        );
        assert_eq!(
            is_document_content_type(&empty, "https://example.com/data.xlsx"),
            Some(DocType::Xlsx)
        );
        assert_eq!(
            is_document_content_type(&empty, "https://example.com/old.xls"),
            Some(DocType::Xls)
        );
        assert_eq!(
            is_document_content_type(&empty, "https://example.com/data.csv"),
            Some(DocType::Csv)
        );
    }

    #[test]
    fn test_detect_url_extension_with_query() {
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            is_document_content_type(&empty, "https://example.com/report.docx?token=abc"),
            Some(DocType::Docx)
        );
    }

    #[test]
    fn test_detect_url_extension_case_insensitive() {
        let empty: HashMap<String, String> = HashMap::new();
        assert_eq!(
            is_document_content_type(&empty, "https://example.com/FILE.XLSX"),
            Some(DocType::Xlsx)
        );
    }

    #[test]
    fn test_detect_none_for_html() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/html".to_string());
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/page"),
            None
        );
    }

    #[test]
    fn test_content_type_takes_precedence_over_url() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "text/csv".to_string());
        // URL says .xlsx but Content-Type says CSV — header wins
        assert_eq!(
            is_document_content_type(&headers, "https://example.com/data.xlsx"),
            Some(DocType::Csv)
        );
    }

    // --- CSV parsing ---

    #[test]
    fn test_csv_simple() {
        let csv = "Name,Age,City\nAlice,30,NYC\nBob,25,LA\n";
        let result = extract_csv(csv.as_bytes()).unwrap();
        assert!(result.contains("| Name | Age | City |"));
        assert!(result.contains("| --- | --- | --- |"));
        assert!(result.contains("| Alice | 30 | NYC |"));
        assert!(result.contains("| Bob | 25 | LA |"));
    }

    #[test]
    fn test_csv_quoted_fields() {
        let csv = "Name,Description\nAlice,\"Has a, comma\"\nBob,\"Said \"\"hello\"\"\"\n";
        let result = extract_csv(csv.as_bytes()).unwrap();
        assert!(result.contains("Has a, comma"));
        assert!(result.contains("Said \"hello\""));
    }

    #[test]
    fn test_csv_empty() {
        let result = extract_csv(b"").unwrap();
        assert_eq!(result, "(empty CSV)");
    }

    #[test]
    fn test_csv_windows_line_endings() {
        let csv = "A,B\r\n1,2\r\n3,4\r\n";
        let result = extract_csv(csv.as_bytes()).unwrap();
        assert!(result.contains("| A | B |"));
        assert!(result.contains("| 1 | 2 |"));
    }

    // --- DOCX XML parsing ---

    #[test]
    fn test_docx_xml_simple_paragraphs() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:r><w:t>Hello world</w:t></w:r></w:p>
    <w:p><w:r><w:t>Second paragraph</w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let result = parse_docx_xml(xml).unwrap();
        assert_eq!(result, "Hello world\n\nSecond paragraph");
    }

    #[test]
    fn test_docx_xml_headings() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:t>Title</w:t></w:r>
    </w:p>
    <w:p><w:r><w:t>Body text</w:t></w:r></w:p>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading2"/></w:pPr>
      <w:r><w:t>Subtitle</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let result = parse_docx_xml(xml).unwrap();
        assert!(result.contains("# Title"));
        assert!(result.contains("Body text"));
        assert!(result.contains("## Subtitle"));
    }

    #[test]
    fn test_docx_xml_multiple_runs() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p>
      <w:r><w:t>Hello </w:t></w:r>
      <w:r><w:t>world</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;
        let result = parse_docx_xml(xml).unwrap();
        assert_eq!(result, "Hello world");
    }

    #[test]
    fn test_docx_xml_empty_paragraphs_skipped() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p></w:p>
    <w:p><w:r><w:t>Content</w:t></w:r></w:p>
    <w:p><w:r><w:t>   </w:t></w:r></w:p>
  </w:body>
</w:document>"#;
        let result = parse_docx_xml(xml).unwrap();
        assert_eq!(result, "Content");
    }

    // --- Markdown table ---

    #[test]
    fn test_rows_to_markdown_table() {
        let rows = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["1".to_string(), "2".to_string()],
            vec!["3".to_string(), "4".to_string()],
        ];
        let table = rows_to_markdown_table(&rows);
        assert_eq!(table, "| A | B |\n| --- | --- |\n| 1 | 2 |\n| 3 | 4 |");
    }

    #[test]
    fn test_rows_to_markdown_table_ragged() {
        let rows = vec![
            vec!["A".to_string(), "B".to_string(), "C".to_string()],
            vec!["1".to_string()], // fewer columns
        ];
        let table = rows_to_markdown_table(&rows);
        assert!(table.contains("| 1 |  |  |"));
    }

    // --- Extract result ---

    #[test]
    fn test_extract_csv_result() {
        let csv = "Name,Score\nAlice,100\n";
        let result = extract_document(csv.as_bytes(), DocType::Csv).unwrap();
        assert!(result.content.markdown.contains("| Name | Score |"));
        assert!(result.metadata.word_count > 0);
        assert!(result.content.links.is_empty());
        assert!(result.domain_data.is_none());
    }

    // --- Strip markdown ---

    #[test]
    fn test_strip_markdown() {
        let md = "# Title\n\nSome text\n\n| A | B |\n| --- | --- |\n| 1 | 2 |";
        let plain = strip_markdown_formatting(md);
        assert!(plain.contains("Title"));
        assert!(plain.contains("Some text"));
        assert!(plain.contains("A B"));
        assert!(!plain.contains("---"));
    }
}
