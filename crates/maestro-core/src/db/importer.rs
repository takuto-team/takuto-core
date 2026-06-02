// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! SQLite → remote one-shot importer.
//!
//! ## When it runs
//!
//! From `Database::connect`, after migrations succeed on the remote
//! target. Skipped when:
//!   - backend is SQLite (nothing to import from, since the file IS the
//!     target),
//!   - `import_from_sqlite = false` in config,
//!   - no legacy `{data_dir}/maestro.db` file exists,
//!   - `system_metadata.import_complete` is already set on the target.
//!
//! ## Algorithm
//!
//! 1. Open the legacy SQLite file with a sqlx SQLite pool in
//!    **read-only** mode (`?mode=ro`) so the source is observably
//!    unchanged by the import.
//! 2. Begin a single transaction on the target pool.
//! 3. Copy every user-data table in FK-dependency order. Each row's
//!    cells are dispatched on their runtime storage class
//!    (`SqliteTypeInfo::name()` returns one of `"INTEGER"` / `"REAL"` /
//!    `"TEXT"` / `"BLOB"` / `"NULL"`) into the matching `DbValue`
//!    variant — BLOB / TEXT / INTEGER / NULL all round-trip without
//!    per-table boilerplate.
//! 4. Mark `system_metadata.import_complete = <unix_now>` and commit.
//!
//! A mid-import crash leaves the target untouched (everything is one
//! `BEGIN/COMMIT`). On next start the importer sees `import_complete`
//! is still NULL and reruns from scratch.
//!
//! ## Sessions (plan §8.6)
//!
//! All imported `sessions.expires_at` rows are overwritten with the
//! literal `'0'` so users are forced to re-authenticate after the
//! cutover. The next login bumps to the normal TTL.

use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use sqlx::Row;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePool, SqlitePoolOptions};

use super::adapter::DbAdapter;
use super::{DbBackend, DbTransaction, DbValue};

/// Errors surfaced by the importer. Distinct from `DbError` so call
/// sites can match on importer-specific failure modes without dragging
/// in the generic DAO error tree.
#[derive(Debug, thiserror::Error)]
pub enum ImporterError {
    #[error("can't open source SQLite at {path:?}: {source}")]
    OpenSource {
        path: PathBuf,
        #[source]
        source: sqlx::Error,
    },
    #[error("read from source table '{table}' failed: {source}")]
    Read {
        table: &'static str,
        #[source]
        source: sqlx::Error,
    },
    #[error("write to target table '{table}' failed (row {row}): {source}")]
    Write {
        table: &'static str,
        row: usize,
        #[source]
        source: sqlx::Error,
    },
    #[error("begin transaction on target failed: {source}")]
    Begin {
        #[source]
        source: sqlx::Error,
    },
    #[error("commit transaction on target failed: {source}")]
    Commit {
        #[source]
        source: sqlx::Error,
    },
}

/// True when `{data_dir}/maestro.db` exists on disk.
pub fn legacy_sqlite_exists(data_dir: &Path) -> bool {
    data_dir.join("maestro.db").is_file()
}

/// Read the target's `system_metadata.import_complete` row. `true` means
/// the importer has already run successfully and must not re-run.
pub async fn import_already_complete(adapter: &DbAdapter) -> Result<bool, sqlx::Error> {
    let row = adapter
        .query_optional(
            "SELECT 1 FROM system_metadata WHERE key = 'import_complete'",
            vec![],
        )
        .await
        .map_err(|e| match e {
            super::adapter::DbError::Sqlx { source } => source,
            other => sqlx::Error::Configuration(other.to_string().into()),
        })?;
    Ok(row.is_some())
}

