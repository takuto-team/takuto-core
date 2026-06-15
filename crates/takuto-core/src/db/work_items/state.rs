// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::str::FromStr;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::{StateCounts, WorkItemListQuery, WorkItemRow, WorkItemStateKind};

// ── work_items: read ─────────────────────────────────────────────────────

const SELECT_WORK_ITEM: &str = "SELECT \
    id, ticket_key, workspace_name, user_id, private, started_manually, \
    counts_toward_manual_cap, driver_started, jira_available, \
    ticket_summary, ticket_description, ticket_type, ticket_url, acceptance_criteria, \
    base_branch, branch_name, worktree_path, pr_url, pr_merged, \
    last_session_id, state_kind, state_payload, current_step_label, \
    created_at, started_at, updated_at, repository_id \
    FROM work_items";

fn decode_work_item(r: &crate::db::DbRow) -> Result<WorkItemRow> {
    let state_kind_s = r.get_text(20)?;
    let state_kind = WorkItemStateKind::from_str(&state_kind_s).map_err(|e| {
        crate::error::TakutoError::Db(crate::db::DbError::Adapter(
            crate::db::adapter::DbError::Sqlx {
                source: sqlx::Error::Configuration(e.into()),
            },
        ))
    })?;
    Ok(WorkItemRow {
        id: r.get_text(0)?,
        ticket_key: r.get_text(1)?,
        workspace_name: r.get_text(2)?,
        user_id: r.get_text_opt(3)?,
        // Appended at column 26 to keep the
        // pre-existing positional indexes stable.
        repository_id: r.get_text_opt(26)?,
        private: r.get_i64(4)? != 0,
        started_manually: r.get_i64(5)? != 0,
        counts_toward_manual_cap: r.get_i64(6)? != 0,
        driver_started: r.get_i64(7)? != 0,
        jira_available: r.get_i64(8)? != 0,
        ticket_summary: r.get_text_opt(9)?,
        ticket_description: r.get_text_opt(10)?,
        ticket_type: r.get_text_opt(11)?,
        ticket_url: r.get_text_opt(12)?,
        acceptance_criteria: r.get_text_opt(13)?,
        base_branch: r.get_text_opt(14)?,
        branch_name: r.get_text_opt(15)?,
        worktree_path: r.get_text_opt(16)?,
        pr_url: r.get_text_opt(17)?,
        pr_merged: r.get_i64(18)? != 0,
        last_session_id: r.get_text_opt(19)?,
        state_kind,
        state_payload: r.get_text_opt(21)?,
        current_step_label: r.get_text_opt(22)?,
        created_at: r.get_i64(23)?,
        started_at: r.get_i64(24)?,
        updated_at: r.get_i64(25)?,
    })
}

/// Fetch a single work item by id, with the caller's visibility predicate
/// applied. Returns `Ok(None)` both when the row doesn't exist and when
/// it exists but the caller can't see it — callers that need to
/// distinguish "missing" from "forbidden" should fetch admin-side first.
/// Focused getter for the three fields `require_workflow_access`
/// consults, keyed by `ticket_key`. The route's path id is the
/// ticket_key (matches the in-memory map key), not the row's `id`
/// column (a UUID). We pick the most-recently started row when more
/// than one matches — only one workflow is active per ticket_key at a
/// time but historical rows from prior runs can accumulate.
///
/// Returns `None` when no row matches; caller falls back to the
/// in-memory HashMap.
pub async fn get_access_fields_by_ticket_key(
    adapter: &DbAdapter,
    ticket_key: &str,
) -> Result<Option<(Option<String>, Option<String>, String)>> {
    let row = adapter
        .query_optional(
            "SELECT user_id, repository_id, workspace_name \
             FROM work_items WHERE ticket_key = ? AND deleted_at IS NULL \
             ORDER BY started_at DESC LIMIT 1",
            vec![DbValue::Text(ticket_key.to_string())],
        )
        .await?;
    let Some(r) = row else { return Ok(None) };
    Ok(Some((
        r.get_text_opt(0)?,
        r.get_text_opt(1)?,
        r.get_text(2)?,
    )))
}

