// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::str::FromStr;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::*;

// ── work_item_definition_runs ────────────────────────────────────────────

/// Upsert the per-(work-item, definition) run state. Idempotent.
pub async fn upsert_definition_run(
    adapter: &DbAdapter,
    work_item_id: &str,
    definition_filename: &str,
    state: DefRunState,
    error_message: Option<&str>,
    started_at: Option<i64>,
    ended_at: Option<i64>,
) -> Result<()> {
    let tail = crate::db::upsert::build_update_tail(
        adapter.backend(),
        &["work_item_id", "definition_filename"],
        &["state", "error_message", "started_at", "ended_at"],
    );
    let sql = format!(
        "INSERT INTO work_item_definition_runs \
            (work_item_id, definition_filename, state, error_message, started_at, ended_at) \
         VALUES (?, ?, ?, ?, ?, ?) {tail}"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::Text(definition_filename.to_string()),
                DbValue::Text(state.as_str().to_string()),
                DbValue::TextOpt(error_message.map(str::to_string)),
                DbValue::I64Opt(started_at),
                DbValue::I64Opt(ended_at),
            ],
        )
        .await?;
    Ok(())
}

/// Mark a (work-item, definition) pair as Running with `started_at`.
/// Idempotent — re-running clears any prior `error_message` /
/// `ended_at` so a fresh run looks fresh in the DB row even when the
/// caller previously transitioned through Error.
pub async fn start_definition_run(
    adapter: &DbAdapter,
    work_item_id: &str,
    definition_filename: &str,
    started_at: i64,
) -> Result<()> {
    upsert_definition_run(
        adapter,
        work_item_id,
        definition_filename,
        DefRunState::Running,
        None,
        Some(started_at),
        None,
    )
    .await
}

/// Transition an existing (work-item, definition) row to its final
/// state. UPDATE-only so we never overwrite `started_at` set by the
/// matching [`start_definition_run`]; if no prior row exists, this is
/// a silent no-op (0 rows affected). The shadow-write contract
/// requires that a missing start row never break the engine.
pub async fn finish_definition_run(
    adapter: &DbAdapter,
    work_item_id: &str,
    definition_filename: &str,
    state: DefRunState,
    error_message: Option<&str>,
    ended_at: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_item_definition_runs SET \
                state = ?, error_message = ?, ended_at = ? \
             WHERE work_item_id = ? AND definition_filename = ?",
            vec![
                DbValue::Text(state.as_str().to_string()),
                DbValue::TextOpt(error_message.map(str::to_string)),
                DbValue::I64(ended_at),
                DbValue::Text(work_item_id.to_string()),
                DbValue::Text(definition_filename.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// List all definition runs for a work item.
pub async fn list_definition_runs(
    adapter: &DbAdapter,
    work_item_id: &str,
) -> Result<Vec<DefinitionRunRow>> {
    let rows = adapter
        .query_all(
            "SELECT work_item_id, definition_filename, state, error_message, started_at, ended_at \
             FROM work_item_definition_runs WHERE work_item_id = ? \
             ORDER BY definition_filename ASC",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let state_s = r.get_text(2)?;
        let state = DefRunState::from_str(&state_s).map_err(|e| {
            crate::error::TakutoError::Db(crate::db::DbError::Adapter(
                crate::db::adapter::DbError::Sqlx {
                    source: sqlx::Error::Configuration(e.into()),
                },
            ))
        })?;
        out.push(DefinitionRunRow {
            work_item_id: r.get_text(0)?,
            definition_filename: r.get_text(1)?,
            state,
            error_message: r.get_text_opt(3)?,
            started_at: r.get_i64_opt(4)?,
            ended_at: r.get_i64_opt(5)?,
        });
    }
    Ok(out)
}
