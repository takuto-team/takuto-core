// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Session driver for the `opencode` CLI (SST/opencode-ai).
//!
//! Mirrors [`crate::cursor::CursorSession`] / [`crate::claude::ClaudeSession`]:
//!   * Spawn the agent CLI as a child process via [`ProcessHandle`].
//!   * Optionally stream lines back through the workflow dashboard.
//!   * Parse the NDJSON event stream into a final `output` string and
//!     surface the `sessionID` for downstream resume support.

use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::actions::AgentError;
use crate::config::AiAgentProvider;
use crate::container::ContainerRunner;
use crate::error::{Result, TakutoError};
use crate::process::{OutputLine, ProcessHandle};

/// Result of a single non-interactive `opencode` invocation.
///
/// `session_id` is opencode's own `ses_…` identifier extracted from the first
/// event that carries one (typically `step_start`). When the stream produces
/// nothing usable, a synthetic UUID is returned so callers always have a key.
pub struct OpenCodeSession {
    pub session_id: String,
    pub output: String,
}

impl OpenCodeSession {
    /// Run OpenCode with the given full prompt (interpolated user text + any
    /// headless suffix the driver already injected).
    ///
    /// Arguments mirror [`crate::cursor::session::CursorSession::run_prompt`] —
    /// keep them aligned so the driver can swap providers with a flat match.
    // Agent session parameters are inherently numerous.
    #[allow(clippy::too_many_arguments)]
    pub async fn run_prompt(
        worktree: &Path,
        prompt: &str,
        cancel_token: CancellationToken,
        timeout_secs: u64,
        line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
        model: Option<&str>,
        resume_session_id: Option<&str>,
        container_runner: Option<&ContainerRunner>,
    ) -> Result<Self> {
        // === DEV-MODE MOCK ===
        // Off by default. Enabled by [dev] mock_agent = true OR TAKUTO_DEV_MOCK_AGENT=1
        // OR dev_mock::set_test_override(Some(true)).
        //
        // No opencode-flavoured mock exists yet (dev_mock only has claude/cursor
        // shapes), so we return a minimal stub that still satisfies the contract:
        // a `ses_<uuid>` id and a non-empty body. The dashboard humanizer falls
        // back to raw passthrough for OpenCode, so this is enough to exercise the
        // workflow plumbing in tests without burning a token.
        if crate::dev_mock::is_enabled_from_runtime() {
            return Ok(Self {
                session_id: format!("ses_{}", uuid::Uuid::new_v4().simple()),
                output: "[dev-mock] opencode stub output".to_string(),
            });
        }
        // === /DEV-MODE MOCK ===

        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            prompt_len = prompt.len(),
            "Starting OpenCode session"
        );

        let (session_id, output) = run_opencode_session(
            worktree,
            prompt,
            cancel_token,
            timeout_secs,
            line_tx,
            model,
            resume_session_id,
            container_runner,
        )
        .await?;

        Ok(Self { session_id, output })
    }
}

