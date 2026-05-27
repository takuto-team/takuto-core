// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-11 step 3 — backend-agnostic database adapter.
//!
//! Source: `tmp/plan-11-pluggable-database-backends.md` §4. Caller mandate
//! (2026-05-27): "all database calls are made through an agnostic adapter
//! that is in charge to use the right DB driver."
//!
//! Hides the [`DbPool`] enum from every DAO + call site. The adapter exposes
//! four query operations (`execute`, `query_one`, `query_optional`,
//! `query_all`) plus `begin()` for transactions, and dispatches internally
//! to the per-backend sqlx pool. SQL is written with `?` placeholders (sqlx
//! auto-rewrites to `$N` for Postgres). Params and rows flow through
//! type-erased [`DbValue`] / [`DbRow`] containers so call sites never
//! mention `sqlx::sqlite::SqlitePool` etc.
//!
//! Why not `sqlx::Any` (the trait sqlx ships for this exact problem): see
//! plan §4. `Any` upcasts results to dynamic types and silently masks
//! per-backend syntax differences. We pay a small dispatch cost (one
//! `match` per query) in exchange for keeping per-backend bind support and
//! catching dialect issues at well-defined points.
//!
//! ### What lives where
//! - [`DbAdapter`] — wraps a [`DbPool`]; entry point for every DAO.
//! - [`DbTransaction`] — RAII transaction handle; commit or rollback.
//! - [`DbValue`] — type-erased bind parameter.
//! - [`DbRow`] — type-erased fetched row with `get_*` accessors.
//! - [`DbError`] — adapter-layer typed errors. Wraps `sqlx::Error`.
//!
//! ### Placeholder convention
//! All SQL uses `?` placeholders. sqlx's Postgres driver rewrites them to
//! `$1..$N` at execution time, so the same SQL works on every backend
//! supported by the adapter. The rare per-backend cases (`ON CONFLICT`,
//! `RETURNING`, etc.) inspect `adapter.backend()` and pick the right SQL
//! at the call site.

use std::borrow::Cow;

use sqlx::{Column, Row, TypeInfo, ValueRef};

use super::pool::{DbBackend, DbPool};

// ── Types ────────────────────────────────────────────────────────────────

/// Type-erased database value for binding into a query. The variants cover
/// every column type Maestro ever stores. Null-of-type variants exist where
/// the corresponding non-null type isn't already nullable in Rust (e.g.
/// `Bytes` vs `NullBytes`); use `Null` when you don't care what SQL type
/// the null is coerced to.
#[derive(Debug, Clone, PartialEq)]
pub enum DbValue {
    /// Untyped NULL. The driver binds as the SQL NULL of whatever type the
    /// column expects; equivalent to `Option::<T>::None` in sqlx terms.
    Null,
    Bool(bool),
    I32(i32),
    I64(i64),
    F64(f64),
    Text(String),
    Bytes(Vec<u8>),
    /// `NULL` or text. Maps to `Option<String>` for sqlx binding.
    TextOpt(Option<String>),
    /// `NULL` or i64. Maps to `Option<i64>`.
    I64Opt(Option<i64>),
    /// `NULL` or i32. Maps to `Option<i32>`.
    I32Opt(Option<i32>),
    /// `NULL` or bytes. Maps to `Option<Vec<u8>>`.
    BytesOpt(Option<Vec<u8>>),
}

impl From<&str> for DbValue {
    fn from(s: &str) -> Self {
        DbValue::Text(s.to_string())
    }
}
impl From<String> for DbValue {
    fn from(s: String) -> Self {
        DbValue::Text(s)
    }
}
impl From<&String> for DbValue {
    fn from(s: &String) -> Self {
        DbValue::Text(s.clone())
    }
}
impl From<i64> for DbValue {
    fn from(v: i64) -> Self {
        DbValue::I64(v)
    }
}
impl From<i32> for DbValue {
    fn from(v: i32) -> Self {
        DbValue::I32(v)
    }
}
impl From<bool> for DbValue {
    fn from(v: bool) -> Self {
        DbValue::Bool(v)
    }
}
impl From<f64> for DbValue {
    fn from(v: f64) -> Self {
        DbValue::F64(v)
    }
}
impl From<Vec<u8>> for DbValue {
    fn from(v: Vec<u8>) -> Self {
        DbValue::Bytes(v)
    }
}
impl From<&[u8]> for DbValue {
    fn from(v: &[u8]) -> Self {
        DbValue::Bytes(v.to_vec())
    }
}
impl From<Option<String>> for DbValue {
    fn from(v: Option<String>) -> Self {
        DbValue::TextOpt(v)
    }
}
impl From<Option<i64>> for DbValue {
    fn from(v: Option<i64>) -> Self {
        DbValue::I64Opt(v)
    }
}
impl From<Option<i32>> for DbValue {
    fn from(v: Option<i32>) -> Self {
        DbValue::I32Opt(v)
    }
}
impl From<Option<Vec<u8>>> for DbValue {
    fn from(v: Option<Vec<u8>>) -> Self {
        DbValue::BytesOpt(v)
    }
}