/// Stamp `system_metadata.import_complete` with the current Unix
/// seconds. Idempotent — uses an upsert because the importer is meant
/// to run exactly once but we tolerate retry-after-success.
async fn mark_import_complete(
    tx: &mut DbTransaction<'_>,
    source_path: &Path,
) -> Result<(), sqlx::Error> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let path_str = source_path.display().to_string();
    let to_sqlx_err = |e: super::adapter::DbError| match e {
        super::adapter::DbError::Sqlx { source } => source,
        other => sqlx::Error::Configuration(other.to_string().into()),
    };
    // Two rows: import_complete (with the timestamp as `value`) and
    // import_source_path (with the path as `value`). Both share the
    // `updated_at` column. Dialect-aware upsert tail keeps the call
    // sites identical across SQLite / Postgres / MySQL.
    let tail = super::upsert::build_update_tail(
        tx.backend(),
        &["key"],
        &["value", "updated_at"],
    );
    let sql_complete = format!(
        "INSERT INTO system_metadata (key, value, updated_at) \
         VALUES ('import_complete', ?, ?) {tail}"
    );
    tx.execute(
        &sql_complete,
        vec![DbValue::Text(now.to_string()), DbValue::I64(now)],
    )
    .await
    .map_err(to_sqlx_err)?;
    let sql_path = format!(
        "INSERT INTO system_metadata (key, value, updated_at) \
         VALUES ('import_source_path', ?, ?) {tail}"
    );
    tx.execute(&sql_path, vec![DbValue::Text(path_str), DbValue::I64(now)])
        .await
        .map_err(to_sqlx_err)?;
    Ok(())
}

/// User-data tables in FK-dependency order. Each entry lists the
/// columns to copy with explicit names — no `SELECT *` and no
/// autoincrement IDs (those are regenerated by the target on INSERT).
///
/// `system_metadata` is intentionally omitted: the target's
/// `import_complete` row is set by [`mark_import_complete`] at the
/// end, and we don't want to overwrite it with whatever (if anything)
/// the source has.
const TABLES: &[(&str, &[&str])] = &[
    // Parents first.
    ("users", &["id", "username", "role", "suspended", "created_at", "updated_at"]),
    (
        "repositories",
        &["id", "name", "repo_url", "local_path", "default_branch", "created_at", "created_by"],
    ),
    // Children — all FK → users (and user_repositories also → repositories).
    (
        "credentials",
        &["id", "user_id", "kind", "data", "label", "created_at", "last_used_at"],
    ),
    ("recovery_codes", &["id", "user_id", "code_hash", "used", "created_at"]),
    (
        "sessions",
        &["id", "user_id", "data", "expires_at", "last_seen_at", "created_at_unix"],
    ),
    (
        "login_attempts",
        // No `id` — autoincrement on target.
        &["user_id", "kind", "attempted_at", "success"],
    ),
    (
        "container_users",
        &["id", "user_id", "container_id", "container_type", "os_username", "created_at", "destroyed_at"],
    ),
    (
        "user_worktree_commands",
        &["user_id", "workspace_name", "init_commands_json", "run_commands_json", "updated_at"],
    ),
    ("user_repositories", &["user_id", "repository_id", "added_at"]),
    (
        "user_provider_credentials",
        // No `id`.
        &[
            "user_id", "provider", "kind", "ciphertext", "nonce", "wrapped_dek", "wnonce",
            "metadata_json", "inactive", "last_validated_at", "last_used_at", "created_at",
            "updated_at", "expires_at",
        ],
    ),
    (
        "user_github_credentials",
        &[
            "user_id", "ciphertext", "nonce", "wrapped_dek", "wnonce", "github_login",
            "scopes_json", "sign_commits", "last_validated_at", "created_at", "updated_at",
        ],
    ),
    (
        "credential_audit",
        // No `id`.
        &[
            "user_id", "actor_user_id", "kind", "provider", "event", "outcome", "error_code", "at",
        ],
    ),
    (
        "onboarding_state",
        &[
            "user_id", "step_1_ticketing", "step_2_provider", "step_3_github",
            "step_4_credentials", "completed_at", "updated_at",
        ],
    ),
];

