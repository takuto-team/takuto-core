// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::actions::AgentError;
use crate::config::AiAgentProvider;
use crate::container::ContainerRunner;
use crate::error::{Result, TakutoError};
use crate::process::{OutputLine, ProcessHandle};
use crate::workflow::stream_humanize::extract_session_id_from_ndjson;

pub struct CursorSession {
    pub session_id: String,
    pub output: String,
}

/// True when a cursor-agent stream-json line is the terminal `result` event
/// (any subtype). cursor emits exactly one per turn as the last line, so this
/// is a safe early-completion signal for the streaming wait.
fn is_cursor_result_event(line: &str) -> bool {
    let line = line.trim_start();
    if !line.starts_with('{') {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|v| {
            v.get("type")
                .and_then(|t| t.as_str())
                .map(|t| t == "result")
        })
        .unwrap_or(false)
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
        idle_nudge_secs: u64,
        // Per-process env for the non-container (main-container) path — e.g. a
        // per-user `CURSOR_API_KEY` for "Improve with AI". Empty in worker-container
        // runs (credentials come via the bundle).
        extra_env: &[(&str, &str)],
    ) -> Result<Self> {
        // === DEV-MODE MOCK ===
        // Off by default. Enabled by [dev] mock_agent = true OR TAKUTO_DEV_MOCK_AGENT=1
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
            idle_nudge_secs,
            extra_env,
        )
        .await?;

        Ok(Self { session_id, output })
    }
}

/// Max idle-recovery nudges per step before giving up to the wall-clock cap.
const CURSOR_MAX_NUDGES: u32 = 2;

/// Resume prompt sent when a session goes idle (see `session_idle_nudge_secs`).
const CURSOR_NUDGE_PROMPT: &str = "There has been no update for several minutes. \
What is the current status? If all work for this step is already complete, briefly \
confirm what you did and finish. If something is stalled or waiting on input, \
resolve it or stop.";

/// Build the cursor-agent argv for a turn.
fn build_cursor_args(
    prompt: &str,
    workspace: &str,
    model: Option<&str>,
    resume: Option<&str>,
) -> Vec<String> {
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
    if let Some(sid) = resume {
        owned.push("--resume".to_string());
        owned.push(sid.to_string());
    }
    owned
}