/// Project (ticket_key, state_kind) for every work item owned by
/// `user_id`. Used by `workflow_counts` to aggregate by state without
/// pulling full rows.
///
/// Returns one entry per ticket_key — when historical duplicates
/// exist, the most-recently-started one wins. This matches the
/// in-memory HashMap's one-row-per-ticket-key invariant so the
/// HashMap fallback merges cleanly.
pub async fn list_user_state_kinds(
    adapter: &DbAdapter,
    user_id: &str,
) -> Result<Vec<(String, WorkItemStateKind)>> {
    let rows = adapter
        .query_all(
            "SELECT ticket_key, state_kind FROM work_items wi \
             WHERE wi.user_id = ? AND wi.deleted_at IS NULL AND wi.started_at = ( \
                 SELECT MAX(wi2.started_at) FROM work_items wi2 \
                 WHERE wi2.ticket_key = wi.ticket_key AND wi2.user_id = wi.user_id \
                       AND wi2.deleted_at IS NULL \
             )",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let ticket_key = r.get_text(0)?;
        let state_s = r.get_text(1)?;
        let state_kind = WorkItemStateKind::from_str(&state_s).map_err(|e| {
            crate::error::TakutoError::Db(crate::db::DbError::Adapter(
                crate::db::adapter::DbError::Sqlx {
                    source: sqlx::Error::Configuration(e.into()),
                },
            ))
        })?;
        out.push((ticket_key, state_kind));
    }
    Ok(out)
}

/// Full-row fetch keyed by ticket_key. No visibility filter — callers
/// must run their own policy check (the route layer's
/// `require_workflow_access` already does so as the first action on
/// every endpoint that uses this). Picks the most-recently-started row
/// when historical duplicates exist, mirroring
/// [`get_access_fields_by_ticket_key`].
pub async fn get_work_item_by_ticket_key(
    adapter: &DbAdapter,
    ticket_key: &str,
) -> Result<Option<WorkItemRow>> {
    let sql = format!(
        "{SELECT_WORK_ITEM} WHERE ticket_key = ? AND deleted_at IS NULL \
         ORDER BY started_at DESC LIMIT 1"
    );
    let row = adapter
        .query_optional(&sql, vec![DbValue::Text(ticket_key.to_string())])
        .await?;
    let Some(row) = row else { return Ok(None) };
    Ok(Some(decode_work_item(&row)?))
}

pub async fn get_work_item(
    adapter: &DbAdapter,
    id: &str,
    caller_user_id: Option<&str>,
    caller_is_admin: bool,
) -> Result<Option<WorkItemRow>> {
    let sql = format!("{SELECT_WORK_ITEM} WHERE id = ?");
    let row = adapter
        .query_optional(&sql, vec![DbValue::Text(id.to_string())])
        .await?;
    let Some(row) = row else { return Ok(None) };
    let item = decode_work_item(&row)?;
    if !caller_can_see(
        &item,
        caller_user_id,
        caller_is_admin,
        /* include_team_visible= */ false,
    ) {
        return Ok(None);
    }
    Ok(Some(item))
}