/// Adapter-layer error. Sits below `MaestroError` (the crate-wide envelope)
/// so DAOs can match on specific failure modes without depending on the
/// top-level error type.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// sqlx returned an error. Use the underlying `source` to discriminate
    /// (e.g. `sqlx::Error::RowNotFound` for missing rows, `Database` for
    /// constraint violations).
    #[error("database error: {source}")]
    Sqlx {
        #[from]
        source: sqlx::Error,
    },
    /// Asked for a column by index that doesn't exist on this row.
    #[error("column index {idx} out of range (row has {len} columns)")]
    ColumnOutOfRange { idx: usize, len: usize },
    /// Asked for a column by name that isn't in this row.
    #[error("column '{name}' not found in row")]
    ColumnNotFound { name: String },
    /// A value extracted from a row couldn't be converted to the requested
    /// Rust type. Carries the underlying sqlx decode error.
    #[error("decode column {column}: {detail}")]
    Decode { column: String, detail: String },
}

impl DbError {
    /// True when the underlying sqlx error reports no row matched a query
    /// that expected one. DAOs use this to convert "no rows" into typed
    /// `Option::None` results.
    pub fn is_row_not_found(&self) -> bool {
        matches!(
            self,
            DbError::Sqlx {
                source: sqlx::Error::RowNotFound,
            }
        )
    }
}

/// Adapter result alias used throughout the DAO layer.
pub type DbResult<T> = Result<T, DbError>;

// ── DbRow ────────────────────────────────────────────────────────────────

/// A fetched row, type-erased so DAOs never name `SqliteRow` / `PgRow` /
/// `MySqlRow`. Provides typed `get_*` accessors for every type Maestro
/// stores.
///
/// Indexes are zero-based. Column names match the `SELECT` clause as
/// reported by the driver (case-sensitive on Postgres / MySQL when
/// quoted; case-folded for unquoted identifiers per the SQL standard).
pub struct DbRow {
    inner: DbRowInner,
}

enum DbRowInner {
    Sqlite(sqlx::sqlite::SqliteRow),
    Postgres(sqlx::postgres::PgRow),
    MySql(sqlx::mysql::MySqlRow),
}

impl std::fmt::Debug for DbRow {
    /// Render only the backend + column count. Row values can carry
    /// secrets (envelope-sealed credentials, GitHub tokens) so the
    /// default Debug must not leak them.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let backend = match &self.inner {
            DbRowInner::Sqlite(_) => "sqlite",
            DbRowInner::Postgres(_) => "postgres",
            DbRowInner::MySql(_) => "mysql",
        };
        write!(f, "DbRow({}, columns={})", backend, self.len())
    }
}

impl DbRow {
    fn from_sqlite(row: sqlx::sqlite::SqliteRow) -> Self {
        Self {
            inner: DbRowInner::Sqlite(row),
        }
    }
    fn from_postgres(row: sqlx::postgres::PgRow) -> Self {
        Self {
            inner: DbRowInner::Postgres(row),
        }
    }
    fn from_mysql(row: sqlx::mysql::MySqlRow) -> Self {
        Self {
            inner: DbRowInner::MySql(row),
        }
    }

