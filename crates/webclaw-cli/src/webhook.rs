//! Webhook delivery and `--on-change` command execution.

/// Spawn the `--on-change` command with `payload` on stdin.
///
/// Previously this passed the entire user-provided string to `sh -c`, which
/// made `--on-change 'notify "$URL"; rm -rf /'` a plausible disaster the
/// moment an untrusted config file or MCP-driven agent fed us a command.
/// The MCP surface specifically is prompt-injection-exposed: an LLM that
/// controls CLI args can escalate into arbitrary shell on the host.
///
/// We now parse the command with `shlex` (POSIX-ish tokenization with proper
/// quoting) and exec the program directly without an intermediate shell, so
/// metacharacters like `;`, `&&`, `|`, `$()`, and env expansion can't fire.
/// Users who genuinely need a pipeline can set the whole chain behind a
/// script they've written, or opt in per-call via `WEBCLAW_ALLOW_SHELL=1`
/// (documented escape hatch, noisy by design).
pub async fn spawn_on_change(cmd: &str, stdin_payload: &[u8]) {
    eprintln!("[watch] Running: {cmd}");

    let allow_shell = std::env::var("WEBCLAW_ALLOW_SHELL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let mut command = if allow_shell {
        eprintln!("[watch] WEBCLAW_ALLOW_SHELL=1 — executing via sh -c (unsafe)");
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    } else {
        let Some(argv) = shlex::split(cmd) else {
            eprintln!("[watch] Failed to parse --on-change command (unbalanced quotes?)");
            return;
        };
        let Some((program, args)) = argv.split_first() else {
            eprintln!("[watch] --on-change command is empty");
            return;
        };
        let mut c = tokio::process::Command::new(program);
        c.args(args);
        c
    };

    command.stdin(std::process::Stdio::piped());

    match command.spawn() {
        Ok(mut child) => {
            if let Some(mut stdin) = child.stdin.take() {
                use tokio::io::AsyncWriteExt;
                let _ = stdin.write_all(stdin_payload).await;
            }
        }
        Err(e) => eprintln!("[watch] Failed to run command: {e}"),
    }
}

/// Fire a webhook POST with a JSON payload. Non-blocking — errors logged to stderr.
/// Auto-detects Discord and Slack webhook URLs and wraps the payload accordingly.
pub fn fire_webhook(url: &str, payload: &serde_json::Value) {
    let url = url.to_string();
    let is_discord = url.contains("discord.com/api/webhooks");
    let is_slack = url.contains("hooks.slack.com");

    let body = if is_discord {
        let event = payload
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        let details = serde_json::to_string_pretty(payload).unwrap_or_default();
        serde_json::json!({
            "embeds": [{
                "title": format!("webclaw: {event}"),
                "description": format!("```json\n{details}\n```"),
                "color": 5814783
            }]
        })
        .to_string()
    } else if is_slack {
        let event = payload
            .get("event")
            .and_then(|v| v.as_str())
            .unwrap_or("notification");
        let details = serde_json::to_string_pretty(payload).unwrap_or_default();
        serde_json::json!({
            "text": format!("*webclaw: {event}*\n```{details}```")
        })
        .to_string()
    } else {
        serde_json::to_string(payload).unwrap_or_default()
    };
    tokio::spawn(async move {
        // SSRF guard: a webhook URL is user-supplied and otherwise bypasses
        // the fetch-layer protections, so resolve + reject private/internal
        // destinations before sending the payload.
        if let Err(e) = webclaw_fetch::url_security::validate_public_http_url(&url).await {
            eprintln!("[webhook] refusing unsafe URL: {e}");
            return;
        }
        match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
        {
            Ok(c) => match c
                .post(&url)
                .header("Content-Type", "application/json")
                .body(body)
                .send()
                .await
            {
                Ok(resp) => {
                    eprintln!(
                        "[webhook] POST {} -> {}",
                        &url[..url.len().min(60)],
                        resp.status()
                    );
                }
                Err(e) => eprintln!("[webhook] POST failed: {e}"),
            },
            Err(e) => eprintln!("[webhook] client error: {e}"),
        }
    });
}
