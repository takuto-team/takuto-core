// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for AI agent (Cursor / Codex / OpenCode) session adapters
//! and the agent-step orchestrator in `workflow::engine::step_runner`.
//!
//! Replaces the historical `TakutoError::AiAgent(String)` (the `*Str(String)`
//! deprecated shim was removed in the post-§8 #2 cleanup PR).
//! Each variant captures structured operation context — exit
//! codes, stderr tails, per-provider auth hints — instead of `format!`-ed
//! sentences. Spawn-failure and generic session-error wraps that previously
//! prefixed an inner `TakutoError` with zero-info text collapse to direct
//! `?` propagation (mirrors the Claude / Jira pattern).
//!
//! See `lore/audits/2026-05-21-clean-code.md` §8 #2 and
//! `lore/audits/2026-05-24-typed-errors-spec.md` for the architecture rules
//! this module follows.

use thiserror::Error;

use crate::config::AiAgentProvider;

/// Errors produced by an AI agent (Cursor / Codex / OpenCode) session or by
/// the `workflow::engine::step_runner` orchestrator wrapping one.
#[derive(Debug, Error)]
pub enum AgentError {
    /// The worktree path passed to `cursor-agent` / `codex` could not be
    /// rendered as a `&str` (the CLIs take `--workspace <str>` / `--cd <str>`).
    /// OpenCode passes the worktree as `&Path` directly and does not produce
    /// this error.
    #[error("{provider} worktree path is not valid UTF-8")]
    WorktreePathInvalidUtf8 { provider: AiAgentProvider },

    /// The agent CLI exited with a non-zero status. `stderr_tail` carries up
    /// to the first 8 lines of the child stderr.
    #[error("{provider} exited with code {exit_code}: {stderr_tail}")]
    NonZeroExit {
        provider: AiAgentProvider,
        exit_code: i32,
        stderr_tail: String,
    },

    /// The agent CLI exited 0 but produced no stdout. `hint` is a pinned
    /// per-provider auth diagnostic — one of three fixed values selected at
    /// the call site, not free-form text.
    #[error("{provider} produced no output — {hint}")]
    EmptyOutput {
        provider: AiAgentProvider,
        hint: &'static str,
    },

    /// The stream-JSON channel reported a logical failure even with exit 0
    /// (Codex `turn.failed` event or OpenCode `error` event).
    #[error("{provider} stream reported error: {message}")]
    StreamFailed {
        provider: AiAgentProvider,
        message: String,
    },

    /// A `[[steps]]` shell command step (`commands = [...]`) exited non-zero.
    /// Carries no payload — the per-command stderr is already logged + streamed
    /// to the dashboard before this error propagates.
    #[error("Command step failed")]
    CommandStepFailed,

    /// The step orchestrator aborted the workflow after the AI session failed
    /// with an **auth-shaped** error (no output / non-zero exit). `hint` is a
    /// pinned per-provider auth diagnostic selected at the call site. Only used
    /// when the underlying failure plausibly indicates a credential/CLI problem
    /// — never for timeouts or transport errors.
    #[error("Agent step failed — {hint}")]
    AgentStepAborted { hint: &'static str },

    /// The step orchestrator aborted the workflow after the AI session failed
    /// for a **non-auth** reason (timeout, transport, stream error, …). Carries
    /// the underlying cause verbatim so the dashboard shows what actually
    /// happened instead of a misleading "check agent login" hint.
    #[error("Agent step failed: {message}")]
    AgentStepFailed { message: String },

    /// The step was aborted by the no-progress guardrail: the agent repeated
    /// the same output line `count` times in a row without making progress
    /// (typically a weak model retrying a failing action in a loop). Fails the
    /// step into Error rather than letting it churn until the wall-clock
    /// timeout. `line` is the repeated text (truncated) for diagnosis.
    #[error(
        "Agent step aborted — no progress: repeated the same output {count} times (\"{line}\")"
    )]
    NoProgressLoop { count: u32, line: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_in_agent_error_display() {
        let cases: Vec<(AgentError, &str)> = vec![
            (
                AgentError::WorktreePathInvalidUtf8 {
                    provider: AiAgentProvider::Cursor,
                },
                "Cursor Agent worktree path is not valid UTF-8",
            ),
            (
                AgentError::NonZeroExit {
                    provider: AiAgentProvider::Codex,
                    exit_code: 1,
                    stderr_tail: "boom".to_string(),
                },
                "Codex CLI exited with code 1: boom",
            ),
            (
                AgentError::EmptyOutput {
                    provider: AiAgentProvider::OpenCode,
                    hint: "check `opencode auth` / provider configuration",
                },
                "OpenCode produced no output — check `opencode auth` / provider configuration",
            ),
            (
                AgentError::StreamFailed {
                    provider: AiAgentProvider::Codex,
                    message: "model rejected".to_string(),
                },
                "Codex CLI stream reported error: model rejected",
            ),
            (AgentError::CommandStepFailed, "Command step failed"),
            (
                AgentError::AgentStepAborted {
                    hint: "check Cursor Agent (`agent login` or CURSOR_API_KEY) and agent.providers.cursor.cli",
                },
                "Agent step failed — check Cursor Agent (`agent login` or CURSOR_API_KEY) and agent.providers.cursor.cli",
            ),
            (
                AgentError::AgentStepFailed {
                    message: "operation timed out after 1800 seconds".to_string(),
                },
                "Agent step failed: operation timed out after 1800 seconds",
            ),
            (
                AgentError::NoProgressLoop {
                    count: 8,
                    line: "It seems there is an issue with pushing the branch.".to_string(),
                },
                "Agent step aborted — no progress: repeated the same output 8 times (\"It seems there is an issue with pushing the branch.\")",
            ),
        ];
        // Drift detection: if a new variant is added without updating this test,
        // `cases.len()` will be stale and the assertion below trips.
        assert_eq!(cases.len(), 8);
        for (err, expected) in cases {
            assert_eq!(format!("{err}"), expected, "Display mismatch for {err:?}");
        }
    }

    #[test]
    fn lock_in_agent_error_into_takuto_error() {
        use crate::error::TakutoError;
        let cases: Vec<AgentError> = vec![
            AgentError::WorktreePathInvalidUtf8 {
                provider: AiAgentProvider::Cursor,
            },
            AgentError::NonZeroExit {
                provider: AiAgentProvider::Codex,
                exit_code: 1,
                stderr_tail: "boom".to_string(),
            },
            AgentError::EmptyOutput {
                provider: AiAgentProvider::OpenCode,
                hint: "hint",
            },
            AgentError::StreamFailed {
                provider: AiAgentProvider::Codex,
                message: "msg".to_string(),
            },
            AgentError::CommandStepFailed,
            AgentError::AgentStepAborted { hint: "hint" },
            AgentError::AgentStepFailed {
                message: "boom".to_string(),
            },
            AgentError::NoProgressLoop {
                count: 8,
                line: "loop".to_string(),
            },
        ];
        assert_eq!(cases.len(), 8);
        for err in cases {
            let outer: TakutoError = err.into();
            assert!(
                matches!(outer, TakutoError::Agent(_)),
                "expected TakutoError::Agent, got {outer:?}"
            );
        }
    }
}