/// List work items visible to the caller, ordered by `started_at DESC`
/// then `id DESC` (stable tie-break). See [`WorkItemListQuery`] for
/// the visibility predicate.
pub async fn list_work_items(
    adapter: &DbAdapter,
    q: &WorkItemListQuery,
) -> Result<Vec<WorkItemRow>> {
    let mut sql = String::from(SELECT_WORK_ITEM);
    // Soft-deleted runs are history — never surfaced in the live list.
    let mut where_clauses: Vec<String> = vec!["deleted_at IS NULL".to_string()];
    let mut params: Vec<DbValue> = Vec::new();

    if let Some(ws) = q.workspace_name.as_ref() {
        where_clauses.push("workspace_name = ?".to_string());
        params.push(DbValue::Text(ws.clone()));
    }

    if let Some(filter) = q.state_filter.as_ref()
        && !filter.is_empty()
    {
        let placeholders = vec!["?"; filter.len()].join(", ");
        where_clauses.push(format!("state_kind IN ({placeholders})"));
        for k in filter {
            params.push(DbValue::Text(k.as_str().to_string()));
        }
    }

    // Visibility predicate. `caller_is_admin` short-circuits at the SQL
    // level so the planner doesn't fan out the OR branches needlessly.
    if !q.caller_is_admin {
        let caller = q.caller_user_id.clone();
        match (caller.as_ref(), q.include_team_visible) {
            (Some(uid), true) => {
                where_clauses.push("(user_id = ? OR private = 0)".to_string());
                params.push(DbValue::Text(uid.clone()));
            }
            (Some(uid), false) => {
                where_clauses.push("user_id = ?".to_string());
                params.push(DbValue::Text(uid.clone()));
            }
            (None, true) => {
                where_clauses.push("private = 0".to_string());
            }
            (None, false) => {
                // No caller, no admin, no team-visible — nothing is
                // visible. Short-circuit to an empty result.
                return Ok(Vec::new());
            }
        }
    }

    if !where_clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&where_clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY started_at DESC, id DESC LIMIT ? OFFSET ?");
    params.push(DbValue::I64(q.limit.into()));
    params.push(DbValue::I64(q.offset.into()));

    let rows = adapter.query_all(&sql, params).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_work_item(r)?);
    }
    Ok(out)
}

/// Load every live (non-soft-deleted) work item across **all** workspaces
/// with no caller/visibility filter.
///
/// This is the engine's restore source: at startup the in-memory cache is
/// rebuilt from these rows (cutover invariant I3 — DB-first restore), so the
/// per-user/per-workspace predicates that `list_work_items` applies must NOT
/// be applied here. Ordered `started_at` ascending so callers insert
/// oldest-first, matching the snapshot restore order.
pub async fn list_all_for_restore(adapter: &DbAdapter) -> Result<Vec<WorkItemRow>> {
    let sql =
        format!("{SELECT_WORK_ITEM} WHERE deleted_at IS NULL ORDER BY started_at ASC, id ASC");
    let rows = adapter.query_all(&sql, Vec::new()).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_work_item(r)?);
    }
    Ok(out)
}

fn caller_can_see(
    item: &WorkItemRow,
    caller_user_id: Option<&str>,
    caller_is_admin: bool,
    include_team_visible: bool,
) -> bool {
    if caller_is_admin {
        return true;
    }
    let Some(uid) = caller_user_id else {
        return include_team_visible && !item.private;
    };
    if item.user_id.as_deref() == Some(uid) {
        return true;
    }
    include_team_visible && !item.private
}

// ── work_items: write ────────────────────────────────────────────────────

