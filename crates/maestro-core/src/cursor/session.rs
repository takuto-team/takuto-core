// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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
    // Agent session parameters are inherently numerous.
    #[allow(clippy::too_many_arguments)]
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
        // === DEV-MODE MOCK ===
        // Off by default. Enabled by [dev] mock_agent = true OR MAESTRO_DEV_MOCK_AGENT=1
        // OR dev_mock::set_test_override(Some(true)).
        if crate::dev_mock::is_enabled_from_runtime() {
            return crate::dev_mock::run_cursor_mock(
                worktree,
                prompt,
                cancel_token,
                line_tx,
                resume_session_id,
                None, // Cursor's run_prompt has no system_prompt param; the improve path
                      // doesn't hit Cursor today, but mock is consistent either way.
            )
            .await
            .map(|(session_id, output)| Self { session_id, output });
        }
        // === /DEV-MODE MOCK ===

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

        Ok(Self { session_id, output })
    }
}

// Agent session parameters are inherently numerous.
#[allow(clippy::too_many_arguments)]
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
    let workspace = worktree
        .to_str()
        .ok_or_else(|| MaestroError::AiAgent("Worktree path is not valid UTF-8".to_string()))?;

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

    if let Some(m) = model
        && !m.is_empty()
    {
        owned.push("--model".to_string());
        owned.push(m.to_string());
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
                if let Some(r) = value.get("result").and_then(|v| v.as_str())
                    && !r.is_empty()
                {
                    result_parts.push(r.to_string());
                }
            }
            "assistant" => {
                if let Some(message) = value.get("message")
                    && let Some(content) = message.get("content")
                {
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
            _ => {}
        }
    }

    if result_parts.is_empty() {
        raw.to_string()
    } else {
        result_parts.join("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reproduce the argument-building logic from `run_cursor_agent_session` to verify flags
    /// without spawning a real process.
    fn build_cursor_args(
        workspace: &str,
        prompt: &str,
        model: Option<&str>,
        resume_session_id: Option<&str>,
    ) -> Vec<String> {
        let mut args = vec![
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

        if let Some(m) = model
            && !m.is_empty()
        {
            args.push("--model".to_string());
            args.push(m.to_string());
        }

        if let Some(sid) = resume_session_id {
            args.push("--resume".to_string());
            args.push(sid.to_string());
        }

        args
    }

    #[test]
    fn cursor_args_contain_required_flags() {
        let args = build_cursor_args("/workspace/project", "do work", None, None);
        assert!(args.contains(&"-p".to_string()));
        assert!(args.contains(&"--output-format".to_string()));
        assert!(args.contains(&"stream-json".to_string()));
        assert!(args.contains(&"--stream-partial-output".to_string()));
        assert!(args.contains(&"--trust".to_string()));
        assert!(args.contains(&"--force".to_string()));
        assert!(args.contains(&"--approve-mcps".to_string()));
        assert!(args.contains(&"--sandbox".to_string()));
        assert!(args.contains(&"disabled".to_string()));
        assert!(args.contains(&"--workspace".to_string()));
        assert!(args.contains(&"/workspace/project".to_string()));
    }

    #[test]
    fn cursor_args_include_resume_when_session_id_present() {
        let args = build_cursor_args("/ws", "prompt", None, Some("cur-session-1"));
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"cur-session-1".to_string()));
    }

    #[test]
    fn cursor_args_no_resume_when_session_id_absent() {
        let args = build_cursor_args("/ws", "prompt", None, None);
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn cursor_args_include_model_when_non_empty() {
        let args = build_cursor_args("/ws", "prompt", Some("gpt-4.1"), None);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-4.1".to_string()));
    }

    #[test]
    fn cursor_args_skip_model_when_empty() {
        let args = build_cursor_args("/ws", "prompt", Some(""), None);
        assert!(!args.contains(&"--model".to_string()));
    }

    // --- parse_cursor_stream_json_output ---

    #[test]
    fn parse_cursor_stream_extracts_result() {
        let raw = r#"{"type":"result","result":"Done!"}"#;
        let parsed = parse_cursor_stream_json_output(raw);
        assert_eq!(parsed, "Done!");
    }

    #[test]
    fn parse_cursor_stream_extracts_assistant_string_content() {
        let raw = r#"{"type":"assistant","message":{"content":"assistant text"}}"#;
        let parsed = parse_cursor_stream_json_output(raw);
        assert_eq!(parsed, "assistant text");
    }

    #[test]
    fn parse_cursor_stream_extracts_assistant_array_content() {
        let raw = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"part1"},{"type":"text","text":"part2"}]}}"#;
        let parsed = parse_cursor_stream_json_output(raw);
        assert_eq!(parsed, "part1part2");
    }

    #[test]
    fn parse_cursor_stream_skips_non_text_array_items() {
        let raw = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"123"},{"type":"text","text":"only this"}]}}"#;
        let parsed = parse_cursor_stream_json_output(raw);
        assert_eq!(parsed, "only this");
    }

    #[test]
    fn parse_cursor_stream_returns_raw_when_no_events() {
        let raw = "plain output with no json";
        let parsed = parse_cursor_stream_json_output(raw);
        assert_eq!(parsed, raw);
    }

    #[test]
    fn parse_cursor_stream_skips_empty_result() {
        let raw = r#"{"type":"result","result":""}"#;
        let parsed = parse_cursor_stream_json_output(raw);
        // Empty result is skipped, so we get the raw back
        assert_eq!(parsed, raw);
    }
}
