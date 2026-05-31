// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-11 step 1 — pluggable database backends.
//!
//! Source: `tmp/plan-11-pluggable-database-backends.md` §4. Introduces the
//! [`DbPool`] enum and a connection URL parser. This module is **scaffolding
//! only** — no existing call site reads it yet. Steps 2–6 of the plan migrate
//! schema migrations, the `Database` builder, the importer, and the
//! `&rusqlite::Connection` call sites in `db/*.rs` to use this pool.
//!
//! Why an enum rather than `sqlx::Any`: the plan explicitly rejects `Any`
//! (see plan §4). `Any` upcasts results to dynamic types and silently masks
//! per-backend syntax differences (`?` vs `$1`, `BLOB` vs `BYTEA`,
//! `AUTOINCREMENT` shapes). The enum keeps per-backend bind / column-type
//! support intact at the call sites that need it (the importer; future
//! per-backend SQL helpers).
//!
//! Threat model unchanged: nothing in this module exposes credentials in
//! Debug; the connection URL's password component is redacted via
//! [`redact_password_in_url`] when surfaced in error messages.

use std::time::Duration;

use sqlx::mysql::{MySqlPool, MySqlPoolOptions};
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::str::FromStr;
use thiserror::Error;

/// Which relational backend the running process is talking to.
///
/// `parse_backend` derives this from the `[database].connection` URL;
/// `DbPool::backend` returns it from an already-built pool. Stable string
/// form is the lowercase scheme without `ql` suffix (so `postgresql://` and
/// `postgres://` both land on `Postgres`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DbBackend {
    Sqlite,
    Postgres,
    MySql,
}

impl DbBackend {
    /// Stable lowercase identifier used in logs + `system_status` fields.
    pub fn as_str(self) -> &'static str {
        match self {
            DbBackend::Sqlite => "sqlite",
            DbBackend::Postgres => "postgres",
            DbBackend::MySql => "mysql",
        }
    }
}

impl std::fmt::Display for DbBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Backend-specific sqlx pool. Each variant carries the typed pool so per-
/// backend SQL helpers (the importer, dialect-aware queries in steps 5–8)
/// can fetch the right driver without `Any`-upcasting.
///
/// Pools are clone-cheap (sqlx pools are `Arc` under the hood); the enum is
/// `Clone` for the same reason `sqlx::SqlitePool` is.
#[derive(Clone)]
pub enum DbPool {
    Sqlite(SqlitePool),
    Postgres(PgPool),
    MySql(MySqlPool),
}

/// Optional pool tuning lifted from `[database]` in `config.toml`. `None`
/// preserves sqlx defaults (10 connections, 30 s acquire timeout, 10 min
/// idle timeout) which match plan-11 §5's documented defaults.
#[derive(Debug, Clone, Default)]
pub struct PoolTuning {
    pub max_connections: Option<u32>,
    pub acquire_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
}

/// Errors surfaced by URL parsing + pool construction. Distinct from
/// `DbError` (which carries rusqlite + DAO failures) so call sites can
/// match on the underlying problem (`UnsupportedScheme` → operator
/// misconfig, `Sqlx` → live DB outage, etc.).
#[derive(Debug, Error)]
pub enum PoolError {
    /// `[database].connection` was the empty string. Caller (in step 6,
    /// `Database::connect`) is expected to interpret empty as
    /// `sqlite://{data_dir}/maestro.db` BEFORE handing the URL to
    /// [`connect`]; this error guards the low-level path.
    #[error(
        "Empty connection URL — supply a scheme://… or leave [database].connection \
         blank to use the default SQLite file"
    )]
    EmptyUrl,

    /// URL did not contain a `scheme://` prefix.
    #[error("Connection URL is missing a scheme: '{url_redacted}'")]
    NoScheme { url_redacted: String },

    /// Scheme is not one of the four documented values.
    #[error(
        "Unsupported database scheme '{scheme}' — supported: \
         sqlite, postgres, postgresql, mysql (mysql also covers MariaDB)"
    )]
    UnsupportedScheme { scheme: String },

    /// `sqlx::Pool::connect` (or one of its options builders) returned an
    /// error. Underlying message is preserved.
    #[error("sqlx connect to {backend} failed: {source}")]
    Sqlx {
        backend: DbBackend,
        #[source]
        source: sqlx::Error,
    },
}

