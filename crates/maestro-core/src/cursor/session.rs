use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::error::{MaestroError, Result};
use crate::process::{OutputLine, ProcessHandle};
use crate::workflow::stream_humanize::extract_session_id_from_ndjson;

pub struct CursorSession {
    pub session_id: String,
    pub output: String,
}

impl CursorSession {
    pub async fn start_address_ticket(
        cursor_cli: &str,
        worktree: &Path,
        ticket_context: &str,
        cancel_token: CancellationToken,
        timeout_secs: u64,
        line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
        model: Option<&str>,
        resume_session_id: Option<&str>,
    ) -> Result<Self> {
        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            "Starting Cursor Agent address-ticket session"
        );

        let prompt = format!(
            "You are implementing a Jira ticket in this repository. Follow the project's conventions.\n\n\
            Ticket context:\n{ticket_context}\n\n\
            IMPORTANT: Fully automated headless run — no human operator. \
            Do not ask questions or wait for user input. \
            Implement the ticket, add or update tests as appropriate, and summarize what you did.\n\n\
            If the project uses a skill or rule named address-ticket, follow it; otherwise proceed from the ticket context alone."
        );

        let (session_id, output) = run_cursor_agent_session(
            cursor_cli,
            worktree,
            &prompt,
            cancel_token,
            timeout_secs,
            line_tx,
            model,
            resume_session_id,
        )
        .await?;

        Ok(Self {
            session_id,
            output,
        })
    }

    pub async fn start_review_changes(
        cursor_cli: &str,
        worktree: &Path,
        cancel_token: CancellationToken,
        timeout_secs: u64,
        line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
        model: Option<&str>,
        resume_session_id: Option<&str>,
    ) -> Result<Self> {
        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            "Starting Cursor Agent review session"
        );

        let prompt = concat!(
            "Review all uncommitted and recent changes in this repository for correctness, security, and style. ",
            "Fix any issues you find. This is a fully automated headless run — do not ask the user questions; ",
            "apply fixes directly.\n\n",
            "If the project defines review-changes guidance in docs or rules, follow it."
        )
        .to_string();

        let (session_id, output) = run_cursor_agent_session(
            cursor_cli,
            worktree,
            &prompt,
            cancel_token,
            timeout_secs,
            line_tx,
            model,
            resume_session_id,
        )
        .await?;

        Ok(Self {
            session_id,
            output,
        })
    }

    pub async fn start_fix_session(
        cursor_cli: &str,
        worktree: &Path,
        error_output: &str,
        fix_instructions: &str,
        cancel_token: CancellationToken,
        timeout_secs: u64,
        line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
        model: Option<&str>,
        resume_session_id: Option<&str>,
    ) -> Result<Self> {
        info!(
            worktree = %worktree.display(),
            resume = ?resume_session_id,
            "Starting Cursor Agent fix session"
        );

        let prompt = format!(
            "The following command failed with this output:\n\n```\n{error_output}\n```\n\n{fix_instructions}\n\n\
            This is a fully automated headless run — fix the issues without asking for confirmation."
        );

        let (session_id, output) = run_cursor_agent_session(
            cursor_cli,
            worktree,
            &prompt,
            cancel_token,
            timeout_secs,
            line_tx,
            model,
            resume_session_id,
        )
        .await?;

        Ok(Self {
            session_id,
            output,
        })
    }
}

async fn run_cursor_agent_session(
    cursor_cli: &str,
    worktree: &Path,
    prompt: &str,
    cancel_token: CancellationToken,
    timeout_secs: u64,
    line_tx: Option<tokio::sync::mpsc::UnboundedSender<OutputLine>>,
    model: Option<&str>,
    resume_session_id: Option<&str>,
) -> Result<(String, String)> {
    let workspace = worktree.to_str().ok_or_else(|| {
        MaestroError::AiAgent("Worktree path is not valid UTF-8".to_string())
    })?;

    let mut owned: Vec<String> = vec![
        "-p".to_string(),
        prompt.to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--stream-partial-output".to_string(),
        "--trust".to_string(),
        "--force".to_string(),
        "--approve-mcps".to_string(),
        "--sandbox".to_string(),
        "disabled".to_string(),
        "--workspace".to_string(),
        workspace.to_string(),
    ];

    if let Some(m) = model {
        if !m.is_empty() {
            owned.push("--model".to_string());
            owned.push(m.to_string());
        }
    }

    if let Some(sid) = resume_session_id {
        owned.push("--resume".to_string());
        owned.push(sid.to_string());
    }

    let arg_refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

    info!(
        program = %cursor_cli,
        args_len = arg_refs.len(),
        worktree = %worktree.display(),
        timeout_secs = timeout_secs,
        "Spawning Cursor Agent CLI"
    );

    let handle = ProcessHandle::spawn(cursor_cli, &arg_refs, worktree, cancel_token)
        .await
        .map_err(|e| MaestroError::AiAgent(format!("Failed to spawn Cursor Agent: {e}")))?;

    let result = if let Some(tx) = line_tx {
        handle.wait_with_streaming_timeout(timeout_secs, tx).await
    } else {
        handle.wait_with_timeout(timeout_secs).await
    };

    match result {
        Ok(output) => {
            if !output.success() {
                return Err(MaestroError::AiAgent(format!(
                    "Cursor Agent exited with code {}: {}",
                    output.exit_code,
                    output.stderr.lines().take(8).collect::<Vec<_>>().join("\n")
                )));
            }

            if output.stdout.trim().is_empty() {
                return Err(MaestroError::AiAgent(
                    "Cursor Agent produced no output — check `agent login` / CURSOR_API_KEY in the environment"
                        .to_string(),
                ));
            }

            let real_session_id = extract_session_id_from_ndjson(&output.stdout)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let parsed = parse_cursor_stream_json_output(&output.stdout);
            info!(
                session_id = %real_session_id,
                session_output_len = parsed.len(),
                "Cursor Agent session completed"
            );
            Ok((real_session_id, parsed))
        }
        Err(MaestroError::Timeout(secs)) => {
            warn!(timeout_secs = secs, "Cursor Agent session timed out");
            Err(MaestroError::Timeout(secs))
        }
        Err(e) => Err(MaestroError::AiAgent(format!("Cursor Agent error: {e}"))),
    }
}

fn parse_cursor_stream_json_output(raw: &str) -> String {
    let mut result_parts = Vec::new();

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };

        let Some(event_type) = value.get("type").and_then(|v| v.as_str()) else {
            continue;
        };

        match event_type {
            "result" => {
                if let Some(r) = value.get("result").and_then(|v| v.as_str()) {
                    if !r.is_empty() {
                        result_parts.push(r.to_string());
                    }
                }
            }
            "assistant" => {
                if let Some(message) = value.get("message") {
                    if let Some(content) = message.get("content") {
                        if let Some(text) = content.as_str() {
                            result_parts.push(text.to_string());
                        } else if let Some(arr) = content.as_array() {
                            let texts: Vec<&str> = arr
                                .iter()
                                .filter_map(|item| {
                                    if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                                        item.get("text").and_then(|t| t.as_str())
                                    } else {
                                        None
                                    }
                                })
                                .collect();
                            if !texts.is_empty() {
                                result_parts.push(texts.join(""));
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if result_parts.is_empty() {
        raw.to_string()
    } else {
        result_parts.join("")
    }
}
