// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::container::ContainerRunner;
use crate::error::{MaestroError, Result};
use crate::process::{OutputLine, ProcessHandle};
use crate::workflow::stream_humanize::extract_session_id_from_ndjson;

pub struct ClaudeSession {
    /// The Claude Code session ID (from the init event), used for --resume
    pub session_id: String,
    pub output: String,
}

impl ClaudeSession {
    /// Run a Claude Code session with the given full prompt (interpolated user text + headless suffix).
    ///
    /// `system_prompt` — optional skill content injected via `--system-prompt` (Claude `--bare` mode).
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
        system_prompt: Option<&str>,
    ) -> Result<Self> {
        // === DEV-MODE MOCK ===
        // Off by default. Enabled by [dev] mock_agent = true OR MAESTRO_DEV_MOCK_AGENT=1
        // OR dev_mock::set_test_override(Some(true)).
        if crate::dev_mock::is_enabled_from_runtime() {
            return crate::dev_mock::run_claude_mock(
                worktree,
                prompt,
                cancel_token,
                line_tx,
                resume_session_id,
                system_prompt,
            )
            .await
            .map(|(session_id, output)| Self { session_id, output });
        }
        // === /DEV-MODE MOCK ===

        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            prompt_len = prompt.len(),
            system_prompt_len = system_prompt.map(|s| s.len()).unwrap_or(0),
            "Starting Claude Code session"
        );

        let (session_id, output) = run_claude_session(
            worktree,
            prompt,
            cancel_token,
            timeout_secs,
            line_tx,
            model,
            resume_session_id,
            container_runner,
            system_prompt,
        )
        .await?;

        Ok(Self { session_id, output })
    }
}

/// Run a Claude Code session. Returns (session_id, output).
/// If resume_session_id is provided, continues that session instead of starting fresh.
// Agent session parameters are inherently numerous.
#[allow(clippy::too_many_arguments)]
async fn run_claude_session(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    timeout_secs: u64,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    container_runner: Option<&ContainerRunner>,
    system_prompt: Option<&str>,
) -> Result<(String, String)> {
    // --system-prompt is ignored on --resume (the resumed session keeps its original
    // system prompt). When resuming with skill content, inject it into the -p prompt
    // instead so Claude actually sees the instructions.
    let effective_prompt;
    let effective_system_prompt;
    match (system_prompt, resume_session_id) {
        (Some(sp), Some(_)) => {
            effective_prompt = format!("## Instructions for this step\n\n{sp}\n\n---\n\n{prompt}");
            effective_system_prompt = None;
        }
        (Some(sp), None) => {
            effective_prompt = prompt.to_string();
            effective_system_prompt = Some(sp);
        }
        _ => {
            effective_prompt = prompt.to_string();
            effective_system_prompt = None;
        }
    }

    let prompt_preview: String = effective_prompt.chars().take(200).collect();
    info!(
        prompt_len = effective_prompt.len(),
        prompt_preview = %prompt_preview,
        resume = ?resume_session_id,
        "Claude session prompt"
    );

    let mut args_vec = vec![
        "--dangerously-skip-permissions",
        "--print",
        "--verbose",
        "-p",
        &effective_prompt,
        "--output-format",
        "stream-json",
    ];

    // Only pass --system-prompt on fresh sessions (it's ignored on --resume).
    if let Some(sp) = effective_system_prompt {
        args_vec.push("--system-prompt");
        args_vec.push(sp);
    }

    // Resume a previous session to keep conversation context
    let resume_flag;
    if let Some(sid) = resume_session_id {
        resume_flag = sid.to_string();
        args_vec.push("--resume");
        args_vec.push(&resume_flag);
    }

    // Add model flag if configured
    let model_flag;
    if let Some(m) = model
        && !m.is_empty()
    {
        model_flag = m.to_string();
        args_vec.push("--model");
        args_vec.push(&model_flag);
    }

    let args: &[&str] = &args_vec;
    info!(
        program = "claude",
        args = ?args,
        worktree = %worktree.display(),
        timeout_secs = timeout_secs,
        container = container_runner.is_some(),
        "Spawning Claude Code process"
    );

    let handle = if let Some(runner) = container_runner {
        let (prog, docker_args) = runner.wrap_command("claude", args);
        let docker_arg_refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
        ProcessHandle::spawn(&prog, &docker_arg_refs, worktree, cancel_token).await
    } else {
        ProcessHandle::spawn("claude", args, worktree, cancel_token).await
    }
    .map_err(|e| {
        #[allow(deprecated)]
        MaestroError::ClaudeStr(format!("Failed to spawn Claude Code: {e}"))
    })?;

    let result = if let Some(tx) = line_tx {
        handle.wait_with_streaming_timeout(timeout_secs, tx).await
    } else {
        handle.wait_with_timeout(timeout_secs).await
    };

    match result {
        Ok(output) => {
            info!(
                exit_code = output.exit_code,
                stdout_len = output.stdout.len(),
                stderr_len = output.stderr.len(),
                "Claude Code session finished"
            );

            if !output.success() {
                // Include both stderr and parsed stdout in the error — Claude may
                // have produced diagnostic output before failing.
                let stderr_snippet = output.stderr.lines().take(5).collect::<Vec<_>>().join("\n");
                let stdout_snippet = parse_stream_json_output(&output.stdout);
                let detail = if !stdout_snippet.is_empty() {
                    stdout_snippet
                } else if !stderr_snippet.is_empty() {
                    stderr_snippet
                } else {
                    "(no output)".to_string()
                };
                #[allow(deprecated)]
                return Err(MaestroError::ClaudeStr(format!(
                    "Claude Code exited with code {}: {}",
                    output.exit_code, detail
                )));
            }

            if output.stdout.trim().is_empty() {
                #[allow(deprecated)]
                return Err(MaestroError::ClaudeStr(
                    "Claude Code session produced no output — check that Claude is authenticated in the container".to_string()
                ));
            }

            // Extract the real session ID from the init event
            let real_session_id = extract_session_id_from_ndjson(&output.stdout)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

            // Parse stream-json output to extract the final result
            let parsed = parse_stream_json_output(&output.stdout);
            info!(
                session_id = %real_session_id,
                session_output_len = parsed.len(),
                exit_code = output.exit_code,
                "Claude session completed: output {} chars",
                parsed.len()
            );
            Ok((real_session_id, parsed))
        }
        Err(MaestroError::Timeout(secs)) => {
            warn!(timeout_secs = secs, "Claude Code session timed out");
            Err(MaestroError::Timeout(secs))
        }
        Err(e) => {
            #[allow(deprecated)]
            Err(MaestroError::ClaudeStr(format!(
                "Claude Code session error: {e}"
            )))
        }
    }
}

