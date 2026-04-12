/// Gemini CLI provider — shells out to `gemini -p` for completions.
/// Primary provider in the default chain; requires the `gemini` binary on PATH.
///
/// Prompts are passed via the `-p` flag (not via stdin or as a positional) to prevent
/// command injection from web-scraped content. Output is parsed from `--output-format json`.
///
/// # Startup optimizations
///
/// The gemini CLI is an agentic Node.js application that connects to every configured MCP
/// server at startup (the user has 6). Without mitigation this can add 10-60+ seconds per
/// call as those servers spin up and time out.
///
/// Two flags reduce this:
/// - `--extensions ""` — skips extension loading (~3 s saved)
/// - `current_dir` set to a temp workdir containing `.gemini/settings.json` with
///   `{"mcpServers":{}}` — workspace settings override user settings, so all 6 MCP
///   servers are skipped at subprocess startup (major speedup).
///
/// The workdir is created once at construction and reused for every call.
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tracing::debug;

use crate::clean::strip_thinking_tags;
use crate::error::LlmError;
use crate::provider::{CompletionRequest, LlmProvider};

/// Maximum concurrent Gemini subprocess calls.
const MAX_CONCURRENT: usize = 6;
/// Subprocess deadline — prevents hung `gemini` processes blocking the chain.
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(60);

/// Fixed workdir used for every subprocess call.
/// A workspace-level `.gemini/settings.json` here overrides the user's MCP server config.
const NOXA_GEMINI_WORKDIR: &str = "/tmp/noxa-gemini";

pub struct GeminiCliProvider {
    default_model: String,
    semaphore: Arc<Semaphore>,
    /// Workdir with a minimal `.gemini/settings.json` that disables MCP servers.
    workdir: PathBuf,
}

impl GeminiCliProvider {
    /// Construct the provider.
    /// Model resolves as: `model` arg → `GEMINI_MODEL` env → `"gemini-2.5-pro"`.
    pub fn new(model: Option<String>) -> Self {
        let default_model = model
            .or_else(|| std::env::var("GEMINI_MODEL").ok())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "gemini-2.5-pro".into());

        let workdir = PathBuf::from(NOXA_GEMINI_WORKDIR);
        ensure_gemini_workdir(&workdir);

        Self {
            default_model,
            semaphore: Arc::new(Semaphore::new(MAX_CONCURRENT)),
            workdir,
        }
    }

    #[cfg(test)]
    fn default_model(&self) -> &str {
        &self.default_model
    }
}

#[async_trait]
impl LlmProvider for GeminiCliProvider {
    async fn complete(&self, request: &CompletionRequest) -> Result<String, LlmError> {
        let model = if request.model.is_empty() {
            &self.default_model
        } else {
            &request.model
        };

        // Build the prompt text from all messages.
        let prompt = build_prompt(&request.messages);

        // Acquire concurrency slot before spawning.
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| LlmError::ProviderError("gemini semaphore closed".into()))?;

        let mut cmd = Command::new("gemini");
        // -p STRING: headless mode with prompt as the flag value (never positional arg).
        // Passing via -p prevents command injection; the value is never interpreted as a shell command.
        cmd.arg("-p").arg(&prompt);
        cmd.arg("--model").arg(model);
        // Always request structured JSON output so we can extract the `response` field
        // and skip any preceding noise lines (e.g. MCP status warnings).
        cmd.arg("--output-format").arg("json");
        // --yolo suppresses any interactive confirmation prompts in headless mode.
        cmd.arg("--yolo");
        // --extensions "" skips loading user extensions (~3 s startup savings).
        cmd.arg("--extensions").arg("");
        // Workspace settings in self.workdir override the user's ~/.gemini/settings.json,
        // replacing the user's MCP server list with {} so none are spawned at startup.
        // Without this, each of the user's MCP servers adds latency to every call.
        cmd.current_dir(&self.workdir);

        cmd.stdin(std::process::Stdio::null());
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        debug!(model, workdir = %self.workdir.display(), "spawning gemini subprocess");

        let child = cmd.spawn().map_err(LlmError::Subprocess)?;

        // Bounded wait — prevents indefinite hangs on auth expiry or network stall.
        let output = match timeout(SUBPROCESS_TIMEOUT, child.wait_with_output()).await {
            Ok(Ok(out)) => out,
            Ok(Err(e)) => return Err(LlmError::Subprocess(e)),
            Err(_elapsed) => return Err(LlmError::Timeout),
        };