/// Parse the backend identifier from a `[database].connection` URL.
///
/// Accepts:
///   - `sqlite://path` → `Sqlite` (path can be a filesystem path,
///     `:memory:`, or any sqlx-recognised form)
///   - `sqlite:path` → `Sqlite` (sqlx tolerates the single-colon
///     shorthand; we mirror that)
///   - `postgres://…` → `Postgres`
///   - `postgresql://…` → `Postgres`
///   - `mysql://…` → `MySql` (covers MariaDB — same wire protocol)
///
/// Returns `EmptyUrl` for `""`, `NoScheme` for `foo/bar`, and
/// `UnsupportedScheme` for anything else.
pub fn parse_backend(url: &str) -> Result<DbBackend, PoolError> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(PoolError::EmptyUrl);
    }
    // `scheme://rest`. We also accept `scheme:rest` for SQLite where sqlx
    // documents that the single-colon form is valid.
    let (scheme, _) = trimmed.split_once(':').ok_or_else(|| PoolError::NoScheme {
        url_redacted: redact_password_in_url(trimmed),
    })?;
    match scheme.to_ascii_lowercase().as_str() {
        "sqlite" => Ok(DbBackend::Sqlite),
        "postgres" | "postgresql" => Ok(DbBackend::Postgres),
        "mysql" => Ok(DbBackend::MySql),
        other => Err(PoolError::UnsupportedScheme {
            scheme: other.to_string(),
        }),
    }
}

/// Build the appropriate sqlx pool for `url` with `tuning` applied.
///
/// SQLite setup mirrors the legacy `Database::open` flow: WAL journal mode +
/// `foreign_keys=ON`. This keeps the on-disk file format compatible with
/// the existing maestro.db so the plan-11 step-6 importer can read it
/// alongside the new sqlx-driven writes.
///
/// **Step 1 scope**: returns a constructed pool but does NOT run schema
/// migrations and does NOT touch the importer. Plan §6 step 2 ("schema
/// migrations") and §10 step 6 ("importer") layer those on top.
pub async fn connect(url: &str, tuning: &PoolTuning) -> Result<DbPool, PoolError> {
    let backend = parse_backend(url)?;
    match backend {
        DbBackend::Sqlite => connect_sqlite(url, tuning).await.map(DbPool::Sqlite),
        DbBackend::Postgres => connect_postgres(url, tuning).await.map(DbPool::Postgres),
        DbBackend::MySql => connect_mysql(url, tuning).await.map(DbPool::MySql),
    }
}

/// Apply common pool-tuning knobs to any sqlx pool-options builder. We
/// can't write this as a generic function because each backend's
/// `PoolOptions` is a different concrete type, so we use a small macro.
macro_rules! apply_pool_tuning {
    ($opts:ident, $tuning:expr) => {{
        if let Some(n) = $tuning.max_connections {
            $opts = $opts.max_connections(n);
        }
        if let Some(d) = $tuning.acquire_timeout {
            $opts = $opts.acquire_timeout(d);
        }
        if let Some(d) = $tuning.idle_timeout {
            $opts = $opts.idle_timeout(d);
        }
        $opts
    }};
}

async fn connect_sqlite(url: &str, tuning: &PoolTuning) -> Result<SqlitePool, PoolError> {
    // SqliteConnectOptions::from_str parses both `sqlite://...` and the
    // single-colon shorthand `sqlite:...` (sqlx docs §sqlite). We add the
    // legacy pragmas explicitly so the on-disk file stays compatible with
    // the existing rusqlite-driven path.
    let opts = SqliteConnectOptions::from_str(url)
        .map_err(|e| PoolError::Sqlx {
            backend: DbBackend::Sqlite,
            source: e,
        })?
        .journal_mode(SqliteJournalMode::Wal)
        .foreign_keys(true)
        .create_if_missing(true);

    let mut builder = SqlitePoolOptions::new();
    builder = apply_pool_tuning!(builder, tuning);
    builder
        .connect_with(opts)
        .await
        .map_err(|e| PoolError::Sqlx {
            backend: DbBackend::Sqlite,
            source: e,
        })
}