    /// Column count.
    pub fn len(&self) -> usize {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.columns().len(),
            DbRowInner::Postgres(r) => r.columns().len(),
            DbRowInner::MySql(r) => r.columns().len(),
        }
    }

    /// True when the row carries zero columns. Useful for empty-query
    /// sentinels (returned by `RETURNING` with no projection).
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Get a `String` from column index. NULL → returns
    /// [`DbError::Decode`] (use [`Self::get_text_opt`] for nullable
    /// columns).
    pub fn get_text(&self, idx: usize) -> DbResult<String> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<String, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<String, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<String, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get an `Option<String>` from column index. Both `NULL` and missing
    /// columns surface as `None` vs `Err` per `try_get_optional`'s
    /// contract on each driver.
    pub fn get_text_opt(&self, idx: usize) -> DbResult<Option<String>> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<Option<String>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<Option<String>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<Option<String>, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get an `i64` from column index.
    pub fn get_i64(&self, idx: usize) -> DbResult<i64> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<i64, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<i64, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<i64, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get an `Option<i64>` from column index.
    pub fn get_i64_opt(&self, idx: usize) -> DbResult<Option<i64>> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<Option<i64>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<Option<i64>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<Option<i64>, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get an `i32` from column index.
    pub fn get_i32(&self, idx: usize) -> DbResult<i32> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<i32, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<i32, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<i32, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get a `bool` from column index. Backend rules:
    ///   • SQLite: 0/1 INTEGER round-trip as `bool` via sqlx.
    ///   • Postgres: native `BOOLEAN`.
    ///   • MySQL: `TINYINT(1)` is the canonical bool storage.
    pub fn get_bool(&self, idx: usize) -> DbResult<bool> {
        match &self.inner {
            // SQLite and MySQL: bool is stored as int; sqlx will decode 0/1
            // from INTEGER / TINYINT columns into bool. Try bool first; if
            // the column is stored as a wider int, fall back to i64 → bool.
            DbRowInner::Sqlite(r) => match r.try_get::<bool, _>(idx) {
                Ok(b) => Ok(b),
                Err(_) => r
                    .try_get::<i64, _>(idx)
                    .map(|v| v != 0)
                    .map_err(decode_err(idx)),
            },
            DbRowInner::Postgres(r) => r.try_get::<bool, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => match r.try_get::<bool, _>(idx) {
                Ok(b) => Ok(b),
                Err(_) => r
                    .try_get::<i64, _>(idx)
                    .map(|v| v != 0)
                    .map_err(decode_err(idx)),
            },
        }
    }

    /// Get bytes from column index. The transformer maps `BLOB` to `BYTEA`
    /// for Postgres in migration files, so the column type matches what
    /// each backend's driver hands back for `Vec<u8>`.
    pub fn get_bytes(&self, idx: usize) -> DbResult<Vec<u8>> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<Vec<u8>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<Vec<u8>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<Vec<u8>, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get optional bytes from column index.
    pub fn get_bytes_opt(&self, idx: usize) -> DbResult<Option<Vec<u8>>> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<Option<Vec<u8>>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<Option<Vec<u8>>, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<Option<Vec<u8>>, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// Get `f64` from column index.
    pub fn get_f64(&self, idx: usize) -> DbResult<f64> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.try_get::<f64, _>(idx).map_err(decode_err(idx)),
            DbRowInner::Postgres(r) => r.try_get::<f64, _>(idx).map_err(decode_err(idx)),
            DbRowInner::MySql(r) => r.try_get::<f64, _>(idx).map_err(decode_err(idx)),
        }
    }

    /// True when the column at `idx` is SQL NULL. Cheaper than calling a
    /// typed accessor and inspecting `_opt` variants.
    pub fn is_null(&self, idx: usize) -> bool {
        match &self.inner {
            DbRowInner::Sqlite(r) => r
                .try_get_raw(idx)
                .map(|raw| raw.is_null())
                .unwrap_or(false),
            DbRowInner::Postgres(r) => r
                .try_get_raw(idx)
                .map(|raw| raw.is_null())
                .unwrap_or(false),
            DbRowInner::MySql(r) => r
                .try_get_raw(idx)
                .map(|raw| raw.is_null())
                .unwrap_or(false),
        }
    }

    /// The SQL type name for column `idx` as reported by the driver
    /// (`"TEXT"`, `"INTEGER"`, `"BYTEA"`, …). Useful for diagnostics; do
    /// not pattern-match on it from production code — names differ across
    /// backends.
    pub fn column_type_name(&self, idx: usize) -> Option<Cow<'_, str>> {
        match &self.inner {
            DbRowInner::Sqlite(r) => r.columns().get(idx).map(|c| Cow::Borrowed(c.type_info().name())),
            DbRowInner::Postgres(r) => r.columns().get(idx).map(|c| Cow::Borrowed(c.type_info().name())),
            DbRowInner::MySql(r) => r.columns().get(idx).map(|c| Cow::Borrowed(c.type_info().name())),
        }
    }
}

