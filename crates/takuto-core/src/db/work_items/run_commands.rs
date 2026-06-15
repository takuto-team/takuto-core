// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::*;

// ── work_item_run_commands ───────────────────────────────────────────────

/// Upsert run-command state for a (work_item, command_index) pair.
#[allow(clippy::too_many_arguments)]
pub async fn upsert_run_command(
    adapter: &DbAdapter,
    work_item_id: &str,
    command_index: i32,
    name: &str,
    running: bool,
    container_id: Option<&str>,
    started_at: Option<i64>,
    ended_at: Option<i64>,
) -> Result<()> {
    let tail = crate::db::upsert::build_update_tail(
        adapter.backend(),
        &["work_item_id", "command_index"],
        &["name", "running", "container_id", "started_at", "ended_at"],
    );
    let sql = format!(
        "INSERT INTO work_item_run_commands \
            (work_item_id, command_index, name, running, container_id, started_at, ended_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?) {tail}"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(command_index),
                DbValue::Text(name.to_string()),
                DbValue::I64(running.into()),
                DbValue::TextOpt(container_id.map(str::to_string)),
                DbValue::I64Opt(started_at),
                DbValue::I64Opt(ended_at),
            ],
        )
        .await?;
    Ok(())
}

/// Mark a (work_item, command_index) run-command pair as Running
/// with `started_at`. Idempotent — re-running clears any prior
/// `ended_at` so a restarted command looks freshly started in the
/// DB row even when the caller previously stopped it.
pub async fn start_run_command_row(
    adapter: &DbAdapter,
    work_item_id: &str,
    command_index: i32,
    name: &str,
    container_id: Option<&str>,
    started_at: i64,
) -> Result<()> {
    upsert_run_command(
        adapter,
        work_item_id,
        command_index,
        name,
        true,
        container_id,
        Some(started_at),
        None,
    )
    .await
}

/// Transition an existing run-command row to stopped. UPDATE-only
/// so we never overwrite the `started_at` set by
/// [`start_run_command_row`]; a missing row is a silent no-op so
/// race conditions between the route handler and the DB cannot
/// surface as user-visible errors.
pub async fn finish_run_command_row(
    adapter: &DbAdapter,
    work_item_id: &str,
    command_index: i32,
    ended_at: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_item_run_commands SET \
                running = 0, ended_at = ? \
             WHERE work_item_id = ? AND command_index = ?",
            vec![
                DbValue::I64(ended_at),
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(command_index),
            ],
        )
        .await?;
    Ok(())
}

/// Shadow-write the start of a run-command container. Marks the
/// (work_item, command_index) row as Running with `started_at` set and
/// `container_id` populated. Failures (and `None` `db`) log at WARN
/// and never propagate — the container has already started; the
/// secondary store catching up is best-effort.
pub async fn shadow_start_run_command_row(
    db: Option<&crate::db::Database>,
    work_item_id: &str,
    command_index: i32,
    name: &str,
    container_id: Option<&str>,
    started_at_unix: i64,
) {
    let Some(db) = db else { return };
    if let Err(e) = start_run_command_row(
        db.adapter(),
        work_item_id,
        command_index,
        name,
        container_id,
        started_at_unix,
    )
    .await
    {
        tracing::warn!(
            work_item_id,
            command_index,
            error = %e,
            "Ushadow-write of run-command start failed (route handler progress unaffected)"
        );
    }
}

/// Shadow-write the stop of a run-command container. UPDATE-only: an
/// absent row stays absent so an out-of-order stop (e.g. stop fires
/// before the start row landed) silently no-ops rather than producing
/// an inconsistent row.
pub async fn shadow_finish_run_command_row(
    db: Option<&crate::db::Database>,
    work_item_id: &str,
    command_index: i32,
    ended_at_unix: i64,
) {
    let Some(db) = db else { return };
    if let Err(e) =
        finish_run_command_row(db.adapter(), work_item_id, command_index, ended_at_unix).await
    {
        tracing::warn!(
            work_item_id,
            command_index,
            error = %e,
            "Ushadow-write of run-command finish failed (route handler progress unaffected)"
        );
    }
}

/// List run commands for a work item, command-index-ascending.
pub async fn list_run_commands(
    adapter: &DbAdapter,
    work_item_id: &str,
) -> Result<Vec<RunCommandRow>> {
    let rows = adapter
        .query_all(
            "SELECT work_item_id, command_index, name, running, container_id, \
                    started_at, ended_at \
             FROM work_item_run_commands WHERE work_item_id = ? \
             ORDER BY command_index ASC",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(RunCommandRow {
            work_item_id: r.get_text(0)?,
            command_index: r.get_i64(1)? as i32,
            name: r.get_text(2)?,
            running: r.get_i64(3)? != 0,
            container_id: r.get_text_opt(4)?,
            started_at: r.get_i64_opt(5)?,
            ended_at: r.get_i64_opt(6)?,
        });
    }
    Ok(out)
}
