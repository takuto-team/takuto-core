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
        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            prompt_len = prompt.len(),
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
        )
        .await?;

        Ok(Self { session_id, output })
    }
}

/// Run a Claude Code session. Returns (session_id, output).
/// If resume_session_id is provided, continues that session instead of starting fresh.
async fn run_claude_session(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    timeout_secs: u64,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
    container_runner: Option<&ContainerRunner>,
) -> Result<(String, String)> {
    let prompt_preview = &prompt[..prompt.len().min(200)];
    info!(
        prompt_len = prompt.len(),
        prompt_preview = %prompt_preview,
        resume = ?resume_session_id,
        "Claude session prompt"
    );

    let mut args_vec = vec![
        "--dangerously-skip-permissions",
        "--print",
        "--verbose",
        "-p",
        prompt,
        "--output-format",
        "stream-json",
    ];

    // Resume a previous session to keep conversation context
    let resume_flag;
    if let Some(sid) = resume_session_id {
        resume_flag = sid.to_string();
        args_vec.push("--resume");
        args_vec.push(&resume_flag);
    }

    // Add model flag if configured
    let model_flag;
    if let Some(m) = model {
        if !m.is_empty() {
            model_flag = m.to_string();
            args_vec.push("--model");
            args_vec.push(&model_flag);
        }
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
    .map_err(|e| MaestroError::Claude(format!("Failed to spawn Claude Code: {e}")))?;

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
                return Err(MaestroError::Claude(format!(
                    "Claude Code exited with code {}: {}",
                    output.exit_code,
                    output.stderr.lines().take(5).collect::<Vec<_>>().join("\n")
                )));
            }

            if output.stdout.trim().is_empty() {
                return Err(MaestroError::Claude(
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
        Err(e) => Err(MaestroError::Claude(format!(
            "Claude Code session error: {e}"
        ))),
    }
}

fn parse_stream_json_output(raw: &str) -> String {
    let mut result_parts = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(event_type) = value.get("type").and_then(|v| v.as_str()) {
                match event_type {
                    "system" => {
                        let subtype = value
                            .get("subtype")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        if subtype == "api_retry" {
                            let attempt =
                                value.get("attempt").and_then(|v| v.as_u64()).unwrap_or(0);
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
