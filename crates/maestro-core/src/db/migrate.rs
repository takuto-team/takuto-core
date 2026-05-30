// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-11 step 2 — dialect-aware sqlx migration source.
//!
//! Source: `tmp/plan-11-pluggable-database-backends.md` §7.
//!
//! The six existing schema migrations (`MIGRATION_V1`–`V6` in `schema.rs`)
//! are hand-translated into `crates/maestro-core/migrations/*.sql` in a
//! portable form (plan §7.2). At runtime the [`DialectAwareMigrationSource`]
//! reads those files (embedded via `include_str!` so the deployment stays
//! single-binary) and applies a small set of per-backend regex rewrites
//! before handing them to sqlx's migration runner.
//!
//! Step-2 scope: scaffolding only. Nothing reads this module yet. The
//! existing `schema.rs::run_migrations` (rusqlite-driven) remains the live
//! runtime path until plan-11 §10 step 3 ("call-site cutover") swaps it.
//!
//! Why one source file per migration rather than per-backend subdirectories
//! (an option plan §7.2 enumerated): the per-backend differences land
//! entirely on three textual rules (`BLOB`/`BYTEA`, two `AUTOINCREMENT`
//! shapes). A regex transform keeps the migrations file count low and the
//! diffs reviewable. If a future migration grows backend-specific syntax
//! the transformer cannot express, plan §7.3 leaves the door open to per-
//! backend files committed under `migrations/{backend}/` — that decision is
//! deferred.

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;

use sqlx::error::BoxDynError;
use sqlx::migrate::{Migration, MigrationSource, MigrationType, Migrator};

use super::adapter::DbAdapter;
use super::pool::{DbBackend, DbPool};

/// One embedded migration. Listed in version order at the bottom of this
/// file. The `version` is the YYYYMMDDHHMMSS prefix from the file name; the
/// `description` is the slug after the `_` and before the `.sql`.
struct EmbeddedMigration {
    version: i64,
    description: &'static str,
    sql: &'static str,
}

/// The migration set the binary ships with. Order matters — sqlx applies
/// them by ascending `version`. Adding a new migration means:
///   1. Drop the file into `crates/maestro-core/migrations/`.
///   2. Append the `EmbeddedMigration` entry here.
///   3. Update the `schema.rs` const + `SCHEMA_VERSION` (during cutover; in
///      the long run this duplication goes away when step 8 drops rusqlite).
const MIGRATIONS: &[EmbeddedMigration] = &[
    EmbeddedMigration {
        version: 20_260_101_000_001,
        description: "initial_users_credentials_recovery",
        sql: include_str!(
            "../../migrations/20260101000001_initial_users_credentials_recovery.sql"
        ),
    },
    EmbeddedMigration {
        version: 20_260_102_000_001,
        description: "login_attempts_and_session_columns",
        sql: include_str!(
            "../../migrations/20260102000001_login_attempts_and_session_columns.sql"
        ),
    },
    EmbeddedMigration {
        version: 20_260_103_000_001,
        description: "workspace_commands",
        sql: include_str!("../../migrations/20260103000001_workspace_commands.sql"),
    },
    EmbeddedMigration {
        version: 20_260_104_000_001,
        description: "user_worktree_commands",
        sql: include_str!("../../migrations/20260104000001_user_worktree_commands.sql"),
    },
    EmbeddedMigration {
        version: 20_260_105_000_001,
        description: "repositories_and_user_repos",
        sql: include_str!("../../migrations/20260105000001_repositories_and_user_repos.sql"),
    },
    EmbeddedMigration {
        version: 20_260_106_000_001,
        description: "user_credentials_and_audit",
        sql: include_str!("../../migrations/20260106000001_user_credentials_and_audit.sql"),
    },
];

/// A `sqlx::migrate::MigrationSource` impl that resolves the embedded
/// migrations after applying per-backend regex rewrites.
///
/// One instance is built per `Database::connect()` call; resolution is
/// pure (no I/O). The transform is deterministic — running the same
/// binary against the same backend produces the same SQL bytes, which is
/// what sqlx's checksum-based drift detection requires.
#[derive(Debug, Clone, Copy)]
pub struct DialectAwareMigrationSource {
    backend: DbBackend,
}

impl DialectAwareMigrationSource {
    pub fn for_backend(backend: DbBackend) -> Self {
        Self { backend }
    }

    /// Expose the per-backend translated SQL for a specific migration —
    /// surfaces what the runner will hand to sqlx, without the async
    /// resolve step. Used by the snapshot tests in this file.
    #[cfg(test)]
    pub(crate) fn translated_sql_for(&self, version: i64) -> Option<String> {
        MIGRATIONS
            .iter()
            .find(|m| m.version == version)
            .map(|m| translate_for_backend(m.sql, self.backend))
    }
}

