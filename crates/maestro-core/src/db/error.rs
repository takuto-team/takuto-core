// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed errors for the SQLite-backed persistence layer.
//!
//! Sub-enum that captures every failure mode produced inside
//! `crates/maestro-core/src/db/`. Lifted from `MaestroError::Database(String)`
//! per the 2026-05-24 typed-errors-spec (Part B) — every variant cites the
//! call site it replaces so the migration commits can be traced back.
//!
//! Wired into the workspace error envelope via
//! `MaestroError::Db(#[from] DbError)` so existing `?` propagation across
//! `Result<T, MaestroError>` boundaries keeps working unchanged.

use std::path::PathBuf;

/// Failures originating inside the db subsystem. Public for matching, but
/// callers should generally just `?`-propagate into a `MaestroError`.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// Every `?`-propagated `rusqlite::Error` plus the `db/users.rs:50`
    /// fallthrough arm of the username-UNIQUE branch.
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    /// `db/schema.rs:375` — version mismatch after `run_migrations`.
    #[error("schema migration failed: expected version {expected}, got {actual}")]
    Migrations { expected: i32, actual: i32 },

    /// `db/mod.rs:89` — `create_dir_all` failed when opening the database.
    #[error("failed to create data directory {path}")]
    DataDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `db/user_worktree_commands.rs:125/131/138` — application-layer NUL-byte
    /// guardrail. `field` ∈ {`"user_id_or_workspace_name"`, `"init_command"`,
    /// `"run_command_name_or_command"`}.
    #[error("{field} contains a NUL byte")]
    NulByte { field: &'static str },

    /// `db/user_worktree_commands.rs:145/147` — `serde_json::to_string` failed.
    /// `column` ∈ {`"init_commands_json"`, `"run_commands_json"`}.
    #[error("encoding {column} failed")]
    CommandsJsonEncode {
        column: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// `db/user_worktree_commands.rs:258/263` — `serde_json::from_str` failed.
    /// `column` ∈ {`"init_commands_json"`, `"run_commands_json"`}.
    #[error("decoding {column} for ({user_id},{workspace_name}) failed")]
    CommandsJsonDecode {
        column: &'static str,
        user_id: String,
        workspace_name: String,
        #[source]
        source: serde_json::Error,
    },
}
