use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::error::{MaestroError, Result};
use crate::process::ProcessHandle;

pub struct ClaudeSession {
    pub session_id: String,
    pub output: String,
}

impl ClaudeSession {
    pub async fn start_address_ticket(
        worktree: &Path,
        ticket_context: &str,
        cancel_token: CancellationToken,
        timeout_secs: u64,
    ) -> Result<Self> {
        info!(
            worktree = %worktree.display(),
            "Starting Claude Code /address-ticket session"
        );

        let session_id = uuid::Uuid::new_v4().to_string();
        let prompt = format!("/address-ticket {ticket_context}");

        let output = run_claude_session(worktree, &prompt, cancel_token, timeout_secs).await?;

        Ok(Self {
            session_id,
            output,
        })
    }

    pub async fn start_review_changes(
        worktree: &Path,
        cancel_token: CancellationToken,
        timeout_secs: u64,
    ) -> Result<Self> {
        info!(
            worktree = %worktree.display(),
            "Starting Claude Code /review-changes session"
        );

        let session_id = uuid::Uuid::new_v4().to_string();
        let prompt = "/review-changes".to_string();

        let output = run_claude_session(worktree, &prompt, cancel_token, timeout_secs).await?;

        Ok(Self {
            session_id,
            output,
        })
    }

    pub async fn start_fix_session(
        worktree: &Path,
        error_output: &str,
        fix_instructions: &str,
        cancel_token: CancellationToken,
        timeout_secs: u64,
    ) -> Result<Self> {
        info!(
            worktree = %worktree.display(),
            "Starting Claude Code fix session"
        );

        let session_id = uuid::Uuid::new_v4().to_string();
        let prompt = format!(
            "The following command failed with this output:\n\n```\n{error_output}\n```\n\n{fix_instructions}"
        );

        let output = run_claude_session(worktree, &prompt, cancel_token, timeout_secs).await?;

        Ok(Self {
            session_id,
            output,
        })
    }
}

async fn run_claude_session(
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    timeout_secs: u64,
) -> Result<String> {
    let handle = ProcessHandle::spawn(
        "claude",
        &[
            "--allow-dangerously-skip-permissions",
            "--print",
            "-p",
            prompt,
            "--output-format",
            "stream-json",
        ],
        worktree,
        cancel_token,
    )
    .await
    .map_err(|e| MaestroError::Claude(format!("Failed to spawn Claude Code: {e}")))?;

    let result = handle.wait_with_timeout(timeout_secs).await;

    match result {
        Ok(output) => {
            if !output.success() {
                warn!(
                    exit_code = output.exit_code,
                    stderr = %output.stderr,
                    "Claude Code session exited with non-zero status"
                );
            }

            // Parse stream-json output to extract the final result
            let parsed = parse_stream_json_output(&output.stdout);
            debug!(
                session_output_len = parsed.len(),
                "Claude Code session completed"
            );
            Ok(parsed)
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
            // Look for result/content events in the stream
            if let Some(event_type) = value.get("type").and_then(|v| v.as_str()) {
                match event_type {
                    "result" => {
                        if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                            result_parts.push(result.to_string());
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

    if result_parts.is_empty() {
        // Fallback: return raw output if no structured events found
        raw.to_string()
    } else {
        result_parts.join("")
    }
}
