// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Typed DAO over the work-item tables.
//!
//! Six tables:
//!   - `work_items` — one row per Jira/GitHub ticket or manual item;
//!     state + ticket metadata + git/PR state + agent state.
//!   - `work_item_steps` — per-step execution log.
//!   - `work_item_definition_runs` — per-(work-item, definition) run state.
//!   - `work_item_log_lines` — stdout/stderr/info/system lines.
//!   - `work_item_port_mappings` — persisted port forwards.
//!   - `work_item_run_commands` — run-command running state.
//!
//! ## Style
//!
//! Mirror the existing DAO modules (`db::users`, `db::repositories`):
//!   - Public surface is small typed functions + flat row structs.
//!   - SQL stays inside the module. Callers don't see `?` placeholders.
//!   - Enum columns (state_kind, status, stream, kind) round-trip
//!     through a Rust enum's `as_str()` / `parse()`.
//!   - Visibility predicate (`list_work_items`, `get_work_item`) takes
//!     `caller_user_id` + `caller_is_admin` and filters in one
//!     `WHERE` clause.

use std::str::FromStr;

use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

// ── Enums ────────────────────────────────────────────────────────────────

/// State-machine variants stored in `work_items.state_kind`.
/// Variants that carry data (e.g. `Paused { source_state }`,
/// `Error { source_state, message }`, `AddressingTicket { pass }`)
/// keep that data as JSON in `state_payload`; the kind alone drives
/// indexed queries.
///
/// Covers every variant the engine's `WorkflowState` enum has, so the
/// engine's persist-to-DB pass can map 1:1 without information loss.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkItemStateKind {
    Pending,
    Assigning,
    RetrievingDetails,
    CreatingWorktree,
    AddressingTicket,
    /// Legacy variant — kept for snapshot round-trip.
    AddressingPrComments,
    /// Legacy variant — kept for snapshot round-trip.
    MergingBaseBranch,
    Reviewing,
    CreatingPr,
    Done,
    Stopped,
    Error,
    Paused,
}

impl WorkItemStateKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Assigning => "assigning",
            Self::RetrievingDetails => "retrieving_details",
            Self::CreatingWorktree => "creating_worktree",
            Self::AddressingTicket => "addressing_ticket",
            Self::AddressingPrComments => "addressing_pr_comments",
            Self::MergingBaseBranch => "merging_base_branch",
            Self::Reviewing => "reviewing",
            Self::CreatingPr => "creating_pr",
            Self::Done => "done",
            Self::Stopped => "stopped",
            Self::Error => "error",
            Self::Paused => "paused",
        }
    }
}

impl FromStr for WorkItemStateKind {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "pending" => Self::Pending,
            "assigning" => Self::Assigning,
            "retrieving_details" => Self::RetrievingDetails,
            "creating_worktree" => Self::CreatingWorktree,
            "addressing_ticket" => Self::AddressingTicket,
            "addressing_pr_comments" => Self::AddressingPrComments,
            "merging_base_branch" => Self::MergingBaseBranch,
            "reviewing" => Self::Reviewing,
            "creating_pr" => Self::CreatingPr,
            "done" => Self::Done,
            "stopped" => Self::Stopped,
            "error" => Self::Error,
            "paused" => Self::Paused,
            other => return Err(format!("unknown WorkItemStateKind: {other}")),
        })
    }
}

/// Status of a single step in `work_item_steps`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Running,
    Success,
    Failed,
    Skipped,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Success => "success",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

impl FromStr for StepStatus {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "running" => Self::Running,
            "success" => Self::Success,
            "failed" => Self::Failed,
            "skipped" => Self::Skipped,
            other => return Err(format!("unknown StepStatus: {other}")),
        })
    }
}

/// State of a per-definition run on a work item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefRunState {
    Idle,
    Running,
    Completed,
    Error,
}

impl DefRunState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Error => "error",
        }
    }
}

impl FromStr for DefRunState {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "idle" => Self::Idle,
            "running" => Self::Running,
            "completed" => Self::Completed,
            "error" => Self::Error,
            other => return Err(format!("unknown DefRunState: {other}")),
        })
    }
}

/// Stream identifier for log lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogStream {
    Stdout,
    Stderr,
    Info,
    System,
}

impl LogStream {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
            Self::Info => "info",
            Self::System => "system",
        }
    }
}

impl FromStr for LogStream {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "stdout" => Self::Stdout,
            "stderr" => Self::Stderr,
            "info" => Self::Info,
            "system" => Self::System,
            other => return Err(format!("unknown LogStream: {other}")),
        })
    }
}

/// Kind of port mapping. Drives both the unique-port-per-kind allocation
/// rule (in app code) and the per-card UI button layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMappingKind {
    Editor,
    Terminal,
    Dynamic,
    RunCommand,
}

impl PortMappingKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Editor => "editor",
            Self::Terminal => "terminal",
            Self::Dynamic => "dynamic",
            Self::RunCommand => "run_command",
        }
    }
}

impl FromStr for PortMappingKind {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(match s {
            "editor" => Self::Editor,
            "terminal" => Self::Terminal,
            "dynamic" => Self::Dynamic,
            "run_command" => Self::RunCommand,
            other => return Err(format!("unknown PortMappingKind: {other}")),
        })
    }
}

// ── Row structs ──────────────────────────────────────────────────────────

