// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dialect-aware sqlx migration source.
//!
//! The six existing schema migrations (`MIGRATION_V1`–`V6` in `schema.rs`)
//! are hand-translated into `crates/maestro-core/migrations/*.sql` in a
//! portable form. At runtime the [`DialectAwareMigrationSource`] reads
//! those files (embedded via `include_str!` so the deployment stays
//! single-binary) and applies a small set of per-backend regex rewrites
//! before handing them to sqlx's migration runner.
//!
//! Why one source file per migration rather than per-backend subdirectories:
//! the per-backend differences land entirely on three textual rules
//! (`BLOB`/`BYTEA`, two `AUTOINCREMENT` shapes). A regex transform keeps
//! the migrations file count low and the diffs reviewable. If a future
//! migration grows backend-specific syntax the transformer cannot express,
//! per-backend files committed under `migrations/{backend}/` remain an
//! option — that decision is deferred.

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
///   3. Update the `schema.rs` const + `SCHEMA_VERSION` (this duplication
///      will go away when rusqlite is dropped).
const MIGRATIONS: &[EmbeddedMigration] = &[
    EmbeddedMigration {
        version: 20_260_101_000_001,
        description: "initial_users_credentials_recovery",
        sql: include_str!("../../migrations/20260101000001_initial_users_credentials_recovery.sql"),
    },
    EmbeddedMigration {
        version: 20_260_102_000_001,
        description: "login_attempts_and_session_columns",
        sql: include_str!("../../migrations/20260102000001_login_attempts_and_session_columns.sql"),
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
    EmbeddedMigration {
        version: 20_260_116_000_001,
        description: "pluggable_db_system_metadata",
        sql: include_str!("../../migrations/20260116000001_pluggable_db_system_metadata.sql"),
    },
    EmbeddedMigration {
        version: 20_260_117_000_001,
        description: "work_items_and_logs",
        sql: include_str!("../../migrations/20260117000001_work_items_and_logs.sql"),
    },
    EmbeddedMigration {
        version: 20_260_118_000_001,
        description: "work_items_repository_id",
        sql: include_str!("../../migrations/20260118000001_work_items_repository_id.sql"),
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
            let s = replace_whole_token(
                sql,
                "INTEGER PRIMARY KEY AUTOINCREMENT",
                "BIGINT AUTO_INCREMENT PRIMARY KEY",
            );
            // `CREATE INDEX IF NOT EXISTS` / `DROP INDEX IF EXISTS` are
            // SQLite + Postgres syntax; MySQL's CREATE INDEX grammar has
            // no `IF [NOT] EXISTS` clause. Migrations run against a
            // fresh schema in this test/CI path, so dropping the clause
            // is safe — the index won't pre-exist.
            let s = s.replace("CREATE INDEX IF NOT EXISTS", "CREATE INDEX");
            let s = s.replace("DROP INDEX IF EXISTS", "DROP INDEX");
            // MySQL requires a prefix length when a TEXT/BLOB column
            // participates in a key. Several portable migrations
            // declare ISO-timestamp / URL-shaped columns as `TEXT` for
            // SQLite + Postgres simplicity but then index them or
            // include them in a UNIQUE — MySQL rejects that with
            // error 1170 unless we widen to VARCHAR. Each entry below
            // covers exactly one (column, declaration) pair that
            // appears in a key.
            let s = s
                .replace("repo_url TEXT NOT NULL", "repo_url VARCHAR(512) NOT NULL")
                .replace(
                    "expires_at TEXT NOT NULL",
                    "expires_at VARCHAR(64) NOT NULL",
                );
            // `credential_audit.at` is a bare-name column — bare
            // `str::replace` on "at TEXT NOT NULL DEFAULT ''" would
            // also chew up "created_at TEXT NOT NULL DEFAULT ''" and
            // friends, so anchor the match to a whole-word boundary.
            let s = replace_whole_token(
                &s,
                "at TEXT NOT NULL DEFAULT ''",
                "at VARCHAR(64) NOT NULL DEFAULT ''",
            );
            // MySQL 8.0.13+ accepts `DEFAULT` on `TEXT` / `BLOB` / `JSON`
            // columns ONLY when the literal is wrapped in parentheses
            // (`DEFAULT ('...')`). The source files use the simpler
            // `DEFAULT '...'` form which is valid SQLite + Postgres.
            // Rewrite per-MySQL: `DEFAULT '<x>'` → `DEFAULT ('<x>')`.
            wrap_text_defaults_for_mysql(&s)
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
        let before_ok = start == 0 || !is_ident_char(haystack.as_bytes()[start - 1]);
        let after_ok = end == haystack.len() || !is_ident_char(haystack.as_bytes()[end]);
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

/// MySQL-specific: wrap `DEFAULT '<literal>'` as `DEFAULT ('<literal>')`.
///
/// MySQL 8.0.13+ only accepts column defaults on TEXT/BLOB/JSON when the
/// expression sits inside parentheses. Wrapping every literal default is
/// harmless on VARCHAR / INTEGER columns (MySQL accepts both forms there
/// too), so we apply it uniformly rather than trying to inspect each
/// column's declared type.
///
/// Conservative: only single-quoted literals, no escaped quotes. Our
/// migration files use only simple literals like `''`, `'{}'`, `'[]'` —
/// no inner-quote complications. If a future migration needs escapes,
/// extend this with proper quote-aware scanning.
fn wrap_text_defaults_for_mysql(sql: &str) -> String {
    let needle = "DEFAULT '";
    let mut out = String::with_capacity(sql.len() + 16);
    let mut cursor = 0;
    while let Some(rel) = sql[cursor..].find(needle) {
        let kw_start = cursor + rel;
        let lit_start = kw_start + needle.len();
        // Skip if already wrapped: previous non-space byte is `(`.
        let already_wrapped = sql[..kw_start]
            .bytes()
            .rev()
            .find(|b| !b.is_ascii_whitespace())
            .map(|b| b == b'(')
            .unwrap_or(false);
        // Find the closing quote. Bail on broken syntax rather than
        // silently corrupting.
        let Some(end_rel) = sql[lit_start..].find('\'') else {
            out.push_str(&sql[cursor..]);
            return out;
        };
        let lit_end = lit_start + end_rel; // index of closing quote
        out.push_str(&sql[cursor..kw_start]);
        if already_wrapped {
            // Pass the original `DEFAULT '...'` through unchanged.
            out.push_str(&sql[kw_start..=lit_end]);
        } else {
            out.push_str("DEFAULT ('");
            out.push_str(&sql[lit_start..lit_end]);
            out.push_str("')");
        }
        cursor = lit_end + 1;
    }
    out.push_str(&sql[cursor..]);
    out
}

/// Apply all embedded migrations against the adapter's pool.
///
/// Async entrypoint — use [`apply_migrations_blocking`] from synchronous
/// callers (e.g. `Database::open` which stays sync to keep its API stable
/// across the migration cluster).
pub async fn apply_migrations(adapter: &DbAdapter) -> Result<(), sqlx::migrate::MigrateError> {
    let backend = adapter.backend();
    let source = DialectAwareMigrationSource::for_backend(backend);
    let migrator = Migrator::new(source).await.map_err(|e| match e {
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
        // SAFETY: `join()` only returns `Err` when the spawned thread itself
        // panicked. Re-propagating that panic preserves the original migration
        // failure (and its backtrace) instead of masking it behind a synthetic
        // error variant; the migration body returns its errors via the inner
        // `Result`, which is what this function forwards on the success path.
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
        assert!(
            !got.contains("BLOB"),
            "PG output still contains BLOB: {got}"
        );
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
    fn translate_mysql_widens_text_columns_used_in_keys() {
        // Three columns participate in indexes / UNIQUE constraints:
        // `repo_url`, `expires_at`, and the bare `at` column on
        // `credential_audit`. MySQL rejects TEXT in a key spec, so
        // the transform widens them to VARCHAR. SQLite + Postgres
        // keep TEXT.
        let input = "\
            CREATE TABLE user_repositories (\n\
                repo_url TEXT NOT NULL,\n\
                UNIQUE(user_id, repo_url)\n\
            );\n\
            CREATE TABLE sessions (\n\
                expires_at TEXT NOT NULL\n\
            );\n\
            CREATE TABLE credential_audit (\n\
                created_at TEXT NOT NULL DEFAULT '',\n\
                updated_at TEXT NOT NULL DEFAULT '',\n\
                at TEXT NOT NULL DEFAULT ''\n\
            );\n";
        let got = translate_for_backend(input, DbBackend::MySql);
        assert!(got.contains("repo_url VARCHAR(512) NOT NULL"), "{got}");
        assert!(got.contains("expires_at VARCHAR(64) NOT NULL"), "{got}");
        // Defaults get wrapped in parens by the unconditional
        // `wrap_text_defaults_for_mysql` pass, so the post-transform
        // shape is `DEFAULT ('')`.
        assert!(
            got.contains("at VARCHAR(64) NOT NULL DEFAULT ('')"),
            "the bare `at` column must be widened, got: {got}"
        );
        // The whole-word boundary check protects every other `_at`
        // column — they must keep TEXT.
        assert!(
            got.contains("created_at TEXT NOT NULL DEFAULT ('')"),
            "created_at must NOT be widened (boundary mismatch), got: {got}"
        );
        assert!(
            got.contains("updated_at TEXT NOT NULL DEFAULT ('')"),
            "updated_at must NOT be widened, got: {got}"
        );

        // SQLite + Postgres keep the original.
        let sqlite = translate_for_backend(input, DbBackend::Sqlite);
        assert!(sqlite.contains("repo_url TEXT NOT NULL"), "{sqlite}");
        assert!(sqlite.contains("at TEXT NOT NULL DEFAULT ''"));
        let pg = translate_for_backend(input, DbBackend::Postgres);
        assert!(pg.contains("repo_url TEXT NOT NULL"), "{pg}");
        assert!(pg.contains("at TEXT NOT NULL DEFAULT ''"));
    }

    #[test]
    fn translate_mysql_strips_index_if_not_exists() {
        let input =
            "CREATE INDEX IF NOT EXISTS idx_foo ON foo(bar);\nDROP INDEX IF EXISTS idx_bar;";
        let got = translate_for_backend(input, DbBackend::MySql);
        assert!(
            got.contains("CREATE INDEX idx_foo ON foo(bar)"),
            "MySQL must strip `IF NOT EXISTS` from CREATE INDEX, got: {got}"
        );
        assert!(
            got.contains("DROP INDEX idx_bar"),
            "MySQL must strip `IF EXISTS` from DROP INDEX, got: {got}"
        );
        assert!(!got.contains("IF NOT EXISTS"), "{got}");
        assert!(!got.contains("IF EXISTS"), "{got}");
    }

    #[test]
    fn translate_mysql_wraps_text_default_literals() {
        // MySQL 8.0.13+ requires `DEFAULT ('...')` for TEXT/BLOB/JSON
        // columns; bare `DEFAULT '...'` is rejected.
        let input = "CREATE TABLE t (\
            metadata TEXT NOT NULL DEFAULT '{}', \
            note TEXT NOT NULL DEFAULT ''\
        );";
        let got = translate_for_backend(input, DbBackend::MySql);
        assert!(
            got.contains("DEFAULT ('{}')"),
            "MySQL must wrap '{{}}' default, got: {got}"
        );
        assert!(
            got.contains("DEFAULT ('')"),
            "MySQL must wrap empty default, got: {got}"
        );
        // The original bare forms must be gone.
        assert!(!got.contains("DEFAULT '{}'"), "{got}");
        assert!(!got.contains("DEFAULT ''"), "{got}");
    }

    #[test]
    fn translate_mysql_passes_already_wrapped_defaults_through() {
        // If a future migration source uses the parenthesised form, the
        // transformer must not double-wrap it.
        let input = "CREATE TABLE t (s TEXT NOT NULL DEFAULT ('hi'));";
        let got = translate_for_backend(input, DbBackend::MySql);
        assert!(
            got.contains("DEFAULT ('hi')"),
            "must pass parenthesised default through, got: {got}"
        );
        assert!(
            !got.contains("DEFAULT (('hi'))"),
            "must not double-wrap, got: {got}"
        );
    }

    #[test]
    fn translate_sqlite_leaves_text_defaults_alone() {
        // Postgres/SQLite accept bare literal defaults on TEXT — the
        // MySQL-specific wrap must NOT fire for them.
        let input = "CREATE TABLE t (s TEXT NOT NULL DEFAULT '');";
        let sqlite = translate_for_backend(input, DbBackend::Sqlite);
        let postgres = translate_for_backend(input, DbBackend::Postgres);
        assert!(sqlite.contains("DEFAULT ''"), "SQLite: {sqlite}");
        assert!(postgres.contains("DEFAULT ''"), "Postgres: {postgres}");
        assert!(
            !sqlite.contains("DEFAULT ('')"),
            "SQLite over-wrapped: {sqlite}"
        );
        assert!(
            !postgres.contains("DEFAULT ('')"),
            "PG over-wrapped: {postgres}"
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
        assert!(
            got.contains("my_blob_col"),
            "identifier must survive: {got}"
        );
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
    fn embedded_migration_count() {
        // V1..V6 from the legacy schema.rs port, V7 = importer's
        // system_metadata, V8 = work_items, V9 = repository_id on
        // work_items. Bump when adding new migrations.
        assert_eq!(MIGRATIONS.len(), 9);
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
                upper.contains("CREATE") || upper.contains("ALTER") || upper.contains("DROP"),
                "migration {} ({}) does not appear to do any DDL",
                m.version,
                m.description
            );
        }
    }

    // ── End-to-end: run all migrations on in-memory SQLite ─────────────

    /// The full migration set applies cleanly via sqlx
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
            "system_metadata",
            // Work-item state tables.
            "work_items",
            "work_item_steps",
            "work_item_definition_runs",
            "work_item_log_lines",
            "work_item_port_mappings",
            "work_item_run_commands",
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

        let count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM _sqlx_migrations")
            .fetch_one(&pool)
            .await
            .expect("count rows");
        assert_eq!(
            count.0, 9,
            "expected 9 applied migrations recorded by sqlx, got {}",
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
            assert_eq!(
                got, m.sql,
                "SQLite output differs from source for {}",
                m.description
            );
        }
    }

    // ── Postgres / MySQL — ignored unless a container DSN is provided ───
    //
    // The CI matrix sets `DATABASE_URL` for these tests. Until then they
    // are `#[ignore]` so local `cargo test` stays fast and offline. To run
    // them locally:
    //   cargo test --lib db::migrate -- --ignored
    // with `DATABASE_URL=postgres://...` (or `mysql://...`) exported.

    /// Skip the test body when `DATABASE_URL` is unset or points at a
    /// different backend. Returns `true` when the test should proceed.
    /// Lets the matrix CI run a single `cargo test --ignored` command
    /// per job and have each test self-gate to the right backend
    /// without relying on substring filters.
    fn database_url_matches(expected_scheme: &str) -> Option<String> {
        let url = std::env::var("DATABASE_URL").ok()?;
        if url.starts_with(&format!("{expected_scheme}://")) {
            Some(url)
        } else {
            eprintln!("skipping: DATABASE_URL scheme is not {expected_scheme}:// (got {url})");
            None
        }
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL=postgres://..."]
    async fn migrations_apply_clean_to_postgres() {
        let Some(url) = database_url_matches("postgres") else {
            return;
        };
        let pool = sqlx::postgres::PgPool::connect(&url)
            .await
            .expect("connect");
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
        let Some(url) = database_url_matches("mysql") else {
            return;
        };
        let pool = sqlx::mysql::MySqlPool::connect(&url)
            .await
            .expect("connect");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::MySql);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations against MySQL");
    }

    /// End-to-end smoke against an external backend: drives the entire
    /// production stack — `pool::connect` → `DbAdapter` →
    /// `apply_migrations` → DAOs (`users::create_user` /
    /// `get_user_by_id` / `list_admins` / `delete_user`). Catches issues
    /// the migration-only tests above can't see: the `?` → `$N`
    /// placeholder rewrite, dialect-aware ON CONFLICT upserts, the
    /// `i64` ↔ INT4 widening on Postgres (`users.suspended`), transaction
    /// scopes, decode paths for every column type.
    ///
    /// Postgres only — MySQL has its own follow-up cluster.
    ///
    /// Idempotent across reruns: usernames carry a UUID suffix so
    /// repeated `cargo test` invocations against the same Postgres
    /// instance don't collide on the UNIQUE constraint.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL=postgres://..."]
    async fn postgres_crud_smoke_via_adapter() {
        use crate::db::adapter::DbAdapter;
        use crate::db::models::UserRole;
        use crate::db::{pool, users};

        let Some(url) = database_url_matches("postgres") else {
            return;
        };

        let backend_pool = pool::connect(&url, &pool::PoolTuning::default())
            .await
            .expect("connect via pool::connect");
        let adapter = DbAdapter::new(backend_pool);

        super::apply_migrations(&adapter)
            .await
            .expect("apply migrations against Postgres");

        // Regular user round-trip — exercises the `?` → `$N` rewrite on
        // INSERT and the i64-widening on `suspended` (INTEGER → INT4)
        // through `get_user_by_id`.
        let username = format!("ci_smoke_{}", uuid::Uuid::new_v4());
        let created = users::create_user(&adapter, &username, UserRole::User)
            .await
            .expect("users::create_user against Postgres");

        let fetched = users::get_user_by_id(&adapter, &created.id)
            .await
            .expect("users::get_user_by_id round-trip")
            .expect("user must exist immediately after create_user");

        assert_eq!(fetched.username, username);
        assert_eq!(fetched.id, created.id);
        assert!(
            !fetched.suspended,
            "freshly-created user must not be suspended"
        );

        // Admin round-trip plus `list_admins`. `list_admins` is the
        // SELECT path the poller-owner resolver uses at boot — it was
        // the first failure post-import before the i64-widening fix,
        // because it reads `users.suspended` (INT4 on Postgres) via
        // `get_i64`. Pin it here so any future regression of the
        // widening fallback is caught.
        let admin_username = format!("ci_smoke_admin_{}", uuid::Uuid::new_v4());
        let admin = users::create_user(&adapter, &admin_username, UserRole::Admin)
            .await
            .expect("create admin");
        let admins = users::list_admins(&adapter)
            .await
            .expect("list_admins after creating an admin (i64-widening of suspended)");
        assert!(
            admins.iter().any(|u| u.id == admin.id),
            "list_admins must include the just-created admin (got {} admins): {:?}",
            admins.len(),
            admins.iter().map(|u| &u.username).collect::<Vec<_>>()
        );

        // Best-effort cleanup so the DB stays tidy across reruns. A
        // failure here is non-fatal — we want the assertions above to
        // be the test's verdict, not the housekeeping.
        let _ = users::delete_user(&adapter, &created.id).await;
        let _ = users::delete_user(&adapter, &admin.id).await;
    }

    /// Importer round-trip against a real Postgres target. Catches
    /// importer-specific bugs the simple CRUD smoke doesn't reach:
    ///
    ///   - NULL TEXT cells must arrive as SQL NULL on the target
    ///     (not as the empty string). Before the
    ///     `ValueRef::is_null()` check, NULL `created_by` would write
    ///     `''`, then trip the FK constraint.
    ///   - Within a single SQL string, mixing NULL and non-NULL bind
    ///     types would pin sqlx-postgres's prepared-statement parameter
    ///     types on first call and reject the second. The fix inlines
    ///     `NULL` literals into per-row SQL; this test exercises both
    ///     a NULL-FK row and a non-NULL-FK row in the same table so
    ///     the bug would re-surface immediately if anyone reverts.
    ///
    /// The test cleans every importer-touched table on entry so it's
    /// re-runnable against the same Postgres instance.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL=postgres://..."]
    async fn postgres_importer_handles_null_and_non_null_fks() {
        use crate::db::adapter::DbAdapter;
        use crate::db::models::UserRole;
        use crate::db::{DbValue, importer, pool, repositories, users};

        let Some(url) = database_url_matches("postgres") else {
            return;
        };

        let backend_pool = pool::connect(&url, &pool::PoolTuning::default())
            .await
            .expect("connect");
        let target = DbAdapter::new(backend_pool);
        super::apply_migrations(&target)
            .await
            .expect("migrate target");

        // Wipe importer-touched tables so the test starts from zero
        // and is re-runnable. `TRUNCATE ... RESTART IDENTITY CASCADE`
        // resets the autoincrement counters and cascades through every
        // FK. `system_metadata` carries the `import_complete` marker
        // and must go too.
        target
            .execute(
                "TRUNCATE \
                    users, credentials, recovery_codes, sessions, login_attempts, \
                    container_users, user_worktree_commands, repositories, \
                    user_repositories, user_provider_credentials, \
                    user_github_credentials, credential_audit, onboarding_state, \
                    system_metadata \
                 RESTART IDENTITY CASCADE",
                vec![],
            )
            .await
            .expect("wipe target tables");

        // Build a source SQLite Database with two users and two
        // repositories — one with `created_by = NULL`, one with a
        // valid FK to the admin.
        let src_tmp = tempfile::tempdir().expect("source tempdir");
        let source = crate::db::Database::open(src_tmp.path(), true).expect("open source SQLite");
        let admin = users::create_user(source.adapter(), "ci_imp_admin", UserRole::Admin)
            .await
            .expect("seed admin");
        users::create_user(source.adapter(), "ci_imp_alice", UserRole::User)
            .await
            .expect("seed alice");
        let null_fk_id = repositories::upsert(
            source.adapter(),
            "ci-imp-null-fk",
            None,
            "/tmp/ci-imp-null-fk",
            "main",
            None, // <-- NULL created_by, the bug-3/4 trigger
        )
        .await
        .expect("seed null-FK repo");
        let valid_fk_id = repositories::upsert(
            source.adapter(),
            "ci-imp-valid-fk",
            None,
            "/tmp/ci-imp-valid-fk",
            "main",
            Some(&admin.id),
        )
        .await
        .expect("seed valid-FK repo");

        // Run the importer.
        let copied = importer::import_from_sqlite(src_tmp.path(), &target)
            .await
            .expect("importer must succeed against Postgres target");
        assert!(
            copied >= 4,
            "expected ≥ 4 rows imported (2 users + 2 repos), got {copied}"
        );

        // Marker was set.
        assert!(
            importer::import_already_complete(&target).await.unwrap(),
            "system_metadata.import_complete must be set after a successful import"
        );

        // Both users round-trip.
        let admins_on_target = users::list_admins(&target)
            .await
            .expect("list_admins on target after import");
        assert!(
            admins_on_target
                .iter()
                .any(|u| u.username == "ci_imp_admin"),
            "imported admin must show up in list_admins: {:?}",
            admins_on_target
                .iter()
                .map(|u| &u.username)
                .collect::<Vec<_>>()
        );

        // The NULL-FK repo arrived with NULL (not "") in created_by —
        // i.e. no junk empty-string FK that would have tripped on the
        // way in.
        let null_fk_created_by: Option<String> = {
            let row = target
                .query_one(
                    "SELECT created_by FROM repositories WHERE id = ?",
                    vec![DbValue::Text(null_fk_id.clone())],
                )
                .await
                .expect("read null-FK repo");
            row.get_text_opt(0).expect("decode created_by")
        };
        assert!(
            null_fk_created_by.is_none(),
            "NULL-FK repo's created_by must arrive as NULL, got: {null_fk_created_by:?}"
        );

        // The valid-FK repo arrived with the right user id.
        let valid_fk_created_by: Option<String> = {
            let row = target
                .query_one(
                    "SELECT created_by FROM repositories WHERE id = ?",
                    vec![DbValue::Text(valid_fk_id.clone())],
                )
                .await
                .expect("read valid-FK repo");
            row.get_text_opt(0).expect("decode created_by")
        };
        assert_eq!(
            valid_fk_created_by.as_deref(),
            Some(admin.id.as_str()),
            "valid-FK repo must keep its created_by reference"
        );
    }

    /// End-to-end smoke against a real MySQL/MariaDB service container.
    /// Mirrors [`postgres_crud_smoke_via_adapter`] — runs the full
    /// production stack against the MySQL dialect to catch upsert,
    /// migration-translation, and DAO issues the SQLite tests can't see.
    #[tokio::test]
    #[ignore = "requires DATABASE_URL=mysql://..."]
    async fn mysql_crud_smoke_via_adapter() {
        use crate::db::adapter::DbAdapter;
        use crate::db::models::UserRole;
        use crate::db::{pool, users};

        let Some(url) = database_url_matches("mysql") else {
            return;
        };

        let backend_pool = pool::connect(&url, &pool::PoolTuning::default())
            .await
            .expect("connect via pool::connect");
        let adapter = DbAdapter::new(backend_pool);

        super::apply_migrations(&adapter)
            .await
            .expect("apply migrations against MySQL");

        let username = format!("ci_smoke_{}", uuid::Uuid::new_v4());
        let created = users::create_user(&adapter, &username, UserRole::User)
            .await
            .expect("users::create_user against MySQL");

        let fetched = users::get_user_by_id(&adapter, &created.id)
            .await
            .expect("users::get_user_by_id round-trip")
            .expect("user must exist immediately after create_user");

        assert_eq!(fetched.username, username);
        assert_eq!(fetched.id, created.id);

        // Best-effort cleanup.
        let _ = users::delete_user(&adapter, &created.id).await;
    }
}
