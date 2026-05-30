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
/// Wraps a backend-agnostic [`DbAdapter`] (over sqlx) plus the deployment's
/// resolved master-key state. Every DAO and call site takes
/// `&DbAdapter` from [`Database::adapter`]; production code does NOT touch
/// the underlying driver type. When plan-11 step 5 lands, `adapter`'s
/// inner pool may be SQLite, Postgres, or MySQL, transparently — all DAOs
/// keep working.
///
/// Plan-11 step 3 cluster RusqliteDrop: the legacy `Arc<Mutex<rusqlite::Connection>>`
/// field is gone. SQLite-side pragmas (WAL, foreign keys) are now applied
/// by sqlx's `SqliteConnectOptions` on every connection it opens; the
/// rusqlite handle no longer serves any purpose.
#[derive(Clone)]
pub struct Database {
    /// Plan-11: agnostic adapter wrapping a sqlx pool. DAOs take
    /// `&DbAdapter` from here.
    adapter: DbAdapter,
    /// Test-only: a long-lived rusqlite connection to the shared-cache
    /// in-memory SQLite. SQLite tears the in-memory DB down once the
    /// LAST open connection closes — sqlx's pool drains its connections
    /// between queries (and the migrator's runtime disposes its
    /// connection on shutdown), so without this anchor the database
    /// would disappear between operations. File-backed `Database::open`
    /// doesn't need this; the file persists on disk.
    #[cfg(test)]
    _mem_anchor: Option<Arc<std::sync::Mutex<rusqlite::Connection>>>,
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

/// Build an in-memory sqlx adapter pointing at the same shared-cache
/// in-memory SQLite database identified by `mem_id`. This is the
/// counterpart to opening rusqlite via the same URI; both views see
/// the same data so DAOs that have migrated to the adapter can be
/// tested alongside still-rusqlite DAOs in the same `Database`
/// instance.
#[cfg(test)]
fn build_shared_in_memory_adapter(mem_id: &str) -> DbAdapter {
    use sqlx::sqlite::SqlitePool;
    // SQLite shared-cache URI: every connection that opens the same
    // `file:<name>?mode=memory&cache=shared` URL within this process
    // attaches to the same in-memory database. We pin a per-Database
    // unique `mem_id` so parallel tests don't cross-contaminate.
    //
    // Plan-11 step 3 cluster RusqliteDrop: `min_connections = 1` keeps
    // one connection permanently in the pool so the shared-cache
    // memory DB stays alive for the lifetime of this `Database`.
    // Without this, the DB would vanish whenever the pool drained.
    let url = format!("file:{mem_id}?mode=memory&cache=shared");
    let opts = SqliteConnectOptions::from_str(&url)
        .expect("shared-cache URI parses")
        .foreign_keys(true)
        .create_if_missing(true);
    let pool: SqlitePool = SqlitePoolOptions::new()
        .min_connections(1)
        .connect_lazy_with(opts);
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

        // Plan-11 step 3 cluster Schema: build the sqlx adapter and run
        // all migrations through it. This makes `migrations/*.sql` the
        // single source of truth for schema; the rusqlite handle no
        // longer drives DDL.
        let adapter = build_sqlite_adapter(&db_path);
        migrate::apply_migrations_blocking(&adapter).map_err(|e| DbError::Adapter(
            adapter::DbError::Sqlx {
                source: sqlx::Error::Migrate(Box::new(e)),
            },
        ))?;

        Ok(Self {
            adapter,
            master_key,
            data_dir: Some(data_dir.to_path_buf()),
            #[cfg(test)]
            _mem_anchor: None,
        })
    }

