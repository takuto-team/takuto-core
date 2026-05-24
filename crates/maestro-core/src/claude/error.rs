// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the Claude Code session subsystem.
//!
//! Sub-enum that captures every distinct failure mode produced inside
//! `crates/maestro-core/src/claude/`. Lifted from `MaestroError::Claude(String)`
//! per the 2026-05-24 typed-errors-claude-spec — every variant cites the
//! `claude/session.rs` call site it replaces so the migration commits can be
//! traced back.
//!
//! Wired into the workspace error envelope via
//! `MaestroError::Claude(#[from] ClaudeError)` so existing `?` propagation
//! across `Result<T, MaestroError>` boundaries keeps working unchanged.

/// Failures originating inside the Claude Code session subsystem. Public for
/// matching, but callers should generally just `?`-propagate into a
/// `MaestroError`.
#[derive(Debug, thiserror::Error)]
pub enum ClaudeError {
    /// `claude/session.rs:208` — the `claude` process exited non-zero.
    /// `detail` is the parsed stream-json result OR a 5-line stderr snippet
    /// OR the literal `"(no output)"` — operator diagnostic, not a sentence.
    #[error("Claude Code exited with code {exit_code}: {detail}")]
    NonZeroExit { exit_code: i32, detail: String },

    /// `claude/session.rs:215` — the `claude` process exited zero but stdout
    /// was empty, which in practice means Claude is unauthenticated inside
    /// the container.
    #[error(
        "Claude Code session produced no output — check that Claude is authenticated in the container"
    )]
    EmptyOutput,
}

#[cfg(test)]
mod tests {
    //! Lock-in tests for the typed Claude-error surface.
    //!
    //! These tests pin two contracts against future drift:
    //!   1. The `Display` rendering of every `ClaudeError` variant — the
    //!      messages flow into log lines and (via `MaestroError`) HTTP error
    //!      bodies, so a silent reword would be observable to operators.
    //!   2. The `#[from] ClaudeError` chain into `MaestroError::Claude(..)` —
    //!      every `?`-propagation inside `crates/maestro-core/src/claude/`
    //!      relies on this exact path; if a refactor accidentally wraps via
    //!      a different variant (e.g. the deprecated `ClaudeStr` shim) these
    //!      tests fail.
    use super::*;
    use crate::error::MaestroError;

    #[test]
    fn lock_in_claude_error_display() {
        let non_zero = ClaudeError::NonZeroExit {
            exit_code: 137,
            detail: "killed by OOM".to_string(),
        };
        assert_eq!(
            format!("{}", non_zero),
            "Claude Code exited with code 137: killed by OOM"
        );

        let empty = ClaudeError::EmptyOutput;
        assert_eq!(
            format!("{}", empty),
            "Claude Code session produced no output — check that Claude is authenticated in the container"
        );
    }

    #[test]
    fn lock_in_claude_error_into_maestro_error() {
        let non_zero = ClaudeError::NonZeroExit {
            exit_code: 2,
            detail: "(no output)".to_string(),
        };
        let wrapped: MaestroError = non_zero.into();
        assert!(matches!(
            wrapped,
            MaestroError::Claude(ClaudeError::NonZeroExit { .. })
        ));

        let empty = ClaudeError::EmptyOutput;
        let wrapped: MaestroError = empty.into();
        assert!(matches!(
            wrapped,
            MaestroError::Claude(ClaudeError::EmptyOutput)
        ));
    }
}
