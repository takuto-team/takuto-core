// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! SQLite-backed persistence for multi-user authentication and access control.
//!
//! The database file lives at `{data_dir}/maestro.db`. Schema migrations run
//! automatically on first open via [`Database::open`].

// Plan-11 step 3 (tmp/plan-11-pluggable-database-backends.md §4 +
// 2026-05-27 user mandate): backend-agnostic DB adapter. Every DAO and
// call site takes `&DbAdapter`; the adapter dispatches to the right
// sqlx driver internally. This is the cutover from the legacy
// `&rusqlite::Connection` API.
pub mod adapter;
pub mod credential_audit;
pub mod credentials;
pub mod error;
pub mod github_credentials;
pub mod login_attempts;
// Legacy rusqlite migration runner (versioned schema_migrations table).
pub mod migration;
// Plan-11 step 2 (tmp/plan-11-pluggable-database-backends.md §7): sqlx-based
// migration source with per-backend dialect transforms. Lives alongside
// `migration.rs`; nothing reads it yet (call-site cutover is plan §10 step 3).
pub mod migrate;
pub mod models;
pub mod onboarding;
// Plan-11 step 1 (tmp/plan-11-pluggable-database-backends.md §4): pluggable
// database backends via sqlx. The module is scaffolding only at this step —
// no existing call site reads it. Subsequent plan-11 steps cut the
// `Database` builder, schema migrations, the importer, and every DAO over
// to `DbPool` + `.await`.
pub mod pool;
pub mod provider_credentials;
pub mod repositories;
pub mod schema;
pub mod users;
pub mod user_worktree_commands;

pub use adapter::{
    DbAdapter, DbError as AdapterError, DbResult, DbRow, DbTransaction, DbValue,
};
pub use error::DbError;
pub use pool::{DbBackend, DbPool, PoolError, PoolTuning};

#[cfg(test)]
mod tests_phase2a_master_key;

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
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

/// Thread-safe database handle.
///
/// Holds two parallel views of the same SQLite file during the plan-11
/// step-3 cutover:
///
/// 1. Legacy: `Arc<Mutex<rusqlite::Connection>>` for the 296 not-yet-
///    migrated call sites. Accessor: [`Database::conn`].
/// 2. New: [`DbAdapter`] (wrapping a sqlx `SqlitePool`) for DAOs that have
///    been moved to the agnostic adapter API. Accessor:
///    [`Database::adapter`].
///
/// Both views point at the same `maestro.db` file (or in-memory database
/// for tests — see `open_in_memory`). The sqlx pool is opened **lazily**:
/// `connect_lazy_with` returns the pool without testing the connection,
/// and the first async query opens a real connection. This keeps
/// [`Database::open`] synchronous (no tokio runtime requirement) which is
/// what the existing main.rs + test fixtures expect.
///
/// When `provider` graduates to per-backend in plan-11 step 5, the
/// `pool` field becomes the full `DbPool` enum (sqlite + postgres +
/// mysql). For step 3 the pool is hard-coded to SQLite.
#[derive(Clone)]
pub struct Database {
    conn: Arc<Mutex<rusqlite::Connection>>,
    /// Plan-11 step 3: agnostic adapter wrapping a sqlx SQLite pool that
    /// opens lazily against the same `maestro.db` file as `conn`. DAOs
    /// migrated to the new API take `&DbAdapter` from here.
    adapter: DbAdapter,
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

/// Build the sqlx pool used by [`Database::adapter`]. Opens lazily — no
/// async runtime required at construction. Mirrors the WAL + foreign-keys
/// pragmas applied by the rusqlite open so both views see the same on-disk
/// behaviour.
fn build_sqlite_adapter(db_path: &Path) -> DbAdapter {
    // sqlx's SqliteConnectOptions accepts both `sqlite://path` and a
    // bare path. We prefix `sqlite://` for clarity and to remove any
    // ambiguity with the parser.
    let url = format!("sqlite://{}", db_path.display());
    // SAFETY: sqlx's SQLite parser only fails on malformed URI syntax,
    // and we construct the URL ourselves from `{db_path.display()}`. The
    // panic message names the exact path so a future code change that
    // breaks this invariant (e.g. injecting `?mode=` query params with
    // bad encoding) is surfaced loudly.
    let opts = SqliteConnectOptions::from_str(&url)
        .expect("constructed sqlite:// URL must parse — invariant from build_sqlite_adapter")
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .create_if_missing(true);
    let pool = SqlitePoolOptions::new().connect_lazy_with(opts);
    DbAdapter::new(DbPool::Sqlite(pool))
}

/// Build an in-memory sqlx adapter for test fixtures. Each invocation
/// gets its own private in-memory database (sqlx `:memory:` URLs are
/// per-connection unless you use a shared cache, which we deliberately
/// don't here so test parallelism doesn't cross-contaminate).
#[cfg(test)]
fn build_in_memory_adapter() -> DbAdapter {
    use sqlx::sqlite::SqlitePool;
    // SAFETY: the literal `sqlite::memory:` URL is the canonical in-memory
    // form documented by sqlx; the parser cannot fail on it.
    let opts = SqliteConnectOptions::from_str("sqlite::memory:")
        .expect("in-memory sqlite URL is a sqlx-documented literal")
        .foreign_keys(true);
    let pool: SqlitePool = SqlitePoolOptions::new().connect_lazy_with(opts);
    DbAdapter::new(DbPool::Sqlite(pool))
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

        // Plan-11 step 3: build the lazy sqlx adapter against the same
        // file. No connection is opened yet; first async query opens it.
        let adapter = build_sqlite_adapter(&db_path);

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            adapter,
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
        // Plan-11 step 3: the adapter here points at a SEPARATE in-memory
        // database, NOT the rusqlite one. SQLite `:memory:` is
        // per-connection by default. Tests that exercise both views
        // simultaneously must set up the adapter's tables independently
        // (see crates/maestro-core/src/db/login_attempts.rs tests for the
        // canonical pattern). This is a known wart of the hybrid
        // transition; it goes away in step 8 when rusqlite is dropped.
        let adapter = build_in_memory_adapter();
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            adapter,
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

    /// Get a reference to the inner rusqlite connection (legacy path,
    /// used by every DAO not yet migrated to the agnostic adapter).
    pub fn conn(&self) -> &Arc<Mutex<rusqlite::Connection>> {
        &self.conn
    }

    /// Plan-11 step 3: return the backend-agnostic adapter for DAOs that
    /// have been migrated to the new API. During the hybrid transition
    /// both `conn()` and `adapter()` are valid entry points — pick the
    /// one that matches the DAO's signature.
    pub fn adapter(&self) -> &DbAdapter {
        &self.adapter
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