fn decode_err(idx: usize) -> impl FnOnce(sqlx::Error) -> DbError {
    move |e| DbError::Decode {
        column: format!("#{idx}"),
        detail: e.to_string(),
    }
}

// ── Binding helper ───────────────────────────────────────────────────────

/// Binds a `Vec<DbValue>` onto a sqlx query builder of the right
/// backend. Defined as a macro because the three pool/builder types are
/// distinct concrete types — a generic function would require
/// `sqlx::Database`-aware bounds we deliberately don't want at the
/// adapter call site.
macro_rules! bind_params {
    ($query:ident, $params:expr) => {{
        for v in $params {
            $query = match v {
                DbValue::Null => $query.bind::<Option<i32>>(None),
                DbValue::Bool(b) => $query.bind(b),
                DbValue::I32(i) => $query.bind(i),
                DbValue::I64(i) => $query.bind(i),
                DbValue::F64(f) => $query.bind(f),
                DbValue::Text(s) => $query.bind(s),
                DbValue::Bytes(b) => $query.bind(b),
                DbValue::TextOpt(s) => $query.bind(s),
                DbValue::I64Opt(i) => $query.bind(i),
                DbValue::I32Opt(i) => $query.bind(i),
                DbValue::BytesOpt(b) => $query.bind(b),
            };
        }
        $query
    }};
}

// ── DbAdapter ────────────────────────────────────────────────────────────

/// Backend-agnostic database adapter. Construct via [`DbAdapter::new`]
/// from a [`DbPool`]; pass `&DbAdapter` to every DAO.
#[derive(Clone)]
pub struct DbAdapter {
    pool: DbPool,
}

impl std::fmt::Debug for DbAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DbAdapter({})", self.pool.backend())
    }
}

impl DbAdapter {
    /// Wrap a [`DbPool`]. Cheap — the pool itself is clone-cheap (Arc'd).
    pub fn new(pool: DbPool) -> Self {
        Self { pool }
    }

    /// Which backend is on the other end. Call sites use this only for
    /// the rare per-backend SQL escape hatch (`ON CONFLICT`, `RETURNING`).
    pub fn backend(&self) -> DbBackend {
        self.pool.backend()
    }

    /// Borrow the underlying pool. Reserved for the importer and any
    /// future helper that genuinely needs the typed pool (per plan §4,
    /// "the importer path" uses a backend-specific pool reference).
    /// **Production DAOs MUST go through `execute` / `query_*`.**
    pub fn pool(&self) -> &DbPool {
        &self.pool
    }

    /// Run a connectivity check. Wraps `DbPool::ping`.
    pub async fn ping(&self) -> Result<(), sqlx::Error> {
        self.pool.ping().await
    }

