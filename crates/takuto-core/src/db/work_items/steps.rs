// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::str::FromStr;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::{StepRow, StepStatus};

// ── work_item_steps ──────────────────────────────────────────────────────

const SELECT_STEP: &str = "SELECT \
    id, work_item_id, sequence, name, definition_filename, status, \
    started_at, ended_at, exit_code, error_message \
    FROM work_item_steps";

fn decode_step(r: &crate::db::DbRow) -> Result<StepRow> {
    let status_s = r.get_text(5)?;
    let status = StepStatus::from_str(&status_s).map_err(|e| {
        crate::error::TakutoError::Db(crate::db::DbError::Adapter(
            crate::db::adapter::DbError::Sqlx {
                source: sqlx::Error::Configuration(e.into()),
            },
        ))
    })?;
    Ok(StepRow {
        id: r.get_i64(0)?,
        work_item_id: r.get_text(1)?,
        sequence: r.get_i64(2)?,
        name: r.get_text(3)?,
        definition_filename: r.get_text_opt(4)?,
        status,
        started_at: r.get_i64(6)?,
        ended_at: r.get_i64_opt(7)?,
        // `exit_code` is INTEGER NULL — read as i64_opt and downcast.
        exit_code: r.get_i64_opt(8)?.map(|v| v as i32),
        error_message: r.get_text_opt(9)?,
    })
}

/// Record the start of a step. Computes the next `sequence` for the
/// work item under a single round-trip via `SELECT MAX(sequence) + 1`.
/// Returns the autoincrement `id` for later [`record_step_end`].
///
/// Note: not atomic against concurrent step starts on the same work
/// item — that's fine, the engine never starts two steps in parallel
/// on the same item.
pub async fn record_step_start(
    adapter: &DbAdapter,
    work_item_id: &str,
    name: &str,
    definition_filename: Option<&str>,
    started_at: i64,
) -> Result<i64> {
    // Next sequence = (MAX(sequence) + 1) for this work item, or 0 if none.
    let row = adapter
        .query_one(
            "SELECT COALESCE(MAX(sequence), -1) + 1 FROM work_item_steps WHERE work_item_id = ?",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    let next_seq = row.get_i64(0)?;

    // Insert. We use a separate SELECT-by-(work_item_id, sequence) to
    // recover the autoincrement id rather than a RETURNING clause —
    // RETURNING is Postgres/SQLite but not pre-8.0.21 MySQL.
    adapter
        .execute(
            "INSERT INTO work_item_steps \
                (work_item_id, sequence, name, definition_filename, status, started_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I64(next_seq),
                DbValue::Text(name.to_string()),
                DbValue::TextOpt(definition_filename.map(str::to_string)),
                DbValue::Text(StepStatus::Running.as_str().to_string()),
                DbValue::I64(started_at),
            ],
        )
        .await?;
    let id_row = adapter
        .query_one(
            "SELECT id FROM work_item_steps WHERE work_item_id = ? AND sequence = ?",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I64(next_seq),
            ],
        )
        .await?;
    Ok(id_row.get_i64(0)?)
}

/// Finish a step — set the status, optional exit code, optional error
/// message, and `ended_at`.
pub async fn record_step_end(
    adapter: &DbAdapter,
    step_id: i64,
    status: StepStatus,
    exit_code: Option<i32>,
    error_message: Option<&str>,
    ended_at: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_item_steps SET \
                status = ?, exit_code = ?, error_message = ?, ended_at = ? \
             WHERE id = ?",
            vec![
                DbValue::Text(status.as_str().to_string()),
                DbValue::I32Opt(exit_code),
                DbValue::TextOpt(error_message.map(str::to_string)),
                DbValue::I64(ended_at),
                DbValue::I64(step_id),
            ],
        )
        .await?;
    Ok(())
}

/// List steps for a work item, sequence-ascending.
pub async fn list_steps(adapter: &DbAdapter, work_item_id: &str) -> Result<Vec<StepRow>> {
    let sql = format!("{SELECT_STEP} WHERE work_item_id = ? ORDER BY sequence ASC");
    let rows = adapter
        .query_all(&sql, vec![DbValue::Text(work_item_id.to_string())])
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_step(r)?);
    }
    Ok(out)
}

/// For each requested `work_item_id`, the number of `work_item_steps` belonging
/// to that item's most-recently-completed definition run (the completed
/// `work_item_definition_runs` row with the greatest `ended_at`). Work items
/// with no completed run — or no recorded steps — are absent from the map.
///
/// This is the authoritative "steps in the latest completed flow" count the
/// dashboard renders for terminal workflows, where the in-memory `steps_log`
/// and `current_def_total_steps` estimate are unavailable after a restart.
pub async fn count_steps_of_latest_completed_def(
    adapter: &DbAdapter,
    work_item_ids: &[String],
) -> Result<std::collections::HashMap<String, u32>> {
    if work_item_ids.is_empty() {
        return Ok(std::collections::HashMap::new());
    }

    let placeholders = vec!["?"; work_item_ids.len()].join(", ");
    // Inner-most subquery: latest completed run per requested work item.
    // Joined back to its definition_filename, then to the steps recorded
    // under that filename. `COUNT(*)` is the flow's executed step count.
    let sql = format!(
        "SELECT s.work_item_id, COUNT(*) \
         FROM work_item_steps s \
         JOIN ( \
             SELECT d.work_item_id, d.definition_filename \
             FROM work_item_definition_runs d \
             JOIN ( \
                 SELECT work_item_id, MAX(ended_at) AS max_ended \
                 FROM work_item_definition_runs \
                 WHERE state = 'completed' AND work_item_id IN ({placeholders}) \
                 GROUP BY work_item_id \
             ) latest \
               ON latest.work_item_id = d.work_item_id \
              AND latest.max_ended = d.ended_at \
             WHERE d.state = 'completed' \
         ) chosen \
           ON chosen.work_item_id = s.work_item_id \
          AND chosen.definition_filename = s.definition_filename \
         GROUP BY s.work_item_id"
    );

    let params: Vec<DbValue> = work_item_ids
        .iter()
        .map(|id| DbValue::Text(id.clone()))
        .collect();
    let rows = adapter.query_all(&sql, params).await?;

    let mut out = std::collections::HashMap::with_capacity(rows.len());
    for r in &rows {
        let id = r.get_text(0)?;
        let count = r.get_i64(1)?.max(0) as u32;
        out.insert(id, count);
    }
    Ok(out)
}
