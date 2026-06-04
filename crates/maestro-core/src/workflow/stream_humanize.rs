// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Turn newline-delimited JSON from AI CLIs into short lines for the dashboard.

use serde_json::Value;
use tracing::info;

use crate::config::AiAgentProvider;

pub fn extract_session_id_from_ndjson(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if let Ok(value) = serde_json::from_str::<Value>(line)
            && value.get("type").and_then(|v| v.as_str()) == Some("system")
            && value.get("subtype").and_then(|v| v.as_str()) == Some("init")
        {
            return value
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }
    None
}

pub fn humanize_agent_stream_line(provider: AiAgentProvider, raw: &str) -> Option<String> {
    match provider {
        AiAgentProvider::Claude => humanize_claude_output(raw),
        AiAgentProvider::Cursor => humanize_cursor_output(raw),
        AiAgentProvider::Codex => humanize_codex_output(raw),
        AiAgentProvider::OpenCode => humanize_opencode_output(raw),
    }
}

fn humanize_claude_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if !trimmed.starts_with('{') {
        return Some(raw.to_string());
    }

    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Some(raw.to_string()),
    };

    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        "system" => {
            let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            match subtype {
                "init" => Some("Claude Code session initialized".to_string()),
                "api_retry" => {
                    let attempt = value.get("attempt").and_then(|v| v.as_u64()).unwrap_or(0);
                    let error = value
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    Some(format!(
                        "Retrying API connection (attempt {attempt}): {error}"
                    ))
                }
                _ => None,
            }
        }
        "result" => {
            if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                if !result.is_empty() {
                    Some(result.to_string())
                } else {
                    Some("Session completed.".to_string())
                }
            } else {
                Some("Session completed.".to_string())
            }
        }
        "assistant" => {
            if let Some(message) = value.get("message")
                && let Some(content) = message.get("content")
            {
                if let Some(text) = content.as_str() {
                    return Some(text.to_string());
                }
                if let Some(arr) = content.as_array() {
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
                        return Some(texts.join(""));
                    }
                }
            }
            None
        }
        "tool_use" | "tool_call" => {
            let tool_name = value.get("name").and_then(|v| v.as_str()).unwrap_or("");
            if (tool_name == "Bash" || tool_name == "bash")
                && let Some(cmd) = value
                    .get("input")
                    .and_then(|i| i.get("command"))
                    .and_then(|c| c.as_str())
            {
                info!(command = %cmd, "Agent shell command (Claude)");
                let short = if cmd.len() > 120 { &cmd[..120] } else { cmd };
                return Some(format!("$ {short}"));
            }
            if !tool_name.is_empty() {
                Some(format!("Tool: {tool_name}"))
            } else {
                Some("Tool call started".to_string())
            }
        }
        "content_block_delta" => value
            .get("delta")
            .and_then(|d| d.get("text"))
            .and_then(|t| t.as_str())
            .map(|t| t.to_string()),
        _ => None,
    }
}

fn humanize_cursor_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    if !trimmed.starts_with('{') {
        return Some(raw.to_string());
    }

    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Some(raw.to_string()),
    };

    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");

    match event_type {
        "system" => {
            let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
            if subtype == "init" {
                let model = value
                    .get("model")
                    .and_then(|v| v.as_str())
                    .unwrap_or("default");
                return Some(format!("Cursor Agent initialized ({model})"));
            }
            None
        }
        "user" => None,
        "assistant" => {
            if let Some(message) = value.get("message")
                && let Some(content) = message.get("content")
            {
                let joined = if let Some(text) = content.as_str() {
                    Some(text.to_string())
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
                    if texts.is_empty() {
                        None
                    } else {
                        Some(texts.join(""))
                    }
                } else {
                    None
                };
                if let Some(s) = joined {
                    let t = s.trim();
                    if t.is_empty() {
                        return None;
                    }
                    return Some(t.to_string());
                }
            }
            None
        }
        "tool_call" => summarize_cursor_tool_event(&value),
        "result" => {
            if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                let t = result.trim();
                if !t.is_empty() {
                    Some(t.to_string())
                } else {
                    Some("Cursor Agent session completed.".to_string())
                }
            } else {
                Some("Cursor Agent session completed.".to_string())
            }
        }
        _ => None,
    }
}