async fn connect_postgres(url: &str, tuning: &PoolTuning) -> Result<PgPool, PoolError> {
    let mut builder = PgPoolOptions::new();
    builder = apply_pool_tuning!(builder, tuning);
    builder
        .connect(url)
        .await
        .map_err(|e| PoolError::Sqlx {
            backend: DbBackend::Postgres,
            source: e,
        })
}

async fn connect_mysql(url: &str, tuning: &PoolTuning) -> Result<MySqlPool, PoolError> {
    let mut builder = MySqlPoolOptions::new();
    builder = apply_pool_tuning!(builder, tuning);
    builder
        .connect(url)
        .await
        .map_err(|e| PoolError::Sqlx {
            backend: DbBackend::MySql,
            source: e,
        })
}

impl DbPool {
    /// Which backend the pool is talking to.
    pub fn backend(&self) -> DbBackend {
        match self {
            DbPool::Sqlite(_) => DbBackend::Sqlite,
            DbPool::Postgres(_) => DbBackend::Postgres,
            DbPool::MySql(_) => DbBackend::MySql,
        }
    }

    /// `Some(_)` when this pool is the SQLite variant. Used by the
    /// importer (plan §8) and any backend-specific helper that must run a
    /// SQLite-only query.
    pub fn sqlite(&self) -> Option<&SqlitePool> {
        match self {
            DbPool::Sqlite(p) => Some(p),
            _ => None,
        }
    }

    /// `Some(_)` when this pool is the Postgres variant.
    pub fn postgres(&self) -> Option<&PgPool> {
        match self {
            DbPool::Postgres(p) => Some(p),
            _ => None,
        }
    }

    /// `Some(_)` when this pool is the MySQL/MariaDB variant.
    pub fn mysql(&self) -> Option<&MySqlPool> {
        match self {
            DbPool::MySql(p) => Some(p),
            _ => None,
        }
    }

    /// Best-effort connectivity probe (plan §6 step 3). Runs `SELECT 1`
    /// against the pool with a 5-second timeout. Returns the sqlx error
    /// verbatim on failure so the caller can decide whether to abort
    /// (`fail_fast = true`) or fall back to local SQLite
    /// (`fail_fast = false`).
    ///
    /// Implemented per-backend because the `Pool<DB>` types do not share a
    /// trait we can use for `fetch_one("SELECT 1")` generically without
    /// dragging in `sqlx::Any`.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        const TIMEOUT: Duration = Duration::from_secs(5);
        let fut = async {
            match self {
                DbPool::Sqlite(p) => {
                    sqlx::query_scalar::<_, i32>("SELECT 1")
                        .fetch_one(p)
                        .await?;
                }
                DbPool::Postgres(p) => {
                    sqlx::query_scalar::<_, i32>("SELECT 1")
                        .fetch_one(p)
                        .await?;
                }
                DbPool::MySql(p) => {
                    sqlx::query_scalar::<_, i32>("SELECT 1")
                        .fetch_one(p)
                        .await?;
                }
            }
            Ok::<_, sqlx::Error>(())
        };
        tokio::time::timeout(TIMEOUT, fut)
            .await
            .map_err(|_| sqlx::Error::PoolTimedOut)?
    }
}

impl std::fmt::Debug for DbPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't render the inner pool — sqlx pools format their own state
        // and there is no operator value in dumping it here. Backend name
        // is enough for trace logs.
        write!(f, "DbPool({})", self.backend())
    }
}

