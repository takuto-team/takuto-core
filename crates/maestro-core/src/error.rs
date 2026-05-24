// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;

use crate::db::DbError;

#[derive(Debug, thiserror::Error)]
pub enum MaestroError {
    #[error("Jira error: {0}")]
    Jira(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("GitHub App error: {0}")]
    GitHubApp(String),

    #[error("Claude session error: {0}")]
    Claude(String),

    #[error("AI agent error: {0}")]
    AiAgent(String),

    #[error("Command failed: {cmd} (exit code {code})\n{stderr}")]
    Command {
        cmd: String,
        code: i32,
        stderr: String,
    },

    #[error("Timeout after {0}s")]
    Timeout(u64),

    #[error("Workflow cancelled")]
    Cancelled,

    /// Typed db error envelope. New code path — produced inside
    /// `crates/maestro-core/src/db/` via `DbError::Variant` then
    /// `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Db(#[from] DbError),

    /// Deprecated free-form String shim for non-db callers. Retained so that
    /// callers outside `crates/maestro-core/src/db/` (admin / worktree_commands
    /// routes today) keep compiling while the typed-errors migration progresses.
    /// Will be removed by the cleanup PR after the AuthError + ConfigError
    /// phases land.
    #[deprecated(note = "use MaestroError::Db with a typed DbError instead")]
    #[error("Database error: {0}")]
    DatabaseStr(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Config error: {0}")]
    Config(String),

    #[error("Config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),
}

impl From<rusqlite::Error> for MaestroError {
    fn from(e: rusqlite::Error) -> Self {
        MaestroError::Db(DbError::Sqlite(e))
    }
}

pub type Result<T> = std::result::Result<T, MaestroError>;
