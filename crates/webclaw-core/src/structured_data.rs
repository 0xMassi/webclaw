/// Extract structured data from HTML.
///
/// Handles three sources:
/// 1. JSON-LD (`<script type="application/ld+json">`) — e-commerce, news, recipes
/// 2. `__NEXT_DATA__` (`<script id="__NEXT_DATA__" type="application/json">`) — Next.js pages
/// 3. SvelteKit data islands (`kit.start(app, element, { data: [...] })`) — SPAs
use serde_json::Value;

/// Extract all JSON-LD blocks from raw HTML.
///
/// Returns parsed JSON values, skipping any blocks that fail to parse.
/// Most e-commerce sites include Schema.org Product markup with prices,
/// sizes, availability, and images.
pub fn extract_json_ld(html: &str) -> Vec<Value> {
    let mut results = Vec::new();
    let needle = "application/ld+json";

    // Walk through the HTML finding <script type="application/ld+json"> blocks.
    // Using simple string scanning instead of a full HTML parser — these blocks
    // are self-contained and reliably structured.
    let mut search_from = 0;
    while let Some(tag_start) = html[search_from..].find("<script") {
        let abs_start = search_from + tag_start;
        let tag_region = &html[abs_start..];

        // Find the end of the opening tag
        let Some(tag_end_offset) = tag_region.find('>') else {
            search_from = abs_start + 7;
            continue;
        };

        let opening_tag = &tag_region[..tag_end_offset];

        // Check if this is a JSON-LD script
        if !opening_tag.to_lowercase().contains(needle) {
            search_from = abs_start + tag_end_offset + 1;
            continue;
        }

        // Find the closing </script>
        let content_start = abs_start + tag_end_offset + 1;
        let remaining = &html[content_start..];
        let Some(close_offset) = remaining.to_lowercase().find("</script>") else {
            search_from = content_start;
            continue;
        };

        let json_str = remaining[..close_offset].trim();
        search_from = content_start + close_offset + 9;

        if json_str.is_empty() {
            continue;
        }

        // Parse — some sites have arrays at top level
        match serde_json::from_str::<Value>(json_str) {
            Ok(Value::Array(arr)) => results.extend(arr),
            Ok(val) => results.push(val),
            Err(_) => {}
        }
    }

    results
}

/// Extract `__NEXT_DATA__` from Next.js pages.
///
/// Next.js embeds server-rendered page data in:
/// `<script id="__NEXT_DATA__" type="application/json">{...}</script>`
///
/// Returns the `pageProps` object (the actual page data), skipping Next.js
/// internals like `buildId`, `isFallback`, etc.
pub fn extract_next_data(html: &str) -> Vec<Value> {
    let Some(id_pos) = html.find("__NEXT_DATA__") else {
        return Vec::new();
    };

    // Find the enclosing <script> tag
    let Some(tag_start) = html[..id_pos].rfind("<script") else {
        return Vec::new();
    };
    let tag_region = &html[tag_start..];

    let Some(tag_end) = tag_region.find('>') else {
        return Vec::new();
    };

    let content_start = tag_start + tag_end + 1;
    let remaining = &html[content_start..];
    let Some(close) = remaining.find("</script>") else {
        return Vec::new();
    };

    let json_str = remaining[..close].trim();
    if json_str.len() < 20 {
        return Vec::new();
    }

    let Ok(data) = serde_json::from_str::<Value>(json_str) else {
        return Vec::new();
    };

    // Extract pageProps — the actual page data
    if let Some(page_props) = data.get("props").and_then(|p| p.get("pageProps"))
        && page_props.is_object()
        && page_props.as_object().is_some_and(|m| !m.is_empty())
    {
        return vec![page_props.clone()];
    }

    // Fallback: return the whole thing if pageProps is missing/empty
    if data.is_object() {
        vec![data]
    } else {
        Vec::new()
    }
}

/// Extract data from SvelteKit's `kit.start()` pattern.
///
/// SvelteKit embeds page data inside:
/// `kit.start(app, element, { data: [null, null, {"type":"data","data":{...}}] })`
///
/// Returns parsed JSON objects from the data array (skipping nulls).
pub fn extract_sveltekit(html: &str) -> Vec<Value> {
    let Some(kit_pos) = html.find("kit.start(") else {
        return Vec::new();
    };
    let region = &html[kit_pos..];

    let Some(data_offset) = region.find("data: [") else {
        return Vec::new();
    };
    let bracket_start = kit_pos + data_offset + "data: ".len();
    let bracket_region = &html[bracket_start..];

    let Some(balanced) = extract_balanced(bracket_region, b'[', b']') else {
        return Vec::new();
    };
    if balanced.len() < 50 {
        return Vec::new();
    }

    // SvelteKit uses JS object literals (unquoted keys). Convert to valid JSON.
    let json_str = js_literal_to_json(&balanced);
    let Ok(arr) = serde_json::from_str::<Vec<Value>>(&json_str) else {
        return Vec::new();
    };

    let mut results = Vec::new();
    for item in arr {
        if item.is_null() {
            continue;
        }
        // SvelteKit wraps as {"type":"data","data":{...}} — unwrap if present
        if let Some(inner) = item.get("data")
            && (inner.is_object() || inner.is_array())
        {
            results.push(inner.clone());
            continue;
        }
        if item.is_object() || item.is_array() {
            results.push(item);
        }
    }
    results
}