// Agent session parameters are inherently numerous.
#[allow(clippy::too_many_arguments)]
async fn run_opencode_session(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    timeout_secs: u64,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    container_runner: Option<&ContainerRunner>,
) -> Result<(String, String)> {
    let owned = build_opencode_args(prompt, model, resume_session_id);
    let arg_refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

    info!(
        program = "opencode",
        args_len = arg_refs.len(),
        worktree = %worktree.display(),
        timeout_secs = timeout_secs,
        container = container_runner.is_some(),
        "Spawning OpenCode CLI"
    );

    let handle = if let Some(runner) = container_runner {
        let (prog, docker_args) = runner.wrap_command("opencode", &arg_refs);
        let docker_arg_refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
        ProcessHandle::spawn(&prog, &docker_arg_refs, worktree, cancel_token).await
    } else {
        ProcessHandle::spawn("opencode", &arg_refs, worktree, cancel_token).await
    }?;

    let result = if let Some(tx) = line_tx {
        handle.wait_with_streaming_timeout(timeout_secs, tx).await
    } else {
        handle.wait_with_timeout(timeout_secs).await
    };

    match result {
        Ok(output) => {
            if !output.success() {
                // opencode often surfaces the *real* failure as a JSON
                // `type=error` event on stdout (provider auth, model not
                // found, upstream LM Studio / Ollama context overflow, …)
                // and prints only generic startup noise to stderr. If we
                // can pull a structured error out of stdout, prefer it —
                // otherwise fall back to the stderr tail. Take a larger
                // tail too: opencode's first-run sqlite migration eats 3
                // of the 8 lines we used to capture, leaving room for
                // almost no real diagnostic.
                let stdout_err = first_opencode_error(&output.stdout);
                let stderr_tail = output.stderr.lines().rev().take(40).collect::<Vec<_>>();
                let stderr_tail = stderr_tail
                    .iter()
                    .rev()
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n");
                let detail = match stdout_err {
                    Some(msg) => format!("{msg}\n--- stderr tail ---\n{stderr_tail}"),
                    None => stderr_tail,
                };
                return Err(AgentError::NonZeroExit {
                    provider: AiAgentProvider::OpenCode,
                    exit_code: output.exit_code,
                    stderr_tail: detail,
                }
                .into());
            }

            if output.stdout.trim().is_empty() {
                return Err(AgentError::EmptyOutput {
                    provider: AiAgentProvider::OpenCode,
                    hint: "check `opencode auth` / provider configuration",
                }
                .into());
            }

            // Surface an `error` event from the stream as a session failure.
            if let Some(err_msg) = first_opencode_error(&output.stdout) {
                return Err(AgentError::StreamFailed {
                    provider: AiAgentProvider::OpenCode,
                    message: err_msg,
                }
                .into());
            }

            let real_session_id = extract_opencode_session_id(&output.stdout)
                .unwrap_or_else(|| format!("ses_{}", uuid::Uuid::new_v4().simple()));
            let parsed = parse_opencode_stream_json_output(&output.stdout);
            info!(
                session_id = %real_session_id,
                session_output_len = parsed.len(),
                "OpenCode session completed"
            );
            Ok((real_session_id, parsed))
        }
        Err(TakutoError::Timeout(secs)) => {
            warn!(timeout_secs = secs, "OpenCode session timed out");
            Err(TakutoError::Timeout(secs))
        }
        Err(e) => Err(e),
    }
}

/// Build the argv passed to `opencode`. Extracted so unit tests can exercise the
/// flag-shaping logic without spawning a real process.
fn build_opencode_args(
    prompt: &str,
    model: Option<&str>,
    resume_session_id: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "run".to_string(),
        "--format".to_string(),
        "json".to_string(),
        "--dangerously-skip-permissions".to_string(),
        // Stream opencode's internal logs to stderr so a failure surfaces
        // the real reason (provider not found, upstream OpenAI-compat
        // server error, context overflow, …) instead of just the boot-time
        // sqlite-migration noise the parser sees today.
        "--print-logs".to_string(),
        "--log-level".to_string(),
        "WARN".to_string(),
    ];

    if let Some(m) = model
        && !m.is_empty()
    {
        // OpenCode's `-m` flag wants `<providerId>/<modelId>` and Takuto's
        // synthesised `opencode.json` always names the provider
        // `self_hosted`. Accept the model in either form — `qwen/x` or
        // `self_hosted/qwen/x` — by always prepending the prefix when it
        // isn't already there. Matches the symmetric strip in
        // `write_opencode_config`.
        let with_provider = if m.starts_with("self_hosted/") {
            m.to_string()
        } else {
            format!("self_hosted/{m}")
        };
        args.push("-m".to_string());
        args.push(with_provider);
    }

    if let Some(sid) = resume_session_id
        && !sid.is_empty()
    {
        // opencode uses `-s <session_id>` to continue an existing session
        // (NOT `--resume` — that's claude/cursor).
        args.push("-s".to_string());
        args.push(sid.to_string());
    }

    // Positional prompt argument goes last so opencode does not misread leading
    // dashes inside the prompt as flags.
    args.push(prompt.to_string());

    args
}

/// Walk the NDJSON stream and return the first `sessionID` seen. The first
/// event with one is normally `step_start`, but any event suffices.
fn extract_opencode_session_id(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(sid) = value.get("sessionID").and_then(|v| v.as_str())
            && !sid.is_empty()
        {
            return Some(sid.to_string());
        }
    }
    None
}