/// Insert a fresh work item. Caller is responsible for generating the
/// UUID `id` and supplying timestamps. Returns an error on UNIQUE
/// violation against `(workspace_name, ticket_key)`.
pub async fn insert_work_item(adapter: &DbAdapter, row: &WorkItemRow) -> Result<()> {
    adapter
        .execute(
            "INSERT INTO work_items (\
                id, ticket_key, workspace_name, user_id, private, started_manually, \
                counts_toward_manual_cap, driver_started, jira_available, \
                ticket_summary, ticket_description, ticket_type, ticket_url, acceptance_criteria, \
                base_branch, branch_name, worktree_path, pr_url, pr_merged, \
                last_session_id, state_kind, state_payload, current_step_label, \
                created_at, started_at, updated_at, repository_id\
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(row.id.clone()),
                DbValue::Text(row.ticket_key.clone()),
                DbValue::Text(row.workspace_name.clone()),
                DbValue::TextOpt(row.user_id.clone()),
                DbValue::I64(row.private.into()),
                DbValue::I64(row.started_manually.into()),
                DbValue::I64(row.counts_toward_manual_cap.into()),
                DbValue::I64(row.driver_started.into()),
                DbValue::I64(row.jira_available.into()),
                DbValue::TextOpt(row.ticket_summary.clone()),
                DbValue::TextOpt(row.ticket_description.clone()),
                DbValue::TextOpt(row.ticket_type.clone()),
                DbValue::TextOpt(row.ticket_url.clone()),
                DbValue::TextOpt(row.acceptance_criteria.clone()),
                DbValue::TextOpt(row.base_branch.clone()),
                DbValue::TextOpt(row.branch_name.clone()),
                DbValue::TextOpt(row.worktree_path.clone()),
                DbValue::TextOpt(row.pr_url.clone()),
                DbValue::I64(row.pr_merged.into()),
                DbValue::TextOpt(row.last_session_id.clone()),
                DbValue::Text(row.state_kind.as_str().to_string()),
                DbValue::TextOpt(row.state_payload.clone()),
                DbValue::TextOpt(row.current_step_label.clone()),
                DbValue::I64(row.created_at),
                DbValue::I64(row.started_at),
                DbValue::I64(row.updated_at),
                DbValue::TextOpt(row.repository_id.clone()),
            ],
        )
        .await?;
    Ok(())
}