    /// Execute a statement that doesn't return rows. Returns rows
    /// affected. SQL uses `?` placeholders (sqlx rewrites to `$N` for
    /// Postgres).
    pub async fn execute(&self, sql: &str, params: Vec<DbValue>) -> DbResult<u64> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let res = q.execute(p).await?;
                Ok(res.rows_affected())
            }
            DbPool::Postgres(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let res = q.execute(p).await?;
                Ok(res.rows_affected())
            }
            DbPool::MySql(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let res = q.execute(p).await?;
                Ok(res.rows_affected())
            }
        }
    }

    /// Fetch exactly one row. Returns `DbError::Sqlx { source:
    /// RowNotFound }` if zero rows match. DAOs that expect "0 or 1 row"
    /// should use [`Self::query_optional`] instead.
    pub async fn query_one(&self, sql: &str, params: Vec<DbValue>) -> DbResult<DbRow> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_one(p).await?;
                Ok(DbRow::from_sqlite(row))
            }
            DbPool::Postgres(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_one(p).await?;
                Ok(DbRow::from_postgres(row))
            }
            DbPool::MySql(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_one(p).await?;
                Ok(DbRow::from_mysql(row))
            }
        }
    }

    /// Fetch zero or one row.
    pub async fn query_optional(
        &self,
        sql: &str,
        params: Vec<DbValue>,
    ) -> DbResult<Option<DbRow>> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_optional(p).await?;
                Ok(row.map(DbRow::from_sqlite))
            }
            DbPool::Postgres(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_optional(p).await?;
                Ok(row.map(DbRow::from_postgres))
            }
            DbPool::MySql(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_optional(p).await?;
                Ok(row.map(DbRow::from_mysql))
            }
        }
    }

    /// Fetch all matching rows. Use for small/bounded result sets; for
    /// large scans, add a streaming variant later.
    pub async fn query_all(&self, sql: &str, params: Vec<DbValue>) -> DbResult<Vec<DbRow>> {
        match &self.pool {
            DbPool::Sqlite(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let rows = q.fetch_all(p).await?;
                Ok(rows.into_iter().map(DbRow::from_sqlite).collect())
            }
            DbPool::Postgres(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let rows = q.fetch_all(p).await?;
                Ok(rows.into_iter().map(DbRow::from_postgres).collect())
            }
            DbPool::MySql(p) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let rows = q.fetch_all(p).await?;
                Ok(rows.into_iter().map(DbRow::from_mysql).collect())
            }
        }
    }

    /// Begin a transaction. The returned [`DbTransaction`] must be
    /// `commit()`ed or `rollback()`ed before drop; sqlx will roll back
    /// implicitly on drop (which surfaces as a `tracing::warn!`).
    pub async fn begin(&self) -> DbResult<DbTransaction<'_>> {
        let inner = match &self.pool {
            DbPool::Sqlite(p) => DbTxInner::Sqlite(p.begin().await?),
            DbPool::Postgres(p) => DbTxInner::Postgres(p.begin().await?),
            DbPool::MySql(p) => DbTxInner::MySql(p.begin().await?),
        };
        Ok(DbTransaction { inner })
    }
}

// ── Transactions ─────────────────────────────────────────────────────────

/// RAII transaction handle. `commit()` or `rollback()` explicitly; the
/// `Drop` impl rolls back if neither was called.
pub struct DbTransaction<'a> {
    inner: DbTxInner<'a>,
}