/// One row in `work_items`. Mirrors the table 1:1 — all decoded columns,
/// no parsing of `state_payload` (callers JSON-decode if they need the
/// inner variant data).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkItemRow {
    pub id: String,
    pub ticket_key: String,
    pub workspace_name: String,
    pub user_id: Option<String>,
    // Repo association. Nullable to match
    // `Workflow::repository_id: Option<String>` — legacy workflows have
    // no repo association.
    pub repository_id: Option<String>,
    pub private: bool,
    pub started_manually: bool,
    pub counts_toward_manual_cap: bool,
    pub driver_started: bool,
    pub jira_available: bool,

    pub ticket_summary: Option<String>,
    pub ticket_description: Option<String>,
    pub ticket_type: Option<String>,
    pub ticket_url: Option<String>,
    pub acceptance_criteria: Option<String>,

    pub base_branch: Option<String>,
    pub branch_name: Option<String>,
    pub worktree_path: Option<String>,
    pub pr_url: Option<String>,
    pub pr_merged: bool,

    pub last_session_id: Option<String>,

    pub state_kind: WorkItemStateKind,
    pub state_payload: Option<String>,
    pub current_step_label: Option<String>,

    pub created_at: i64,
    pub started_at: i64,
    pub updated_at: i64,
}

/// One row in `work_item_steps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepRow {
    pub id: i64,
    pub work_item_id: String,
    pub sequence: i64,
    pub name: String,
    pub definition_filename: Option<String>,
    pub status: StepStatus,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

/// One row in `work_item_definition_runs`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefinitionRunRow {
    pub work_item_id: String,
    pub definition_filename: String,
    pub state: DefRunState,
    pub error_message: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

/// One row in `work_item_log_lines`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLine {
    pub id: i64,
    pub work_item_id: String,
    pub step_id: Option<i64>,
    pub stream: LogStream,
    pub content: String,
    /// Unix milliseconds.
    pub emitted_at: i64,
}

/// What `append_log_lines` accepts — same as [`LogLine`] minus the
/// autoincrement `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogLineInsert {
    pub work_item_id: String,
    pub step_id: Option<i64>,
    pub stream: LogStream,
    pub content: String,
    pub emitted_at: i64,
}

/// One row in `work_item_port_mappings`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortMappingRow {
    pub id: i64,
    pub work_item_id: String,
    pub container_port: i32,
    pub host_port: i32,
    pub proxy_url: String,
    pub path_token: String,
    pub kind: PortMappingKind,
    pub run_command_index: Option<i32>,
    pub created_at: i64,
}

/// One row in `work_item_run_commands`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCommandRow {
    pub work_item_id: String,
    pub command_index: i32,
    pub name: String,
    pub running: bool,
    pub container_id: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
}

// ── Query types ──────────────────────────────────────────────────────────

/// Pagination + slicing for [`fetch_log_lines`].
#[derive(Debug, Clone, Copy)]
pub struct LogPaging {
    /// If `Some`, return only lines whose `step_id` matches this.
    pub step_id: Option<i64>,
    /// Return at most `limit` rows, oldest first.
    pub limit: u32,
    /// Skip `offset` rows. Used by the dashboard's "Load more" button.
    pub offset: u32,
}

impl Default for LogPaging {
    fn default() -> Self {
        Self {
            step_id: None,
            limit: 500,
            offset: 0,
        }
    }
}

/// Visibility-aware list query. Per plan §3, the predicate is:
///
/// ```text
/// WHERE workspace_name = :workspace
///   AND ( :caller_is_admin
///      OR user_id = :caller
///      OR (private = 0 AND :include_team_visible = 1)
///   )
/// ```
///
/// `include_team_visible` defaults to `false` (creator-only model)
/// until the team-visibility policy lands.
#[derive(Debug, Clone)]
pub struct WorkItemListQuery {
    pub caller_user_id: Option<String>,
    pub caller_is_admin: bool,
    pub workspace_name: Option<String>,
    pub state_filter: Option<Vec<WorkItemStateKind>>,
    /// When `true`, the query returns non-private items owned by other
    /// users in the same workspace. `false` keeps the strict
    /// creator-only view.
    pub include_team_visible: bool,
    pub limit: u32,
    pub offset: u32,
}

/// Aggregated counts by `state_kind` for the dashboard summary tiles.
/// The mid-pipeline states (`retrieving_details` / `reviewing` /
/// `creating_pr`) and legacy snapshot states (`addressing_pr_comments`
/// / `merging_base_branch`) fold into `in_progress` because the
/// dashboard renders them all as "running" — operators care about
/// terminal vs paused vs in-progress, not which mid-step you're at.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StateCounts {
    pub pending: u64,
    pub in_progress: u64,
    pub done: u64,
    pub stopped: u64,
    pub error: u64,
    pub paused: u64,
}

