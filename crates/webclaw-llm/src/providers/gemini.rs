/// Google Gemini provider — Gemini models via the Generative Language API.
/// Gemini's request shape differs from OpenAI/Anthropic: the system message is a
/// top-level `systemInstruction`, conversation turns live in `contents` (with the
/// assistant role renamed to `model`), and generation knobs sit under
/// `generationConfig`. API-key auth is sent as an `x-goog-api-key` header.
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;

use crate::clean::strip_thinking_tags;
use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

use super::load_api_key;

const DEFAULT_GEMINI_BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta";
/// Default model. Gemini 2.5 Flash/Pro are "thinking" models: internal reasoning
/// tokens count against `maxOutputTokens`, so the output budget must comfortably
/// exceed the visible response (see `request_body`) or the model returns
/// `finishReason=MAX_TOKENS` with no text. Set `GEMINI_MODEL` to a non-thinking
/// model (e.g. `gemini-2.0-flash`) to avoid the reasoning overhead entirely.
const DEFAULT_GEMINI_MODEL: &str = "gemini-2.5-flash";

/// Gemini puts the model in the URL path, so only plain model identifiers are
/// safe to interpolate. Real model names are ASCII alphanumerics plus `-`/`.`/`_`
/// (e.g. `gemini-2.5-flash`, `gemini-2.0-flash-001`); anything else (`/`, `:`,
/// `?`, `#`, whitespace) could redirect the request to a different path/method.
fn is_safe_model_name(model: &str) -> bool {
    !model.is_empty()
        && model
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_'))
}

pub struct GeminiProvider {
    client: reqwest::Client,
    key: String,
    base_url: String,
    default_model: String,
}

impl GeminiProvider {
    /// Returns `None` if no API key is available (param or `GEMINI_API_KEY` env).
    pub fn new(
        key_override: Option<String>,
        base_url: Option<String>,
        model: Option<String>,
    ) -> Option<Self> {
        let key = load_api_key(key_override, "GEMINI_API_KEY")?;

        Some(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .connect_timeout(Duration::from_secs(10))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
            key,
            base_url: base_url
                .or_else(|| std::env::var("GEMINI_BASE_URL").ok())
                .unwrap_or_else(|| DEFAULT_GEMINI_BASE_URL.into())
                .trim_end_matches('/')
                .to_string(),
            default_model: model
                .or_else(|| std::env::var("GEMINI_MODEL").ok())
                .unwrap_or_else(|| DEFAULT_GEMINI_MODEL.into()),
        })
    }

    pub fn default_model(&self) -> &str {
        &self.default_model
    }

    /// Build the `generateContent` body from a generic completion request.
    /// System messages become `systemInstruction`; user/assistant turns become
    /// `contents` (assistant → `model`); `json_mode` constrains the model to
    /// valid JSON via `responseMimeType`.
    fn request_body(&self, request: &CompletionRequest) -> serde_json::Value {
        let contents: Vec<serde_json::Value> = request
            .messages
            .iter()
            .filter(|m| m.role != "system")
            .map(|m| {
                let role = if m.role == "assistant" {
                    "model"
                } else {
                    "user"
                };
                json!({ "role": role, "parts": [{ "text": m.content }] })
            })
            .collect();

        let system_parts: Vec<serde_json::Value> = request
            .messages
            .iter()
            .filter(|m| m.role == "system")
            .map(|m| json!({ "text": m.content }))
            .collect();

        // `maxOutputTokens` is a ceiling, not a reservation — you're billed per
        // token actually produced — so default generously. Gemini 2.5 "thinking"
        // models spend part of this budget on internal reasoning; too low a cap
        // makes them return `finishReason=MAX_TOKENS` with no visible text.
        let mut generation_config = json!({
            "maxOutputTokens": request.max_tokens.unwrap_or(8192),
        });
        if let Some(temp) = request.temperature {
            generation_config["temperature"] = json!(temp);
        }
        if request.json_mode {
            generation_config["responseMimeType"] = json!("application/json");
        }

        let mut body = json!({
            "contents": contents,
            "generationConfig": generation_config,
        });

        // Gemini rejects an empty `systemInstruction`, so only attach it when a
        // system message is actually present.
        if !system_parts.is_empty() {
            body["systemInstruction"] = json!({ "parts": system_parts });
        }

        body
    }
}

