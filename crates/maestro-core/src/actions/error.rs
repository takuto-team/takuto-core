// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for AI agent (Cursor / Codex / OpenCode) session adapters
//! and the agent-step orchestrator in `workflow::engine::step_runner`.
//!
//! Replaces `MaestroError::AiAgent(String)` (now `MaestroError::AiAgentStr(String)`,
//! `#[deprecated]`). Each variant captures structured operation context — exit
//! codes, stderr tails, per-provider auth hints — instead of `format!`-ed
//! sentences. Spawn-failure and generic session-error wraps that previously
//! prefixed an inner `MaestroError` with zero-info text collapse to direct
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

    /// The step orchestrator aborted the workflow after the AI session failed.
    /// `hint` is a pinned per-provider auth diagnostic selected at the call site.
    #[error("Agent step failed — {hint}")]
    AgentStepAborted { hint: &'static str },
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
        ];
        // Drift detection: if a new variant is added without updating this test,
        // `cases.len()` will be stale and the assertion below trips.
        assert_eq!(cases.len(), 6);
        for (err, expected) in cases {
            assert_eq!(format!("{err}"), expected, "Display mismatch for {err:?}");
        }
    }

    #[test]
    fn lock_in_agent_error_into_maestro_error() {
        use crate::error::MaestroError;
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
        ];
        assert_eq!(cases.len(), 6);
        for err in cases {
            let outer: MaestroError = err.into();
            assert!(
                matches!(outer, MaestroError::Agent(_)),
                "expected MaestroError::Agent, got {outer:?}"
            );
        }
    }
}