/// Update a work item's state-machine kind + payload + current-step
/// label. `updated_at` is bumped automatically to `now`.
pub async fn update_work_item_state(
    adapter: &DbAdapter,
    id: &str,
    state_kind: WorkItemStateKind,
    state_payload: Option<&str>,
    current_step_label: Option<&str>,
    now: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET \
                state_kind = ?, state_payload = ?, current_step_label = ?, updated_at = ? \
             WHERE id = ?",
            vec![
                DbValue::Text(state_kind.as_str().to_string()),
                DbValue::TextOpt(state_payload.map(str::to_string)),
                DbValue::TextOpt(current_step_label.map(str::to_string)),
                DbValue::I64(now),
                DbValue::Text(id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Atomically set the PR URL **and** the state-machine columns in a single
/// statement.
///
/// The engine resolves the PR URL and transitions to a terminal state
/// (`Done`) as one logical event ("finished — here is the PR"). Writing both
/// in one UPDATE means a crash can never leave the row with the new PR URL but
/// the old (non-terminal) state, or a terminal state with a stale PR URL
/// (cutover invariant I2 — no torn rows).
pub async fn update_work_item_pr_url_and_state(
    adapter: &DbAdapter,
    id: &str,
    pr_url: Option<&str>,
    state_kind: WorkItemStateKind,
    state_payload: Option<&str>,
    current_step_label: Option<&str>,
    now: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET \
                pr_url = ?, state_kind = ?, state_payload = ?, current_step_label = ?, updated_at = ? \
             WHERE id = ?",
            vec![
                DbValue::TextOpt(pr_url.map(str::to_string)),
                DbValue::Text(state_kind.as_str().to_string()),
                DbValue::TextOpt(state_payload.map(str::to_string)),
                DbValue::TextOpt(current_step_label.map(str::to_string)),
                DbValue::I64(now),
                DbValue::Text(id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Update the per-work-item PR URL (set by the GitHub PR-create hook).
pub async fn update_pr_url(
    adapter: &DbAdapter,
    id: &str,
    pr_url: Option<&str>,
    now: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET pr_url = ?, updated_at = ? WHERE id = ?",
            vec![
                DbValue::TextOpt(pr_url.map(str::to_string)),
                DbValue::I64(now),
                DbValue::Text(id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Update the per-work-item branch name (set once the worktree/branch is
/// created during bootstrap). The work item is keyed by `id`.
pub async fn update_branch_name(
    adapter: &DbAdapter,
    id: &str,
    branch_name: Option<&str>,
    now: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET branch_name = ?, updated_at = ? WHERE id = ?",
            vec![
                DbValue::TextOpt(branch_name.map(str::to_string)),
                DbValue::I64(now),
                DbValue::Text(id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Atomically set the branch name **and** the worktree path in a single
/// statement.
///
/// Worktree creation during bootstrap assigns both at once (and previously
/// only the branch was persisted, leaving `worktree_path` permanently NULL in
/// the DB). Writing them together keeps the row internally consistent if a
/// crash interrupts bootstrap — never a branch-new + worktree-old (or NULL)
/// torn row (cutover invariant I2) — and makes `worktree_path` available to
/// the DB-first restore path.
pub async fn update_work_item_branch_and_worktree(
    adapter: &DbAdapter,
    id: &str,
    branch_name: Option<&str>,
    worktree_path: Option<&str>,
    now: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET branch_name = ?, worktree_path = ?, updated_at = ? WHERE id = ?",
            vec![
                DbValue::TextOpt(branch_name.map(str::to_string)),
                DbValue::TextOpt(worktree_path.map(str::to_string)),
                DbValue::I64(now),
                DbValue::Text(id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Update the per-work-item `pr_merged` flag (set by the PR merge poller).
pub async fn update_pr_merged(
    adapter: &DbAdapter,
    id: &str,
    pr_merged: bool,
    now: i64,
) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET pr_merged = ?, updated_at = ? WHERE id = ?",
            vec![
                DbValue::I64(pr_merged.into()),
                DbValue::I64(now),
                DbValue::Text(id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Hard-delete a work item. CASCADE on the child tables wipes
/// steps, definition runs, log lines, port mappings, and run commands.
pub async fn delete_work_item(adapter: &DbAdapter, id: &str) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM work_items WHERE id = ?",
            vec![DbValue::Text(id.to_string())],
        )
        .await?;
    Ok(())
}

/// Soft-delete a work item: stamp `deleted_at` with `now` (Unix seconds) so
/// the row drops out of every live query but survives as run history. Child
/// rows (steps, definition runs, log lines) are intentionally retained — they
/// belong to the historical run. Re-adding the same ticket inserts a fresh
/// row; the soft-deleted one no longer collides because the UNIQUE
/// (workspace_name, ticket_key) index was dropped in the soft-delete migration.
pub async fn soft_delete_work_item(adapter: &DbAdapter, id: &str, now: i64) -> Result<()> {
    adapter
        .execute(
            "UPDATE work_items SET deleted_at = ? WHERE id = ?",
            vec![DbValue::I64(now), DbValue::Text(id.to_string())],
        )
        .await?;
    Ok(())
}

// ── counts ───────────────────────────────────────────────────────────────

/// Aggregate counts by `state_kind` across either a single workspace or
/// the whole deployment. Scope is admin-only — there's no per-caller
/// filter; counts feed the dashboard summary tiles which are themselves
/// access-gated upstream.
pub async fn count_by_state(
    adapter: &DbAdapter,
    workspace_name: Option<&str>,
) -> Result<StateCounts> {
    let (sql, params): (&str, Vec<DbValue>) = match workspace_name {
        Some(ws) => (
            "SELECT state_kind, COUNT(*) FROM work_items \
             WHERE workspace_name = ? AND deleted_at IS NULL GROUP BY state_kind",
            vec![DbValue::Text(ws.to_string())],
        ),
        None => (
            "SELECT state_kind, COUNT(*) FROM work_items \
             WHERE deleted_at IS NULL GROUP BY state_kind",
            Vec::new(),
        ),
    };
    let rows = adapter.query_all(sql, params).await?;
    let mut counts = StateCounts::default();
    for r in &rows {
        let k_s = r.get_text(0)?;
        let n = r.get_i64(1)? as u64;
        if let Ok(k) = WorkItemStateKind::from_str(&k_s) {
            counts.add(k, n);
        }
    }
    Ok(counts)
}
