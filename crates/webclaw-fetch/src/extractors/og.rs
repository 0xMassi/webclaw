//! Shared Open Graph (`og:*`) meta-tag parsing for the HTML vertical
//! extractors.
//!
//! Several site extractors read a handful of `og:*` properties (title,
//! description, image, ...) from the page `<head>`. Each used to carry a
//! verbatim copy of the same regex + scan helper. This module centralises
//! that logic and adds [`parse_og`], which collects every `og:*` pair in a
//! single `captures_iter` pass so an extractor that needs multiple fields
//! scans the document once instead of once per field.
//!
//! Values are stored raw. Callers that need HTML entity decoding apply
//! [`html_unescape`] themselves — some extractors intentionally keep the
//! raw value, so decoding is opt-in per call site to preserve output.

use std::collections::HashMap;
use std::sync::OnceLock;

use regex::Regex;

/// Matches `<meta property="og:<name>" content="<value>">`, case-insensitive.
/// Capture 1 is the property suffix (after `og:`), capture 2 is the content.
fn og_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)<meta[^>]+property="og:([a-z_]+)"[^>]+content="([^"]+)""#).unwrap()
    })
}

/// Return the raw content of the first `og:<prop>` meta tag, if present.
///
/// Single-pass per call. For extractors reading several properties, prefer
/// [`parse_og`] to scan the document only once.
pub(crate) fn og(html: &str, prop: &str) -> Option<String> {
    for c in og_regex().captures_iter(html) {
        if c.get(1).is_some_and(|m| m.as_str() == prop) {
            return c.get(2).map(|m| m.as_str().to_string());
        }
    }
    None
}

/// Parse every `og:*` meta tag in one pass into a `suffix -> content` map.
///
/// First occurrence wins, matching the short-circuit-on-first-match
/// behaviour of [`og`] when called per property. Values are raw (not
/// entity-decoded); use [`OgMeta::unescaped`] / [`OgMeta::raw`] to read.
pub(crate) fn parse_og(html: &str) -> OgMeta {
    let mut map: HashMap<String, String> = HashMap::new();
    for c in og_regex().captures_iter(html) {
        if let (Some(name), Some(content)) = (c.get(1), c.get(2)) {
            map.entry(name.as_str().to_string())
                .or_insert_with(|| content.as_str().to_string());
        }
    }
    OgMeta(map)
}

/// Parsed `og:*` properties from a single document scan.
pub(crate) struct OgMeta(HashMap<String, String>);

impl OgMeta {
    /// Raw content of `og:<prop>`, exactly as it appeared in the HTML.
    pub(crate) fn raw(&self, prop: &str) -> Option<String> {
        self.0.get(prop).cloned()
    }

    /// Content of `og:<prop>` with the common HTML entities decoded.
    pub(crate) fn unescaped(&self, prop: &str) -> Option<String> {
        self.0.get(prop).map(|v| html_unescape(v))
    }
}

/// Decode the small set of HTML entities that show up in `og:*` content.
pub(crate) fn html_unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let decoded = rest[1..]
            .find(';')
            .filter(|&end| end <= 10)
            .and_then(|end| decode_entity(&rest[1..=end]).map(|c| (c, end + 2)));
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn decode_entity(name: &str) -> Option<char> {
    match name {
        "quot" => Some('"'),
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "apos" => Some('\''),
        "nbsp" => Some('\u{a0}'),
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::html_unescape;

    #[test]
    fn html_unescape_leaves_text_that_is_not_an_entity() {
        assert_eq!(
            html_unescape("AT&T &#xZZ; &unknown; 5 & 6"),
            "AT&T &#xZZ; &unknown; 5 & 6"
        );
        assert_eq!(html_unescape("&amp;quot; &#39;x&#39;"), "&quot; 'x'");
    }
}