/// Track cursor stream-json state needed by the idle nudge: tool-call depth
/// (so we never nudge mid-tool) and the session id (so we can `--resume`).
fn cursor_track_line(content: &str, tool_depth: &mut i32, seen_session: &mut Option<String>) {
    let c = content.trim_start();
    if !c.starts_with('{') {
        return;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(c) else {
        return;
    };
    if seen_session.is_none()
        && let Some(s) = v.get("session_id").and_then(|x| x.as_str())
    {
        *seen_session = Some(s.to_string());
    }
    if v.get("type").and_then(|t| t.as_str()) == Some("tool_call") {
        match v.get("subtype").and_then(|s| s.as_str()) {
            Some("started") => *tool_depth += 1,
            Some("completed") => *tool_depth = (*tool_depth - 1).max(0),
            _ => {}
        }
    }
}

fn finalize_cursor_output(output: crate::process::CommandOutput) -> Result<(String, String)> {
    if !output.success() {
        return Err(AgentError::NonZeroExit {
            provider: AiAgentProvider::Cursor,
            exit_code: output.exit_code,
            stderr_tail: output.stderr.lines().take(8).collect::<Vec<_>>().join("\n"),
        }
        .into());
    }
    if output.stdout.trim().is_empty() {
        return Err(AgentError::EmptyOutput {
            provider: AiAgentProvider::Cursor,
            hint: "check `agent login` / CURSOR_API_KEY in the environment",
        }
        .into());
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
    idle_nudge_secs: u64,
    extra_env: &[(&str, &str)],
) -> Result<(String, String)> {
    let workspace = worktree
        .to_str()
        .ok_or(AgentError::WorktreePathInvalidUtf8 {
            provider: AiAgentProvider::Cursor,
        })?;

    // `timeout_secs` is the hard wall-clock cap across ALL attempts (the
    // original turn plus any idle-recovery resumes) — idle nudges never extend
    // it.
    let started = tokio::time::Instant::now();
    let overall = std::time::Duration::from_secs(timeout_secs);

    let mut cur_prompt = prompt.to_string();
    let mut cur_resume: Option<String> = resume_session_id.map(|s| s.to_string());
    let mut nudges_used: u32 = 0;

    loop {
        let remaining_secs = overall
            .checked_sub(started.elapsed())
            .unwrap_or(std::time::Duration::ZERO)
            .as_secs();
        if remaining_secs == 0 {
            return Err(TakutoError::Timeout(timeout_secs));
        }

        let owned = build_cursor_args(&cur_prompt, workspace, model, cur_resume.as_deref());
        let arg_refs: Vec<&str> = owned.iter().map(|s| s.as_str()).collect();

        info!(
            program = %cursor_cli,
            args_len = arg_refs.len(),
            worktree = %worktree.display(),
            remaining_secs = remaining_secs,
            nudges_used = nudges_used,
            container = container_runner.is_some(),
            "Spawning Cursor Agent CLI"
        );

        // Per-attempt token: a child of the caller's token so user pause/stop
        // still propagates, while an idle nudge can cancel just this attempt.
        let attempt_token = cancel_token.child_token();
        let handle = if let Some(runner) = container_runner {
            let (prog, docker_args) = runner.wrap_command(cursor_cli, &arg_refs);
            let docker_arg_refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
            ProcessHandle::spawn(&prog, &docker_arg_refs, worktree, attempt_token.clone()).await
        } else {
            ProcessHandle::spawn_with_env(
                cursor_cli,
                &arg_refs,
                worktree,
                attempt_token.clone(),
                extra_env,
            )
            .await
        }?;

        // No streaming channel → no line observation → no idle nudge possible.
        let Some(real_tx) = line_tx.as_ref() else {
            return match handle.wait_with_timeout(remaining_secs).await {
                Ok(output) => finalize_cursor_output(output),
                Err(e) => Err(e),
            };
        };

        // Interpose an inner channel so we can watch idle + tool state while the
        // wait owns the read loop. The wait finishes on the terminal `result`
        // event (B1) or the wall-clock cap.
        let nudging = idle_nudge_secs > 0 && nudges_used < CURSOR_MAX_NUDGES;
        let (inner_tx, mut inner_rx) = tokio::sync::mpsc::unbounded_channel::<OutputLine>();

        let waiter = handle.wait_with_streaming_timeout_until(
            remaining_secs,
            inner_tx,
            is_cursor_result_event,
        );

        let forward_tx = real_tx.clone();
        let sup_token = attempt_token.clone();
        let idle_dur = std::time::Duration::from_secs(idle_nudge_secs.max(1));
        let supervisor = async move {
            let mut tool_depth: i32 = 0;
            let mut idle_fired = false;
            let mut seen_session: Option<String> = None;
            loop {
                let line = if nudging {
                    match tokio::time::timeout(idle_dur, inner_rx.recv()).await {
                        Ok(Some(l)) => l,
                        Ok(None) => break,
                        Err(_) => {
                            // Idle elapsed. Only nudge when no tool is running —
                            // long tools legitimately stream nothing and have
                            // their own timeouts.
                            if tool_depth == 0 {
                                idle_fired = true;
                                sup_token.cancel();
                                break;
                            }
                            continue;
                        }
                    }
                } else {
                    match inner_rx.recv().await {
                        Some(l) => l,
                        None => break,
                    }
                };
                if line.stream == "stdout" {
                    cursor_track_line(&line.content, &mut tool_depth, &mut seen_session);
                }
                let _ = forward_tx.send(line);
            }
            (idle_fired, seen_session)
        };

        let (result, (idle_fired, seen_session)) = tokio::join!(waiter, supervisor);

        match result {
            Ok(output) => return finalize_cursor_output(output),
            Err(TakutoError::Cancelled) => {
                // A user pause/stop (parent token) takes precedence over a nudge.
                if cancel_token.is_cancelled() {
                    return Err(TakutoError::Cancelled);
                }
                if idle_fired
                    && nudges_used < CURSOR_MAX_NUDGES
                    && let Some(sid) = seen_session.or_else(|| cur_resume.clone())
                {
                    nudges_used += 1;
                    warn!(
                        session_id = %sid,
                        nudges_used = nudges_used,
                        idle_secs = idle_nudge_secs,
                        "Cursor session idle with no tool in flight — resuming with status nudge"
                    );
                    cur_prompt = CURSOR_NUDGE_PROMPT.to_string();
                    cur_resume = Some(sid);
                    continue;
                }
                Err(TakutoError::Cancelled)
            }
            Err(TakutoError::Timeout(secs)) => {
                warn!(timeout_secs = secs, "Cursor Agent session timed out");
                Err(TakutoError::Timeout(secs))
            }
            Err(e) => Err(e),
        }?;
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

    #[test]
    fn cursor_args_contain_required_flags() {
        let args = build_cursor_args("do work", "/workspace/project", None, None);
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
        let args = build_cursor_args("prompt", "/ws", None, Some("cur-session-1"));
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"cur-session-1".to_string()));
    }

    #[test]
    fn cursor_args_no_resume_when_session_id_absent() {
        let args = build_cursor_args("prompt", "/ws", None, None);
        assert!(!args.contains(&"--resume".to_string()));
    }

    #[test]
    fn cursor_args_include_model_when_non_empty() {
        let args = build_cursor_args("prompt", "/ws", Some("gpt-4.1"), None);
        assert!(args.contains(&"--model".to_string()));
        assert!(args.contains(&"gpt-4.1".to_string()));
    }

    #[test]
    fn cursor_args_skip_model_when_empty() {
        let args = build_cursor_args("prompt", "/ws", Some(""), None);
        assert!(!args.contains(&"--model".to_string()));
    }

    // --- idle-nudge helpers (B2) ---

    #[test]
    fn result_event_detected_any_subtype() {
        assert!(is_cursor_result_event(
            r#"{"type":"result","subtype":"success","is_error":false}"#
        ));
        assert!(is_cursor_result_event(r#"  {"type":"result"}  "#));
        assert!(!is_cursor_result_event(
            r#"{"type":"assistant","message":{"content":"x"}}"#
        ));
        assert!(!is_cursor_result_event(
            r#"{"type":"tool_call","subtype":"started"}"#
        ));
        assert!(!is_cursor_result_event("not json"));
    }

    #[test]
    fn track_line_counts_tool_depth_and_captures_session() {
        let mut depth = 0i32;
        let mut seen: Option<String> = None;

        cursor_track_line(
            r#"{"type":"system","subtype":"init","session_id":"sess-42"}"#,
            &mut depth,
            &mut seen,
        );
        assert_eq!(seen.as_deref(), Some("sess-42"));
        assert_eq!(depth, 0);

        cursor_track_line(
            r#"{"type":"tool_call","subtype":"started"}"#,
            &mut depth,
            &mut seen,
        );
        cursor_track_line(
            r#"{"type":"tool_call","subtype":"started"}"#,
            &mut depth,
            &mut seen,
        );
        assert_eq!(depth, 2, "two tools in flight");
        cursor_track_line(
            r#"{"type":"tool_call","subtype":"completed"}"#,
            &mut depth,
            &mut seen,
        );
        assert_eq!(depth, 1, "one still running — must not nudge");
        cursor_track_line(
            r#"{"type":"tool_call","subtype":"completed"}"#,
            &mut depth,
            &mut seen,
        );
        assert_eq!(depth, 0, "all tools done — idle nudge now allowed");

        // Never goes negative, and the first session id wins.
        cursor_track_line(
            r#"{"type":"tool_call","subtype":"completed"}"#,
            &mut depth,
            &mut seen,
        );
        assert_eq!(depth, 0);
        cursor_track_line(
            r#"{"type":"assistant","session_id":"other"}"#,
            &mut depth,
            &mut seen,
        );
        assert_eq!(seen.as_deref(), Some("sess-42"));
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