fn parse_stream_json_output(raw: &str) -> String {
    let mut result_parts = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line)
            && let Some(event_type) = value.get("type").and_then(|v| v.as_str())
        {
            match event_type {
                "system" => {
                    let subtype = value
                        .get("subtype")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    if subtype == "api_retry" {
                        let attempt = value.get("attempt").and_then(|v| v.as_u64()).unwrap_or(0);
                        let error = value
                            .get("error")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        warn!(
                            attempt = attempt,
                            error = error,
                            "Claude API retry detected in output"
                        );
                    } else {
                        info!(subtype = subtype, "Claude stream event: system/{}", subtype);
                    }
                }
                "result" => {
                    if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                        info!(result_len = result.len(), "Claude stream: result received");
                        result_parts.push(result.to_string());
                    }
                    if let Some(usage) = value.get("usage") {
                        let input_tokens = usage
                            .get("input_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        let output_tokens = usage
                            .get("output_tokens")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
                        info!(
                            input_tokens = input_tokens,
                            output_tokens = output_tokens,
                            "Claude session token usage"
                        );
                    }
                    if let Some(cost) = value.get("total_cost_usd").and_then(|v| v.as_f64()) {
                        info!(cost_usd = cost, "Claude session cost");
                    }
                }
                "content_block_delta" => {
                    if let Some(text) = value
                        .get("delta")
                        .and_then(|d| d.get("text"))
                        .and_then(|v| v.as_str())
                    {
                        result_parts.push(text.to_string());
                    }
                }
                "assistant" => {
                    if let Some(content) = value
                        .get("message")
                        .and_then(|m| m.get("content"))
                        .and_then(|c| c.as_str())
                    {
                        result_parts.push(content.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let total_len: usize = result_parts.iter().map(|p| p.len()).sum();
    info!(
        parts = result_parts.len(),
        total_len = total_len,
        "Parsed stream-json output"
    );

    if result_parts.is_empty() {
        raw.to_string()
    } else {
        result_parts.join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduce the argument-building logic from `run_claude_session` to verify flags
    /// without spawning a real process.
    fn build_claude_args(
        prompt: &str,
        model: Option<&str>,
        resume_session_id: Option<&str>,
        system_prompt: Option<&str>,
    ) -> Vec<String> {
        // Replicate the effective_prompt / effective_system_prompt logic
        let effective_prompt;
        let effective_system_prompt;
        match (system_prompt, resume_session_id) {
            (Some(sp), Some(_)) => {
                effective_prompt =
                    format!("## Instructions for this step\n\n{sp}\n\n---\n\n{prompt}");
                effective_system_prompt = None;
            }
            (Some(sp), None) => {
                effective_prompt = prompt.to_string();
                effective_system_prompt = Some(sp.to_string());
            }
            _ => {
                effective_prompt = prompt.to_string();
                effective_system_prompt = None;
            }
        }

        let mut args = vec![
            "--dangerously-skip-permissions".to_string(),
            "--print".to_string(),
            "--verbose".to_string(),
            "-p".to_string(),
            effective_prompt.clone(),
            "--output-format".to_string(),
            "stream-json".to_string(),
        ];

        if let Some(sp) = &effective_system_prompt {
            args.push("--system-prompt".to_string());
            args.push(sp.clone());
        }

        if let Some(sid) = resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.to_string());
        }

        if let Some(m) = model
            && !m.is_empty()
        {
            args.push("--model".to_string());
            args.push(m.to_string());
        }

        args
    }

    #[test]
    fn claude_args_contain_required_flags() {
        let args = build_claude_args("fix the bug", None, None, None);
        assert!(args.contains(&"--dangerously-skip-permissions".to_string()));
        assert!(args.contains(&"--print".to_string()));
        assert!(args.contains(&"--verbose".to_string()));
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
    }

    #[test]
    fn claude_args_include_resume_when_session_id_present() {
        let args = build_claude_args("continue", None, Some("sess-123"), None);
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"sess-123".to_string()));
    }

    #[test]
    fn claude_args_no_resume_when_session_id_absent() {
        let args = build_claude_args("start fresh", None, None, None);
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn claude_args_include_model_when_non_empty() {
        let args = build_claude_args("prompt", Some("claude-opus-4-6"), None, None);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"claude-opus-4-6".to_string()));
    }

    #[test]
    fn claude_args_skip_model_when_empty() {
        let args = build_claude_args("prompt", Some(""), None, None);
        assert!(!args.contains(&"--model".to_string()));
    }

    #[test]
    fn claude_args_system_prompt_on_fresh_session() {
        let args = build_claude_args("do stuff", None, None, Some("You are a linter"));
        assert!(args.contains(&"--system-prompt".to_string()));
        assert!(args.contains(&"You are a linter".to_string()));
        // Prompt should be the original, not prefixed
        assert!(args.contains(&"do stuff".to_string()));
    }

    #[test]
    fn claude_args_system_prompt_injected_into_prompt_on_resume() {
        let args = build_claude_args("resume task", None, Some("sess-456"), Some("Be careful"));
        // --system-prompt should NOT be present on resume
        assert!(!args.contains(&"--system-prompt".to_string()));
        // The effective prompt should contain the system prompt content
        let prompt_idx = args.iter().position(|a| a == "-p").unwrap();
        let effective_prompt = &args[prompt_idx + 1];
        assert!(effective_prompt.contains("Be careful"));
        assert!(effective_prompt.contains("resume task"));
    }

    // --- parse_stream_json_output ---

    #[test]
    fn parse_stream_json_extracts_result() {
        let raw = r#"{"type":"system","subtype":"init"}
{"type":"result","result":"Hello world","usage":{"input_tokens":10,"output_tokens":5}}"#;
        let parsed = parse_stream_json_output(raw);
        assert_eq!(parsed, "Hello world");
    }

    #[test]
    fn parse_stream_json_extracts_content_block_delta() {
        let raw = r#"{"type":"content_block_delta","delta":{"text":"chunk1"}}
{"type":"content_block_delta","delta":{"text":"chunk2"}}"#;
        let parsed = parse_stream_json_output(raw);
        assert_eq!(parsed, "chunk1chunk2");
    }

    #[test]
    fn parse_stream_json_extracts_assistant_message() {
        let raw = r#"{"type":"assistant","message":{"content":"assistant output"}}"#;
        let parsed = parse_stream_json_output(raw);
        assert_eq!(parsed, "assistant output");
    }

    #[test]
    fn parse_stream_json_returns_raw_when_no_recognized_events() {
        let raw = "just some plain text output";
        let parsed = parse_stream_json_output(raw);
        assert_eq!(parsed, raw);
    }

    #[test]
    fn parse_stream_json_skips_empty_lines() {
        let raw = "\n\n{\"type\":\"result\",\"result\":\"ok\"}\n\n";
        let parsed = parse_stream_json_output(raw);
        assert_eq!(parsed, "ok");
    }
}