/// Humanize codex `exec --json` JSONL events into one-line dashboard text.
///
/// Codex events (see `codex/session.rs` and arch §7.1):
///   `thread.started{thread_id}`            → "Codex session initialized"
///   `turn.started`                         → suppressed (noise)
///   `item.completed{agent_message,text}`   → the assistant text
///   `item.completed{reasoning,...}`        → suppressed (chain-of-thought)
///   `item.completed{command_execution,...}`→ short command preview
///   `turn.completed`                       → "Codex session completed"
///   `turn.failed{error.message}`           → "Codex error: <msg>"
///   `error{message}`                       → "Codex error: <msg>"
///
/// Non-JSON lines (codex sometimes leaks tracing lines to stdout) pass
/// through verbatim so operators can still see them — the line filter
/// in `codex/session.rs::parse_codex_stream_json_output` skips them for
/// the final output buffer, but the live dashboard wants visibility.
fn humanize_codex_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.starts_with('{') {
        return Some(raw.to_string());
    }
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Some(raw.to_string()),
    };
    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match event_type {
        "thread.started" => Some("Codex session initialized".to_string()),
        "turn.started" => None,
        "turn.completed" => Some("Codex session completed".to_string()),
        "turn.failed" => {
            let msg = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            Some(format!("Codex error: {msg}"))
        }
        "error" => {
            let msg = value
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            Some(format!("Codex error: {msg}"))
        }
        "item.completed" => {
            let item = value.get("item")?;
            let item_type = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
            match item_type {
                "agent_message" => {
                    let text = item.get("text").and_then(|v| v.as_str())?.trim();
                    if text.is_empty() {
                        None
                    } else {
                        Some(text.to_string())
                    }
                }
                "command_execution" => {
                    let cmd = item
                        .get("command")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(command)");
                    Some(format!("$ {}", short_shell_command(cmd, 120)))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Humanize opencode `run --format json` JSONL events into dashboard text.
///
/// OpenCode events (see `opencode/session.rs` and arch §7.4):
///   `step_start{sessionID}`               → "OpenCode session initialized"
///   `text{part.text}`                     → the assistant text
///   `tool_use`                            → suppressed (tool calls noise)
///   `message.part.updated{thinking}`      → suppressed (chain-of-thought)
///   `step_finish{reason:"stop"}`          → "OpenCode session completed"
///   `step_finish{reason:"tool-calls"}`    → suppressed (intermediate)
///   `error{...}`                          → "OpenCode error: <msg>"
fn humanize_opencode_output(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if !trimmed.starts_with('{') {
        return Some(raw.to_string());
    }
    let value: Value = match serde_json::from_str(trimmed) {
        Ok(v) => v,
        Err(_) => return Some(raw.to_string()),
    };
    let event_type = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match event_type {
        "step_start" => Some("OpenCode session initialized".to_string()),
        "step_finish" => {
            let reason = value.get("reason").and_then(|v| v.as_str()).unwrap_or("");
            if reason == "stop" {
                Some("OpenCode session completed".to_string())
            } else {
                None
            }
        }
        "text" => {
            // `part.text` is the canonical location; fall back to top-level
            // `text` for legacy / partial events.
            let text = value
                .get("part")
                .and_then(|p| p.get("text"))
                .and_then(|v| v.as_str())
                .or_else(|| value.get("text").and_then(|v| v.as_str()))?
                .trim();
            if text.is_empty() {
                None
            } else {
                Some(text.to_string())
            }
        }
        "error" => {
            // opencode's error shape varies by source. Try the documented
            // fields first, then walk a few known-deeper paths emitted by
            // the OpenAI-compat adapter (LM Studio / Ollama / vLLM wrap
            // their errors as `error.data.error` and `error.cause.error`).
            let msg = value
                .get("message")
                .and_then(|v| v.as_str())
                .or_else(|| {
                    value
                        .get("error")
                        .and_then(|e| e.get("message"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| {
                    value
                        .get("error")
                        .and_then(|e| e.get("data"))
                        .and_then(|d| d.get("error"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| {
                    value
                        .get("error")
                        .and_then(|e| e.get("cause"))
                        .and_then(|c| c.get("message"))
                        .and_then(|v| v.as_str())
                })
                .or_else(|| value.get("error").and_then(|v| v.as_str()))
                .unwrap_or("unknown error");
            Some(format!("OpenCode error: {msg}"))
        }
        _ => None,
    }
}

/// Shorten a shell command for one-line dashboard display (`max` is a character count).
fn short_shell_command(cmd: &str, max_chars: usize) -> String {
    let cmd = cmd.trim();
    let count = cmd.chars().count();
    if count <= max_chars {
        cmd.to_string()
    } else {
        let prefix: String = cmd.chars().take(max_chars).collect();
        format!("{prefix}…")
    }
}

/// Prefer stderr, then stdout, then other text fields from a Cursor shell result object.
fn cursor_shell_output_pick(r: &Value) -> Option<String> {
    for key in ["stderr", "stdout", "output", "message"] {
        if let Some(s) = r.get(key).and_then(|v| v.as_str()) {
            let t = s.trim();
            if !t.is_empty() {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Collapse newlines/spaces and take the tail for dashboard overflow (`max_chars` is characters).
fn cursor_tail_for_dashboard(s: &str, max_chars: usize) -> String {
    let one_line: String = s
        .chars()
        .map(|c| if c.is_control() && c != '\t' { ' ' } else { c })
        .collect();
    let collapsed = one_line.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = collapsed.chars().count();
    if n <= max_chars {
        collapsed
    } else {
        let skip = n - max_chars;
        let tail: String = collapsed.chars().skip(skip).collect();
        format!("…{tail}")
    }
}

fn cursor_shell_completed_dashboard_line(
    outcome: &str,
    cmd: &str,
    exit_code: i64,
    detail: &Value,
) -> String {
    let short = short_shell_command(cmd, 120);
    let mut line = if outcome == "success" {
        format!("Done (exit {exit_code}): {short}")
    } else {
        format!("Failed (exit {exit_code}): {short}")
    };
    if let Some(out) = cursor_shell_output_pick(detail) {
        let tail = cursor_tail_for_dashboard(&out, 300);
        if !tail.is_empty() {
            line.push_str(" — ");
            line.push_str(&tail);
        }
    }
    line
}

fn cursor_function_args_snippet(f: &Value) -> Option<String> {
    let raw = f.get("arguments").or_else(|| f.get("args"))?;
    let s = if let Some(t) = raw.as_str() {
        t.trim().to_string()
    } else {
        serde_json::to_string(raw).ok()?.trim().to_string()
    };
    if s.is_empty() {
        return None;
    }
    const MAX: usize = 100;
    Some(if s.chars().count() > MAX {
        let prefix: String = s.chars().take(MAX).collect();
        format!("{prefix}…")
    } else {
        s
    })
}

fn summarize_cursor_tool_event(value: &Value) -> Option<String> {
    let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
    let tc = value.get("tool_call")?;

    if subtype == "started" {
        if let Some(s) = tc.get("shellToolCall") {
            let cmd = s.get("args")?.get("command")?.as_str()?;
            let desc = s.get("description").and_then(|v| v.as_str()).unwrap_or("");
            info!(command = %cmd, description = %desc, "Agent shell command");
            let short = short_shell_command(cmd, 120);
            return Some(format!("$ {short}"));
        }
        if let Some(r) = tc.get("readToolCall") {
            let path = r.get("args")?.get("path")?.as_str()?;
            return Some(format!("Reading {path}"));
        }
        if let Some(w) = tc.get("writeToolCall") {
            let path = w.get("args")?.get("path")?.as_str()?;
            return Some(format!("Writing {path}"));
        }
        if let Some(e) = tc.get("editToolCall") {
            let path = e.get("args")?.get("file_path")?.as_str()?;
            return Some(format!("Editing {path}"));
        }
        if let Some(f) = tc.get("function") {
            let name = f.get("name").and_then(|v| v.as_str()).unwrap_or("unknown");
            if let Some(snippet) = cursor_function_args_snippet(f) {
                return Some(format!("Tool: {name} ({snippet})"));
            }
            return Some(format!("Tool: {name}"));
        }
        return Some("Tool call started".to_string());
    }

    if subtype == "completed" {
        if let Some(s) = tc.get("shellToolCall")
            && let Some(result) = s.get("result")
        {
            let (key, r) = if let Some(r) = result.get("success") {
                ("success", r)
            } else if let Some(r) = result.get("failure") {
                ("failure", r)
            } else {
                return None;
            };
            let cmd = r.get("command").and_then(|v| v.as_str()).unwrap_or("");
            let exit_code = r.get("exitCode").and_then(|v| v.as_i64()).unwrap_or(-1);
            info!(command = %cmd, exit_code = exit_code, result = %key, "Agent shell command completed");
            return Some(cursor_shell_completed_dashboard_line(
                key, cmd, exit_code, r,
            ));
        }
        return None;
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AiAgentProvider;

    fn cursor_humanize(raw: &str) -> Option<String> {
        humanize_agent_stream_line(AiAgentProvider::Cursor, raw)
    }

    fn codex_humanize(raw: &str) -> Option<String> {
        humanize_agent_stream_line(AiAgentProvider::Codex, raw)
    }

    fn opencode_humanize(raw: &str) -> Option<String> {
        humanize_agent_stream_line(AiAgentProvider::OpenCode, raw)
    }

    // ── Codex humanizer ──────────────────────────────────────────────────

    #[test]
    fn codex_thread_started_yields_init_line() {
        let raw = r#"{"type":"thread.started","thread_id":"019e44e5-615f-7681-a6a3-58dd23081c10"}"#;
        assert_eq!(
            codex_humanize(raw).as_deref(),
            Some("Codex session initialized")
        );
    }

    #[test]
    fn codex_agent_message_yields_assistant_text() {
        let raw =
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello world"}}"#;
        assert_eq!(codex_humanize(raw).as_deref(), Some("hello world"));
    }

    #[test]
    fn codex_reasoning_is_suppressed() {
        let raw =
            r#"{"type":"item.completed","item":{"type":"reasoning","text":"chain of thought"}}"#;
        assert_eq!(codex_humanize(raw), None);
    }

    #[test]
    fn codex_turn_failed_surfaces_error_message() {
        let raw = r#"{"type":"turn.failed","error":{"message":"unexpected status 401"}}"#;
        let line = codex_humanize(raw).expect("must surface");
        assert!(line.starts_with("Codex error:"), "{line}");
        assert!(line.contains("401"), "{line}");
    }

    #[test]
    fn codex_non_json_passes_through() {
        // codex sometimes leaks tracing lines to stdout — keep them visible
        // on the dashboard rather than filtering silently.
        let raw = "2026-05-20T... ERROR codex_api::xyz: something failed";
        assert_eq!(codex_humanize(raw).as_deref(), Some(raw));
    }

    // ── OpenCode humanizer ───────────────────────────────────────────────

    #[test]
    fn opencode_step_start_yields_init_line() {
        let raw = r#"{"type":"step_start","sessionID":"ses_abc","part":{"type":"step-start"}}"#;
        assert_eq!(
            opencode_humanize(raw).as_deref(),
            Some("OpenCode session initialized")
        );
    }

    #[test]
    fn opencode_text_part_yields_assistant_text() {
        let raw = r#"{"type":"text","sessionID":"ses_abc","part":{"text":"hello world"}}"#;
        assert_eq!(opencode_humanize(raw).as_deref(), Some("hello world"));
    }

    #[test]
    fn opencode_tool_use_is_suppressed() {
        let raw = r#"{"type":"tool_use","sessionID":"ses_abc"}"#;
        assert_eq!(opencode_humanize(raw), None);
    }

    #[test]
    fn opencode_step_finish_stop_yields_completed_line() {
        let raw = r#"{"type":"step_finish","sessionID":"ses_abc","reason":"stop"}"#;
        assert_eq!(
            opencode_humanize(raw).as_deref(),
            Some("OpenCode session completed")
        );
    }

    #[test]
    fn opencode_step_finish_tool_calls_is_suppressed() {
        let raw = r#"{"type":"step_finish","sessionID":"ses_abc","reason":"tool-calls"}"#;
        assert_eq!(opencode_humanize(raw), None);
    }

    #[test]
    fn opencode_error_surfaces_with_prefix() {
        let raw = r#"{"type":"error","sessionID":"ses_abc","message":"no provider configured"}"#;
        let line = opencode_humanize(raw).expect("must surface");
        assert!(line.starts_with("OpenCode error:"), "{line}");
        assert!(line.contains("no provider"), "{line}");
    }

    #[test]
    fn cursor_shell_tool_started_shows_dollar_command() {
        let raw = r#"{"type":"tool_call","subtype":"started","tool_call":{"shellToolCall":{"args":{"command":"npm test"},"description":"run tests"}}}"#;
        assert_eq!(cursor_humanize(raw).as_deref(), Some("$ npm test"));
    }

    #[test]
    fn cursor_shell_completed_success_shows_done_and_exit() {
        let raw = r#"{"type":"tool_call","subtype":"completed","tool_call":{"shellToolCall":{"result":{"success":{"command":"npm test","exitCode":0}}}}}"#;
        let line = cursor_humanize(raw).expect("expected dashboard line");
        assert!(line.starts_with("Done (exit 0):"), "{line}");
        assert!(line.contains("npm test"), "{line}");
    }

    #[test]
    fn cursor_shell_completed_failure_includes_truncated_output_tail() {
        let long = "x".repeat(400);
        let raw = format!(
            r#"{{"type":"tool_call","subtype":"completed","tool_call":{{"shellToolCall":{{"result":{{"failure":{{"command":"npm run lint","exitCode":2,"stderr":"{long}"}}}}}}}}}}"#
        );
        let line = cursor_humanize(&raw).expect("expected dashboard line");
        assert!(line.starts_with("Failed (exit 2):"), "{line}");
        assert!(line.contains("npm run lint"), "{line}");
        assert!(line.contains(" — …"), "{line}");
        assert!(line.ends_with('x'), "{line}");
    }

    #[test]
    fn cursor_assistant_empty_after_trim_yields_none() {
        let raw =
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"   \n\t  "}]}}"#;
        assert_eq!(cursor_humanize(raw), None);
    }

    #[test]
    fn cursor_function_started_includes_args_snippet() {
        let raw = r#"{"type":"tool_call","subtype":"started","tool_call":{"function":{"name":"read_file","arguments":"{\"path\":\"/tmp/secret.txt\"}"}}}"#;
        let line = cursor_humanize(raw).expect("expected line");
        assert!(line.starts_with("Tool: read_file ("), "unexpected: {line}");
        assert_ne!(line.as_str(), "Tool call started");
        assert!(line.contains("path"), "{line}");
    }
}