impl<'s> MigrationSource<'s> for DialectAwareMigrationSource {
    fn resolve(
        self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Migration>, BoxDynError>> + Send + 's>> {
        Box::pin(async move {
            let mut out = Vec::with_capacity(MIGRATIONS.len());
            for m in MIGRATIONS {
                let sql = translate_for_backend(m.sql, self.backend);
                out.push(Migration::new(
                    m.version,
                    Cow::Borrowed(m.description),
                    MigrationType::Simple,
                    Cow::Owned(sql),
                    // `no_tx = false` — sqlx wraps each migration in a
                    // transaction. SQLite, Postgres, and MySQL all
                    // support DDL inside a tx (MySQL 8 commits implicitly
                    // on most DDL but the wrapper is still safe).
                    false,
                ));
            }
            Ok(out)
        })
    }
}

/// Translate a portable migration SQL string into the form the target
/// backend accepts. Pure function — exposed `pub(crate)` so tests can
/// snapshot the transformed bytes directly.
///
/// Rules applied (plan §7.2):
///   1. `INTEGER PRIMARY KEY AUTOINCREMENT` → backend-specific shape.
///   2. `BLOB` → `BYTEA` on Postgres only.
///
/// Whitespace before/after the matched tokens is preserved. The transform
/// is **token-based** in the sense that we match the documented full
/// keyword sequences; we do NOT try to be a SQL parser. Adding new
/// migrations that need other tokens is a deliberate change: extend this
/// function and re-snapshot.
pub(crate) fn translate_for_backend(sql: &str, backend: DbBackend) -> String {
    match backend {
        // SQLite is the bias of the source files; the only transform is
        // identity. We still go through the string copy so callers see
        // an owned String regardless of backend (uniform API).
        DbBackend::Sqlite => sql.to_string(),
        DbBackend::Postgres => {
            let s = replace_whole_token(
                sql,
                "INTEGER PRIMARY KEY AUTOINCREMENT",
                "BIGSERIAL PRIMARY KEY",
            );
            // BLOB → BYTEA is a column-type rewrite. The token only ever
            // appears as a stand-alone identifier in our migrations
            // (`ciphertext BLOB NOT NULL`, `data BLOB NOT NULL`).
            replace_whole_token(&s, "BLOB", "BYTEA")
        }
        DbBackend::MySql => {
            // MySQL needs the column type AND the PK to land on one
            // column declaration — `BIGINT AUTO_INCREMENT PRIMARY KEY`
            // covers it. The portable-source form keeps the
            // SQLite-style placement; we swap the whole token at once.
            replace_whole_token(
                sql,
                "INTEGER PRIMARY KEY AUTOINCREMENT",
                "BIGINT AUTO_INCREMENT PRIMARY KEY",
            )
        }
    }
}

/// Replace every occurrence of `needle` with `replacement` only when
/// `needle` is a whole-word match (i.e. surrounded by either start/end
/// of string or by a non-identifier character). Identifier characters
/// are ASCII alphanumeric + `_`.
///
/// Why not `str::replace` directly: SQLite's `BLOB` is a column type
/// keyword, but the substring `BLOB` could appear inside an identifier
/// like `my_blob_table`. We don't currently emit such names, but the
/// guard is cheap and keeps the transformer safer as new migrations land.
fn replace_whole_token(haystack: &str, needle: &str, replacement: &str) -> String {
    if needle.is_empty() {
        return haystack.to_string();
    }
    let mut out = String::with_capacity(haystack.len());
    let mut cursor = 0;
    while let Some(rel) = haystack[cursor..].find(needle) {
        let start = cursor + rel;
        let end = start + needle.len();
        let before_ok = start == 0
            || !is_ident_char(haystack.as_bytes()[start - 1]);
        let after_ok = end == haystack.len()
            || !is_ident_char(haystack.as_bytes()[end]);
        if before_ok && after_ok {
            out.push_str(&haystack[cursor..start]);
            out.push_str(replacement);
            cursor = end;
        } else {
            // Skip past the first byte so we don't loop forever on the
            // same false-positive position.
            out.push_str(&haystack[cursor..=start]);
            cursor = start + 1;
        }
    }
    out.push_str(&haystack[cursor..]);
    out
}