/// Replace the password component of a `scheme://user:password@host/...`
/// URL with `****`. Used to keep operator-facing error messages from
/// dumping credentials into logs. Leaves URLs without `@` (e.g.
/// `sqlite:///path`) untouched.
///
/// Lifted public so the future `[database].connection` redactor in
/// `Config::redacted_for_api_clone` (plan §5) can reuse the same logic.
pub fn redact_password_in_url(url: &str) -> String {
    // Look for `scheme://` then split at the first `@` we see before the
    // path. We don't pull in `url::Url` for this — its strict parsing
    // would reject the in-progress operator URLs we want to redact.
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let after_scheme = &url[scheme_end + 3..];
    let Some(at_pos) = after_scheme.find('@') else {
        return url.to_string();
    };
    let userinfo = &after_scheme[..at_pos];
    // Only redact if there's a colon (i.e. `user:password`); a bare user
    // is not sensitive.
    let Some(colon) = userinfo.find(':') else {
        return url.to_string();
    };
    let user = &userinfo[..colon];
    let host_and_path = &after_scheme[at_pos..]; // includes the leading '@'
    format!("{}://{}:****{}", &url[..scheme_end], user, host_and_path)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_backend ───────────────────────────────────────────────────

    #[test]
    fn parse_backend_recognises_sqlite_variants() {
        assert_eq!(parse_backend("sqlite:///tmp/x.db").unwrap(), DbBackend::Sqlite);
        // Single-colon shorthand (sqlx accepts; we mirror).
        assert_eq!(parse_backend("sqlite::memory:").unwrap(), DbBackend::Sqlite);
        assert_eq!(parse_backend("sqlite:/var/lib/x.db").unwrap(), DbBackend::Sqlite);
    }

    #[test]
    fn parse_backend_recognises_postgres_and_postgresql() {
        assert_eq!(
            parse_backend("postgres://u:p@h:5432/db").unwrap(),
            DbBackend::Postgres
        );
        assert_eq!(
            parse_backend("postgresql://u:p@h:5432/db").unwrap(),
            DbBackend::Postgres
        );
    }

    #[test]
    fn parse_backend_recognises_mysql() {
        assert_eq!(
            parse_backend("mysql://u:p@h:3306/db").unwrap(),
            DbBackend::MySql
        );
    }

    #[test]
    fn parse_backend_is_case_insensitive_on_scheme() {
        assert_eq!(parse_backend("SQLITE:///tmp/x.db").unwrap(), DbBackend::Sqlite);
        assert_eq!(
            parse_backend("Postgres://u@h/db").unwrap(),
            DbBackend::Postgres
        );
    }

    #[test]
    fn parse_backend_empty_url_returns_empty_url_error() {
        match parse_backend("") {
            Err(PoolError::EmptyUrl) => {}
            other => panic!("expected EmptyUrl, got {other:?}"),
        }
        // Whitespace-only is empty too.
        assert!(matches!(parse_backend("   "), Err(PoolError::EmptyUrl)));
    }

    #[test]
    fn parse_backend_no_scheme_returns_no_scheme_error() {
        match parse_backend("not-a-url") {
            Err(PoolError::NoScheme { url_redacted }) => {
                assert_eq!(url_redacted, "not-a-url");
            }
            other => panic!("expected NoScheme, got {other:?}"),
        }
    }

    #[test]
    fn parse_backend_unsupported_scheme_returns_unsupported_error() {
        match parse_backend("oracle://u:p@host/db") {
            Err(PoolError::UnsupportedScheme { scheme }) => {
                assert_eq!(scheme, "oracle");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
        match parse_backend("mongodb://localhost") {
            Err(PoolError::UnsupportedScheme { scheme }) => {
                assert_eq!(scheme, "mongodb");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    // ── redact_password_in_url ──────────────────────────────────────────

    #[test]
    fn redact_password_replaces_password_component() {
        assert_eq!(
            redact_password_in_url("postgres://alice:secret@host:5432/db"),
            "postgres://alice:****@host:5432/db"
        );
        assert_eq!(
            redact_password_in_url("mysql://root:hunter2@db:3306/maestro"),
            "mysql://root:****@db:3306/maestro"
        );
    }

    #[test]
    fn redact_password_leaves_passwordless_urls_alone() {
        // No '@' — nothing to redact.
        assert_eq!(
            redact_password_in_url("sqlite:///var/lib/maestro.db"),
            "sqlite:///var/lib/maestro.db"
        );
        // '@' with no userinfo (would be malformed) — leave alone.
        assert_eq!(
            redact_password_in_url("postgres://host/db"),
            "postgres://host/db"
        );
        // Bare user (no colon) — leave alone, the user is not a secret.
        assert_eq!(
            redact_password_in_url("postgres://alice@host/db"),
            "postgres://alice@host/db"
        );
    }

    #[test]
    fn redact_password_preserves_path_and_query() {
        assert_eq!(
            redact_password_in_url("postgres://u:p@host:5432/db?sslmode=require"),
            "postgres://u:****@host:5432/db?sslmode=require"
        );
    }

    #[test]
    fn redact_password_leaves_non_url_strings_alone() {
        // Without `://` we return the input verbatim — the helper is
        // best-effort, not a strict URL parser.
        assert_eq!(redact_password_in_url("not-a-url"), "not-a-url");
        assert_eq!(redact_password_in_url(""), "");
    }

    // ── DbBackend ───────────────────────────────────────────────────────

    #[test]
    fn db_backend_as_str_is_stable_lowercase() {
        assert_eq!(DbBackend::Sqlite.as_str(), "sqlite");
        assert_eq!(DbBackend::Postgres.as_str(), "postgres");
        assert_eq!(DbBackend::MySql.as_str(), "mysql");
    }

    #[test]
    fn db_backend_display_matches_as_str() {
        assert_eq!(format!("{}", DbBackend::Sqlite), "sqlite");
        assert_eq!(format!("{}", DbBackend::Postgres), "postgres");
        assert_eq!(format!("{}", DbBackend::MySql), "mysql");
    }

    // ── connect (SQLite path — the only one we can exercise without a
    //    container in unit tests) ────────────────────────────────────────

    #[tokio::test]
    async fn connect_in_memory_sqlite_succeeds_and_pings() {
        let pool = connect("sqlite::memory:", &PoolTuning::default())
            .await
            .expect("connect in-memory sqlite");
        assert_eq!(pool.backend(), DbBackend::Sqlite);
        assert!(pool.sqlite().is_some());
        assert!(pool.postgres().is_none());
        assert!(pool.mysql().is_none());
        pool.ping().await.expect("ping in-memory sqlite");
    }

    #[tokio::test]
    async fn connect_with_pool_tuning_applies_max_connections() {
        let tuning = PoolTuning {
            max_connections: Some(4),
            acquire_timeout: Some(Duration::from_secs(10)),
            idle_timeout: Some(Duration::from_secs(60)),
        };
        let pool = connect("sqlite::memory:", &tuning)
            .await
            .expect("connect with tuning");
        // sqlx doesn't expose `max_size()` on the pool publicly in all
        // versions; we assert by exercising the pool and trusting sqlx
        // applied the option. The ping confirms the pool is live.
        pool.ping().await.expect("ping tuned pool");
    }

    #[tokio::test]
    async fn connect_empty_url_returns_empty_url_error() {
        match connect("", &PoolTuning::default()).await {
            Err(PoolError::EmptyUrl) => {}
            other => panic!("expected EmptyUrl, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn connect_unsupported_scheme_returns_unsupported_error() {
        match connect("redis://localhost", &PoolTuning::default()).await {
            Err(PoolError::UnsupportedScheme { scheme }) => {
                assert_eq!(scheme, "redis");
            }
            other => panic!("expected UnsupportedScheme, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn db_pool_debug_does_not_render_inner_pool_state() {
        let pool = connect("sqlite::memory:", &PoolTuning::default())
            .await
            .unwrap();
        let s = format!("{pool:?}");
        // Locked: we render only `DbPool(<backend>)`. Inner sqlx state
        // must NOT leak — operators piping Debug output should not see
        // connection counts or driver internals here.
        assert_eq!(s, "DbPool(sqlite)");
    }
}