/// Find the first `type:"error"` event and return a best-effort human message.
fn first_opencode_error(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("error") {
            continue;
        }
        // Preferred shape: { "type":"error", "error": { "message": "…" } }
        if let Some(msg) = value
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            && !msg.is_empty()
        {
            return Some(msg.to_string());
        }
        // Fall back to a top-level message field.
        if let Some(msg) = value.get("message").and_then(|v| v.as_str())
            && !msg.is_empty()
        {
            return Some(msg.to_string());
        }
        // Last resort: surface the raw event so the user sees something.
        return Some(line.to_string());
    }
    None
}

/// Filter the NDJSON stream and concatenate the `type:"text"` event bodies into
/// the final user-visible output string. Tool-use, thinking deltas, and the
/// `step_start` / `step_finish` envelopes are skipped.
///
/// When the stream parses but yields no text parts, the raw output is returned
/// so callers still see something actionable.
pub fn parse_opencode_stream_json_output(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if !line.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let Some(event_type) = value.get("type").and_then(|v| v.as_str()) else {
            continue;
        };

        if event_type != "text" {
            continue;
        }

        // Preferred shape from the docs: { "type":"text", "part": { "text": "…" } }
        if let Some(text) = value
            .get("part")
            .and_then(|p| p.get("text"))
            .and_then(|v| v.as_str())
            && !text.is_empty()
        {
            parts.push(text.to_string());
            continue;
        }
        // Fallback shape some opencode versions emit: { "type":"text", "text":"…" }.
        if let Some(text) = value.get("text").and_then(|v| v.as_str())
            && !text.is_empty()
        {
            parts.push(text.to_string());
        }
    }

    if parts.is_empty() {
        raw.to_string()
    } else {
        parts.join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- build_opencode_args ---

    #[test]
    fn opencode_args_contain_required_flags() {
        let args = build_opencode_args("do work", None, None);
        assert_eq!(args[0], "run");
        assert!(args.contains(&"--format".to_string()));
        assert!(args.contains(&"json".to_string()));
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        // Positional prompt is the final argument.
        assert_eq!(args.last(), Some(&"do work".to_string()));
    }

    #[test]
    fn opencode_args_include_session_when_resume_present() {
        let args = build_opencode_args("continue", None, Some("ses_abc123"));
        assert!(args.contains(&"-s".to_string()));
        assert!(args.contains(&"ses_abc123".to_string()));
        // -s must NOT be aliased to --resume (that's claude/cursor).
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn opencode_args_no_session_when_resume_absent() {
        let args = build_opencode_args("fresh", None, None);
        assert!(!args.contains(&"-s".to_string()));
    }

    #[test]
    fn opencode_args_no_session_when_resume_empty() {
        let args = build_opencode_args("fresh", None, Some(""));
        assert!(!args.contains(&"-s".to_string()));
    }

    #[test]
    fn opencode_args_include_model_when_non_empty() {
        // Bare model id gets the synthesised `self_hosted/` provider prefix
        // so it matches the key in the materialised opencode.json.
        let args = build_opencode_args("prompt", Some("qwen/qwen2.5-vl-7b"), None);
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"self_hosted/qwen/qwen2.5-vl-7b".to_string()));
    }

    #[test]
    fn opencode_args_keep_existing_self_hosted_prefix() {
        // User config that already has the prefix must NOT get double-prefixed.
        let args = build_opencode_args("prompt", Some("self_hosted/qwen/foo"), None);
        assert!(args.contains(&"self_hosted/qwen/foo".to_string()));
        assert!(!args.contains(&"self_hosted/self_hosted/qwen/foo".to_string()));
    }

    #[test]
    fn opencode_args_skip_model_when_empty() {
        let args = build_opencode_args("prompt", Some(""), None);
        assert!(!args.contains(&"-m".to_string()));
    }

    #[test]
    fn opencode_prompt_is_positional_last_argument() {
        // Guard against regressions where the prompt is passed via -p (claude style).
        let args = build_opencode_args("hello", Some("anthropic/claude"), Some("ses_xyz"));
        assert_eq!(args.last(), Some(&"hello".to_string()));
        assert!(!args.contains(&"-p".to_string()));
    }

    // --- extract_opencode_session_id ---

    #[test]
    fn extract_session_id_from_step_start() {
        let raw = r#"{"type":"step_start","sessionID":"ses_abcdefghij1234567890ab","part":{"type":"step-start"}}
{"type":"text","sessionID":"ses_abcdefghij1234567890ab","part":{"text":"hi"}}"#;
        let sid = extract_opencode_session_id(raw).expect("session id present");
        assert_eq!(sid, "ses_abcdefghij1234567890ab");
    }

    #[test]
    fn extract_session_id_from_any_event() {
        let raw = r#"{"type":"text","sessionID":"ses_xyz","part":{"text":"hello"}}"#;
        let sid = extract_opencode_session_id(raw).expect("session id present");
        assert_eq!(sid, "ses_xyz");
    }

    #[test]
    fn extract_session_id_returns_none_when_missing() {
        let raw = r#"{"type":"text","part":{"text":"no session id here"}}"#;
        assert!(extract_opencode_session_id(raw).is_none());
    }

    #[test]
    fn extract_session_id_skips_non_json_lines() {
        let raw = "starting opencode...\n{\"type\":\"step_start\",\"sessionID\":\"ses_42\"}";
        let sid = extract_opencode_session_id(raw).expect("session id present");
        assert_eq!(sid, "ses_42");
    }

    // --- first_opencode_error ---

    #[test]
    fn first_error_extracts_message_field() {
        let raw = r#"{"type":"step_start","sessionID":"ses_1"}
{"type":"error","sessionID":"ses_1","error":{"message":"no provider configured"}}"#;
        let err = first_opencode_error(raw).expect("error detected");
        assert_eq!(err, "no provider configured");
    }

    #[test]
    fn first_error_falls_back_to_top_level_message() {
        let raw = r#"{"type":"error","sessionID":"ses_1","message":"boom"}"#;
        let err = first_opencode_error(raw).expect("error detected");
        assert_eq!(err, "boom");
    }

    #[test]
    fn first_error_returns_none_on_clean_stream() {
        let raw = r#"{"type":"step_start","sessionID":"ses_1"}
{"type":"text","part":{"text":"ok"}}
{"type":"step_finish","sessionID":"ses_1","reason":"stop"}"#;
        assert!(first_opencode_error(raw).is_none());
    }

    // --- parse_opencode_stream_json_output ---

    #[test]
    fn parse_stream_extracts_text_part() {
        let raw = r#"{"type":"step_start","sessionID":"ses_1"}
{"type":"text","sessionID":"ses_1","part":{"text":"Hello, "}}
{"type":"text","sessionID":"ses_1","part":{"text":"world!"}}
{"type":"step_finish","sessionID":"ses_1","reason":"stop"}"#;
        let parsed = parse_opencode_stream_json_output(raw);
        assert_eq!(parsed, "Hello, world!");
    }

    #[test]
    fn parse_stream_skips_tool_use_and_thinking() {
        let raw = r#"{"type":"step_start","sessionID":"ses_1"}
{"type":"tool_use","sessionID":"ses_1"}
{"type":"message.part.updated","part":{"type":"thinking","text":"hmm"}}
{"type":"text","sessionID":"ses_1","part":{"text":"answer"}}
{"type":"step_finish","sessionID":"ses_1","reason":"stop"}"#;
        let parsed = parse_opencode_stream_json_output(raw);
        assert_eq!(parsed, "answer");
    }

    #[test]
    fn parse_stream_accepts_flat_text_field() {
        // Fallback shape: { "type":"text", "text":"…" }
        let raw = r#"{"type":"text","text":"flat"}"#;
        let parsed = parse_opencode_stream_json_output(raw);
        assert_eq!(parsed, "flat");
    }

    #[test]
    fn parse_stream_returns_raw_when_no_text_events() {
        let raw = "plain output with no json";
        let parsed = parse_opencode_stream_json_output(raw);
        assert_eq!(parsed, raw);
    }

    #[test]
    fn parse_stream_skips_empty_lines() {
        let raw = "\n\n{\"type\":\"text\",\"part\":{\"text\":\"ok\"}}\n\n";
        let parsed = parse_opencode_stream_json_output(raw);
        assert_eq!(parsed, "ok");
    }

    #[test]
    fn parse_stream_ignores_non_json_lines() {
        let raw = "starting opencode...\n{\"type\":\"text\",\"part\":{\"text\":\"ok\"}}\nstray log";
        let parsed = parse_opencode_stream_json_output(raw);
        assert_eq!(parsed, "ok");
    }
}
