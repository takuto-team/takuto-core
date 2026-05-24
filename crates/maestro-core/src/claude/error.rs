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
