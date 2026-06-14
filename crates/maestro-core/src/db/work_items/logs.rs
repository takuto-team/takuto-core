// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::str::FromStr;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::*;

// ── work_item_log_lines ──────────────────────────────────────────────────

/// Append a batch of log lines. Wrapped in a single transaction so a
/// burst from one step lands atomically — partial failure rolls back
/// the whole batch and the caller can retry.
///
/// Empty batches are a no-op (no transaction overhead).
pub async fn append_log_lines(adapter: &DbAdapter, batch: &[LogLineInsert]) -> Result<()> {
    if batch.is_empty() {
        return Ok(());
    }
    let mut tx = adapter.begin().await?;
    for l in batch {
        tx.execute(
            "INSERT INTO work_item_log_lines \
                (work_item_id, step_id, stream, content, emitted_at) \
             VALUES (?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(l.work_item_id.clone()),
                DbValue::I64Opt(l.step_id),
                DbValue::Text(l.stream.as_str().to_string()),
                DbValue::Text(l.content.clone()),
                DbValue::I64(l.emitted_at),
            ],
        )
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Fetch log lines for a work item, oldest-first, with optional
/// per-step filtering and pagination.
pub async fn fetch_log_lines(
    adapter: &DbAdapter,
    work_item_id: &str,
    paging: LogPaging,
) -> Result<Vec<LogLine>> {
    let mut sql = String::from(
        "SELECT id, work_item_id, step_id, stream, content, emitted_at \
         FROM work_item_log_lines WHERE work_item_id = ?",
    );
    let mut params = vec![DbValue::Text(work_item_id.to_string())];
    if let Some(step_id) = paging.step_id {
        sql.push_str(" AND step_id = ?");
        params.push(DbValue::I64(step_id));
    }
    sql.push_str(" ORDER BY emitted_at ASC, id ASC LIMIT ? OFFSET ?");
    params.push(DbValue::I64(paging.limit.into()));
    params.push(DbValue::I64(paging.offset.into()));

    let rows = adapter.query_all(&sql, params).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let stream_s = r.get_text(3)?;
        let stream = LogStream::from_str(&stream_s).map_err(|e| {
            crate::error::MaestroError::Db(crate::db::DbError::Adapter(
                crate::db::adapter::DbError::Sqlx {
                    source: sqlx::Error::Configuration(e.into()),
                },
            ))
        })?;
        out.push(LogLine {
            id: r.get_i64(0)?,
            work_item_id: r.get_text(1)?,
            step_id: r.get_i64_opt(2)?,
            stream,
            content: r.get_text(4)?,
            emitted_at: r.get_i64(5)?,
        });
    }
    Ok(out)
}

/// Delete log lines older than `cutoff_emitted_at` (unix milliseconds).
/// Used by the retention runner (plan §5).
pub async fn purge_log_lines_older_than(
    adapter: &DbAdapter,
    cutoff_emitted_at: i64,
) -> Result<u64> {
    let affected = adapter
        .execute(
            "DELETE FROM work_item_log_lines WHERE emitted_at < ?",
            vec![DbValue::I64(cutoff_emitted_at)],
        )
        .await?;
    Ok(affected)
}