    /// Plan-11 step 5: open the deployment's configured backend.
    ///
    /// Resolution:
    ///   - empty / omitted `connection` → SQLite at `{data_dir}/maestro.db`
    ///     (identical to [`Database::open`]).
    ///   - `sqlite://path`  → SQLite at the supplied path.
    ///   - `postgres://…` / `postgresql://…` → PostgreSQL pool.
    ///   - `mysql://…` → MySQL/MariaDB pool.
    ///
    /// The connectivity probe + `fail_fast` semantics from plan §6 are
    /// deferred to a follow-up cluster — this constructor just builds the
    /// pool and runs migrations.
    pub async fn connect(
        data_dir: &Path,
        db_config: &crate::config::DatabaseConfig,
        allow_auto_generate_secret_key: bool,
    ) -> Result<Self> {
        // Empty / whitespace-only `connection` keeps the legacy default
        // behaviour. Hand off to the sync `open` so the build path stays
        // identical to today's single-file SQLite deployments.
        if db_config.is_default_sqlite() {
            return Self::open(data_dir, allow_auto_generate_secret_key);
        }

        std::fs::create_dir_all(data_dir).map_err(|e| DbError::DataDir {
            path: data_dir.to_path_buf(),
            source: e,
        })?;

        // Build the backend-specific pool via pool::connect (already async,
        // already applies WAL + foreign_keys for SQLite).
        let tuning = pool::PoolTuning {
            max_connections: db_config.max_connections,
            acquire_timeout: db_config
                .acquire_timeout_secs
                .map(std::time::Duration::from_secs),
            idle_timeout: db_config
                .idle_timeout_secs
                .map(std::time::Duration::from_secs),
        };
        let url = db_config.connection_url();
        let backend_pool = pool::connect(url, &tuning).await.map_err(|e| {
            DbError::Adapter(adapter::DbError::Sqlx {
                source: match e {
                    pool::PoolError::Sqlx { source, .. } => source,
                    other => sqlx::Error::Configuration(other.to_string().into()),
                },
            })
        })?;
        let adapter = DbAdapter::new(backend_pool);

        // Migrate.
        migrate::apply_migrations(&adapter).await.map_err(|e| {
            DbError::Adapter(adapter::DbError::Sqlx {
                source: sqlx::Error::Migrate(Box::new(e)),
            })
        })?;

        // Master key resolution — same logic as `open`.
        let master_key = match load_or_init_master_key(data_dir, allow_auto_generate_secret_key) {
            Ok(loaded) => {
                tracing::info!(
                    source = ?loaded.source,
                    keyfile_world_readable = loaded.keyfile_world_readable,
                    backend = %adapter.backend(),
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
            adapter,
            master_key,
            data_dir: Some(data_dir.to_path_buf()),
            #[cfg(test)]
            _mem_anchor: None,
        })
    }

    /// Open an in-memory database for testing. No master key resolution —
    /// callers that need the key can call `with_test_master_key`.
    ///
    /// Plan-11 step 3 cluster B: rusqlite and the sqlx adapter both
    /// attach to the SAME in-memory SQLite via a shared-cache URI
    /// (`file:<uuid>?mode=memory&cache=shared`). Both views see the
    /// same data — DAOs that have migrated to the adapter can be
    /// tested alongside still-rusqlite ones in one `Database`
    /// instance. The per-Database UUID isolates parallel tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        let mem_id = uuid::Uuid::new_v4().to_string();
        // Anchor connection — keeps the shared-cache in-memory DB alive
        // for the lifetime of this `Database`. SQLite drops the in-memory
        // DB the moment the LAST open connection closes; sqlx's pool
        // releases connections back and the migrator thread's runtime
        // disposes its own on shutdown, so without this anchor the data
        // would vanish between operations.
        let url = format!("file:{mem_id}?mode=memory&cache=shared");
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let anchor = rusqlite::Connection::open_with_flags(&url, flags)?;

        let adapter = build_shared_in_memory_adapter(&mem_id);
        migrate::apply_migrations_blocking(&adapter).map_err(|e| DbError::Adapter(
            adapter::DbError::Sqlx {
                source: sqlx::Error::Migrate(Box::new(e)),
            },
        ))?;
        Ok(Self {
            adapter,
            master_key: None,
            data_dir: None,
            _mem_anchor: Some(Arc::new(std::sync::Mutex::new(anchor))),
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

    /// Plan-11: return the backend-agnostic adapter. Every DAO and call
    /// site takes `&DbAdapter` from here.
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
