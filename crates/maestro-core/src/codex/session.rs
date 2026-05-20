// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Codex CLI session adapter.
//!
//! Invokes `codex exec --json …` (non-interactive JSONL streaming mode) and
//! collects the `agent_message` text from `item.completed` events. The CLI
//! occasionally interleaves tracing lines on stdout — the JSONL parser skips
//! any line that doesn't begin with `{`.

use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::container::ContainerRunner;
use crate::error::{MaestroError, Result};
use crate::process::{OutputLine, ProcessHandle};

/// Default binary name on the maestro Docker image. Kept as a const so tests
/// and integration code share the same value.
const CODEX_BIN: &str = "codex";

pub struct CodexSession {
    /// The Codex thread/session id (UUIDv7 emitted by `thread.started`),
    /// used for `codex exec resume …`.
    pub session_id: String,
    pub output: String,
}

impl CodexSession {
    /// Run a Codex CLI session with the given full prompt.
    ///
    /// * `worktree` — workspace directory passed to `--cd`.
    /// * `prompt` — final positional arg (already includes any headless suffix).
    /// * `model` — optional, omitted from argv when `None` or empty.
    /// * `resume_session_id` — when `Some`, uses `codex exec resume <id> …`.
    /// * `container_runner` — when `Some`, the command is wrapped for Docker.
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
        // No dedicated codex mock yet — return a synthetic success so dev flows
        // that exercise the agent abstraction don't fail when `[dev] mock_agent`
        // is on. The Claude/Cursor mocks emit richer line streams; we keep this
        // stub minimal until a real mock lands.
        if crate::dev_mock::is_enabled_from_runtime() {
            return Ok(Self {
                session_id: uuid::Uuid::new_v4().to_string(),
                output: "[dev-mock] codex stub output".to_string(),
            });
        }
        // === /DEV-MODE MOCK ===

        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            prompt_len = prompt.len(),
            "Starting Codex CLI session"
        );

        let (session_id, output) = run_codex_session(
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
async fn run_codex_session(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    timeout_secs: u64,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    container_runner: Option<&ContainerRunner>,
) -> Result<(String, String)> {
    let workspace = worktree
        .to_str()
        .ok_or_else(|| MaestroError::AiAgent("Worktree path is not valid UTF-8".to_string()))?;

    let owned = build_codex_args(workspace, prompt, model, resume_session_id);
    let arg_refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

    info!(
        program = CODEX_BIN,
        args_len = arg_refs.len(),
        worktree = %worktree.display(),
        timeout_secs = timeout_secs,
        container = container_runner.is_some(),
        "Spawning Codex CLI"
    );

    let handle = if let Some(runner) = container_runner {
        let (prog, docker_args) = runner.wrap_command(CODEX_BIN, &arg_refs);
        let docker_arg_refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
        ProcessHandle::spawn(&prog, &docker_arg_refs, worktree, cancel_token).await
    } else {
        ProcessHandle::spawn(CODEX_BIN, &arg_refs, worktree, cancel_token).await
    }
    .map_err(|e| MaestroError::AiAgent(format!("Failed to spawn Codex CLI: {e}")))?;

    let result = if let Some(tx) = line_tx {
        handle.wait_with_streaming_timeout(timeout_secs, tx).await
    } else {
        handle.wait_with_timeout(timeout_secs).await
    };

    match result {
        Ok(output) => {
            if !output.success() {
                return Err(MaestroError::AiAgent(format!(
                    "Codex CLI exited with code {}: {}",
                    output.exit_code,
                    output.stderr.lines().take(8).collect::<Vec<_>>().join("\n")
                )));
            }

            if output.stdout.trim().is_empty() {
                return Err(MaestroError::AiAgent(
                    "Codex CLI produced no output — check OPENAI_API_KEY in the environment"
                        .to_string(),
                ));
            }

            // turn.failed inside the stream is a logical failure even with exit 0.
            if let Some(msg) = find_codex_turn_failure(&output.stdout) {
                return Err(MaestroError::AiAgent(format!(
                    "Codex CLI reported turn.failed: {msg}"
                )));
            }

            let real_session_id = extract_codex_session_id(&output.stdout)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let parsed = parse_codex_stream_json_output(&output.stdout);
            info!(
                session_id = %real_session_id,
                session_output_len = parsed.len(),
                "Codex CLI session completed"
            );
            Ok((real_session_id, parsed))
        }
        Err(MaestroError::Timeout(secs)) => {
            warn!(timeout_secs = secs, "Codex CLI session timed out");
            Err(MaestroError::Timeout(secs))
        }
        Err(e) => Err(MaestroError::AiAgent(format!("Codex CLI error: {e}"))),
    }
}

/// Build argv for the codex CLI. Kept as a free function so unit tests can
/// exercise the flag shape without spawning a real process.
fn build_codex_args(
    workspace: &str,
    prompt: &str,
    model: Option<&str>,
    resume_session_id: Option<&str>,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    args.push("exec".to_string());

    // Resume must immediately follow `exec`.
    if let Some(sid) = resume_session_id {
        args.push("resume".to_string());
        args.push(sid.to_string());
    }

    args.push("--json".to_string());
    args.push("--skip-git-repo-check".to_string());
    args.push("--dangerously-bypass-approvals-and-sandbox".to_string());
    args.push("--ignore-user-config".to_string());
    args.push("--cd".to_string());
    args.push(workspace.to_string());

    if let Some(m) = model
        && !m.is_empty()
    {
        args.push("-m".to_string());
        args.push(m.to_string());
    }

    // Prompt is the final positional argument.
    args.push(prompt.to_string());

    args
}

/// Parse the codex JSONL stdout stream and concatenate every assistant
/// `agent_message` text. Lines that don't start with `{` (e.g. stray tracing
/// log lines from `codex_api::…`) are skipped. Reasoning and command execution
/// items are skipped too — they're not user-visible content.
///
/// Returns the joined text, or — if nothing recognisable parsed — the raw
/// stdout, so the caller never silently loses content.
pub fn parse_codex_stream_json_output(raw: &str) -> String {
    let mut parts: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        // Codex sometimes interleaves tracing lines on stdout — skip anything
        // that isn't a JSON object.
        if !trimmed.starts_with('{') {
            continue;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };

        let Some(event_type) = value.get("type").and_then(|v| v.as_str()) else {
            continue;
        };

        if event_type != "item.completed" {
            continue;
        }

        let Some(item) = value.get("item") else {
            continue;
        };
        let Some(item_type) = item.get("type").and_then(|v| v.as_str()) else {
            continue;
        };

        if item_type == "agent_message"
            && let Some(text) = item.get("text").and_then(|v| v.as_str())
            && !text.is_empty()
        {
            parts.push(text.to_string());
        }
        // reasoning, command_execution, and any other item kinds are skipped.
    }

    if parts.is_empty() {
        raw.to_string()
    } else {
        parts.join("")
    }
}

