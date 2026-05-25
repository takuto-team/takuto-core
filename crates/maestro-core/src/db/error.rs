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

    /// `routes/worktree_commands.rs:449` — `get()` returned `None` immediately
    /// after `upsert()` succeeded. Race-condition / corruption guard mirroring
    /// `AuthError::UserDisappearedAfterUpdate`.
    #[error("row was just upserted but vanished")]
    RowDisappearedAfterUpsert,
}

#[cfg(test)]
mod tests {
    //! Lock-in tests for the typed db-error surface.
    //!
    //! These tests pin two contracts against future drift:
    //!   1. The `Display` rendering of every `DbError` variant — the messages
    //!      flow into log lines and (via `MaestroError`) HTTP error bodies,
    //!      so a silent reword would be observable to operators.
    //!   2. The `#[from] DbError` chain into `MaestroError::Db(..)` — every
    //!      `?`-propagation inside `crates/maestro-core/src/db/` relies on
    //!      this exact path; if a refactor accidentally wraps via a different
    //!      variant (e.g. the deprecated `DatabaseStr` shim) these tests fail.
    use super::*;
    use crate::error::MaestroError;
    use std::io;
    use std::path::PathBuf;

    fn sample_io_err() -> io::Error {
        io::Error::new(io::ErrorKind::PermissionDenied, "denied")
    }

    fn sample_serde_err() -> serde_json::Error {
        // Deterministic, no I/O. Any malformed JSON-to-i32 conversion suffices.
        serde_json::from_str::<i32>("\"not-an-int\"").unwrap_err()
    }

    #[test]
    fn lock_in_db_error_display() {
        // Sqlite — `#[error(transparent)]` delegates to the inner rusqlite::Error.
        // `QueryReturnedNoRows` has a stable Display: "Query returned no rows".
        let sqlite = DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows);
        assert_eq!(format!("{}", sqlite), "Query returned no rows");

        let mig = DbError::Migrations {
            expected: 2,
            actual: 1,
        };
        assert_eq!(
            format!("{}", mig),
            "schema migration failed: expected version 2, got 1"
        );

        let dd = DbError::DataDir {
            path: PathBuf::from("/tmp/foo"),
            source: sample_io_err(),
        };
        assert_eq!(
            format!("{}", dd),
            "failed to create data directory /tmp/foo"
        );

        let nb = DbError::NulByte {
            field: "init_command",
        };
        assert_eq!(format!("{}", nb), "init_command contains a NUL byte");

        let enc = DbError::CommandsJsonEncode {
            column: "init_commands_json",
            source: sample_serde_err(),
        };
        assert_eq!(format!("{}", enc), "encoding init_commands_json failed");

        let dec = DbError::CommandsJsonDecode {
            column: "run_commands_json",
            user_id: "u1".to_string(),
            workspace_name: "ws1".to_string(),
            source: sample_serde_err(),
        };
        assert_eq!(
            format!("{}", dec),
            "decoding run_commands_json for (u1,ws1) failed"
        );

        let vanished = DbError::RowDisappearedAfterUpsert;
        assert_eq!(format!("{}", vanished), "row was just upserted but vanished");
    }

    #[test]
    fn lock_in_db_error_into_maestro_error() {
        let sqlite_err = DbError::Sqlite(rusqlite::Error::QueryReturnedNoRows);
        assert!(matches!(
            MaestroError::from(sqlite_err),
            MaestroError::Db(DbError::Sqlite(_))
        ));

        let mig = DbError::Migrations {
            expected: 2,
            actual: 1,
        };
        assert!(matches!(
            MaestroError::from(mig),
            MaestroError::Db(DbError::Migrations { .. })
        ));

        let dd = DbError::DataDir {
            path: PathBuf::from("/tmp/foo"),
            source: sample_io_err(),
        };
        assert!(matches!(
            MaestroError::from(dd),
            MaestroError::Db(DbError::DataDir { .. })
        ));

        let nb = DbError::NulByte {
            field: "init_command",
        };
        assert!(matches!(
            MaestroError::from(nb),
            MaestroError::Db(DbError::NulByte { .. })
        ));

        let enc = DbError::CommandsJsonEncode {
            column: "init_commands_json",
            source: sample_serde_err(),
        };
        assert!(matches!(
            MaestroError::from(enc),
            MaestroError::Db(DbError::CommandsJsonEncode { .. })
        ));

        let dec = DbError::CommandsJsonDecode {
            column: "run_commands_json",
            user_id: "u1".to_string(),
            workspace_name: "ws1".to_string(),
            source: sample_serde_err(),
        };
        assert!(matches!(
            MaestroError::from(dec),
            MaestroError::Db(DbError::CommandsJsonDecode { .. })
        ));

        assert!(matches!(
            MaestroError::from(DbError::RowDisappearedAfterUpsert),
            MaestroError::Db(DbError::RowDisappearedAfterUpsert)
        ));
    }
}