impl StateCounts {
    fn add(&mut self, kind: WorkItemStateKind, n: u64) {
        match kind {
            WorkItemStateKind::Pending => self.pending += n,
            WorkItemStateKind::Done => self.done += n,
            WorkItemStateKind::Stopped => self.stopped += n,
            WorkItemStateKind::Error => self.error += n,
            WorkItemStateKind::Paused => self.paused += n,
            // Every mid-pipeline state rolls up to "in progress".
            WorkItemStateKind::Assigning
            | WorkItemStateKind::RetrievingDetails
            | WorkItemStateKind::CreatingWorktree
            | WorkItemStateKind::AddressingTicket
            | WorkItemStateKind::AddressingPrComments
            | WorkItemStateKind::MergingBaseBranch
            | WorkItemStateKind::Reviewing
            | WorkItemStateKind::CreatingPr => self.in_progress += n,
        }
    }
}

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
        crate::error::MaestroError::Db(crate::db::DbError::Adapter(
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
             FROM work_items WHERE ticket_key = ? \
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
             WHERE wi.user_id = ? AND wi.started_at = ( \
                 SELECT MAX(wi2.started_at) FROM work_items wi2 \
                 WHERE wi2.ticket_key = wi.ticket_key AND wi2.user_id = wi.user_id \
             )",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let ticket_key = r.get_text(0)?;
        let state_s = r.get_text(1)?;
        let state_kind = WorkItemStateKind::from_str(&state_s).map_err(|e| {
            crate::error::MaestroError::Db(crate::db::DbError::Adapter(
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
        "{SELECT_WORK_ITEM} WHERE ticket_key = ? ORDER BY started_at DESC LIMIT 1"
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
    if !caller_can_see(&item, caller_user_id, caller_is_admin, /* include_team_visible= */ false) {
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
    let mut where_clauses: Vec<String> = Vec::new();
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
             WHERE workspace_name = ? GROUP BY state_kind",
            vec![DbValue::Text(ws.to_string())],
        ),
        None => (
            "SELECT state_kind, COUNT(*) FROM work_items GROUP BY state_kind",
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

// ── work_item_steps ──────────────────────────────────────────────────────

const SELECT_STEP: &str = "SELECT \
    id, work_item_id, sequence, name, definition_filename, status, \
    started_at, ended_at, exit_code, error_message \
    FROM work_item_steps";

fn decode_step(r: &crate::db::DbRow) -> Result<StepRow> {
    let status_s = r.get_text(5)?;
    let status = StepStatus::from_str(&status_s).map_err(|e| {
        crate::error::MaestroError::Db(crate::db::DbError::Adapter(
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
    let tail = super::upsert::build_update_tail(
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
            crate::error::MaestroError::Db(crate::db::DbError::Adapter(
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

// ── work_item_log_lines ──────────────────────────────────────────────────

/// Append a batch of log lines. Wrapped in a single transaction so a
/// burst from one step lands atomically — partial failure rolls back
/// the whole batch and the caller can retry.
///
/// Empty batches are a no-op (no transaction overhead).
pub async fn append_log_lines(
    adapter: &DbAdapter,
    batch: &[LogLineInsert],
) -> Result<()> {
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

// ── work_item_port_mappings ──────────────────────────────────────────────

/// Insert or update a port mapping. Composite uniqueness on
/// `(work_item_id, container_port, kind)` is enforced in app code via
/// "delete then insert" (the schema has a surrogate `id` PK; uniqueness
/// would require a partial index with `COALESCE(run_command_index, -1)`
/// which is awkward cross-backend).
#[allow(clippy::too_many_arguments)]
pub async fn upsert_port_mapping(
    adapter: &DbAdapter,
    work_item_id: &str,
    container_port: i32,
    host_port: i32,
    proxy_url: &str,
    path_token: &str,
    kind: PortMappingKind,
    run_command_index: Option<i32>,
    created_at: i64,
) -> Result<()> {
    // Wipe any existing mapping for this (work_item, port, kind).
    adapter
        .execute(
            "DELETE FROM work_item_port_mappings \
             WHERE work_item_id = ? AND container_port = ? AND kind = ?",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(container_port),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    adapter
        .execute(
            "INSERT INTO work_item_port_mappings \
                (work_item_id, container_port, host_port, proxy_url, path_token, kind, \
                 run_command_index, created_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(container_port),
                DbValue::I32(host_port),
                DbValue::Text(proxy_url.to_string()),
                DbValue::Text(path_token.to_string()),
                DbValue::Text(kind.as_str().to_string()),
                DbValue::I32Opt(run_command_index),
                DbValue::I64(created_at),
            ],
        )
        .await?;
    Ok(())
}

/// Delete a specific port mapping.
pub async fn delete_port_mapping(
    adapter: &DbAdapter,
    work_item_id: &str,
    container_port: i32,
    kind: PortMappingKind,
) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM work_item_port_mappings \
             WHERE work_item_id = ? AND container_port = ? AND kind = ?",
            vec![
                DbValue::Text(work_item_id.to_string()),
                DbValue::I32(container_port),
                DbValue::Text(kind.as_str().to_string()),
            ],
        )
        .await?;
    Ok(())
}

/// Delete every port mapping for a work item, regardless of kind.
/// Used at editor close to wipe the editor + static + dynamic
/// rows in one shot; cheaper than per-(port, kind) deletes and
/// avoids leaking rows when a route handler forgot one.
pub async fn delete_port_mappings_for_work_item(
    adapter: &DbAdapter,
    work_item_id: &str,
) -> Result<()> {
    adapter
        .execute(
            "DELETE FROM work_item_port_mappings WHERE work_item_id = ?",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    Ok(())
}

/// Shadow-write a port-mapping registration. Wraps
/// [`upsert_port_mapping`] with the standard shadow contract: `None`
/// `db` short-circuits, errors WARN and never propagate.
#[allow(clippy::too_many_arguments)]
pub async fn shadow_upsert_port_mapping(
    db: Option<&crate::db::Database>,
    work_item_id: &str,
    container_port: i32,
    host_port: i32,
    proxy_url: &str,
    path_token: &str,
    kind: PortMappingKind,
    run_command_index: Option<i32>,
    created_at_unix: i64,
) {
    let Some(db) = db else { return };
    if let Err(e) = upsert_port_mapping(
        db.adapter(),
        work_item_id,
        container_port,
        host_port,
        proxy_url,
        path_token,
        kind,
        run_command_index,
        created_at_unix,
    )
    .await
    {
        tracing::warn!(
            work_item_id,
            container_port,
            host_port,
            kind = %kind.as_str(),
            error = %e,
            "Ushadow-write of port mapping upsert failed (route handler progress unaffected)"
        );
    }
}

/// Shadow-clean every port mapping for a work item. Used at editor
/// close so the DB row mirrors the in-memory `path_token_registry`
/// cleanup.
pub async fn shadow_delete_port_mappings_for_work_item(
    db: Option<&crate::db::Database>,
    work_item_id: &str,
) {
    let Some(db) = db else { return };
    if let Err(e) = delete_port_mappings_for_work_item(db.adapter(), work_item_id).await {
        tracing::warn!(
            work_item_id,
            error = %e,
            "Ushadow-clean of port mappings failed (route handler progress unaffected)"
        );
    }
}

/// List all port mappings for a work item.
pub async fn list_port_mappings(
    adapter: &DbAdapter,
    work_item_id: &str,
) -> Result<Vec<PortMappingRow>> {
    let rows = adapter
        .query_all(
            "SELECT id, work_item_id, container_port, host_port, proxy_url, path_token, \
                    kind, run_command_index, created_at \
             FROM work_item_port_mappings WHERE work_item_id = ? \
             ORDER BY kind ASC, container_port ASC",
            vec![DbValue::Text(work_item_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        let kind_s = r.get_text(6)?;
        let kind = PortMappingKind::from_str(&kind_s).map_err(|e| {
            crate::error::MaestroError::Db(crate::db::DbError::Adapter(
                crate::db::adapter::DbError::Sqlx {
                    source: sqlx::Error::Configuration(e.into()),
                },
            ))
        })?;
        out.push(PortMappingRow {
            id: r.get_i64(0)?,
            work_item_id: r.get_text(1)?,
            container_port: r.get_i64(2)? as i32,
            host_port: r.get_i64(3)? as i32,
            proxy_url: r.get_text(4)?,
            path_token: r.get_text(5)?,
            kind,
            run_command_index: r.get_i64_opt(7)?.map(|v| v as i32),
            created_at: r.get_i64(8)?,
        });
    }
    Ok(out)
}

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
    let tail = super::upsert::build_update_tail(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

    /// In-memory SQLite adapter with all migrations applied.
    async fn fresh_adapter() -> DbAdapter {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations");
        DbAdapter::new(DbPool::Sqlite(pool))
    }

    /// Build a minimum-required `WorkItemRow` for tests. Caller may
    /// override fields after calling.
    fn sample_row(id: &str, ticket: &str, owner: Option<&str>) -> WorkItemRow {
        WorkItemRow {
            id: id.to_string(),
            ticket_key: ticket.to_string(),
            workspace_name: "demo".to_string(),
            user_id: owner.map(str::to_string),
            repository_id: None,
            private: false,
            started_manually: false,
            counts_toward_manual_cap: false,
            driver_started: false,
            jira_available: true,
            ticket_summary: Some("seed".to_string()),
            ticket_description: None,
            ticket_type: None,
            ticket_url: None,
            acceptance_criteria: None,
            base_branch: None,
            branch_name: None,
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            last_session_id: None,
            state_kind: WorkItemStateKind::Pending,
            state_payload: None,
            current_step_label: None,
            created_at: 1_700_000_000,
            started_at: 1_700_000_000,
            updated_at: 1_700_000_000,
        }
    }

    async fn seed_user(adapter: &DbAdapter, id: &str, username: &str) {
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
                vec![
                    DbValue::Text(id.to_string()),
                    DbValue::Text(username.to_string()),
                ],
            )
            .await
            .expect("seed user");
    }

    // ── enums round-trip ─────────────────────────────────────────────

    #[test]
    fn state_kind_round_trip() {
        for k in [
            WorkItemStateKind::Pending,
            WorkItemStateKind::Assigning,
            WorkItemStateKind::RetrievingDetails,
            WorkItemStateKind::CreatingWorktree,
            WorkItemStateKind::AddressingTicket,
            WorkItemStateKind::AddressingPrComments,
            WorkItemStateKind::MergingBaseBranch,
            WorkItemStateKind::Reviewing,
            WorkItemStateKind::CreatingPr,
            WorkItemStateKind::Done,
            WorkItemStateKind::Stopped,
            WorkItemStateKind::Error,
            WorkItemStateKind::Paused,
        ] {
            assert_eq!(WorkItemStateKind::from_str(k.as_str()).unwrap(), k);
        }
    }

    #[test]
    fn unknown_state_kind_returns_err() {
        assert!(WorkItemStateKind::from_str("bogus").is_err());
    }

    #[test]
    fn other_enums_round_trip() {
        for s in [
            StepStatus::Running,
            StepStatus::Success,
            StepStatus::Failed,
            StepStatus::Skipped,
        ] {
            assert_eq!(StepStatus::from_str(s.as_str()).unwrap(), s);
        }
        for s in [
            DefRunState::Idle,
            DefRunState::Running,
            DefRunState::Completed,
            DefRunState::Error,
        ] {
            assert_eq!(DefRunState::from_str(s.as_str()).unwrap(), s);
        }
        for s in [
            LogStream::Stdout,
            LogStream::Stderr,
            LogStream::Info,
            LogStream::System,
        ] {
            assert_eq!(LogStream::from_str(s.as_str()).unwrap(), s);
        }
        for k in [
            PortMappingKind::Editor,
            PortMappingKind::Terminal,
            PortMappingKind::Dynamic,
            PortMappingKind::RunCommand,
        ] {
            assert_eq!(PortMappingKind::from_str(k.as_str()).unwrap(), k);
        }
    }

    // ── work_items CRUD ──────────────────────────────────────────────

    #[tokio::test]
    async fn insert_then_get_roundtrips() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        let row = sample_row("wi-1", "PROJ-1", Some("u-alice"));
        insert_work_item(&a, &row).await.unwrap();

        let fetched = get_work_item(&a, "wi-1", Some("u-alice"), false)
            .await
            .unwrap()
            .expect("must exist");
        assert_eq!(fetched, row);
    }

    #[tokio::test]
    async fn get_returns_none_when_caller_lacks_visibility() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        seed_user(&a, "u-bob", "bob").await;
        let row = sample_row("wi-1", "PROJ-1", Some("u-alice"));
        insert_work_item(&a, &row).await.unwrap();

        // Bob can't see Alice's item even though it exists.
        let fetched = get_work_item(&a, "wi-1", Some("u-bob"), false).await.unwrap();
        assert!(fetched.is_none());

        // Admin can.
        let fetched = get_work_item(&a, "wi-1", None, true).await.unwrap();
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn insert_duplicate_workspace_ticket_fails() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        let err = insert_work_item(&a, &sample_row("wi-2", "PROJ-1", Some("u-alice")))
            .await
            .err();
        assert!(err.is_some(), "second insert with same (workspace, ticket) must fail UNIQUE");
    }

    #[tokio::test]
    async fn list_filters_by_workspace_and_owner() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        seed_user(&a, "u-bob", "bob").await;

        let mut alice = sample_row("wi-1", "A-1", Some("u-alice"));
        alice.workspace_name = "ws-a".to_string();
        insert_work_item(&a, &alice).await.unwrap();

        let mut bob_in_a = sample_row("wi-2", "A-2", Some("u-bob"));
        bob_in_a.workspace_name = "ws-a".to_string();
        bob_in_a.started_at = 1_700_000_100;
        insert_work_item(&a, &bob_in_a).await.unwrap();

        let mut bob_in_b = sample_row("wi-3", "B-1", Some("u-bob"));
        bob_in_b.workspace_name = "ws-b".to_string();
        insert_work_item(&a, &bob_in_b).await.unwrap();

        // Alice sees only her own item in ws-a.
        let alice_view = list_work_items(
            &a,
            &WorkItemListQuery {
                caller_user_id: Some("u-alice".into()),
                caller_is_admin: false,
                workspace_name: Some("ws-a".into()),
                state_filter: None,
                include_team_visible: false,
                limit: 100,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(alice_view.len(), 1);
        assert_eq!(alice_view[0].id, "wi-1");

        // Bob in ws-a sees only his ws-a item.
        let bob_view = list_work_items(
            &a,
            &WorkItemListQuery {
                caller_user_id: Some("u-bob".into()),
                caller_is_admin: false,
                workspace_name: Some("ws-a".into()),
                state_filter: None,
                include_team_visible: false,
                limit: 100,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(bob_view.len(), 1);
        assert_eq!(bob_view[0].id, "wi-2");

        // Admin, no workspace filter, sees all three (ordered by
        // started_at DESC: bob_in_a > alice > bob_in_b — but bob_in_b
        // and alice share started_at so id-DESC tie-break gives wi-3
        // before wi-1).
        let admin_view = list_work_items(
            &a,
            &WorkItemListQuery {
                caller_user_id: None,
                caller_is_admin: true,
                workspace_name: None,
                state_filter: None,
                include_team_visible: false,
                limit: 100,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(admin_view.len(), 3);
        assert_eq!(admin_view[0].id, "wi-2"); // most recent started_at
    }

    #[tokio::test]
    async fn list_with_state_filter() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        for (id, key, state) in [
            ("wi-1", "PROJ-1", WorkItemStateKind::Pending),
            ("wi-2", "PROJ-2", WorkItemStateKind::Done),
            ("wi-3", "PROJ-3", WorkItemStateKind::Stopped),
        ] {
            let mut row = sample_row(id, key, Some("u-alice"));
            row.state_kind = state;
            insert_work_item(&a, &row).await.unwrap();
        }
        let view = list_work_items(
            &a,
            &WorkItemListQuery {
                caller_user_id: Some("u-alice".into()),
                caller_is_admin: false,
                workspace_name: None,
                state_filter: Some(vec![WorkItemStateKind::Done, WorkItemStateKind::Stopped]),
                include_team_visible: false,
                limit: 100,
                offset: 0,
            },
        )
        .await
        .unwrap();
        let ids: Vec<&str> = view.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"wi-2"));
        assert!(ids.contains(&"wi-3"));
    }

    #[tokio::test]
    async fn private_items_hidden_from_team_visible_strangers() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        seed_user(&a, "u-bob", "bob").await;
        let mut secret = sample_row("wi-1", "PROJ-1", Some("u-alice"));
        secret.private = true;
        insert_work_item(&a, &secret).await.unwrap();

        // Bob with include_team_visible=true STILL can't see Alice's
        // private item.
        let view = list_work_items(
            &a,
            &WorkItemListQuery {
                caller_user_id: Some("u-bob".into()),
                caller_is_admin: false,
                workspace_name: None,
                state_filter: None,
                include_team_visible: true,
                limit: 100,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert!(view.is_empty());
    }

    #[tokio::test]
    async fn update_state_and_pr_url() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        update_work_item_state(
            &a,
            "wi-1",
            WorkItemStateKind::AddressingTicket,
            Some(r#"{"pass":2}"#),
            Some("Implement ticket (cycle 2/3)"),
            1_700_000_500,
        )
        .await
        .unwrap();
        update_pr_url(&a, "wi-1", Some("https://github.com/o/r/pull/42"), 1_700_000_600)
            .await
            .unwrap();
        update_pr_merged(&a, "wi-1", true, 1_700_000_700).await.unwrap();

        let fetched = get_work_item(&a, "wi-1", None, true).await.unwrap().unwrap();
        assert_eq!(fetched.state_kind, WorkItemStateKind::AddressingTicket);
        assert_eq!(fetched.state_payload.as_deref(), Some(r#"{"pass":2}"#));
        assert_eq!(fetched.current_step_label.as_deref(), Some("Implement ticket (cycle 2/3)"));
        assert_eq!(fetched.pr_url.as_deref(), Some("https://github.com/o/r/pull/42"));
        assert!(fetched.pr_merged);
        assert_eq!(fetched.updated_at, 1_700_000_700);
    }

    #[tokio::test]
    async fn delete_cascades_to_children() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        // Seed one row in each child table.
        let step_id = record_step_start(&a, "wi-1", "step-1", None, 100).await.unwrap();
        upsert_definition_run(
            &a,
            "wi-1",
            "implement.toml",
            DefRunState::Running,
            None,
            Some(100),
            None,
        )
        .await
        .unwrap();
        append_log_lines(
            &a,
            &[LogLineInsert {
                work_item_id: "wi-1".into(),
                step_id: Some(step_id),
                stream: LogStream::Stdout,
                content: "hello".into(),
                emitted_at: 1_700_000_000_000,
            }],
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a,
            "wi-1",
            8080,
            18080,
            "http://localhost:18080",
            "tok",
            PortMappingKind::Editor,
            None,
            100,
        )
        .await
        .unwrap();
        upsert_run_command(&a, "wi-1", 0, "make test", true, None, Some(100), None)
            .await
            .unwrap();

        // Now delete the parent.
        delete_work_item(&a, "wi-1").await.unwrap();
        assert!(get_work_item(&a, "wi-1", None, true).await.unwrap().is_none());

        // Every child table is empty for this id.
        assert!(list_steps(&a, "wi-1").await.unwrap().is_empty());
        assert!(list_definition_runs(&a, "wi-1").await.unwrap().is_empty());
        assert!(
            fetch_log_lines(&a, "wi-1", LogPaging::default())
                .await
                .unwrap()
                .is_empty()
        );
        assert!(list_port_mappings(&a, "wi-1").await.unwrap().is_empty());
        assert!(list_run_commands(&a, "wi-1").await.unwrap().is_empty());
    }

    /// Round-trip the run-command lifecycle DAO helpers.
    /// `start_run_command_row` populates `running=true` + `started_at`;
    /// `finish_run_command_row` flips `running=false` + `ended_at`
    /// while leaving `started_at` intact.
    #[tokio::test]
    async fn start_run_command_row_then_finish_preserves_started_at() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-1", "alice").await;
        insert_work_item(&a, &sample_row("wi-rc", "TICK-1", Some("u-1")))
            .await
            .unwrap();

        start_run_command_row(
            &a,
            "wi-rc",
            0,
            "dev server",
            Some("maestro-run-TICK-1-0"),
            200,
        )
        .await
        .unwrap();

        let rows = list_run_commands(&a, "wi-rc").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command_index, 0);
        assert_eq!(rows[0].name, "dev server");
        assert!(rows[0].running, "running flag set");
        assert_eq!(
            rows[0].container_id.as_deref(),
            Some("maestro-run-TICK-1-0")
        );
        assert_eq!(rows[0].started_at, Some(200));
        assert_eq!(rows[0].ended_at, None);

        // Finish.
        finish_run_command_row(&a, "wi-rc", 0, 350).await.unwrap();

        let rows = list_run_commands(&a, "wi-rc").await.unwrap();
        assert_eq!(rows.len(), 1, "UPDATE-only — never duplicates");
        assert!(!rows[0].running, "running flag cleared");
        assert_eq!(
            rows[0].started_at,
            Some(200),
            "finish must preserve start timestamp"
        );
        assert_eq!(rows[0].ended_at, Some(350));
        // container_id and name preserved.
        assert_eq!(
            rows[0].container_id.as_deref(),
            Some("maestro-run-TICK-1-0")
        );
        assert_eq!(rows[0].name, "dev server");

        // Re-starting (e.g. user clicks Run again) must clear
        // `ended_at` and refresh `started_at`.
        start_run_command_row(
            &a,
            "wi-rc",
            0,
            "dev server",
            Some("maestro-run-TICK-1-0"),
            900,
        )
        .await
        .unwrap();
        let rows = list_run_commands(&a, "wi-rc").await.unwrap();
        assert!(rows[0].running);
        assert_eq!(rows[0].started_at, Some(900));
        assert_eq!(rows[0].ended_at, None);
    }

    /// `repository_id` round-trips through insert + decode. Both Some
    /// and None must survive intact; the column is the sole input to
    /// `require_workflow_access`, so a silent loss here would defeat
    /// the visibility check.
    #[tokio::test]
    async fn work_item_repository_id_round_trips_insert_and_get() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-1", "alice").await;

        // Row WITH a repo association.
        let mut with_repo = sample_row("wf-with", "T-1", Some("u-1"));
        with_repo.repository_id = Some("repo-123".into());
        insert_work_item(&a, &with_repo).await.unwrap();

        let got = get_work_item(&a, "wf-with", Some("u-1"), false)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(got.repository_id.as_deref(), Some("repo-123"));

        // Row WITHOUT a repo association (legacy).
        let without_repo = sample_row("wf-without", "T-2", Some("u-1"));
        insert_work_item(&a, &without_repo).await.unwrap();
        let got = get_work_item(&a, "wf-without", Some("u-1"), false)
            .await
            .unwrap()
            .expect("row");
        assert_eq!(got.repository_id, None);
    }

    /// `delete_port_mappings_for_work_item` wipes every port mapping
    /// for a work item regardless of kind, and is a silent no-op when
    /// none exist. Mirrors the contract
    /// `shadow_delete_port_mappings_for_work_item` relies on at
    /// `close_editor`.
    #[tokio::test]
    async fn delete_port_mappings_for_work_item_wipes_all_kinds() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-1", "alice").await;
        insert_work_item(&a, &sample_row("wi-pm", "T-1", Some("u-1")))
            .await
            .unwrap();

        // Bulk-delete on an empty table is a clean no-op.
        delete_port_mappings_for_work_item(&a, "wi-pm")
            .await
            .unwrap();

        // Seed 3 mappings of different kinds.
        upsert_port_mapping(
            &a, "wi-pm", 9100, 9100, "/s/tok-edit/", "tok-edit",
            PortMappingKind::Editor, None, 100,
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a, "wi-pm", 3000, 19100, "/s/tok-app/", "tok-app",
            PortMappingKind::Dynamic, None, 110,
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a, "wi-pm", 5173, 19101, "/s/tok-rc/", "tok-rc",
            PortMappingKind::RunCommand, Some(0), 120,
        )
        .await
        .unwrap();
        assert_eq!(list_port_mappings(&a, "wi-pm").await.unwrap().len(), 3);

        // A second work-item's row must NOT be touched.
        insert_work_item(&a, &sample_row("wi-other", "T-2", Some("u-1")))
            .await
            .unwrap();
        upsert_port_mapping(
            &a, "wi-other", 9100, 9100, "/s/keep/", "keep",
            PortMappingKind::Editor, None, 130,
        )
        .await
        .unwrap();

        delete_port_mappings_for_work_item(&a, "wi-pm")
            .await
            .unwrap();

        assert!(
            list_port_mappings(&a, "wi-pm").await.unwrap().is_empty(),
            "every row for wi-pm should be gone"
        );
        assert_eq!(
            list_port_mappings(&a, "wi-other").await.unwrap().len(),
            1,
            "rows for other work-items must be preserved"
        );
    }

    /// `finish_run_command_row` is a silent no-op when no prior start
    /// row exists. Critical: a stop that races ahead of the start
    /// shadow-write must not produce an inconsistent row.
    #[tokio::test]
    async fn finish_run_command_row_is_silent_noop_without_prior_start() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-1", "alice").await;
        insert_work_item(&a, &sample_row("wi-orphan", "T-1", Some("u-1")))
            .await
            .unwrap();

        finish_run_command_row(&a, "wi-orphan", 0, 42).await.unwrap();

        assert!(
            list_run_commands(&a, "wi-orphan").await.unwrap().is_empty(),
            "finish without start must not synthesise a row"
        );
    }

    #[tokio::test]
    async fn counts_aggregate_by_state() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        for (id, k) in [
            ("a", WorkItemStateKind::Pending),
            ("b", WorkItemStateKind::Pending),
            ("c", WorkItemStateKind::Done),
            ("d", WorkItemStateKind::Error),
        ] {
            let mut row = sample_row(id, &format!("KEY-{id}"), Some("u-alice"));
            row.state_kind = k;
            insert_work_item(&a, &row).await.unwrap();
        }
        let counts = count_by_state(&a, None).await.unwrap();
        assert_eq!(counts.pending, 2);
        assert_eq!(counts.done, 1);
        assert_eq!(counts.error, 1);
        assert_eq!(counts.stopped, 0);
    }

    // ── steps ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn record_step_start_assigns_sequential_ids() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        let id1 = record_step_start(&a, "wi-1", "bootstrap", None, 100).await.unwrap();
        let id2 = record_step_start(&a, "wi-1", "implement", Some("implement.toml"), 200)
            .await
            .unwrap();
        let id3 = record_step_start(&a, "wi-1", "review", Some("review.toml"), 300)
            .await
            .unwrap();

        let steps = list_steps(&a, "wi-1").await.unwrap();
        assert_eq!(steps.len(), 3);
        assert_eq!(steps[0].id, id1);
        assert_eq!(steps[0].sequence, 0);
        assert_eq!(steps[0].status, StepStatus::Running);
        assert_eq!(steps[1].id, id2);
        assert_eq!(steps[1].sequence, 1);
        assert_eq!(steps[2].id, id3);
        assert_eq!(steps[2].sequence, 2);
    }

    #[tokio::test]
    async fn record_step_end_finalises_step() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        let step_id = record_step_start(&a, "wi-1", "compile", None, 100).await.unwrap();
        record_step_end(&a, step_id, StepStatus::Failed, Some(2), Some("rustc err"), 500)
            .await
            .unwrap();
        let steps = list_steps(&a, "wi-1").await.unwrap();
        assert_eq!(steps[0].status, StepStatus::Failed);
        assert_eq!(steps[0].exit_code, Some(2));
        assert_eq!(steps[0].error_message.as_deref(), Some("rustc err"));
        assert_eq!(steps[0].ended_at, Some(500));
    }

    // ── definition runs ──────────────────────────────────────────────

    #[tokio::test]
    async fn upsert_definition_run_is_idempotent() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        upsert_definition_run(
            &a,
            "wi-1",
            "implement.toml",
            DefRunState::Running,
            None,
            Some(100),
            None,
        )
        .await
        .unwrap();
        upsert_definition_run(
            &a,
            "wi-1",
            "implement.toml",
            DefRunState::Completed,
            None,
            Some(100),
            Some(500),
        )
        .await
        .unwrap();

        let runs = list_definition_runs(&a, "wi-1").await.unwrap();
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, DefRunState::Completed);
        assert_eq!(runs[0].ended_at, Some(500));
    }

    // ── log lines ────────────────────────────────────────────────────

    #[tokio::test]
    async fn append_then_fetch_log_lines_paged() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        let step_id = record_step_start(&a, "wi-1", "compile", None, 100).await.unwrap();

        let mut batch = Vec::new();
        for i in 0..10 {
            batch.push(LogLineInsert {
                work_item_id: "wi-1".into(),
                step_id: Some(step_id),
                stream: if i % 2 == 0 {
                    LogStream::Stdout
                } else {
                    LogStream::Stderr
                },
                content: format!("line {i}"),
                emitted_at: 1_700_000_000_000 + i,
            });
        }
        append_log_lines(&a, &batch).await.unwrap();

        let first_five = fetch_log_lines(
            &a,
            "wi-1",
            LogPaging {
                step_id: None,
                limit: 5,
                offset: 0,
            },
        )
        .await
        .unwrap();
        assert_eq!(first_five.len(), 5);
        assert_eq!(first_five[0].content, "line 0");
        assert_eq!(first_five[4].content, "line 4");

        let next_five = fetch_log_lines(
            &a,
            "wi-1",
            LogPaging {
                step_id: None,
                limit: 5,
                offset: 5,
            },
        )
        .await
        .unwrap();
        assert_eq!(next_five.len(), 5);
        assert_eq!(next_five[0].content, "line 5");
    }

    #[tokio::test]
    async fn append_empty_batch_is_no_op() {
        let a = fresh_adapter().await;
        append_log_lines(&a, &[]).await.unwrap(); // must not panic / error
    }

    #[tokio::test]
    async fn purge_drops_older_lines() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        append_log_lines(
            &a,
            &[
                LogLineInsert {
                    work_item_id: "wi-1".into(),
                    step_id: None,
                    stream: LogStream::Stdout,
                    content: "old".into(),
                    emitted_at: 100,
                },
                LogLineInsert {
                    work_item_id: "wi-1".into(),
                    step_id: None,
                    stream: LogStream::Stdout,
                    content: "new".into(),
                    emitted_at: 1000,
                },
            ],
        )
        .await
        .unwrap();
        let purged = purge_log_lines_older_than(&a, 500).await.unwrap();
        assert_eq!(purged, 1);
        let remaining = fetch_log_lines(&a, "wi-1", LogPaging::default()).await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].content, "new");
    }

    // ── port mappings + run commands ─────────────────────────────────

    #[tokio::test]
    async fn upsert_port_mapping_replaces_existing() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        upsert_port_mapping(
            &a,
            "wi-1",
            8080,
            18080,
            "http://localhost:18080",
            "tok-1",
            PortMappingKind::Editor,
            None,
            100,
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a,
            "wi-1",
            8080,
            18099,
            "http://localhost:18099",
            "tok-2",
            PortMappingKind::Editor,
            None,
            200,
        )
        .await
        .unwrap();
        let mappings = list_port_mappings(&a, "wi-1").await.unwrap();
        assert_eq!(mappings.len(), 1, "second upsert must REPLACE not duplicate");
        assert_eq!(mappings[0].host_port, 18099);
        assert_eq!(mappings[0].path_token, "tok-2");
    }

    #[tokio::test]
    async fn delete_port_mapping_targets_exactly_one_kind() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        // Two kinds on the same container port — independent mappings.
        upsert_port_mapping(
            &a, "wi-1", 8080, 18080, "u-editor", "tok-e", PortMappingKind::Editor, None, 100,
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a, "wi-1", 8080, 28080, "u-terminal", "tok-t", PortMappingKind::Terminal, None, 100,
        )
        .await
        .unwrap();
        delete_port_mapping(&a, "wi-1", 8080, PortMappingKind::Editor).await.unwrap();
        let remaining = list_port_mappings(&a, "wi-1").await.unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].kind, PortMappingKind::Terminal);
    }

    #[tokio::test]
    async fn upsert_run_command_is_idempotent() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        upsert_run_command(&a, "wi-1", 0, "make test", true, Some("c-1"), Some(100), None)
            .await
            .unwrap();
        upsert_run_command(&a, "wi-1", 0, "make test", false, Some("c-1"), Some(100), Some(500))
            .await
            .unwrap();
        let rcs = list_run_commands(&a, "wi-1").await.unwrap();
        assert_eq!(rcs.len(), 1);
        assert!(!rcs[0].running);
        assert_eq!(rcs[0].ended_at, Some(500));
    }
}
