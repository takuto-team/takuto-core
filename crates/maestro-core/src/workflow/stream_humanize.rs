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
        // Phase 1: Codex and OpenCode have no adapter wired yet (Phase 4).
        // The driver refuses to spawn sessions against them, so we never
        // reach this code path at runtime. Falling back to raw passthrough
        // keeps the humanizer total in case the stream is fed offline.
        AiAgentProvider::Codex | AiAgentProvider::OpenCode => {
            if raw.trim().is_empty() {
                None
            } else {
                Some(raw.to_string())
            }
        }
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