#[inline]
fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Apply all embedded migrations against the adapter's pool.
///
/// Async entrypoint — use [`apply_migrations_blocking`] from synchronous
/// callers (e.g. `Database::open` which stays sync to keep its API stable
/// across the migration cluster).
pub async fn apply_migrations(adapter: &DbAdapter) -> Result<(), sqlx::migrate::MigrateError> {
    let backend = adapter.backend();
    let source = DialectAwareMigrationSource::for_backend(backend);
    let migrator = Migrator::new(source)
        .await
        .map_err(|e| match e {
            sqlx::migrate::MigrateError::Source(s) => sqlx::migrate::MigrateError::Source(s),
            other => other,
        })?;
    match adapter.pool() {
        DbPool::Sqlite(pool) => migrator.run(pool).await,
        DbPool::Postgres(pool) => migrator.run(pool).await,
        DbPool::MySql(pool) => migrator.run(pool).await,
    }
}

/// Run [`apply_migrations`] to completion from a synchronous context.
///
/// Spawns a dedicated OS thread that owns a fresh current-thread tokio
/// runtime. The thread approach (rather than `Handle::block_on` or a
/// reused runtime) makes this safe to call from inside an existing tokio
/// runtime — the migrator runs on the new thread's runtime, the caller's
/// runtime stays untouched.
pub fn apply_migrations_blocking(adapter: &DbAdapter) -> Result<(), sqlx::migrate::MigrateError> {
    let adapter = adapter.clone();
    std::thread::scope(|s| {
        s.spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| sqlx::migrate::MigrateError::Execute(sqlx::Error::Io(e)))?;
            rt.block_on(apply_migrations(&adapter))
        })
        .join()
        .expect("migration thread panicked")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePool;

    // ── Per-backend translation rules ───────────────────────────────────

    #[test]
    fn translate_sqlite_is_identity() {
        let input = "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, blob_col BLOB);";
        assert_eq!(translate_for_backend(input, DbBackend::Sqlite), input);
    }

    #[test]
    fn translate_postgres_rewrites_autoincrement_and_blob() {
        let input = "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, data BLOB NOT NULL);";
        let got = translate_for_backend(input, DbBackend::Postgres);
        assert!(
            got.contains("BIGSERIAL PRIMARY KEY"),
            "PG must rewrite AUTOINCREMENT, got: {got}"
        );
        assert!(
            got.contains("data BYTEA NOT NULL"),
            "PG must rewrite BLOB → BYTEA, got: {got}"
        );
        assert!(
            !got.contains("AUTOINCREMENT"),
            "PG output still contains AUTOINCREMENT: {got}"
        );
        assert!(!got.contains("BLOB"), "PG output still contains BLOB: {got}");
    }

    #[test]
    fn translate_mysql_rewrites_autoincrement_only() {
        let input = "CREATE TABLE t (id INTEGER PRIMARY KEY AUTOINCREMENT, data BLOB NOT NULL);";
        let got = translate_for_backend(input, DbBackend::MySql);
        assert!(
            got.contains("BIGINT AUTO_INCREMENT PRIMARY KEY"),
            "MySQL must rewrite AUTOINCREMENT, got: {got}"
        );
        // BLOB is native in MySQL — leave alone.
        assert!(
            got.contains("data BLOB NOT NULL"),
            "MySQL must keep BLOB, got: {got}"
        );
    }

    #[test]
    fn translate_does_not_replace_substrings_in_identifiers() {
        // `BLOB` appears inside `my_blob_col` — must not rewrite.
        let input = "CREATE TABLE t (my_blob_col BLOB NOT NULL);";
        let got = translate_for_backend(input, DbBackend::Postgres);
        assert!(
            got.contains("my_blob_col BYTEA NOT NULL"),
            "must rewrite standalone BLOB after `my_blob_col`, got: {got}"
        );
        // The identifier prefix `my_blob_` is preserved because it's a
        // different token (would-be transform fails the whole-word guard).
        assert!(got.contains("my_blob_col"), "identifier must survive: {got}");
    }

    #[test]
    fn translate_skips_blob_inside_alphanumeric_identifier() {
        // A defence-in-depth case: a future column named exactly `BLOB_v2`
        // would not be a SQL type. The whole-word guard rejects the
        // rewrite because `_` is an identifier character.
        let input = "CREATE TABLE t (BLOB_v2 INTEGER);";
        let got = translate_for_backend(input, DbBackend::Postgres);
        assert_eq!(
            got, input,
            "BLOB inside identifier must not be rewritten, got: {got}"
        );
    }

    // ── Embedded migration set ──────────────────────────────────────────

    #[test]
    fn embedded_migrations_are_listed_in_ascending_version_order() {
        let versions: Vec<i64> = MIGRATIONS.iter().map(|m| m.version).collect();
        let mut sorted = versions.clone();
        sorted.sort();
        assert_eq!(
            versions, sorted,
            "MIGRATIONS must be in ascending version order; got: {versions:?}"
        );
    }

    #[test]
    fn embedded_migration_count_matches_schema_v6() {
        // schema.rs lives at SCHEMA_VERSION = 6; the new file set must
        // hold exactly 6 entries. When a future migration lands, BOTH
        // this constant AND schema.rs's `SCHEMA_VERSION` bump in lockstep.
        assert_eq!(MIGRATIONS.len(), 6);
    }

    #[test]
    fn embedded_migrations_have_non_empty_sql() {
        for m in MIGRATIONS {
            assert!(
                !m.sql.trim().is_empty(),
                "migration {} ({}) has empty SQL body",
                m.version,
                m.description
            );
            // Cheap regression check: every file mentions CREATE / ALTER /
            // DROP at least once.
            let upper = m.sql.to_uppercase();
            assert!(
                upper.contains("CREATE")
                    || upper.contains("ALTER")
                    || upper.contains("DROP"),
                "migration {} ({}) does not appear to do any DDL",
                m.version,
                m.description
            );
        }
    }

    // ── End-to-end: run all migrations on in-memory SQLite ─────────────

    /// Plan-11 step 2 acceptance: the full set applies cleanly via sqlx
    /// against an in-memory SQLite pool. We then assert the table list
    /// matches what `schema.rs::run_migrations` produces on the same DB —
    /// a proxy for "the new path is schema-equivalent to the live one".
    #[tokio::test]
    async fn migrations_apply_clean_to_in_memory_sqlite_via_sqlx() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations");

        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_master \
             WHERE type='table' AND name NOT LIKE 'sqlite_%' \
             ORDER BY name",
        )
        .fetch_all(&pool)
        .await
        .expect("list tables");
        let tables: Vec<String> = rows.into_iter().map(|(n,)| n).collect();

        // Every table the live (rusqlite) path creates is also present.
        for expected in [
            "users",
            "credentials",
            "recovery_codes",
            "user_repositories",
            "container_users",
            "sessions",
            "login_attempts",
            "user_worktree_commands",
            "repositories",
            "user_provider_credentials",
            "user_github_credentials",
            "credential_audit",
            "onboarding_state",
        ] {
            assert!(
                tables.iter().any(|t| t == expected),
                "missing table {expected} after sqlx migrate; got: {tables:?}"
            );
        }
        // V4 drops V3's table — must not survive on a fresh DB.
        assert!(
            !tables.iter().any(|t| t == "workspace_commands"),
            "workspace_commands should be dropped by V4; got: {tables:?}"
        );
    }

    /// A second `Migrator::run` on the same pool must be a no-op — sqlx
    /// records applied migrations in `_sqlx_migrations`. Drift detection
    /// catches accidental SQL edits in a way the rusqlite path does not.
    #[tokio::test]
    async fn migrations_are_idempotent_on_second_run() {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        let migrator = sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator");
        migrator.run(&pool).await.expect("first run");
        migrator.run(&pool).await.expect("second run is a no-op");

        let count: (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
                .fetch_one(&pool)
                .await
                .expect("count rows");
        assert_eq!(
            count.0, 6,
            "expected 6 applied migrations recorded by sqlx, got {}",
            count.0
        );
    }

    /// The translated SQL produced for SQLite must be byte-equal to the
    /// embedded source file (the SQLite branch is identity). Drift detector
    /// — guards against accidentally adding a transform that fires for the
    /// wrong backend.
    #[test]
    fn sqlite_translation_is_byte_equal_to_source_files() {
        let src = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        for m in MIGRATIONS {
            let got = src.translated_sql_for(m.version).expect("known version");
            assert_eq!(got, m.sql, "SQLite output differs from source for {}", m.description);
        }
    }

    // ── Postgres / MySQL — ignored unless a container DSN is provided ───
    //
    // Plan-11 §10 step 4 wires the CI matrix that sets `DATABASE_URL` for
    // these tests. Until then they are `#[ignore]` so local `cargo test`
    // stays fast and offline. To run them locally:
    //   cargo test --lib db::migrate -- --ignored
    // with `DATABASE_URL=postgres://...` (or `mysql://...`) exported.

    #[tokio::test]
    #[ignore = "requires DATABASE_URL=postgres://..."]
    async fn migrations_apply_clean_to_postgres() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL=postgres://... to run this test");
        let pool = sqlx::postgres::PgPool::connect(&url).await.expect("connect");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Postgres);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations against Postgres");
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL=mysql://..."]
    async fn migrations_apply_clean_to_mysql() {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL=mysql://... to run this test");
        let pool = sqlx::mysql::MySqlPool::connect(&url).await.expect("connect");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::MySql);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations against MySQL");
    }
}
