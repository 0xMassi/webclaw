/// QuickJS-based extraction of data from inline JavaScript in HTML pages.
///
/// Many modern websites embed page data as JavaScript variable assignments
/// (e.g., `window.__PRELOADED_STATE__`, Next.js `self.__next_f`). The static
/// JSON data island approach (`data_island.rs`) only handles `<script type="application/json">`.
/// This module executes inline `<script>` tags in a sandboxed QuickJS runtime
/// to capture those JS-assigned data blobs.
use once_cell::sync::Lazy;
use regex::Regex;
use rquickjs::{Context, Runtime};
use scraper::{Html, Selector};
use tracing::debug;

static SCRIPT_SELECTOR: Lazy<Selector> = Lazy::new(|| Selector::parse("script").unwrap());
static HTML_TAG_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());

/// A blob of data extracted from JS execution.
pub struct JsDataBlob {
    pub name: String,
    pub data: String,
    pub size: usize,
}

/// Execute inline `<script>` tags in a QuickJS sandbox and extract `window.__*` data.
pub fn extract_js_data(html: &str) -> Vec<JsDataBlob> {
    let doc = Html::parse_document(html);

    let scripts: Vec<String> = doc
        .select(&SCRIPT_SELECTOR)
        .filter(|el| {
            let v = el.value();
            // Skip external scripts and ES modules
            if v.attr("src").is_some() {
                return false;
            }
            if v.attr("type").is_some_and(|t| t == "module") {
                return false;
            }
            true
        })
        .map(|el| el.text().collect::<String>())
        .filter(|s| !s.trim().is_empty())
        .collect();

    if scripts.is_empty() {
        return Vec::new();
    }

    let rt = Runtime::new().expect("QuickJS runtime creation failed");
    rt.set_memory_limit(64 * 1024 * 1024); // 64 MB
    rt.set_max_stack_size(1024 * 1024); // 1 MB

    let ctx = Context::full(&rt).expect("QuickJS context creation failed");

    ctx.with(|ctx| {
        // Set up minimal browser stubs so scripts don't crash on missing globals.
        // We don't need real implementations — just enough to avoid ReferenceErrors.
        let setup = r#"
            globalThis.window = globalThis;
            globalThis.self = globalThis;
            globalThis.document = {
                createElement: function() { return { style: {}, setAttribute: function(){}, appendChild: function(){} }; },
                getElementById: function() { return null; },
                querySelector: function() { return null; },
                querySelectorAll: function() { return []; },
                addEventListener: function() {},
                createEvent: function() { return { initEvent: function(){} }; },
                createTextNode: function() { return {}; },
                head: { appendChild: function(){}, removeChild: function(){} },
                body: { appendChild: function(){}, removeChild: function(){} },
                documentElement: { style: {} },
                cookie: "",
                readyState: "complete",
                location: { href: "", hostname: "", pathname: "/" }
            };
            globalThis.navigator = {
                userAgent: "Mozilla/5.0",
                language: "en-US",
                languages: ["en-US"],
                platform: "Linux x86_64",
                cookieEnabled: true
            };
            globalThis.location = { href: "", hostname: "", pathname: "/", search: "", hash: "" };
            globalThis.history = { pushState: function(){}, replaceState: function(){} };
            globalThis.setTimeout = function(fn) { if (typeof fn === "function") { try { fn(); } catch(e) {} } return 0; };
            globalThis.clearTimeout = function() {};
            globalThis.setInterval = function() { return 0; };
            globalThis.clearInterval = function() {};
            globalThis.requestAnimationFrame = function() { return 0; };
            globalThis.cancelAnimationFrame = function() {};
            globalThis.console = { log: function(){}, warn: function(){}, error: function(){}, info: function(){}, debug: function(){} };
            globalThis.fetch = function() { return Promise.resolve({ json: function(){ return Promise.resolve({}); }, text: function(){ return Promise.resolve(""); } }); };
            globalThis.XMLHttpRequest = function() { this.open = function(){}; this.send = function(){}; this.setRequestHeader = function(){}; };
            globalThis.localStorage = { getItem: function(){ return null; }, setItem: function(){}, removeItem: function(){}, clear: function(){} };
            globalThis.sessionStorage = { getItem: function(){ return null; }, setItem: function(){}, removeItem: function(){}, clear: function(){} };
            globalThis.addEventListener = function() {};
            globalThis.removeEventListener = function() {};
            globalThis.dispatchEvent = function() {};
            globalThis.getComputedStyle = function() { return {}; };
            globalThis.matchMedia = function() { return { matches: false, addListener: function(){}, removeListener: function(){} }; };
            globalThis.Image = function() {};
            globalThis.Event = function() {};
            globalThis.CustomEvent = function() {};
            globalThis.MutationObserver = function() { this.observe = function(){}; this.disconnect = function(){}; };
            globalThis.IntersectionObserver = function() { this.observe = function(){}; this.disconnect = function(){}; };
            globalThis.ResizeObserver = function() { this.observe = function(){}; this.disconnect = function(){}; };
            globalThis.performance = { now: function(){ return 0; }, mark: function(){}, measure: function(){} };
            globalThis.crypto = { getRandomValues: function(arr) { return arr; } };
            globalThis.URL = function(u) { this.href = u || ""; this.searchParams = { get: function(){ return null; } }; };
            globalThis.Promise = Promise;
            self.__next_f = self.__next_f || [];
        "#;
        let _ = ctx.eval::<(), _>(setup);

        // Execute each inline script, silently ignoring errors
        for script in &scripts {
            let _ = ctx.eval::<(), _>(script.as_str());
        }

        // Scan window.__* properties for data blobs
        let scan = r#"
            (function() {
                var results = [];
                var keys = Object.keys(globalThis);
                for (var i = 0; i < keys.length; i++) {
                    var key = keys[i];
                    if (key.indexOf("__") !== 0) continue;
                    var val = globalThis[key];
                    if (val === null || val === undefined) continue;

                    // __next_f is an array of RSC flight data chunks
                    if (key === "__next_f") {
                        if (Array.isArray(val) && val.length > 0) {
                            var json = JSON.stringify(val);
                            if (json.length > 100) {
                                results.push({ name: key, data: json, size: json.length });
                            }
                        }
                        continue;
                    }

                    if (typeof val === "object") {
                        try {
                            var json = JSON.stringify(val);
                            if (json && json.length > 100) {
                                results.push({ name: key, data: json, size: json.length });
                            }
                        } catch(e) {}
                    }
                }
                return JSON.stringify(results);
            })()
        "#;

        let Ok(raw): Result<String, _> = ctx.eval(scan) else {
            return Vec::new();
        };

        let Ok(entries) = serde_json::from_str::<Vec<RawBlob>>(&raw) else {
            return Vec::new();
        };

        let blobs: Vec<JsDataBlob> = entries
            .into_iter()
            .map(|e| JsDataBlob {
                name: e.name,
                size: e.size,
                data: e.data,
            })
            .collect();

        if !blobs.is_empty() {
            debug!(
                count = blobs.len(),
                names = blobs
                    .iter()
                    .map(|b| b.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                "extracted JS data blobs"
            );
        }

        blobs
    })
}

/// Intermediate deserialization target for the scan script output.
#[derive(serde::Deserialize)]
struct RawBlob {
    name: String,
    data: String,
    size: usize,
}

/// Extract readable text from JS data blobs and format as markdown.
///
/// Walks each blob's JSON looking for human-readable strings, filters out
/// URLs/paths/CSS/base64, deduplicates, and joins into a single section.
pub fn extract_readable_text(blobs: &[JsDataBlob]) -> String {
    let mut texts: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for blob in blobs {
        if blob.name == "__next_f" {
            let rsc_texts = extract_next_f_text(&blob.data);
            for t in rsc_texts {
                if seen.insert(t.clone()) {
                    texts.push(t);
                }
            }
            continue;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(&blob.data) else {
            continue;
        };

        let mut found = Vec::new();
        walk_json_for_text(&value, &mut found, 0);

        for t in found {
            if seen.insert(t.clone()) {
                texts.push(t);
            }
        }
    }

    if texts.is_empty() {
        return String::new();
    }

    let mut md = String::from("## Additional Content\n\n");
    md.push_str(&texts.join("\n\n"));
    md
}

/// Recursively walk JSON and collect readable text strings.
fn walk_json_for_text(value: &serde_json::Value, out: &mut Vec<String>, depth: usize) {
    if depth > 15 {
        return;
    }

    match value {
        serde_json::Value::String(s) => {
            if let Some(clean) = filter_readable(s) {
                out.push(clean);
            }
        }
        serde_json::Value::Object(map) => {
            for (_, v) in map {
                walk_json_for_text(v, out, depth + 1);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                walk_json_for_text(v, out, depth + 1);
            }
        }
        _ => {}
    }
}

/// Filter a string for readability: must be >15 chars, mostly alphabetic,
/// not a URL, file path, CSS rule, or base64 blob. Strips inline HTML tags.
fn filter_readable(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() <= 15 {
        return None;
    }

    // Skip URLs
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("//") {
        return None;
    }

    // Skip file paths
    if s.starts_with('/') || s.starts_with("./") || s.starts_with("../") {
        return None;
    }

    // Skip CSS-like strings
    if s.contains('{') && s.contains('}') && (s.contains(':') || s.contains(';')) {
        return None;
    }

    // Skip CSS grid templates, layout strings, and dimension patterns
    if s.contains("1fr")
        || s.contains("grid-")
        || s.contains("max-content")
        || s.contains("divider-v-")
        || s.contains("divider-h-")
    {
        return None;
    }

    // Skip CSS layout area definitions (e.g. "card1 card2 card3")
    // These have repeated dash-separated tokens with digits
    let dash_digit_tokens = s
        .split_whitespace()
        .filter(|w| w.contains('-') && w.chars().any(|c| c.is_ascii_digit()))
        .count();
    if dash_digit_tokens >= 2 {
        return None;
    }

    // Skip strings containing literal quote characters (CSS grid areas, code snippets)
    if s.contains('"') {
        return None;
    }

    // Skip CSS grid area names and layout tokens.
    // These are strings of short lowercase words/dots with no sentence structure.
    if !s.chars().any(|c| c.is_uppercase()) {
        let is_css_layout = s.split_whitespace().all(|w| {
            w == "."
                || (w.len() <= 20
                    && w.chars()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'))
        });
        if is_css_layout {
            return None;
        }
    }

    // Skip CSS dimension strings (e.g. "16px 0px 0px 0px")
    if s.split_whitespace().all(|w| {
        w.ends_with("px") || w.ends_with("em") || w.ends_with("rem") || w.ends_with("%") || w == "0"
    }) {
        return None;
    }

    // Skip base64
    if s.len() > 50 && !s.contains(' ') {
        return None;
    }

    // Skip strings that are mostly HTML tags
    if s.matches('<').count() > 3 && s.matches('>').count() > 3 {
        let stripped = HTML_TAG_RE.replace_all(s, "");
        if stripped.trim().len() < 15 {
            return None;
        }
    }

    // Skip strings ending with file extensions
    if s.ends_with(".js")
        || s.ends_with(".css")
        || s.ends_with(".png")
        || s.ends_with(".jpg")
        || s.ends_with(".svg")
        || s.ends_with(".woff2")
    {
        return None;
    }

    // Must be mostly alphabetic (spaces + letters should dominate)
    let alpha_space = s
        .chars()
        .filter(|c| c.is_alphabetic() || c.is_whitespace())
        .count();
    let ratio = alpha_space as f64 / s.len() as f64;
    if ratio < 0.6 {
        return None;
    }

    // Must contain spaces (prose, not a single token)
    if !s.contains(' ') {
        return None;
    }

    // Strip any inline HTML tags
    let clean = HTML_TAG_RE.replace_all(s, "").trim().to_string();

    if clean.len() <= 15 {
        return None;
    }

    Some(clean)
}

/// Parse Next.js RSC flight data (`self.__next_f`) and extract readable text.
///
/// Wire format: array of `[type, payload]` tuples. Type 1 contains the actual
/// RSC data as newline-delimited entries like `id:TYPE|payload`.
fn extract_next_f_text(raw_json: &str) -> Vec<String> {
    let Ok(entries) = serde_json::from_str::<Vec<serde_json::Value>>(raw_json) else {
        return Vec::new();
    };

    // Concatenate all type=1 payloads
    let mut wire = String::new();
    for entry in &entries {
        let arr = match entry.as_array() {
            Some(a) if a.len() >= 2 => a,
            _ => continue,
        };
        let entry_type = arr[0].as_u64().unwrap_or(0);
        if entry_type != 1 {
            continue;
        }
        if let Some(payload) = arr[1].as_str() {
            wire.push_str(payload);
        }
    }

    if wire.is_empty() {
        return Vec::new();
    }

    let mut texts = Vec::new();

    // Each line is `id:TYPE|payload` — parse the JSON payloads
    for line in wire.lines() {
        // Find the payload after the first `|` or `:` marker
        let payload = if let Some(pos) = line.find('|') {
            &line[pos + 1..]
        } else {
            continue;
        };

        // Try to parse as JSON array (RSC element representation)
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
            walk_rsc_tree(&value, &mut texts, 0);
        }
    }

    texts
}

/// Walk an RSC tree element extracting children text content.
fn walk_rsc_tree(value: &serde_json::Value, out: &mut Vec<String>, depth: usize) {
    if depth > 20 {
        return;
    }

    match value {
        serde_json::Value::String(s) => {
            if let Some(clean) = filter_readable(s) {
                out.push(clean);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                walk_rsc_tree(item, out, depth + 1);
            }
        }
        serde_json::Value::Object(map) => {
            // RSC elements have "children" that contain text
            if let Some(children) = map.get("children") {
                walk_rsc_tree(children, out, depth + 1);
            }
            // Also check other fields
            for (key, v) in map {
                if key == "children" {
                    continue;
                }
                walk_rsc_tree(v, out, depth + 1);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_window_preloaded_data() {
        let html = r#"<html><body>
        <script>
        window.__preloadedData = {
            "page": {
                "title": "Hello World Article Title",
                "body": "This is a longer paragraph of text that should be extracted from the preloaded data blob successfully."
            }
        };
        </script>
        </body></html>"#;

        let blobs = extract_js_data(html);
        assert!(!blobs.is_empty(), "should extract at least one blob");
        assert!(
            blobs.iter().any(|b| b.name == "__preloadedData"),
            "should find __preloadedData"
        );

        let text = extract_readable_text(&blobs);
        assert!(
            text.contains("This is a longer paragraph"),
            "should extract readable text from blob"
        );
    }

    #[test]
    fn skips_external_and_module_scripts() {
        let html = r#"<html><body>
        <script src="https://cdn.example.com/app.js"></script>
        <script type="module">export default {};</script>
        <script>window.__testData = {"content": "This is a test sentence that is long enough to be extracted from the page and it needs over one hundred characters of JSON to pass the threshold."};</script>
        </body></html>"#;

        let blobs = extract_js_data(html);
        assert_eq!(
            blobs.len(),
            1,
            "should only process inline non-module script"
        );
        assert_eq!(blobs[0].name, "__testData");
    }

    #[test]
    fn empty_html_returns_no_blobs() {
        let blobs = extract_js_data("<html><body></body></html>");
        assert!(blobs.is_empty());
    }

    #[test]
    fn filter_readable_rejects_junk() {
        assert!(filter_readable("short").is_none());
        assert!(filter_readable("https://example.com/some/long/path").is_none());
        assert!(filter_readable("/static/js/bundle.min.js").is_none());
        assert!(filter_readable("aGVsbG8gd29ybGQgdGhpcyBpcyBhIGJhc2U2NCBzdHJpbmc=").is_none());
        assert!(filter_readable(".container { display: flex; padding: 10px; }").is_none());
    }

    #[test]
    fn filter_readable_accepts_prose() {
        let result = filter_readable("This is a normal sentence with enough words.");
        assert!(result.is_some());
        assert_eq!(
            result.unwrap(),
            "This is a normal sentence with enough words."
        );
    }

    #[test]
    fn strips_html_tags_from_text() {
        let result = filter_readable(
            "This has <strong>bold</strong> and <em>italic</em> formatting inside it.",
        );
        assert!(result.is_some());
        let clean = result.unwrap();
        assert!(!clean.contains('<'));
        assert!(clean.contains("bold"));
        assert!(clean.contains("italic"));
    }

    #[test]
    fn extract_readable_text_produces_markdown() {
        let blobs = vec![JsDataBlob {
            name: "__data".to_string(),
            data: r#"{"article":"This is the main article content that should appear in the extracted text."}"#
                .to_string(),
            size: 100,
        }];

        let text = extract_readable_text(&blobs);
        assert!(text.starts_with("## Additional Content"));
        assert!(text.contains("main article content"));
    }

    #[test]
    fn extract_next_f_rsc_data() {
        let blobs = vec![JsDataBlob {
            name: "__next_f".to_string(),
            data: r#"[[0,""],
                      [1,"0:T1234|{\"children\":\"This is some Next.js RSC flight data content that we want to extract.\"}\n"]]"#
                .to_string(),
            size: 200,
        }];

        let text = extract_readable_text(&blobs);
        assert!(
            text.contains("Next.js RSC flight data content"),
            "should extract text from RSC flight data. Got: {text}"
        );
    }

    #[test]
    fn handles_script_errors_gracefully() {
        // Scripts that throw errors should be silently ignored
        let html = r#"<html><body>
        <script>throw new Error("intentional crash");</script>
        <script>undefined_function();</script>
        <script>window.__survived = {"message": "This script ran after the errors and the data should still be found in the extracted blobs because it exceeds the minimum threshold."};</script>
        </body></html>"#;

        let blobs = extract_js_data(html);
        assert!(
            blobs.iter().any(|b| b.name == "__survived"),
            "should extract data from scripts that succeed after failures"
        );
    }
}
