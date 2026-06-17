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

mod def_runs;
mod logs;
mod ports;
mod run_commands;
mod state;
mod steps;

pub use def_runs::*;
pub use logs::*;
pub use ports::*;
pub use run_commands::*;
pub use state::*;
pub use steps::*;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::DbValue;
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

    /// Regression for the stale "Show PR" button: the work_items row is
    /// INSERTed at creation with an empty branch / PR URL, and nothing updated
    /// them afterwards — so the DB row stayed NULL for a completed run's PR. A
    /// sibling step write failing (the scenario commit 9336fcd worked around)
    /// must NOT prevent the PR fields from landing on the row, because they are
    /// written by independent `id`-keyed UPDATEs. This test proves the row is
    /// reliably updated post-insert.
    #[tokio::test]
    async fn pr_fields_persist_to_row_after_insert_independent_of_step_writes() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        // The freshly-inserted row has no branch / PR — the stale state the UI
        // fallback used to paper over.
        let before = get_work_item(&a, "wi-1", None, true)
            .await
            .unwrap()
            .expect("row exists");
        assert!(before.branch_name.is_none());
        assert!(before.pr_url.is_none());
        assert!(!before.pr_merged);

        // A sibling step write targeting a non-existent parent (the FK-violation
        // scenario) is best-effort and must not block the PR-field updates that
        // follow. We ignore its outcome exactly as the engine's shadow helpers
        // swallow such failures.
        let _ = record_step_start(&a, "wi-DOES-NOT-EXIST", "ghost step", None, 1_700_000_100).await;

        // The engine learns these as the run progresses; persist each.
        update_branch_name(&a, "wi-1", Some("feat/proj-1"), 1_700_000_200)
            .await
            .unwrap();
        update_pr_url(
            &a,
            "wi-1",
            Some("https://github.com/example/repo/pull/7"),
            1_700_000_300,
        )
        .await
        .unwrap();
        update_pr_merged(&a, "wi-1", true, 1_700_000_400)
            .await
            .unwrap();

        let after = get_work_item(&a, "wi-1", None, true)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(after.branch_name.as_deref(), Some("feat/proj-1"));
        assert_eq!(
            after.pr_url.as_deref(),
            Some("https://github.com/example/repo/pull/7")
        );
        assert!(after.pr_merged);
    }

    /// `update_work_item_branch_and_worktree` writes BOTH columns in one
    /// statement (cutover invariant I2 — no torn branch-new + worktree-NULL
    /// row), and crucially persists `worktree_path`, which the prior
    /// branch-only write never did.
    #[tokio::test]
    async fn branch_and_worktree_update_sets_both_columns() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        // Fresh row: both empty.
        let before = get_work_item(&a, "wi-1", None, true)
            .await
            .unwrap()
            .expect("row exists");
        assert!(before.branch_name.is_none());
        assert!(before.worktree_path.is_none());

        update_work_item_branch_and_worktree(
            &a,
            "wi-1",
            Some("feat/proj-1"),
            Some("/tmp/wt/proj-1"),
            1_700_000_500,
        )
        .await
        .unwrap();

        let after = get_work_item(&a, "wi-1", None, true)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(after.branch_name.as_deref(), Some("feat/proj-1"));
        assert_eq!(after.worktree_path.as_deref(), Some("/tmp/wt/proj-1"));
        assert_eq!(after.updated_at, 1_700_000_500);
    }

    /// `update_work_item_pr_url_and_state` writes the PR URL AND the state
    /// columns in one statement (cutover invariant I2), so the "finished —
    /// here is the PR" event lands atomically: never pr-new + state-old.
    #[tokio::test]
    async fn pr_url_and_state_update_sets_both_atomically() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();

        update_work_item_pr_url_and_state(
            &a,
            "wi-1",
            Some("https://github.com/example/repo/pull/9"),
            WorkItemStateKind::Done,
            None,
            None,
            1_700_000_600,
        )
        .await
        .unwrap();

        let after = get_work_item(&a, "wi-1", None, true)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(
            after.pr_url.as_deref(),
            Some("https://github.com/example/repo/pull/9")
        );
        assert_eq!(after.state_kind, WorkItemStateKind::Done);
        assert_eq!(after.updated_at, 1_700_000_600);
    }

    #[tokio::test]
    async fn get_returns_none_when_caller_lacks_visibility() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        seed_user(&a, "u-bob", "bob").await;
        let row = sample_row("wi-1", "PROJ-1", Some("u-alice"));
        insert_work_item(&a, &row).await.unwrap();

        // Bob can't see Alice's item even though it exists.
        let fetched = get_work_item(&a, "wi-1", Some("u-bob"), false)
            .await
            .unwrap();
        assert!(fetched.is_none());

        // Admin can.
        let fetched = get_work_item(&a, "wi-1", None, true).await.unwrap();
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn duplicate_workspace_ticket_allowed_after_soft_delete_migration() {
        // The soft-delete migration dropped the UNIQUE (workspace, ticket)
        // index so a soft-deleted run and a fresh re-add coexist. Two rows
        // with the same (workspace, ticket) must now BOTH insert.
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        insert_work_item(&a, &sample_row("wi-2", "PROJ-1", Some("u-alice")))
            .await
            .expect("duplicate (workspace, ticket) must be allowed post-migration");
    }

    #[tokio::test]
    async fn soft_deleted_row_drops_out_of_lookup_and_re_add_wins() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        // First run.
        let mut first = sample_row("wi-1", "PROJ-1", Some("u-alice"));
        first.started_at = 100;
        insert_work_item(&a, &first).await.unwrap();
        // Soft-delete it.
        soft_delete_work_item(&a, "wi-1", 150).await.unwrap();
        // Lookup must now miss the deleted row.
        assert!(
            get_work_item_by_ticket_key(&a, "PROJ-1")
                .await
                .unwrap()
                .is_none(),
            "soft-deleted run must not be returned by the live lookup"
        );
        // Re-add (new run) inserts cleanly and becomes the live row.
        let mut second = sample_row("wi-2", "PROJ-1", Some("u-alice"));
        second.started_at = 200;
        insert_work_item(&a, &second).await.unwrap();
        let live = get_work_item_by_ticket_key(&a, "PROJ-1")
            .await
            .unwrap()
            .expect("re-added run must be live");
        assert_eq!(live.id, "wi-2", "the fresh run must win the lookup");
    }

    #[tokio::test]
    async fn count_runs_for_ticket_includes_soft_deleted_history() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        assert_eq!(
            count_runs_for_ticket(&a, "demo", "PROJ-1").await.unwrap(),
            0
        );

        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        soft_delete_work_item(&a, "wi-1", 150).await.unwrap();
        // Even after soft-delete, the run still counts toward history so the
        // next branch gets a unique suffix.
        assert_eq!(
            count_runs_for_ticket(&a, "demo", "PROJ-1").await.unwrap(),
            1
        );

        insert_work_item(&a, &sample_row("wi-2", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        assert_eq!(
            count_runs_for_ticket(&a, "demo", "PROJ-1").await.unwrap(),
            2
        );
        // A different ticket is independent.
        assert_eq!(
            count_runs_for_ticket(&a, "demo", "PROJ-9").await.unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn latest_pr_url_for_ticket_reads_across_soft_deleted_history() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        // No prior PR yet.
        assert!(
            latest_pr_url_for_ticket(&a, "demo", "PROJ-1")
                .await
                .unwrap()
                .is_none()
        );

        // First run records a PR, then is soft-deleted (the reported scenario).
        let mut first = sample_row("wi-1", "PROJ-1", Some("u-alice"));
        first.started_at = 100;
        first.pr_url = Some("https://github.com/o/r/pull/18".to_string());
        insert_work_item(&a, &first).await.unwrap();
        soft_delete_work_item(&a, "wi-1", 150).await.unwrap();

        assert_eq!(
            latest_pr_url_for_ticket(&a, "demo", "PROJ-1")
                .await
                .unwrap()
                .as_deref(),
            Some("https://github.com/o/r/pull/18"),
            "a soft-deleted past run's PR must still be discoverable for the confirmation"
        );

        // A newer run with its own PR wins by started_at.
        let mut second = sample_row("wi-2", "PROJ-1", Some("u-alice"));
        second.started_at = 200;
        second.pr_url = Some("https://github.com/o/r/pull/27".to_string());
        insert_work_item(&a, &second).await.unwrap();
        assert_eq!(
            latest_pr_url_for_ticket(&a, "demo", "PROJ-1")
                .await
                .unwrap()
                .as_deref(),
            Some("https://github.com/o/r/pull/27")
        );
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
        update_pr_url(
            &a,
            "wi-1",
            Some("https://github.com/o/r/pull/42"),
            1_700_000_600,
        )
        .await
        .unwrap();
        update_pr_merged(&a, "wi-1", true, 1_700_000_700)
            .await
            .unwrap();

        let fetched = get_work_item(&a, "wi-1", None, true)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.state_kind, WorkItemStateKind::AddressingTicket);
        assert_eq!(fetched.state_payload.as_deref(), Some(r#"{"pass":2}"#));
        assert_eq!(
            fetched.current_step_label.as_deref(),
            Some("Implement ticket (cycle 2/3)")
        );
        assert_eq!(
            fetched.pr_url.as_deref(),
            Some("https://github.com/o/r/pull/42")
        );
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
        let step_id = record_step_start(&a, "wi-1", "step-1", None, 100)
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
        assert!(
            get_work_item(&a, "wi-1", None, true)
                .await
                .unwrap()
                .is_none()
        );

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
            Some("takuto-run-TICK-1-0"),
            200,
        )
        .await
        .unwrap();

        let rows = list_run_commands(&a, "wi-rc").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].command_index, 0);
        assert_eq!(rows[0].name, "dev server");
        assert!(rows[0].running, "running flag set");
        assert_eq!(rows[0].container_id.as_deref(), Some("takuto-run-TICK-1-0"));
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
        assert_eq!(rows[0].container_id.as_deref(), Some("takuto-run-TICK-1-0"));
        assert_eq!(rows[0].name, "dev server");

        // Re-starting (e.g. user clicks Run again) must clear
        // `ended_at` and refresh `started_at`.
        start_run_command_row(
            &a,
            "wi-rc",
            0,
            "dev server",
            Some("takuto-run-TICK-1-0"),
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
            &a,
            "wi-pm",
            9100,
            9100,
            "/s/tok-edit/",
            "tok-edit",
            PortMappingKind::Editor,
            None,
            100,
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a,
            "wi-pm",
            3000,
            19100,
            "/s/tok-app/",
            "tok-app",
            PortMappingKind::Dynamic,
            None,
            110,
        )
        .await
        .unwrap();
        upsert_port_mapping(
            &a,
            "wi-pm",
            5173,
            19101,
            "/s/tok-rc/",
            "tok-rc",
            PortMappingKind::RunCommand,
            Some(0),
            120,
        )
        .await
        .unwrap();
        assert_eq!(list_port_mappings(&a, "wi-pm").await.unwrap().len(), 3);

        // A second work-item's row must NOT be touched.
        insert_work_item(&a, &sample_row("wi-other", "T-2", Some("u-1")))
            .await
            .unwrap();
        upsert_port_mapping(
            &a,
            "wi-other",
            9100,
            9100,
            "/s/keep/",
            "keep",
            PortMappingKind::Editor,
            None,
            130,
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

        finish_run_command_row(&a, "wi-orphan", 0, 42)
            .await
            .unwrap();

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

        let id1 = record_step_start(&a, "wi-1", "bootstrap", None, 100)
            .await
            .unwrap();
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
        let step_id = record_step_start(&a, "wi-1", "compile", None, 100)
            .await
            .unwrap();
        record_step_end(
            &a,
            step_id,
            StepStatus::Failed,
            Some(2),
            Some("rustc err"),
            500,
        )
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

    #[tokio::test]
    async fn count_steps_of_latest_completed_def_picks_completed_run() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-3", "GH-3", Some("u-alice")))
            .await
            .unwrap();
        // Empty item: a run that never completed must not appear in the map.
        insert_work_item(&a, &sample_row("wi-empty", "GH-9", Some("u-alice")))
            .await
            .unwrap();

        // Mirror the real GH-3 shape: an older errored `implement_ticket`
        // run plus a later completed `implement` run. Only the latter counts.
        upsert_definition_run(
            &a,
            "wi-3",
            "implement_ticket",
            DefRunState::Error,
            Some("boom"),
            Some(50),
            Some(100),
        )
        .await
        .unwrap();
        upsert_definition_run(
            &a,
            "wi-3",
            "implement",
            DefRunState::Completed,
            None,
            Some(200),
            Some(500),
        )
        .await
        .unwrap();
        // wi-empty has a running (not completed) run.
        upsert_definition_run(
            &a,
            "wi-empty",
            "implement",
            DefRunState::Running,
            None,
            Some(200),
            None,
        )
        .await
        .unwrap();

        // One step under the errored run, three under the completed one.
        record_step_start(&a, "wi-3", "Implement ticket", Some("implement_ticket"), 60)
            .await
            .unwrap();
        for (name, ts) in [("Implement", 210), ("Review", 220), ("Create PR", 230)] {
            record_step_start(&a, "wi-3", name, Some("implement"), ts)
                .await
                .unwrap();
        }
        record_step_start(&a, "wi-empty", "Implement", Some("implement"), 210)
            .await
            .unwrap();

        let counts =
            count_steps_of_latest_completed_def(&a, &["wi-3".to_string(), "wi-empty".to_string()])
                .await
                .unwrap();

        // Latest completed flow = `implement` (3 steps); errored run ignored.
        assert_eq!(counts.get("wi-3").copied(), Some(3));
        // No completed run → absent from the map.
        assert!(!counts.contains_key("wi-empty"));

        // Empty input short-circuits to an empty map.
        let empty = count_steps_of_latest_completed_def(&a, &[]).await.unwrap();
        assert!(empty.is_empty());
    }

    // ── log lines ────────────────────────────────────────────────────

    #[tokio::test]
    async fn append_then_fetch_log_lines_paged() {
        let a = fresh_adapter().await;
        seed_user(&a, "u-alice", "alice").await;
        insert_work_item(&a, &sample_row("wi-1", "PROJ-1", Some("u-alice")))
            .await
            .unwrap();
        let step_id = record_step_start(&a, "wi-1", "compile", None, 100)
            .await
            .unwrap();

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
        let remaining = fetch_log_lines(&a, "wi-1", LogPaging::default())
            .await
            .unwrap();
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
        assert_eq!(
            mappings.len(),
            1,
            "second upsert must REPLACE not duplicate"
        );
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
            &a,
            "wi-1",
            8080,
            18080,
            "u-editor",
            "tok-e",
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
            28080,
            "u-terminal",
            "tok-t",
            PortMappingKind::Terminal,
            None,
            100,
        )
        .await
        .unwrap();
        delete_port_mapping(&a, "wi-1", 8080, PortMappingKind::Editor)
            .await
            .unwrap();
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
        upsert_run_command(
            &a,
            "wi-1",
            0,
            "make test",
            true,
            Some("c-1"),
            Some(100),
            None,
        )
        .await
        .unwrap();
        upsert_run_command(
            &a,
            "wi-1",
            0,
            "make test",
            false,
            Some("c-1"),
            Some(100),
            Some(500),
        )
        .await
        .unwrap();
        let rcs = list_run_commands(&a, "wi-1").await.unwrap();
        assert_eq!(rcs.len(), 1);
        assert!(!rcs[0].running);
        assert_eq!(rcs[0].ended_at, Some(500));
    }
}
