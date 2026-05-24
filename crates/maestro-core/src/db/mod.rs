// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! SQLite-backed persistence for multi-user authentication and access control.
//!
//! The database file lives at `{data_dir}/maestro.db`. Schema migrations run
//! automatically on first open via [`Database::open`].

pub mod credential_audit;
pub mod credentials;
pub mod error;
pub mod github_credentials;
pub mod login_attempts;
pub mod migration;
pub mod models;
pub mod onboarding;
pub mod provider_credentials;
pub mod repositories;
pub mod schema;
pub mod users;
pub mod user_worktree_commands;

pub use error::DbError;

#[cfg(test)]
mod tests_phase2a_master_key;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::sync::Mutex;
use tracing::warn;

use crate::auth::{MasterKey, MasterKeySource, load_or_init_master_key};
use crate::error::Result;

/// State of the deployment master key. Held on `Database` so the web layer
/// can:
/// (a) surface degraded-mode warnings in `SystemStatus` when the key is
///     unavailable or the keyfile is world-readable;
/// (b) pass the key into the per-user credential CRUD that lands in Phase 2b.
#[derive(Clone)]
pub struct MasterKeyState {
    pub key: Arc<MasterKey>,
    pub source: MasterKeySource,
    /// Set when the on-disk keyfile permissions are not `0600` on Unix.
    pub keyfile_world_readable: bool,
}

impl std::fmt::Debug for MasterKeyState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKeyState")
            .field("source", &self.source)
            .field("keyfile_world_readable", &self.keyfile_world_readable)
            .finish()
    }
}

/// Thread-safe database handle. Wraps a `rusqlite::Connection` in an `Arc<Mutex<>>`
/// so it can be shared across async tasks via `spawn_blocking`.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// Phase 2a: deployment master key state. `None` when:
    /// - `MAESTRO_SECRET_KEY` is unset AND
    /// - `${data_dir}/secret.key` does not exist AND
    /// - `allow_auto_generate_secret_key = false`
    ///
    /// or when any step of the resolution chain returned an error. The web
    /// layer surfaces this as a critical `SystemStatus` warning so workflows
    /// can't run blind into "I have credentials I can't unseal".
    master_key: Option<MasterKeyState>,
    /// The data dir the master key file lives under. `None` for in-memory
    /// fixtures. Stored so `maestro keys reset` can rewrite the file in place.
    data_dir: Option<PathBuf>,
}

impl Database {
    /// Open (or create) the database at `{data_dir}/maestro.db` and run migrations.
    ///
    /// Phase 2a: also resolves the deployment master key via
    /// [`load_or_init_master_key`]. Resolution failures are NOT fatal — the
    /// caller logs and surfaces a critical warning in `SystemStatus` so the
    /// dashboard renders degraded-mode copy instead of crashing.
    pub fn open(data_dir: &Path, allow_auto_generate_secret_key: bool) -> Result<Self> {
        std::fs::create_dir_all(data_dir).map_err(|e| DbError::DataDir {
            path: data_dir.to_path_buf(),
            source: e,
        })?;

        let db_path = data_dir.join("maestro.db");
        let conn = rusqlite::Connection::open(&db_path)?;

        // Enable WAL mode for better concurrent read performance.
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;

        schema::run_migrations(&conn)?;

        // Resolve master key. Errors are logged and converted to `None` so
        // the server can still boot in degraded mode (04_architecture.md §3.2:
        // "if neither resolves, the DB does not load encrypted columns and
        // the dashboard surfaces a degraded-mode banner").
        let master_key = match load_or_init_master_key(data_dir, allow_auto_generate_secret_key) {
            Ok(loaded) => {
                tracing::info!(
                    source = ?loaded.source,
                    keyfile_world_readable = loaded.keyfile_world_readable,
                    "Master key resolved"
                );
                Some(MasterKeyState {
                    key: Arc::new(loaded.key),
                    source: loaded.source,
                    keyfile_world_readable: loaded.keyfile_world_readable,
                })
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "Master key unavailable — per-user credential CRUD will be disabled (degraded mode)"
                );
                None
            }
        };

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            master_key,
            data_dir: Some(data_dir.to_path_buf()),
        })
    }

    /// Open an in-memory database for testing. No master key resolution —
    /// callers that need the key can call `with_test_master_key`.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory()?;
        conn.execute_batch("PRAGMA foreign_keys=ON;")?;
        schema::run_migrations(&conn)?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            master_key: None,
            data_dir: None,
        })
    }

    /// Test helper: inject a deterministic master key into an in-memory
    /// database so seal/open round-trip tests can exercise the credential
    /// CRUD path without touching disk.
    #[cfg(test)]
    pub fn with_test_master_key(mut self, key: MasterKey) -> Self {
        self.master_key = Some(MasterKeyState {
            key: Arc::new(key),
            source: MasterKeySource::Env,
            keyfile_world_readable: false,
        });
        self
    }

    /// Get a reference to the inner connection (for use with `spawn_blocking`).
    pub fn conn(&self) -> &Arc<Mutex<rusqlite::Connection>> {
        &self.conn
    }

    /// Reference to the resolved master key state, when available. `None` in
    /// degraded mode (no env var, no keyfile, auto-gen disabled).
    pub fn master_key(&self) -> Option<&MasterKeyState> {
        self.master_key.as_ref()
    }

    /// Data directory the database file lives under, when known. `None` for
    /// in-memory fixtures.
    pub fn data_dir(&self) -> Option<&Path> {
        self.data_dir.as_deref()
    }
}