enum DbTxInner<'a> {
    Sqlite(sqlx::Transaction<'a, sqlx::Sqlite>),
    Postgres(sqlx::Transaction<'a, sqlx::Postgres>),
    MySql(sqlx::Transaction<'a, sqlx::MySql>),
}

impl<'a> DbTransaction<'a> {
    /// Execute a statement that doesn't return rows. Identical contract
    /// to [`DbAdapter::execute`] but bound to this transaction.
    pub async fn execute(&mut self, sql: &str, params: Vec<DbValue>) -> DbResult<u64> {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let res = q.execute(&mut **tx).await?;
                Ok(res.rows_affected())
            }
            DbTxInner::Postgres(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let res = q.execute(&mut **tx).await?;
                Ok(res.rows_affected())
            }
            DbTxInner::MySql(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let res = q.execute(&mut **tx).await?;
                Ok(res.rows_affected())
            }
        }
    }

    pub async fn query_one(&mut self, sql: &str, params: Vec<DbValue>) -> DbResult<DbRow> {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_one(&mut **tx).await?;
                Ok(DbRow::from_sqlite(row))
            }
            DbTxInner::Postgres(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_one(&mut **tx).await?;
                Ok(DbRow::from_postgres(row))
            }
            DbTxInner::MySql(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_one(&mut **tx).await?;
                Ok(DbRow::from_mysql(row))
            }
        }
    }

    pub async fn query_optional(
        &mut self,
        sql: &str,
        params: Vec<DbValue>,
    ) -> DbResult<Option<DbRow>> {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_optional(&mut **tx).await?;
                Ok(row.map(DbRow::from_sqlite))
            }
            DbTxInner::Postgres(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_optional(&mut **tx).await?;
                Ok(row.map(DbRow::from_postgres))
            }
            DbTxInner::MySql(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let row = q.fetch_optional(&mut **tx).await?;
                Ok(row.map(DbRow::from_mysql))
            }
        }
    }

    pub async fn query_all(&mut self, sql: &str, params: Vec<DbValue>) -> DbResult<Vec<DbRow>> {
        match &mut self.inner {
            DbTxInner::Sqlite(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let rows = q.fetch_all(&mut **tx).await?;
                Ok(rows.into_iter().map(DbRow::from_sqlite).collect())
            }
            DbTxInner::Postgres(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let rows = q.fetch_all(&mut **tx).await?;
                Ok(rows.into_iter().map(DbRow::from_postgres).collect())
            }
            DbTxInner::MySql(tx) => {
                let mut q = sqlx::query(sql);
                q = bind_params!(q, params);
                let rows = q.fetch_all(&mut **tx).await?;
                Ok(rows.into_iter().map(DbRow::from_mysql).collect())
            }
        }
    }

    /// Commit the transaction. After this the handle is consumed.
    pub async fn commit(self) -> DbResult<()> {
        match self.inner {
            DbTxInner::Sqlite(tx) => tx.commit().await?,
            DbTxInner::Postgres(tx) => tx.commit().await?,
            DbTxInner::MySql(tx) => tx.commit().await?,
        }
        Ok(())
    }

    /// Explicit rollback. Equivalent to dropping without committing, but
    /// surfaces sqlx errors instead of swallowing them.
    pub async fn rollback(self) -> DbResult<()> {
        match self.inner {
            DbTxInner::Sqlite(tx) => tx.rollback().await?,
            DbTxInner::Postgres(tx) => tx.rollback().await?,
            DbTxInner::MySql(tx) => tx.rollback().await?,
        }
        Ok(())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::pool::{connect, PoolTuning};

    async fn sqlite_adapter() -> DbAdapter {
        let pool = connect("sqlite::memory:", &PoolTuning::default())
            .await
            .expect("in-memory sqlite pool");
        DbAdapter::new(pool)
    }

    #[tokio::test]
    async fn execute_creates_and_inserts_returning_affected_rows() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER, name TEXT)", vec![])
            .await
            .expect("create");
        let n = a
            .execute(
                "INSERT INTO t (id, name) VALUES (?), (?)",
                vec![],
            )
            .await
            .expect_err("missing params should error")
            .to_string();
        assert!(n.contains("database error"), "got: {n}");

        let n = a
            .execute(
                "INSERT INTO t (id, name) VALUES (?, ?)",
                vec![DbValue::I64(1), DbValue::Text("alice".into())],
            )
            .await
            .expect("insert");
        assert_eq!(n, 1);
    }

    #[tokio::test]
    async fn query_one_fetches_typed_columns() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER, name TEXT, blob_col BLOB)", vec![])
            .await
            .unwrap();
        a.execute(
            "INSERT INTO t (id, name, blob_col) VALUES (?, ?, ?)",
            vec![
                DbValue::I64(42),
                DbValue::Text("bob".into()),
                DbValue::Bytes(b"hello".to_vec()),
            ],
        )
        .await
        .unwrap();

        let row = a
            .query_one("SELECT id, name, blob_col FROM t WHERE id = ?", vec![DbValue::I64(42)])
            .await
            .expect("fetch row");
        assert_eq!(row.get_i64(0).unwrap(), 42);
        assert_eq!(row.get_text(1).unwrap(), "bob");
        assert_eq!(row.get_bytes(2).unwrap(), b"hello");
        assert_eq!(row.len(), 3);
        assert!(!row.is_empty());
    }

    #[tokio::test]
    async fn query_optional_returns_none_for_no_matches() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER)", vec![])
            .await
            .unwrap();
        let r = a
            .query_optional("SELECT id FROM t WHERE id = ?", vec![DbValue::I64(1)])
            .await
            .expect("query");
        assert!(r.is_none());
    }

    #[tokio::test]
    async fn query_one_returns_row_not_found_when_no_match() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER)", vec![])
            .await
            .unwrap();
        let err = a
            .query_one("SELECT id FROM t WHERE id = ?", vec![DbValue::I64(1)])
            .await
            .expect_err("no match");
        assert!(
            err.is_row_not_found(),
            "expected RowNotFound, got: {err:?}"
        );
    }

    #[tokio::test]
    async fn query_all_round_trips_multiple_rows() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER, name TEXT)", vec![])
            .await
            .unwrap();
        for (id, name) in [(1, "alice"), (2, "bob"), (3, "carol")] {
            a.execute(
                "INSERT INTO t (id, name) VALUES (?, ?)",
                vec![DbValue::I64(id), DbValue::Text(name.into())],
            )
            .await
            .unwrap();
        }
        let rows = a
            .query_all("SELECT id, name FROM t ORDER BY id", vec![])
            .await
            .expect("query_all");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].get_i64(0).unwrap(), 1);
        assert_eq!(rows[2].get_text(1).unwrap(), "carol");
    }

    #[tokio::test]
    async fn null_binding_and_extraction() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (a TEXT, b INTEGER)", vec![])
            .await
            .unwrap();
        a.execute(
            "INSERT INTO t (a, b) VALUES (?, ?)",
            vec![DbValue::Null, DbValue::I64Opt(None)],
        )
        .await
        .unwrap();
        let row = a
            .query_one("SELECT a, b FROM t", vec![])
            .await
            .unwrap();
        assert_eq!(row.get_text_opt(0).unwrap(), None);
        assert_eq!(row.get_i64_opt(1).unwrap(), None);
        assert!(row.is_null(0));
        assert!(row.is_null(1));
    }

    #[tokio::test]
    async fn bool_round_trips_via_int_storage() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (b INTEGER)", vec![])
            .await
            .unwrap();
        a.execute("INSERT INTO t (b) VALUES (?), (?)",
            vec![DbValue::Bool(true), DbValue::Bool(false)])
            .await
            .unwrap();
        let rows = a
            .query_all("SELECT b FROM t ORDER BY b DESC", vec![])
            .await
            .unwrap();
        assert!(rows[0].get_bool(0).unwrap());
        assert!(!rows[1].get_bool(0).unwrap());
    }

    #[tokio::test]
    async fn transaction_commit_persists_changes() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER)", vec![])
            .await
            .unwrap();
        let mut tx = a.begin().await.expect("begin");
        tx.execute("INSERT INTO t (id) VALUES (?)", vec![DbValue::I64(7)])
            .await
            .unwrap();
        tx.commit().await.expect("commit");

        let row = a
            .query_one("SELECT id FROM t", vec![])
            .await
            .expect("row");
        assert_eq!(row.get_i64(0).unwrap(), 7);
    }

    #[tokio::test]
    async fn transaction_rollback_discards_changes() {
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (id INTEGER)", vec![])
            .await
            .unwrap();
        let mut tx = a.begin().await.expect("begin");
        tx.execute("INSERT INTO t (id) VALUES (?)", vec![DbValue::I64(7)])
            .await
            .unwrap();
        tx.rollback().await.expect("rollback");

        let r = a
            .query_optional("SELECT id FROM t", vec![])
            .await
            .unwrap();
        assert!(
            r.is_none(),
            "rollback must discard the insert; got row: {r:?}"
        );
    }

    #[tokio::test]
    async fn debug_only_renders_backend_name_not_pool_state() {
        let a = sqlite_adapter().await;
        let s = format!("{a:?}");
        assert_eq!(s, "DbAdapter(sqlite)");
    }

    #[tokio::test]
    async fn db_value_from_conversions_cover_common_types() {
        // Bind via the From impls — keeps DAO call sites terse.
        let a = sqlite_adapter().await;
        a.execute("CREATE TABLE t (s TEXT, i INTEGER, b INTEGER, by BLOB)", vec![])
            .await
            .unwrap();
        a.execute(
            "INSERT INTO t (s, i, b, by) VALUES (?, ?, ?, ?)",
            vec!["hello".into(), 42_i64.into(), true.into(), vec![1_u8, 2, 3].into()],
        )
        .await
        .unwrap();
        let r = a
            .query_one("SELECT s, i, b, by FROM t", vec![])
            .await
            .unwrap();
        assert_eq!(r.get_text(0).unwrap(), "hello");
        assert_eq!(r.get_i64(1).unwrap(), 42);
        assert!(r.get_bool(2).unwrap());
        assert_eq!(r.get_bytes(3).unwrap(), vec![1_u8, 2, 3]);
    }
}
