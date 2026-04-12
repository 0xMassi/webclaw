pub mod anthropic;
pub mod gemini_cli;
pub mod ollama;
pub mod openai;

/// Load an API key from an explicit override or an environment variable.
/// Returns `None` if neither is set or the value is empty.
pub(crate) fn load_api_key(override_key: Option<String>, env_var: &str) -> Option<String> {
    let key = override_key.or_else(|| std::env::var(env_var).ok())?;
    if key.is_empty() { None } else { Some(key) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_key_takes_precedence() {
        assert_eq!(
            load_api_key(Some("explicit".into()), "NONEXISTENT_VAR"),
            Some("explicit".into())
        );
    }

    #[test]
    fn empty_override_returns_none() {
        assert_eq!(load_api_key(Some(String::new()), "NONEXISTENT_VAR"), None);
    }

    #[test]
    fn none_override_with_no_env_returns_none() {
        assert_eq!(
            load_api_key(None, "NOXA_TEST_NONEXISTENT_KEY_12345"),
            None
        );
    }
}