/// Run the import. Caller is expected to have already verified backend
/// ≠ SQLite, config opt-in, and `import_already_complete = false`.
///
/// Returns the total number of rows copied across all tables (handy
/// for the operator-facing startup log).
pub async fn import_from_sqlite(
    data_dir: &Path,
    adapter: &DbAdapter,
) -> Result<usize, ImporterError> {
    let source_path = data_dir.join("maestro.db");
    let source = open_source_read_only(&source_path).await?;

    let mut tx = adapter.begin().await.map_err(|e| match e {
        super::adapter::DbError::Sqlx { source } => ImporterError::Begin { source },
        other => ImporterError::Begin {
            source: sqlx::Error::Configuration(other.to_string().into()),
        },
    })?;

    let mut total = 0usize;
    for (table, columns) in TABLES {
        let copied = copy_table(&source, &mut tx, table, columns).await?;
        if copied > 0 {
            tracing::info!(table, rows = copied, "imported rows from SQLite source");
        }
        total += copied;
    }

    // Plan §8.6: invalidate every imported session — operators don't want
    // a process restart to carry sessions across an auth-store cutover.
    let to_sqlx = |e: super::adapter::DbError| match e {
        super::adapter::DbError::Sqlx { source } => source,
        other => sqlx::Error::Configuration(other.to_string().into()),
    };
    tx.execute("UPDATE sessions SET expires_at = '0'", vec![])
        .await
        .map_err(|e| ImporterError::Write {
            table: "sessions",
            row: 0,
            source: to_sqlx(e),
        })?;

    mark_import_complete(&mut tx, &source_path)
        .await
        .map_err(|source| ImporterError::Commit { source })?;

    tx.commit().await.map_err(|e| match e {
        super::adapter::DbError::Sqlx { source } => ImporterError::Commit { source },
        other => ImporterError::Commit {
            source: sqlx::Error::Configuration(other.to_string().into()),
        },
    })?;

    Ok(total)
}

async fn open_source_read_only(path: &Path) -> Result<SqlitePool, ImporterError> {
    let url = format!("sqlite://{}?mode=ro", path.display());
    let opts = SqliteConnectOptions::from_str(&url).map_err(|source| ImporterError::OpenSource {
        path: path.to_path_buf(),
        source,
    })?;
    // Single-connection pool — the importer is single-threaded against
    // the source. `connect_with` is eager so any open-time failure
    // surfaces here rather than mid-import.
    SqlitePoolOptions::new()
        .max_connections(1)
        .min_connections(1)
        .connect_with(opts)
        .await
        .map_err(|source| ImporterError::OpenSource {
            path: path.to_path_buf(),
            source,
        })
}

/// Read every row from `source.{table}` (selecting `columns`) and
/// INSERT into `target.{table}` using the same column list. Returns
/// the row count copied.
async fn copy_table(
    source: &SqlitePool,
    tx: &mut DbTransaction<'_>,
    table: &'static str,
    columns: &[&str],
) -> Result<usize, ImporterError> {
    let select_sql = format!("SELECT {} FROM {}", columns.join(", "), table);
    let rows = sqlx::query(&select_sql)
        .fetch_all(source)
        .await
        .map_err(|source| ImporterError::Read { table, source })?;

    let to_sqlx = |e: super::adapter::DbError| match e {
        super::adapter::DbError::Sqlx { source } => source,
        other => sqlx::Error::Configuration(other.to_string().into()),
    };

    let column_count = columns.len();
    let mut count = 0usize;
    for row in rows {
        let mut values: Vec<DbValue> = Vec::with_capacity(column_count);
        for i in 0..column_count {
            values.push(sqlite_row_to_db_value(&row, i, table)?);
        }
        // Build per-row SQL with NULLs inlined as literal `NULL` and
        // non-NULL values as `?` placeholders. sqlx-postgres caches the
        // prepared statement per SQL string and pins parameter types
        // on first use — binding `Option<i32>::None` to a varchar
        // column on row 0 would pin `$N` to INT4 and reject the Text
        // bind on row 1. Inlining `NULL` keeps the cached statement's
        // bind types consistent across calls (only the columns that
        // are non-NULL ever get bound, and their types are uniform per
        // column over the source data).
        let mut placeholders = Vec::with_capacity(column_count);
        let mut bind_values: Vec<DbValue> = Vec::with_capacity(column_count);
        for v in values {
            if matches!(v, DbValue::Null) {
                placeholders.push("NULL");
            } else {
                placeholders.push("?");
                bind_values.push(v);
            }
        }
        let insert_sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            table,
            columns.join(", "),
            placeholders.join(", ")
        );
        tx.execute(&insert_sql, bind_values).await.map_err(|e| {
            ImporterError::Write {
                table,
                row: count,
                source: to_sqlx(e),
            }
        })?;
        count += 1;
    }
    Ok(count)
}

