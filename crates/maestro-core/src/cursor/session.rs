use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::container::ContainerRunner;
use crate::error::{MaestroError, Result};
use crate::process::{OutputLine, ProcessHandle};
use crate::workflow::stream_humanize::extract_session_id_from_ndjson;

pub struct CursorSession {
    pub session_id: String,
    pub output: String,
}

impl CursorSession {
    /// Run Cursor Agent with the given full prompt (interpolated user text + headless suffix).
    pub async fn run_prompt(
        cursor_cli: &str,
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
            "Starting Cursor Agent session"
        );

        let (session_id, output) = run_cursor_agent_session(
            cursor_cli,
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
    container_runner: Option<&ContainerRunner>,
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
        container = container_runner.is_some(),
        "Spawning Cursor Agent CLI"
    );

    let handle = if let Some(runner) = container_runner {
        let (prog, docker_args) = runner.wrap_command(cursor_cli, &arg_refs);
        let docker_arg_refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
        ProcessHandle::spawn(&prog, &docker_arg_refs, worktree, cancel_token).await
    } else {
        ProcessHandle::spawn(cursor_cli, &arg_refs, worktree, cancel_token).await
    }
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
