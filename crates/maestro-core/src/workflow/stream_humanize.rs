//! Turn newline-delimited JSON from AI CLIs into short lines for the dashboard.

use serde_json::Value;
use tracing::info;

use crate::config::AiAgentProvider;

pub fn extract_session_id_from_ndjson(raw: &str) -> Option<String> {
    for line in raw.lines() {
        let line = line.trim();
        if let Ok(value) = serde_json::from_str::<Value>(line) {
            if value.get("type").and_then(|v| v.as_str()) == Some("system")
                && value.get("subtype").and_then(|v| v.as_str()) == Some("init")
            {
                return value
                    .get("session_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
            }
        }
    }
    None
}

pub fn humanize_agent_stream_line(provider: AiAgentProvider, raw: &str) -> Option<String> {
    match provider {
        AiAgentProvider::Claude => humanize_claude_output(raw),
        AiAgentProvider::Cursor => humanize_cursor_output(raw),
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
            if let Some(message) = value.get("message") {
                if let Some(content) = message.get("content") {
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
            }
            None
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
            if let Some(message) = value.get("message") {
                if let Some(content) = message.get("content") {
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
            }
            None
        }
        "tool_call" => summarize_cursor_tool_event(&value),
        "result" => {
            if let Some(result) = value.get("result").and_then(|v| v.as_str()) {
                if !result.is_empty() {
                    Some(result.to_string())
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

fn summarize_cursor_tool_event(value: &Value) -> Option<String> {
    let subtype = value.get("subtype").and_then(|v| v.as_str()).unwrap_or("");
    let tc = value.get("tool_call")?;

    if subtype == "started" {
        if let Some(r) = tc.get("readToolCall") {
            let path = r
                .get("args")?
                .get("path")?
                .as_str()?;
            return Some(format!("Reading {path}"));
        }
        if let Some(w) = tc.get("writeToolCall") {
            let path = w
                .get("args")?
                .get("path")?
                .as_str()?;
            return Some(format!("Writing {path}"));
        }
        if let Some(f) = tc.get("function") {
            let name = f.get("name")?.as_str()?;
            return Some(format!("Tool: {name}"));
        }
        return Some("Tool call started".to_string());
    }

    if subtype == "completed" {
        info!("Cursor tool_call completed");
        return None;
    }

    None
}