/// Walk the JSONL stream and return the `thread_id` from the first
/// `thread.started` event, if any. Mirrors
/// [`crate::workflow::stream_humanize::extract_session_id_from_ndjson`] but
/// for codex's event names. The integration agent adds a matching arm to the
/// shared helper later; we keep this local for now.
fn extract_codex_session_id(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed)
            && value.get("type").and_then(|v| v.as_str()) == Some("thread.started")
            && let Some(id) = value.get("thread_id").and_then(|v| v.as_str())
        {
            return Some(id.to_string());
        }
    }
    None
}

/// Scan for a `turn.failed` event and return the `error.message` payload if
/// present.
fn find_codex_turn_failure(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with('{') {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) else {
            continue;
        };
        if value.get("type").and_then(|v| v.as_str()) != Some("turn.failed") {
            continue;
        }
        let msg = value
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|v| v.as_str())
            .unwrap_or("(no error message)");
        return Some(msg.to_string());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- argv shape ---

    #[test]
    fn codex_args_contain_required_flags() {
        let args = build_codex_args("/workspace/project", "do work", None, None);
        assert_eq!(args.first().map(|s| s.as_str()), Some("exec"));
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"--skip-git-repo-check".to_string()));
        assert!(args.contains(&"--dangerously-bypass-approvals-and-sandbox".to_string()));
        assert!(args.contains(&"--ignore-user-config".to_string()));
        assert!(args.contains(&"--cd".to_string()));
        assert!(args.contains(&"/workspace/project".to_string()));
        // Prompt is the last positional.
        assert_eq!(args.last().map(|s| s.as_str()), Some("do work"));
    }

    #[test]
    fn codex_args_no_resume_when_session_id_absent() {
        let args = build_codex_args("/ws", "prompt", None, None);
        assert!(!args.contains(&"resume".to_string()));
        // `exec` is followed immediately by `--json` (no `resume <id>`).
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "--json");
    }

    #[test]
    fn codex_args_include_resume_when_session_id_present() {
        let args = build_codex_args("/ws", "prompt", None, Some("thread-uuidv7"));
        assert_eq!(args[0], "exec");
        assert_eq!(args[1], "resume");
        assert_eq!(args[2], "thread-uuidv7");
        // After resume, the standard flags follow.
        assert_eq!(args[3], "--json");
    }

    #[test]
    fn codex_args_include_model_when_non_empty() {
        let args = build_codex_args("/ws", "prompt", Some("gpt-5-codex"), None);
        // -m and its value should be adjacent.
        let pos = args
            .iter()
            .position(|a| a == "-m")
            .expect("-m flag missing");
        assert_eq!(args[pos + 1], "gpt-5-codex");
    }

    #[test]
    fn codex_args_skip_model_when_empty() {
        let args = build_codex_args("/ws", "prompt", Some(""), None);
        assert!(!args.contains(&"-m".to_string()));
    }

    #[test]
    fn codex_args_skip_model_when_none() {
        let args = build_codex_args("/ws", "prompt", None, None);
        assert!(!args.contains(&"-m".to_string()));
    }

    #[test]
    fn codex_args_prompt_is_last_even_with_resume_and_model() {
        let args = build_codex_args("/ws", "the prompt", Some("gpt-5-codex"), Some("tid"));
        assert_eq!(args.last().map(|s| s.as_str()), Some("the prompt"));
    }

    // --- parse_codex_stream_json_output ---

    #[test]
    fn parse_codex_stream_extracts_agent_message() {
        let raw = r#"{"type":"thread.started","thread_id":"abc"}
{"type":"turn.started"}
{"type":"item.completed","item":{"type":"agent_message","text":"Hello!"}}
{"type":"turn.completed"}"#;
        let parsed = parse_codex_stream_json_output(raw);
        assert_eq!(parsed, "Hello!");
    }

    #[test]
    fn parse_codex_stream_concatenates_multiple_agent_messages() {
        let raw = r#"{"type":"item.completed","item":{"type":"agent_message","text":"part1 "}}
{"type":"item.completed","item":{"type":"agent_message","text":"part2"}}"#;
        let parsed = parse_codex_stream_json_output(raw);
        assert_eq!(parsed, "part1 part2");
    }

    #[test]
    fn parse_codex_stream_skips_reasoning_and_command_execution() {
        let raw = r#"{"type":"item.completed","item":{"type":"reasoning","text":"thinking..."}}
{"type":"item.completed","item":{"type":"command_execution","command":"ls"}}
{"type":"item.completed","item":{"type":"agent_message","text":"only this"}}"#;
        let parsed = parse_codex_stream_json_output(raw);
        assert_eq!(parsed, "only this");
    }

    #[test]
    fn parse_codex_stream_skips_non_brace_lines() {
        let raw = "2026-05-20T12:00:00Z ERROR codex_api::client request failed\n{\"type\":\"item.completed\",\"item\":{\"type\":\"agent_message\",\"text\":\"ok\"}}\nnot json either";
        let parsed = parse_codex_stream_json_output(raw);
        assert_eq!(parsed, "ok");
    }

    #[test]
    fn parse_codex_stream_returns_raw_when_no_events() {
        let raw = "plain text with no JSON whatsoever";
        let parsed = parse_codex_stream_json_output(raw);
        assert_eq!(parsed, raw);
    }

    #[test]
    fn parse_codex_stream_skips_empty_agent_message_text() {
        let raw = r#"{"type":"item.completed","item":{"type":"agent_message","text":""}}"#;
        let parsed = parse_codex_stream_json_output(raw);
        // Falls back to raw when nothing usable was extracted.
        assert_eq!(parsed, raw);
    }

    // --- extract_codex_session_id ---

    #[test]
    fn extract_session_id_pulls_thread_id_from_thread_started() {
        let raw = r#"{"type":"thread.started","thread_id":"019480d2-2e2a-7c11-aaaa-bbbbccccdddd"}
{"type":"turn.started"}"#;
        let sid = extract_codex_session_id(raw);
        assert_eq!(sid.as_deref(), Some("019480d2-2e2a-7c11-aaaa-bbbbccccdddd"));
    }

    #[test]
    fn extract_session_id_returns_none_when_missing() {
        let raw = r#"{"type":"turn.started"}"#;
        assert!(extract_codex_session_id(raw).is_none());
    }

    #[test]
    fn extract_session_id_skips_non_brace_lines() {
        let raw = "tracing leak line\n{\"type\":\"thread.started\",\"thread_id\":\"tid\"}";
        assert_eq!(extract_codex_session_id(raw).as_deref(), Some("tid"));
    }

    // --- find_codex_turn_failure ---

    #[test]
    fn find_turn_failure_extracts_error_message() {
        let raw = r#"{"type":"turn.failed","error":{"message":"rate limited"}}"#;
        assert_eq!(
            find_codex_turn_failure(raw).as_deref(),
            Some("rate limited")
        );
    }

    #[test]
    fn find_turn_failure_returns_none_on_success_stream() {
        let raw = r#"{"type":"turn.completed"}"#;
        assert!(find_codex_turn_failure(raw).is_none());
    }
}
