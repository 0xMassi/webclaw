/// LLM-specific errors. Kept flat — one enum covers transport, provider, and parsing failures.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LlmError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("no providers available")]
    NoProviders,

    #[error("all providers failed: {0}")]
    AllProvidersFailed(String),

    #[error("invalid JSON response: {0}")]
    InvalidJson(String),

    #[error("provider error: {0}")]
    ProviderError(String),
}

/// Truncate a (possibly network-sourced) error body to at most `max` bytes,
/// stepping back to the nearest UTF-8 char boundary so we never panic on a
/// multibyte split. Shared by all provider error paths.
pub(crate) fn truncate_err(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use super::truncate_err;

    #[test]
    fn short_text_unchanged() {
        assert_eq!(truncate_err("hello", 500), "hello");
    }

    #[test]
    fn exact_length_unchanged() {
        assert_eq!(truncate_err("abcde", 5), "abcde");
    }

    #[test]
    fn truncates_ascii() {
        assert_eq!(truncate_err("abcdef", 3), "abc");
    }

    #[test]
    fn never_splits_multibyte() {
        // "é" is 2 bytes; cutting at 3 would land mid-char on the second "é".
        let s = "aéé"; // bytes: a(1) é(2) é(2) = 5 bytes
        let out = truncate_err(s, 3);
        // Must step back to a valid boundary (after the first "é").
        assert!(s.is_char_boundary(out.len()));
        assert_eq!(out, "aé");
    }

    #[test]
    fn boundary_step_back_to_zero_is_safe() {
        let s = "😀"; // 4 bytes, single char
        assert_eq!(truncate_err(s, 2), "");
    }
}
