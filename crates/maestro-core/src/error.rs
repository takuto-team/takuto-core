// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `MaestroError` — the workspace's outermost error envelope.
//!
//! Per the typed-errors migration (audit 2026-05-21 §8 #2, completed in
//! `lore/audits/2026-05-24-typed-errors-spec.md` and the 8 per-subsystem
//! specs landed across 2026-05-24 / 2026-05-25), the envelope is now a
//! thin `#[from]`-only composition over typed sub-enums. The 8 free-form
//! `*Str(String)` deprecated shims that bridged the migration were
//! removed in the post-§8 #2 cleanup PR (2026-05-25).

use std::path::PathBuf;

use crate::actions::AgentError;
use crate::auth::AuthError;
use crate::claude::ClaudeError;
use crate::config::ConfigError;
use crate::db::DbError;
use crate::git::GitError;
use crate::github_app::GitHubAppError;
use crate::jira::JiraError;

#[derive(Debug, thiserror::Error)]
pub enum MaestroError {
    /// Typed Jira subsystem error envelope. Produced inside
    /// `crates/maestro-core/src/jira/` and the five `actions/{real,dry_run}.rs`
    /// near-duplicate `acli` invocations via `JiraError::Variant` then
    /// `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Jira(#[from] JiraError),

    /// Typed git / `gh`-CLI / bootstrap-step error envelope. Produced
    /// inside `crates/maestro-core/src/git/`,
    /// `crates/maestro-core/src/actions/{real,dry_run,gh_github}.rs`, and
    /// `workflow::engine::bootstrap` via `GitError::Variant` then
    /// `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Git(#[from] GitError),

    /// Typed AI agent (Cursor / Codex / OpenCode session + step
    /// orchestrator) error envelope. Produced inside
    /// `crates/maestro-core/src/{cursor,codex,opencode}/session.rs` and
    /// `workflow::engine::step_runner` via `AgentError::Variant` then
    /// `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Agent(#[from] AgentError),

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

    /// Typed db error envelope. Produced inside `crates/maestro-core/src/db/`
    /// and (post-cleanup) the five sites in
    /// `crates/maestro-web/src/routes/{admin,worktree_commands}.rs` via
    /// `DbError::Variant` then `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Db(#[from] DbError),

    /// Typed Claude session error envelope. Produced inside
    /// `crates/maestro-core/src/claude/` via `ClaudeError::Variant`
    /// then `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Claude(#[from] ClaudeError),

    /// Typed GitHub App authentication error envelope. Produced inside
    /// `crates/maestro-core/src/github_app{.rs,/}` via
    /// `GitHubAppError::Variant` then `?`-propagated through this `#[from]`.
    #[error(transparent)]
    GitHubApp(#[from] GitHubAppError),

    /// Typed auth subsystem error envelope. Produced inside
    /// `crates/maestro-core/src/{auth,db/users,db/credentials}.rs` and
    /// `crates/maestro-web/src/{auth.rs, routes/{auth,admin}.rs}` via
    /// `AuthError::Variant` then `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Auth(#[from] AuthError),

    /// Typed config subsystem error envelope. Produced inside
    /// `crates/maestro-core/src/config/`, `auth/{master_key,seal,bundle}.rs`,
    /// `workflow/engine/*`, and assorted operational paths via
    /// `ConfigError::Variant` then `?`-propagated through this `#[from]`.
    #[error(transparent)]
    Config(#[from] ConfigError),

    #[error("Config file not found: {0}")]
    ConfigNotFound(PathBuf),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    TomlParse(#[from] toml::de::Error),
}

/// Test-only bridge so `Database::open_in_memory`'s rusqlite anchor can
/// `?`-propagate failures into `MaestroError`. Plan-11 production code
/// paths never produce `rusqlite::Error` — they go through the sqlx
/// adapter — so this impl stays behind `#[cfg(test)]` and the crate
/// ships without rusqlite as a runtime dependency.
#[cfg(test)]
impl From<rusqlite::Error> for MaestroError {
    fn from(e: rusqlite::Error) -> Self {
        MaestroError::Db(DbError::Sqlite(e))
    }
}

/// Plan-11 step 3: `?`-propagate adapter errors from DAOs migrated to
/// the agnostic [`crate::db::DbAdapter`] API. The chain
/// `adapter::DbError → DbError::Adapter → MaestroError::Db` needs an
/// explicit `From` because Rust's `?` operator follows at most one
/// `From` conversion. This bridge keeps DAOs terse (`adapter.execute(...).await?`)
/// while preserving the existing `MaestroError::Db` envelope so logging
/// / HTTP-mapping code treats adapter errors identically to legacy
/// rusqlite errors.
impl From<crate::db::adapter::DbError> for MaestroError {
    fn from(e: crate::db::adapter::DbError) -> Self {
        MaestroError::Db(DbError::Adapter(e))
    }
}

pub type Result<T> = std::result::Result<T, MaestroError>;