/// Convert a JS object literal to valid JSON by quoting unquoted keys.
///
/// Handles: `{foo:"bar", baz:123}` → `{"foo":"bar", "baz":123}`
/// Preserves already-quoted keys and string values.
fn js_literal_to_json(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len() + input.len() / 10);
    let mut i = 0;
    let len = bytes.len();

    while i < len {
        let b = bytes[i];

        // Skip through strings
        if b == b'"' {
            out.push('"');
            i += 1;
            while i < len {
                let c = bytes[i];
                out.push(c as char);
                i += 1;
                if c == b'\\' && i < len {
                    out.push(bytes[i] as char);
                    i += 1;
                } else if c == b'"' {
                    break;
                }
            }
            continue;
        }

        // After { or , — look for unquoted key followed by :
        if (b == b'{' || b == b',' || b == b'[') && i + 1 < len {
            out.push(b as char);
            i += 1;
            // Skip whitespace
            while i < len && bytes[i].is_ascii_whitespace() {
                out.push(bytes[i] as char);
                i += 1;
            }
            // Check if next is an unquoted identifier (key)
            if i < len && (bytes[i].is_ascii_alphabetic() || bytes[i] == b'_') {
                let key_start = i;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let key = &input[key_start..i];
                // Skip whitespace after key
                while i < len && bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                // If followed by :, it's an unquoted key — quote it
                if i < len && bytes[i] == b':' {
                    out.push('"');
                    out.push_str(key);
                    out.push('"');
                } else {
                    // Not a key — might be a bare value like true/false/null
                    out.push_str(key);
                }
            }
            continue;
        }

        out.push(b as char);
        i += 1;
    }

    out
}

/// Extract content between balanced brackets, handling string escaping.
fn extract_balanced(text: &str, open: u8, close: u8) -> Option<String> {
    if text.as_bytes().first()? != &open {
        return None;
    }
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, &b) in text.as_bytes().iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if b == b'\\' && in_string {
            escape_next = true;
            continue;
        }
        if b == b'"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(text[..=i].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_single_json_ld() {
        let html = r#"
            <html><head>
            <script type="application/ld+json">{"@type":"Product","name":"Test"}</script>
            </head><body></body></html>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["@type"], "Product");
        assert_eq!(results[0]["name"], "Test");
    }

    #[test]
    fn extracts_multiple_json_ld_blocks() {
        let html = r#"
            <script type="application/ld+json">{"@type":"WebSite","url":"https://example.com"}</script>
            <script type="application/ld+json">{"@type":"Product","name":"Shoe","offers":{"price":99.99}}</script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["@type"], "WebSite");
        assert_eq!(results[1]["@type"], "Product");
    }

    #[test]
    fn handles_array_json_ld() {
        let html = r#"
            <script type="application/ld+json">[{"@type":"BreadcrumbList"},{"@type":"Product"}]</script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn skips_invalid_json() {
        let html = r#"
            <script type="application/ld+json">{invalid json here}</script>
            <script type="application/ld+json">{"@type":"Product","name":"Valid"}</script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], "Valid");
    }

    #[test]
    fn ignores_regular_script_tags() {
        let html = r#"
            <script>console.log("not json-ld")</script>
            <script type="text/javascript">var x = 1;</script>
            <script type="application/ld+json">{"@type":"Product"}</script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn handles_no_json_ld() {
        let html = "<html><body><p>No structured data here</p></body></html>";
        let results = extract_json_ld(html);
        assert!(results.is_empty());
    }

    #[test]
    fn case_insensitive_type() {
        let html = r#"
            <script type="Application/LD+JSON">{"@type":"Product"}</script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn handles_whitespace_in_json() {
        let html = r#"
            <script type="application/ld+json">
                {
                    "@type": "Product",
                    "name": "Test"
                }
            </script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["name"], "Test");
    }

    #[test]
    fn empty_script_tag_skipped() {
        let html = r#"
            <script type="application/ld+json">   </script>
            <script type="application/ld+json">{"@type":"Product"}</script>
        "#;
        let results = extract_json_ld(html);
        assert_eq!(results.len(), 1);
    }
}