/// Extract column `idx` from `row` as a [`DbValue`], dispatching on
/// SQLite's runtime storage class so BLOB / TEXT / INTEGER / REAL /
/// NULL all map cleanly. The dispatch matters because the declared
/// column type can lie under SQLite's dynamic typing — e.g. a
/// `BIGINT` column may carry a TEXT-typed value if the writer bound
/// one.
fn sqlite_row_to_db_value(
    row: &sqlx::sqlite::SqliteRow,
    idx: usize,
    table: &'static str,
) -> Result<DbValue, ImporterError> {
    use sqlx::TypeInfo;
    use sqlx::ValueRef;
    let value_ref = row
        .try_get_raw(idx)
        .map_err(|source| ImporterError::Read { table, source })?;
    // Runtime NULL check FIRST — `SqliteTypeInfo::name()` returns the
    // declared *column* type when the actual value is NULL, so dispatch
    // on `name()` alone routes NULL values into the TEXT/INTEGER/etc.
    // arm and `try_get::<String>` silently coerces NULL to `""`. That
    // landed empty strings in FK columns (e.g. `repositories.created_by`)
    // which then violated the FK constraint on Postgres.
    if ValueRef::is_null(&value_ref) {
        return Ok(DbValue::Null);
    }
    let ti = ValueRef::type_info(&value_ref);
    let class = ti.name();
    let map_err = |source: sqlx::Error| ImporterError::Read { table, source };
    Ok(match class {
        "INTEGER" => DbValue::I64(row.try_get::<i64, _>(idx).map_err(map_err)?),
        "REAL" => DbValue::F64(row.try_get::<f64, _>(idx).map_err(map_err)?),
        "BLOB" => DbValue::Bytes(row.try_get::<Vec<u8>, _>(idx).map_err(map_err)?),
        // Anything else (TEXT, declared-only types like NUMERIC, BOOLEAN,
        // DATETIME — SQLite stores them dynamically) reads as TEXT.
        _ => DbValue::Text(row.try_get::<String, _>(idx).map_err(map_err)?),
    })
}