        if !output.status.success() {
            let stderr_preview = String::from_utf8_lossy(&output.stderr);
            let preview = &stderr_preview[..stderr_preview.len().min(500)];
            return Err(LlmError::ProviderError(format!(
                "gemini exited with {}: {preview}",
                output.status
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let response = extract_response_from_output(&stdout)?;
        let cleaned = strip_code_fences(strip_thinking_tags(&response).trim());
        Ok(cleaned)
    }

    async fn is_available(&self) -> bool {
        // Pure PATH check — no inference call, fast.
        matches!(
            Command::new("gemini")
                .arg("--version")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .await,
            Ok(s) if s.success()
        )
    }

    fn name(&self) -> &str {
        "gemini"
    }
}

/// Parse the `response` field from gemini's `--output-format json` output.
///
/// The CLI emits lines before the JSON object (e.g. MCP status warnings).
/// We find the first `{` to locate the JSON, parse it, and extract `.response`.
fn extract_response_from_output(stdout: &str) -> Result<String, LlmError> {
    let json_start = stdout.find('{').ok_or_else(|| {
        let preview = &stdout[..stdout.len().min(300)];
        LlmError::ProviderError(format!("gemini produced no JSON output: {preview}"))
    })?;

    let json_str = &stdout[json_start..];
    let outer: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
        let preview = &json_str[..json_str.len().min(300)];
        LlmError::ProviderError(format!("failed to parse gemini JSON output: {e} — {preview}"))
    })?;

    // `response` holds the model's actual text output.
    outer["response"]
        .as_str()
        .ok_or_else(|| {
            LlmError::ProviderError(format!(
                "gemini JSON output missing 'response' field: {}",
                &json_str[..json_str.len().min(300)]
            ))
        })
        .map(|s| s.to_string())
}

/// Create the noxa gemini workdir with a minimal workspace settings file.
///
/// The `.gemini/settings.json` written here overrides the user's `~/.gemini/settings.json`
/// for any `gemini` subprocess run from this directory. Setting `mcpServers` to `{}` prevents
/// the CLI from spawning the user's configured MCP servers on every headless call.
///
/// Errors are intentionally ignored — if the write fails, the subprocess still works,
/// just without the startup optimization (and with a warning in the logs).
fn ensure_gemini_workdir(workdir: &std::path::Path) {
    let settings_dir = workdir.join(".gemini");
    let settings_path = settings_dir.join("settings.json");

    if settings_path.exists() {
        return;
    }

    if let Err(e) = std::fs::create_dir_all(&settings_dir) {
        tracing::warn!(path = %settings_dir.display(), error = %e, "failed to create gemini workdir");
        return;
    }

    // Minimal workspace settings: disable all MCP servers.
    // Workspace settings override ~/.gemini/settings.json per gemini CLI docs.
    let content = r#"{"mcpServers":{}}"#;
    if let Err(e) = std::fs::write(&settings_path, content) {
        tracing::warn!(path = %settings_path.display(), error = %e, "failed to write gemini workspace settings");
    }
}

/// Concatenate all messages into a single prompt string for the CLI.
fn build_prompt(messages: &[crate::provider::Message]) -> String {
    messages
        .iter()
        .map(|m| match m.role.as_str() {
            "system" => format!("[System]: {}", m.content),
            "assistant" => format!("[Assistant]: {}", m.content),
            _ => m.content.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Strip markdown code fences from a response string.
fn strip_code_fences(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.starts_with("```") {
        let without_opener = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .unwrap_or(trimmed);
        without_opener
            .strip_suffix("```")
            .unwrap_or(without_opener)
            .trim()
            .to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Construction ──────────────────────────────────────────────────────────

    #[test]
    fn explicit_model_used() {
        let p = GeminiCliProvider::new(Some("gemini-1.5-flash".into()));
        assert_eq!(p.default_model(), "gemini-1.5-flash");
        assert_eq!(p.name(), "gemini");
    }

    #[test]
    fn default_model_fallback() {
        // Explicit None + no GEMINI_MODEL env → hardcoded default.
        // We unset the env to avoid flakiness (it may or may not be set).
        unsafe { std::env::remove_var("GEMINI_MODEL") };
        let p = GeminiCliProvider::new(None);
        assert_eq!(p.default_model(), "gemini-2.5-pro");
    }

    // Env var tests mutate process-global state and race with parallel tests.
    // Run in isolation if needed:
    //   cargo test -p noxa-llm env_model_override -- --ignored --test-threads=1
    #[test]
    #[ignore = "mutates process env; run with --test-threads=1"]
    fn env_model_override() {
        unsafe { std::env::set_var("GEMINI_MODEL", "gemini-1.5-pro") };
        let p = GeminiCliProvider::new(None);
        assert_eq!(p.default_model(), "gemini-1.5-pro");
        unsafe { std::env::remove_var("GEMINI_MODEL") };
    }

    // ── build_prompt ──────────────────────────────────────────────────────────

    #[test]
    fn build_prompt_user_only() {
        use crate::provider::Message;
        let messages = vec![Message {
            role: "user".into(),
            content: "hello world".into(),
        }];
        assert_eq!(build_prompt(&messages), "hello world");
    }

    #[test]
    fn build_prompt_system_and_user() {
        use crate::provider::Message;
        let messages = vec![
            Message {
                role: "system".into(),
                content: "You are helpful.".into(),
            },
            Message {
                role: "user".into(),
                content: "Tell me something.".into(),
            },
        ];
        let result = build_prompt(&messages);
        assert!(result.contains("[System]: You are helpful."));
        assert!(result.contains("Tell me something."));
    }

    // ── extract_response_from_output ──────────────────────────────────────────

    #[test]
    fn extracts_response_from_clean_json() {
        let stdout = r#"{"session_id":"abc","response":"Hello world","stats":{}}"#;
        assert_eq!(extract_response_from_output(stdout).unwrap(), "Hello world");
    }

    #[test]
    fn extracts_response_skipping_mcp_noise() {
        // MCP warning line appears before the JSON object in real gemini output.
        let stdout = "MCP issues detected. Run /mcp list for status.\n{\"session_id\":\"abc\",\"response\":\"the answer\",\"stats\":{}}";
        assert_eq!(
            extract_response_from_output(stdout).unwrap(),
            "the answer"
        );
    }

    #[test]
    fn error_when_no_json_in_output() {
        let result = extract_response_from_output("MCP issues detected. No JSON follows.");
        assert!(matches!(result, Err(LlmError::ProviderError(_))));
    }

    #[test]
    fn error_when_response_field_missing() {
        let stdout = r#"{"session_id":"abc","stats":{}}"#;
        let result = extract_response_from_output(stdout);
        assert!(matches!(result, Err(LlmError::ProviderError(_))));
    }

    // ── strip_code_fences ─────────────────────────────────────────────────────

    #[test]
    fn strips_json_fence() {
        let input = "```json\n{\"key\": \"value\"}\n```";
        assert_eq!(strip_code_fences(input), "{\"key\": \"value\"}");
    }

    #[test]
    fn strips_plain_fence() {
        let input = "```\nhello\n```";
        assert_eq!(strip_code_fences(input), "hello");
    }

    #[test]
    fn passthrough_no_fence() {
        let input = "{\"key\": \"value\"}";
        assert_eq!(strip_code_fences(input), "{\"key\": \"value\"}");
    }

    // ── is_available returns false when binary absent ──────────────────────────

    #[tokio::test]
    async fn unavailable_when_binary_missing() {
        let result = tokio::process::Command::new("__noxa_nonexistent_binary_xyz__")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await;
        assert!(result.is_err(), "missing binary should fail to spawn");
    }

    // ── thinking tag stripping ────────────────────────────────────────────────

    #[test]
    fn strips_thinking_tags_from_output() {
        let raw = "<think>internal reasoning</think>{\"result\": true}";
        let after_thinking = strip_thinking_tags(raw);
        let after_fences = strip_code_fences(after_thinking.trim());
        assert_eq!(after_fences, "{\"result\": true}");
    }

    #[test]
    fn strips_code_fence_after_thinking() {
        let raw = "<think>let me check</think>\n```json\n{\"ok\": 1}\n```";
        let after_thinking = strip_thinking_tags(raw);
        let after_fences = strip_code_fences(after_thinking.trim());
        assert_eq!(after_fences, "{\"ok\": 1}");
    }
}