#[async_trait]
impl LlmProvider for GeminiProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        // The model goes in the URL path (Gemini's API requires it there, unlike
        // OpenAI/Anthropic which take it in the body), so reject anything that
        // isn't a plain model identifier to prevent path/query injection from a
        // caller-supplied `request.model`.
        if !is_safe_model_name(model) {
            return Err(LlmError::ProviderError(format!(
                "invalid gemini model name: {model:?}"
            )));
        }

        let body = self.request_body(request);

        // API-key auth goes in the header, never the URL, so the key can't leak
        // into request logs, proxies, or referrer headers.
        let url = format!("{}/models/{model}:generateContent", self.base_url);
        let resp = self
            .client
            .post(&url)
            .header("x-goog-api-key", &self.key)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            let safe_text = text.chars().take(500).collect::<String>();
            return Err(LlmError::ProviderError(format!(
                "gemini returned {status}: {safe_text}"
            )));
        }

        // Cap response body size to defend against adversarial payloads.
        let json = super::response_json_capped(resp).await?;

        // Gemini response: {"candidates":[{"content":{"parts":[{"text":"..."}]}}]}.
        // A candidate may carry multiple text parts; concatenate them in order.
        let text = json["candidates"][0]["content"]["parts"]
            .as_array()
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(|p| p["text"].as_str())
                    .collect::<String>()
            })
            .unwrap_or_default();

        if text.is_empty() {
            // No usable text. Surface Gemini's finishReason (or a prompt-level
            // block reason) so MAX_TOKENS — e.g. a "thinking" model that spent
            // its whole maxOutputTokens budget on reasoning — and SAFETY blocks
            // are visible in logs/telemetry instead of masquerading as a parse
            // failure. The chain falls through to the next provider on any Err.
            let reason = json["candidates"][0]["finishReason"]
                .as_str()
                .or_else(|| json["promptFeedback"]["blockReason"].as_str())
                .unwrap_or("unknown");
            return Err(LlmError::ProviderError(format!(
                "gemini returned no text (finishReason={reason})"
            )));
        }

        Ok(strip_thinking_tags(&text))
    }

    async fn is_available(&self) -> bool {
        !self.key.is_empty()
    }

    fn name(&self) -> &str {
        "gemini"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Message;

    fn provider() -> GeminiProvider {
        GeminiProvider::new(Some("test-key".into()), None, None).expect("should construct")
    }

    fn msg(role: &str, content: &str) -> Message {
        Message {
            role: role.into(),
            content: content.into(),
        }
    }

    fn request(messages: Vec<Message>, json_mode: bool) -> CompletionRequest {
        CompletionRequest {
            model: String::new(),
            messages,
            temperature: None,
            max_tokens: None,
            json_mode,
        }
    }

    #[test]
    fn empty_key_returns_none() {
        assert!(GeminiProvider::new(Some(String::new()), None, None).is_none());
    }

    #[test]
    fn model_name_validation_blocks_path_injection() {
        // Real model identifiers pass.
        assert!(is_safe_model_name("gemini-2.5-flash"));
        assert!(is_safe_model_name("gemini-2.0-flash-001"));
        assert!(is_safe_model_name("gemini-1.5-pro-002"));
        // Anything that could alter the request path/method is rejected.
        assert!(!is_safe_model_name(""));
        assert!(!is_safe_model_name(
            "gemini-2.5-flash:streamGenerateContent"
        ));
        assert!(!is_safe_model_name("../../models/x"));
        assert!(!is_safe_model_name("model?alt=sse"));
        assert!(!is_safe_model_name("a b"));
    }

    #[test]
    fn explicit_key_constructs_with_defaults() {
        let p = provider();
        assert_eq!(p.name(), "gemini");
        assert_eq!(p.key, "test-key");
        assert_eq!(p.default_model, DEFAULT_GEMINI_MODEL);
        assert_eq!(p.default_model(), DEFAULT_GEMINI_MODEL);
        assert_eq!(p.base_url, DEFAULT_GEMINI_BASE_URL);
    }

    #[test]
    fn custom_base_url_trims_trailing_slash_and_model() {
        let p = GeminiProvider::new(
            Some("test-key".into()),
            Some("https://example.test/v1beta/".into()),
            Some("gemini-2.5-pro".into()),
        )
        .unwrap();
        assert_eq!(p.base_url, "https://example.test/v1beta");
        assert_eq!(p.default_model, "gemini-2.5-pro");
    }

    #[test]
    fn maps_user_and_assistant_roles_into_contents() {
        let p = provider();
        let body = p.request_body(&request(
            vec![msg("user", "hello"), msg("assistant", "hi there")],
            false,
        ));
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 2);
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(contents[0]["parts"][0]["text"], "hello");
        // assistant must be renamed to Gemini's "model" role.
        assert_eq!(contents[1]["role"], "model");
        assert_eq!(contents[1]["parts"][0]["text"], "hi there");
        // No system message -> no systemInstruction key at all.
        assert!(body.get("systemInstruction").is_none());
    }

    #[test]
    fn system_message_becomes_system_instruction_not_contents() {
        let p = provider();
        let body = p.request_body(&request(
            vec![msg("system", "be terse"), msg("user", "hello")],
            false,
        ));
        let contents = body["contents"].as_array().unwrap();
        assert_eq!(contents.len(), 1, "system message lifted out of contents");
        assert_eq!(contents[0]["role"], "user");
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be terse");
    }

    #[test]
    fn json_mode_toggles_response_mime_type() {
        let p = provider();
        let on = p.request_body(&request(vec![msg("user", "x")], true));
        assert_eq!(
            on["generationConfig"]["responseMimeType"],
            "application/json"
        );
        let off = p.request_body(&request(vec![msg("user", "x")], false));
        assert!(off["generationConfig"].get("responseMimeType").is_none());
    }

    #[test]
    fn max_output_tokens_default_and_temperature_override() {
        let p = provider();
        let default_body = p.request_body(&request(vec![msg("user", "x")], false));
        assert_eq!(default_body["generationConfig"]["maxOutputTokens"], 8192);
        // No temperature set -> key omitted.
        assert!(
            default_body["generationConfig"]
                .get("temperature")
                .is_none()
        );

        let mut req = request(vec![msg("user", "x")], false);
        req.max_tokens = Some(256);
        req.temperature = Some(0.5); // 0.5 is exact in both f32 and f64
        let body = p.request_body(&req);
        assert_eq!(body["generationConfig"]["maxOutputTokens"], 256);
        assert_eq!(body["generationConfig"]["temperature"], 0.5);
    }

    // Env var fallback tests mutate process-global state and race with parallel
    // tests. Run in isolation if needed:
    //   cargo test -p webclaw-llm env_var -- --ignored --test-threads=1
    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn env_var_key_fallback() {
        unsafe { std::env::set_var("GEMINI_API_KEY", "gemini-env-key") };
        let p = GeminiProvider::new(None, None, None).expect("should construct from env");
        assert_eq!(p.key, "gemini-env-key");
        unsafe { std::env::remove_var("GEMINI_API_KEY") };
    }

    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn no_key_returns_none() {
        unsafe { std::env::remove_var("GEMINI_API_KEY") };
        assert!(GeminiProvider::new(None, None, None).is_none());
    }
}