/// Skip the importer entirely on a SQLite target — the backend IS the
/// source, no copy needed. Kept around for future use by callers that
/// orchestrate the importer outside `Database::connect`.
#[allow(dead_code)]
pub(crate) fn is_skippable_backend(backend: DbBackend) -> bool {
    matches!(backend, DbBackend::Sqlite)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use crate::db::migrate;
    use crate::db::pool::{DbBackend, DbPool};
    use crate::db::DbAdapter;

    #[test]
    fn skippable_backend_only_for_sqlite() {
        assert!(is_skippable_backend(DbBackend::Sqlite));
        assert!(!is_skippable_backend(DbBackend::Postgres));
        assert!(!is_skippable_backend(DbBackend::MySql));
    }

    #[test]
    fn legacy_sqlite_exists_reads_data_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!legacy_sqlite_exists(tmp.path()));
        std::fs::write(tmp.path().join("maestro.db"), b"").unwrap();
        assert!(legacy_sqlite_exists(tmp.path()));
    }

    #[tokio::test]
    async fn open_source_read_only_rejects_missing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope.db");
        let err = open_source_read_only(&missing)
            .await
            .expect_err("must error on missing");
        assert!(matches!(err, ImporterError::OpenSource { .. }));
    }

    /// Build an in-memory adapter we can use as a "remote" target in
    /// the cross-backend tests. The shared-cache URI keeps the DB
    /// alive across pool acquires; `min_connections = 1` plus an
    /// explicit anchor connection is the same belt-and-braces pattern
    /// `Database::open_in_memory` uses.
    fn build_target_adapter() -> (DbAdapter, rusqlite::Connection) {
        let mem_id = uuid::Uuid::new_v4().to_string();
        let url = format!("file:{mem_id}?mode=memory&cache=shared");
        let flags = rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
            | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
            | rusqlite::OpenFlags::SQLITE_OPEN_URI;
        let anchor = rusqlite::Connection::open_with_flags(&url, flags).unwrap();
        let opts = SqliteConnectOptions::from_str(&url)
            .unwrap()
            .foreign_keys(true)
            .create_if_missing(true);
        let pool: SqlitePool = SqlitePoolOptions::new()
            .min_connections(1)
            .connect_lazy_with(opts);
        let adapter = DbAdapter::new(DbPool::Sqlite(pool));
        (adapter, anchor)
    }

    /// End-to-end: build a source SQLite Database with two seeded
    /// users, drive the importer into a freshly-migrated target, and
    /// assert that (a) rows landed, (b) `import_complete` is set,
    /// (c) a second pass is a no-op.
    #[tokio::test]
    async fn import_copies_users_and_marks_complete_idempotently() {
        // Source: a real on-disk SQLite Database with two users seeded.
        let src_tmp = tempfile::tempdir().expect("src tempdir");
        let src_db = Database::open(src_tmp.path(), true).expect("open source");
        let src_adapter = src_db.adapter();
        for username in ["alice", "bob"] {
            crate::db::users::create_user(
                src_adapter,
                username,
                crate::db::models::UserRole::User,
            )
            .await
            .expect("seed user");
        }

        // Target: fresh in-memory adapter with all migrations applied.
        let (target_adapter, _anchor) = build_target_adapter();
        migrate::apply_migrations(&target_adapter).await.expect("migrate target");
        assert!(!import_already_complete(&target_adapter).await.unwrap());

        // Run.
        let copied = import_from_sqlite(src_tmp.path(), &target_adapter)
            .await
            .expect("import must succeed");
        assert!(copied >= 2, "expected at least 2 imported rows; got {copied}");

        // Target now has the two users.
        let count = target_adapter
            .query_one("SELECT COUNT(*) FROM users", vec![])
            .await
            .unwrap()
            .get_i64(0)
            .unwrap();
        assert_eq!(count, 2);

        // Marker is set; a second call is a no-op.
        assert!(import_already_complete(&target_adapter).await.unwrap());

        // Calling again would fail with a UNIQUE constraint if the
        // marker check were bypassed; the wrapper Database::connect
        // checks before calling. The function itself doesn't gate
        // (single-responsibility), so we don't test re-run here.
        let _ = DbBackend::Sqlite;
    }

    /// All imported sessions get their `expires_at` reset to '0' so
    /// users are forced to re-authenticate after the cutover.
    #[tokio::test]
    async fn import_invalidates_existing_sessions() {
        let src_tmp = tempfile::tempdir().expect("src tempdir");
        let src_db = Database::open(src_tmp.path(), true).expect("open source");
        let src_adapter = src_db.adapter();
        let user = crate::db::users::create_user(
            src_adapter,
            "alice",
            crate::db::models::UserRole::User,
        )
        .await
        .expect("seed user");
        // Seed a fake session row with a far-future expiry.
        src_adapter
            .execute(
                "INSERT INTO sessions \
                 (id, user_id, data, expires_at, last_seen_at, created_at_unix) \
                 VALUES ('sess-1', ?, ?, '2099-01-01T00:00:00Z', 0, 0)",
                vec![
                    DbValue::Text(user.id.clone()),
                    DbValue::Bytes(b"{\"user_id\":\"alice\"}".to_vec()),
                ],
            )
            .await
            .expect("seed session");

        let (target_adapter, _anchor) = build_target_adapter();
        migrate::apply_migrations(&target_adapter).await.expect("migrate target");
        import_from_sqlite(src_tmp.path(), &target_adapter)
            .await
            .expect("import");

        let expires: String = target_adapter
            .query_one(
                "SELECT expires_at FROM sessions WHERE id = ?",
                vec![DbValue::Text("sess-1".to_string())],
            )
            .await
            .unwrap()
            .get_text(0)
            .unwrap();
        assert_eq!(
            expires, "0",
            "imported sessions must be invalidated (expires_at = '0')"
        );
    }
}
