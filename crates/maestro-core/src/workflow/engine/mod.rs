// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

mod types;
mod repository;
mod event_bus;
mod context;
mod driver;
mod persistence;
mod lifecycle;
mod transitions;
mod definitions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::{RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

use repository::WorkflowRepository;
use event_bus::WorkflowEventBus;
use persistence::WorkflowPersistence;
use lifecycle::WorkflowLifecycle;
use transitions::WorkflowTransitions;
use definitions::WorkflowDefinitionManager;

use crate::actions::traits::ExternalActions;
use crate::config::{Config, TicketingSystem};
use crate::error::Result;

pub use types::{Workflow, WorkflowEvent, TerminalLine, MarkDoneOutcome};

pub struct WorkflowEngine {
    pub config: Arc<RwLock<Config>>,
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub actions: Arc<dyn ExternalActions>,
    /// Limits concurrent heavy work (mise/install/agent sessions) across workflows. Paused workflows do not hold a permit.
    agent_run_semaphore: Arc<Semaphore>,
    /// When set, workflow drivers that exit with [`MaestroError::Cancelled`] do not move the workflow to Error (graceful container shutdown + snapshot).
    pub suppress_cancelled_as_error: Arc<AtomicBool>,
    /// `true` when acli (Atlassian CLI) is authenticated. When `false`, workflows skip all Jira
    /// operations (assign, transition, retrieve details) and the poller is not started.
    pub jira_available: Arc<AtomicBool>,
    /// Which ticketing system is configured for this engine run.
    pub ticketing_system: TicketingSystem,
    /// Directory containing dynamic workflow definition YAML files, resolved at construction time.
    pub workflows_dir: PathBuf,
    // Service structs
    persistence: WorkflowPersistence,
    lifecycle: WorkflowLifecycle,
    transitions: WorkflowTransitions,
    definitions: WorkflowDefinitionManager,
}

impl WorkflowEngine {
    pub fn new(
        config: Arc<RwLock<Config>>,
        actions: Arc<dyn ExternalActions>,
        max_concurrent_workflows: usize,
        jira_available: Arc<AtomicBool>,
        ticketing_system: TicketingSystem,
        workflows_dir: PathBuf,
    ) -> Self {
        let repository = Arc::new(WorkflowRepository::new());
        let event_bus = Arc::new(WorkflowEventBus::new());
        let permits = max_concurrent_workflows.max(1);
        let agent_run_semaphore = Arc::new(Semaphore::new(permits));
        let suppress_cancelled_as_error = Arc::new(AtomicBool::new(false));

        let persistence = WorkflowPersistence::new(
            repository.clone(),
            config.clone(),
            event_bus.clone(),
            suppress_cancelled_as_error.clone(),
            actions.clone(),
        );

        let definitions = WorkflowDefinitionManager::new(
            repository.clone(),
            event_bus.clone(),
            config.clone(),
            actions.clone(),
            agent_run_semaphore.clone(),
            suppress_cancelled_as_error.clone(),
            workflows_dir.clone(),
        );

        let lifecycle = WorkflowLifecycle::new(
            repository.clone(),
            event_bus.clone(),
            actions.clone(),
            config.clone(),
            jira_available.clone(),
            ticketing_system.clone(),
            workflows_dir.clone(),
        );

        let transitions = WorkflowTransitions::new(
            repository.clone(),
            event_bus.clone(),
            actions.clone(),
            config.clone(),
            agent_run_semaphore.clone(),
            suppress_cancelled_as_error.clone(),
            jira_available.clone(),
            workflows_dir.clone(),
        );

        Self {
            config,
            repository,
            event_bus,
            actions,
            agent_run_semaphore,
            suppress_cancelled_as_error,
            jira_available,
            ticketing_system,
            workflows_dir,
            persistence,
            lifecycle,
            transitions,
            definitions,
        }
    }

    /// Convenience accessor for the inner `Arc<RwLock<HashMap<String, Workflow>>>`.
    /// Used by external callers (e.g. `maestro-web` routes) that still need direct
    /// map access during the incremental refactor.  Remove once those callers are
    /// migrated to repository methods.
    pub fn workflows_arc(&self) -> Arc<RwLock<HashMap<String, Workflow>>> {
        self.repository.inner_arc()
    }

    /// Returns the number of active broadcast subscribers (for logging / metrics).
    pub fn event_subscriber_count(&self) -> usize {
        self.event_bus.sender().receiver_count()
    }

    /// Returns a clone of the raw broadcast sender (transitional — for callers that
    /// need to pass a `broadcast::Sender` to free functions not yet refactored).
    pub fn event_sender(&self) -> broadcast::Sender<WorkflowEvent> {
        self.event_bus.sender().clone()
    }

    /// Count workflows that reserve a slot against `max_concurrent_workflows` (includes **Paused**).
    pub async fn concurrency_slots_in_use(&self) -> usize {
        self.repository.count_occupying_slot().await
    }

    /// Count workflows still on the dashboard (every row in the map), including **Done**, **Paused**, **Stopped**, **Error**, and in-progress — until **Mark as Done** or **Delete** removes the row.
    pub async fn dashboard_workflow_count(&self) -> usize {
        self.repository.count_all().await
    }

    /// Periodic snapshot sync during normal operation (called by background task).
    pub async fn sync_workflow_snapshot(&self) -> Result<()> {
        self.persistence.sync().await
    }

    /// Write `workflow_snapshot.json` and cancel drivers so processes stop, without Jira unassign / **Stopped** (for container restart).
    pub async fn persist_interrupt_for_restart(&self) -> Result<()> {
        self.persistence.persist_interrupt().await
    }

    /// Load snapshot from disk, insert workflows, spawn drivers. Removes the snapshot file on success.
    pub async fn restore_persisted_workflows(&self) -> Result<usize> {
        self.persistence.restore(
            &self.workflows_dir,
            self.agent_run_semaphore.clone(),
            self.suppress_cancelled_as_error.clone(),
        ).await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_bus.subscribe()
    }

    pub async fn start_workflow(
        &self,
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        ticket_description: Option<String>,
    ) -> Result<String> {
        self.lifecycle.start_workflow(ticket_key, ticket_summary, started_manually, ticket_description, &self.definitions).await
    }

    /// Add a workflow to the dashboard without spawning the driver.
    /// For Jira tickets, assigns the ticket and transitions to In Progress (best-effort).
    pub async fn add_to_dashboard(
        &self,
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        ticket_description: Option<String>,
    ) -> Result<String> {
        self.lifecycle.add_to_dashboard(ticket_key, ticket_summary, started_manually, ticket_description).await
    }

    pub async fn get_workflow_ids(&self) -> Vec<String> {
        self.repository.get_ids().await
    }

    pub async fn active_workflow_count(&self) -> usize {
        self.repository.count_active().await
    }

    pub async fn pause_workflow(&self, ticket_key: &str) -> Result<()> {
        self.transitions.pause_workflow(ticket_key).await
    }

    pub async fn resume_workflow(&self, ticket_key: &str) -> Result<()> {
        self.transitions.resume_workflow(ticket_key).await
    }

    pub async fn stop_workflow(&self, ticket_key: &str) -> Result<()> {
        self.transitions.stop_workflow(ticket_key).await
    }

    pub async fn retry_workflow(&self, ticket_key: &str) -> Result<String> {
        self.transitions.retry_workflow(ticket_key, &self.lifecycle, &self.definitions).await
    }

    /// Resume a failed or stopped workflow by retrying all Error-state workflow definitions.
    pub async fn resume_from_error(&self, ticket_key: &str) -> Result<()> {
        self.transitions.resume_from_error(ticket_key, &self.definitions).await
    }

    /// Manual dashboard starts with **`started_manually`** that are not **Done** / **Stopped** / **Error**.
    pub async fn manual_workflows_toward_cap_count(&self) -> usize {
        self.repository.inner_arc()
            .read()
            .await
            .values()
            .filter(|w| w.started_manually && w.state.occupies_concurrency_slot())
            .count()
    }

    pub async fn stop_all_workflows(&self) {
        self.lifecycle.stop_all_workflows(&self.transitions).await
    }

    /// Remove a workflow from the dashboard when it is not **running** (see [`WorkflowState::is_active`]).
    /// Best-effort worktree removal; no Jira transitions. Cancels the driver token if a paused task is still attached.
    pub async fn delete_workflow(&self, ticket_key: &str) -> Result<()> {
        self.lifecycle.delete_workflow(ticket_key, &self.persistence).await
    }

    /// Jira **Done** transition (configured status name) and remove worktree; remove workflow from the map only if both succeed.
    pub async fn mark_work_done(&self, ticket_key: &str) -> Result<MarkDoneOutcome> {
        let engine_workflows = self.repository.inner_arc();
        let engine_config = Arc::clone(&self.config);
        let ticket_key_owned = ticket_key.to_string();
        let tx = self.event_bus.sender().clone();
        self.lifecycle.mark_work_done(ticket_key, &self.persistence, move |mut event| {
            let workflows = engine_workflows.clone();
            let config = engine_config.clone();
            let ticket = ticket_key_owned.clone();
            let tx2 = tx.clone();
            tokio::spawn(async move {
                if let Some((pct, total)) =
                    driver::progress_dashboard_fields_for_ticket(&workflows, &config, &ticket).await
                {
                    event.progress_percent = Some(pct);
                    event.progress_steps_total = Some(total);
                }
                let _ = tx2.send(event);
            });
        }).await
    }

    pub fn broadcast_event(&self, mut event: WorkflowEvent) {
        if event.event_type != "workflow_updated" {
            self.event_bus.send(event);
            return;
        }
        let workflows = self.repository.inner_arc();
        let config = Arc::clone(&self.config);
        let ticket_key = event.ticket_key.clone();
        let tx = self.event_bus.sender().clone();
        tokio::spawn(async move {
            if let Some((pct, total)) =
                driver::progress_dashboard_fields_for_ticket(&workflows, &config, &ticket_key).await
            {
                event.progress_percent = Some(pct);
                event.progress_steps_total = Some(total);
            }
            let _ = tx.send(event);
        });
    }

    /// Start running a specific workflow definition for a ticket.
    pub async fn start_workflow_def(&self, ticket_key: &str, def_name: &str) -> Result<()> {
        self.definitions.start_workflow_def(ticket_key, def_name).await
    }

    /// Reset a workflow definition run from Error to Idle and start it again.
    pub async fn retry_workflow_def(&self, ticket_key: &str, def_name: &str) -> Result<()> {
        self.definitions.retry_workflow_def(ticket_key, def_name).await
    }

    /// Start a background task that periodically scans the workflows directory for changes
    /// and broadcasts a `workflow_definitions_changed` event when the file list changes.
    pub fn start_definitions_watcher(&self, cancel_token: CancellationToken) {
        self.definitions.start_definitions_watcher(cancel_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;
    use tokio_util::sync::CancellationToken;

    use crate::config::{Config, TicketingSystem};
    use crate::workflow::definitions::WorkflowDefRunState;
    use crate::workflow::snapshot::{PersistedTerminalLine, PersistedWorkflowRecord};
    use crate::workflow::state::WorkflowState;
    use crate::workflow::step::{StepLog, StepStatus};

    /// Helper to build a `Workflow` for testing, following the `dashboard_progress.rs` pattern.
    fn wf_with(
        state: WorkflowState,
        steps_log: Vec<StepLog>,
        current_step_label: Option<String>,
    ) -> Workflow {
        let now = Utc::now();
        Workflow {
            id: "test-id".into(),
            ticket_key: "TEST-1".into(),
            ticket_summary: "Test summary".into(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state,
            started_at: now,
            updated_at: now,
            steps_log,
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            cancel_token: CancellationToken::new(),
            terminal_lines: Vec::new(),
            current_step_label,
            started_manually: false,
            jira_available: true,
            ticketing_available: true,
            ticketing_system: TicketingSystem::Jira,
            last_session_id: None,
            description_session_id: None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
        }
    }

    // -----------------------------------------------------------------------
    // 1. WorkflowState::display_name() — every variant
    // -----------------------------------------------------------------------

    #[test]
    fn display_name_pending() {
        assert_eq!(WorkflowState::Pending.display_name(), "Pending");
    }

    #[test]
    fn display_name_assigning() {
        assert_eq!(WorkflowState::Assigning.display_name(), "Assigning Ticket");
    }

    #[test]
    fn display_name_retrieving_details() {
        assert_eq!(
            WorkflowState::RetrievingDetails.display_name(),
            "Retrieving Details"
        );
    }

    #[test]
    fn display_name_creating_worktree() {
        assert_eq!(
            WorkflowState::CreatingWorktree.display_name(),
            "Creating Worktree"
        );
    }

    #[test]
    fn display_name_addressing_ticket() {
        assert_eq!(
            WorkflowState::AddressingTicket { pass: 1 }.display_name(),
            "Running agent steps"
        );
    }

    #[test]
    fn display_name_addressing_pr_comments() {
        assert_eq!(
            WorkflowState::AddressingPrComments { pass: 1 }.display_name(),
            "Addressing PR comments"
        );
    }

    #[test]
    fn display_name_merging_base_branch() {
        assert_eq!(
            WorkflowState::MergingBaseBranch { pass: 1 }.display_name(),
            "Merging base branch"
        );
    }

    #[test]
    fn display_name_reviewing() {
        assert_eq!(
            WorkflowState::Reviewing.display_name(),
            "Reviewing Changes"
        );
    }

    #[test]
    fn display_name_creating_pr() {
        assert_eq!(WorkflowState::CreatingPR.display_name(), "Creating PR");
    }

    #[test]
    fn display_name_done() {
        assert_eq!(WorkflowState::Done.display_name(), "Done");
    }

    #[test]
    fn display_name_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Assigning),
            message: "timeout".into(),
        };
        assert_eq!(state.display_name(), "Error: timeout");
    }

    #[test]
    fn display_name_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
        };
        assert_eq!(state.display_name(), "Paused");
    }

    #[test]
    fn display_name_stopped() {
        assert_eq!(WorkflowState::Stopped.display_name(), "Stopped");
    }

    // -----------------------------------------------------------------------
    // 2. WorkflowState::is_terminal() — Done/Stopped/Error return true; others false
    // -----------------------------------------------------------------------

    #[test]
    fn is_terminal_done() {
        assert!(WorkflowState::Done.is_terminal());
    }

    #[test]
    fn is_terminal_stopped() {
        assert!(WorkflowState::Stopped.is_terminal());
    }

    #[test]
    fn is_terminal_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "fail".into(),
        };
        assert!(state.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_pending() {
        assert!(!WorkflowState::Pending.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_assigning() {
        assert!(!WorkflowState::Assigning.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_addressing_ticket() {
        assert!(!WorkflowState::AddressingTicket { pass: 1 }.is_terminal());
    }

    #[test]
    fn is_terminal_false_for_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::Assigning),
        };
        assert!(!state.is_terminal());
    }

    // -----------------------------------------------------------------------
    // 3. WorkflowState::is_active() — Paused and Error are not active
    // -----------------------------------------------------------------------

    #[test]
    fn is_active_true_for_pending() {
        assert!(WorkflowState::Pending.is_active());
    }

    #[test]
    fn is_active_true_for_assigning() {
        assert!(WorkflowState::Assigning.is_active());
    }

    #[test]
    fn is_active_true_for_addressing_ticket() {
        assert!(WorkflowState::AddressingTicket { pass: 1 }.is_active());
    }

    #[test]
    fn is_active_true_for_creating_worktree() {
        assert!(WorkflowState::CreatingWorktree.is_active());
    }

    #[test]
    fn is_active_false_for_done() {
        assert!(!WorkflowState::Done.is_active());
    }

    #[test]
    fn is_active_false_for_stopped() {
        assert!(!WorkflowState::Stopped.is_active());
    }

    #[test]
    fn is_active_false_for_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "fail".into(),
        };
        assert!(!state.is_active());
    }

    #[test]
    fn is_active_false_for_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::Assigning),
        };
        assert!(!state.is_active());
    }

    // -----------------------------------------------------------------------
    // 4. WorkflowState::occupies_concurrency_slot() — Done/Stopped/Error do not
    // -----------------------------------------------------------------------

    #[test]
    fn occupies_slot_true_for_pending() {
        assert!(WorkflowState::Pending.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_true_for_assigning() {
        assert!(WorkflowState::Assigning.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_true_for_paused() {
        let state = WorkflowState::Paused {
            source_state: Box::new(WorkflowState::Assigning),
        };
        assert!(state.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_true_for_addressing_ticket() {
        assert!(
            WorkflowState::AddressingTicket { pass: 1 }.occupies_concurrency_slot()
        );
    }

    #[test]
    fn occupies_slot_false_for_done() {
        assert!(!WorkflowState::Done.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_false_for_stopped() {
        assert!(!WorkflowState::Stopped.occupies_concurrency_slot());
    }

    #[test]
    fn occupies_slot_false_for_error() {
        let state = WorkflowState::Error {
            source_state: Box::new(WorkflowState::Pending),
            message: "fail".into(),
        };
        assert!(!state.occupies_concurrency_slot());
    }

    // -----------------------------------------------------------------------
    // 5. Workflow::status_display() — delegates to current_step_label for
    //    AddressingTicket/AddressingPrComments, falls back to display_name for others
    // -----------------------------------------------------------------------

    #[test]
    fn status_display_addressing_ticket_uses_step_label() {
        let w = wf_with(
            WorkflowState::AddressingTicket { pass: 1 },
            vec![],
            Some("Implement ticket (cycle 1/3, run 1/1)".into()),
        );
        assert_eq!(
            w.status_display(),
            "Implement ticket (cycle 1/3, run 1/1)"
        );
    }

    #[test]
    fn status_display_addressing_ticket_fallback_no_label() {
        let w = wf_with(
            WorkflowState::AddressingTicket { pass: 1 },
            vec![],
            None,
        );
        assert_eq!(w.status_display(), "Running agent steps");
    }

    #[test]
    fn status_display_addressing_pr_comments_uses_step_label() {
        let w = wf_with(
            WorkflowState::AddressingPrComments { pass: 1 },
            vec![],
            Some("Review PR (cycle 2/3, run 1/1)".into()),
        );
        assert_eq!(w.status_display(), "Review PR (cycle 2/3, run 1/1)");
    }

    #[test]
    fn status_display_addressing_pr_comments_fallback_no_label() {
        let w = wf_with(
            WorkflowState::AddressingPrComments { pass: 1 },
            vec![],
            None,
        );
        assert_eq!(w.status_display(), "Addressing PR comments");
    }

    #[test]
    fn status_display_done_delegates_to_display_name() {
        let w = wf_with(WorkflowState::Done, vec![], None);
        assert_eq!(w.status_display(), "Done");
    }

    #[test]
    fn status_display_paused_delegates_to_display_name() {
        let w = wf_with(
            WorkflowState::Paused {
                source_state: Box::new(WorkflowState::Assigning),
            },
            vec![],
            Some("should be ignored".into()),
        );
        assert_eq!(w.status_display(), "Paused");
    }

    #[test]
    fn status_display_error_delegates_to_display_name() {
        let w = wf_with(
            WorkflowState::Error {
                source_state: Box::new(WorkflowState::Pending),
                message: "oops".into(),
            },
            vec![],
            None,
        );
        assert_eq!(w.status_display(), "Error: oops");
    }

    #[test]
    fn status_display_stopped_delegates_to_display_name() {
        let w = wf_with(WorkflowState::Stopped, vec![], None);
        assert_eq!(w.status_display(), "Stopped");
    }

    #[test]
    fn status_display_assigning_delegates_to_display_name() {
        let w = wf_with(WorkflowState::Assigning, vec![], None);
        assert_eq!(w.status_display(), "Assigning Ticket");
    }

    #[test]
    fn status_display_creating_worktree_delegates_to_display_name() {
        let w = wf_with(WorkflowState::CreatingWorktree, vec![], None);
        assert_eq!(w.status_display(), "Creating Worktree");
    }

    // -----------------------------------------------------------------------
    // 6. Workflow::from_persisted_record()
    // -----------------------------------------------------------------------

    /// Build a minimal `PersistedWorkflowRecord` for testing.
    fn make_persisted_record(state: WorkflowState, driver_started: bool) -> PersistedWorkflowRecord {
        let now = Utc::now();
        PersistedWorkflowRecord {
            id: "rec-id".into(),
            ticket_key: "REC-1".into(),
            ticket_summary: "Record summary".into(),
            ticket_description: "desc".into(),
            ticket_type: "Bug".into(),
            state,
            started_at: now,
            updated_at: now,
            steps_log: vec![],
            branch_name: "feat/rec-1".into(),
            worktree_path: Some(PathBuf::from("/tmp/wt")),
            pr_url: Some("https://github.com/foo/bar/pull/42".into()),
            pr_merged: true,
            terminal_lines: vec![PersistedTerminalLine {
                text: "hello".into(),
                stream: "stdout".into(),
            }],
            current_step_label: Some("Step label".into()),
            started_manually: true,
            jira_available: false,
            last_session_id: Some("sess-abc".into()),
            description_session_id: Some("desc-xyz".into()),
            ticketing_system: TicketingSystem::GitHub,
            driver_started,
            workflow_def_runs: {
                let mut m = HashMap::new();
                m.insert("implement_ticket".into(), WorkflowDefRunState::Completed);
                m
            },
            worktree_bootstrapped: true,
        }
    }

    #[test]
    fn from_persisted_record_round_trips_modern_state() {
        let rec = make_persisted_record(WorkflowState::AddressingTicket { pass: 1 }, true);
        let w = Workflow::from_persisted_record(rec);
        assert_eq!(w.id, "rec-id");
        assert_eq!(w.ticket_key, "REC-1");
        assert_eq!(w.ticket_summary, "Record summary");
        assert_eq!(w.ticket_description, "desc");
        assert_eq!(w.ticket_type, "Bug");
        assert!(matches!(w.state, WorkflowState::AddressingTicket { pass: 1 }));
        assert_eq!(w.branch_name, "feat/rec-1");
        assert_eq!(w.worktree_path, Some(PathBuf::from("/tmp/wt")));
        assert_eq!(w.pr_url.as_deref(), Some("https://github.com/foo/bar/pull/42"));
        assert!(w.pr_merged);
        assert_eq!(w.terminal_lines.len(), 1);
        assert_eq!(w.terminal_lines[0].text, "hello");
        assert_eq!(w.terminal_lines[0].stream, "stdout");
        assert_eq!(w.current_step_label.as_deref(), Some("Step label"));
        assert!(w.started_manually);
        assert!(!w.jira_available);
        assert_eq!(w.last_session_id.as_deref(), Some("sess-abc"));
        assert_eq!(w.description_session_id.as_deref(), Some("desc-xyz"));
        assert_eq!(w.ticketing_system, TicketingSystem::GitHub);
        assert!(w.ticketing_available);
        assert!(w.driver_started);
        assert_eq!(
            w.workflow_def_runs.get("implement_ticket"),
            Some(&WorkflowDefRunState::Completed)
        );
        assert!(w.worktree_bootstrapped);
    }

    #[test]
    fn from_persisted_record_strips_running_steps() {
        let now = Utc::now();
        let rec = PersistedWorkflowRecord {
            steps_log: vec![
                StepLog {
                    step_name: "done step".into(),
                    started_at: now,
                    completed_at: Some(now),
                    status: StepStatus::Success,
                    output: vec![],
                    error: None,
                },
                StepLog {
                    step_name: "running step".into(),
                    started_at: now,
                    completed_at: None,
                    status: StepStatus::Running,
                    output: vec![],
                    error: None,
                },
            ],
            ..make_persisted_record(WorkflowState::AddressingTicket { pass: 1 }, true)
        };
        let w = Workflow::from_persisted_record(rec);
        assert_eq!(w.steps_log.len(), 1, "Running steps should be stripped");
        assert_eq!(w.steps_log[0].step_name, "done step");
    }

    #[test]
    fn from_persisted_record_legacy_states_deserialize() {
        // Legacy states should not panic when loaded from snapshot
        for json_state in [
            r#"{"AddressingPrComments":{"pass":1}}"#,
            r#"{"MergingBaseBranch":{"pass":2}}"#,
            r#""Reviewing""#,
            r#""CreatingPR""#,
        ] {
            let state: WorkflowState = serde_json::from_str(json_state)
                .unwrap_or_else(|e| panic!("Failed to deserialize {json_state}: {e}"));
            let rec = make_persisted_record(state, true);
            let w = Workflow::from_persisted_record(rec);
            // Just verify it doesn't panic and produces a valid workflow
            let _ = w.status_display();
        }
    }

    #[test]
    fn from_persisted_record_pending_driver_not_started() {
        let rec = make_persisted_record(WorkflowState::Pending, false);
        let w = Workflow::from_persisted_record(rec);
        assert!(matches!(w.state, WorkflowState::Pending));
        assert!(!w.driver_started);
    }

    #[test]
    fn from_persisted_record_backward_compat_ticketing_system() {
        // Old snapshots have ticketing_system = None but jira_available = true.
        // from_persisted_record should derive TicketingSystem::Jira.
        let now = Utc::now();
        let rec = PersistedWorkflowRecord {
            id: "old-id".into(),
            ticket_key: "OLD-1".into(),
            ticket_summary: "s".into(),
            ticket_description: String::new(),
            ticket_type: "Task".into(),
            state: WorkflowState::Done,
            started_at: now,
            updated_at: now,
            steps_log: vec![],
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: vec![],
            current_step_label: None,
            started_manually: false,
            jira_available: true,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: TicketingSystem::None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
        };
        let w = Workflow::from_persisted_record(rec);
        assert_eq!(
            w.ticketing_system,
            TicketingSystem::Jira,
            "Old snapshot with jira_available=true should derive Jira ticketing"
        );
        assert!(w.ticketing_available);
    }

    // -----------------------------------------------------------------------
    // 7. Snapshot serde round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn snapshot_record_serde_round_trip() {
        let mut def_runs = HashMap::new();
        def_runs.insert("implement_ticket".into(), WorkflowDefRunState::Completed);
        def_runs.insert("address_pr_comments".into(), WorkflowDefRunState::Idle);

        let now = Utc::now();
        let rec = PersistedWorkflowRecord {
            id: "uuid-1".into(),
            ticket_key: "SNAP-1".into(),
            ticket_summary: "Snapshot test".into(),
            ticket_description: "description".into(),
            ticket_type: "Story".into(),
            state: WorkflowState::AddressingTicket { pass: 1 },
            started_at: now,
            updated_at: now,
            steps_log: vec![StepLog {
                step_name: "Implement".into(),
                started_at: now,
                completed_at: Some(now),
                status: StepStatus::Success,
                output: vec!["line1".into()],
                error: None,
            }],
            branch_name: "feat/snap-1".into(),
            worktree_path: Some(PathBuf::from("/workspace/worktrees/snap-1")),
            pr_url: Some("https://github.com/org/repo/pull/99".into()),
            pr_merged: true,
            terminal_lines: vec![
                PersistedTerminalLine {
                    text: "stdout line".into(),
                    stream: "stdout".into(),
                },
                PersistedTerminalLine {
                    text: "stderr line".into(),
                    stream: "stderr".into(),
                },
            ],
            current_step_label: Some("Implement (cycle 1/2, run 1/1)".into()),
            started_manually: true,
            jira_available: false,
            last_session_id: Some("session-123".into()),
            description_session_id: Some("desc-456".into()),
            ticketing_system: TicketingSystem::GitHub,
            driver_started: true,
            workflow_def_runs: def_runs,
            worktree_bootstrapped: true,
        };

        let json = serde_json::to_string_pretty(&rec).expect("serialize PersistedWorkflowRecord");
        let back: PersistedWorkflowRecord =
            serde_json::from_str(&json).expect("deserialize PersistedWorkflowRecord");

        assert_eq!(back.id, "uuid-1");
        assert_eq!(back.ticket_key, "SNAP-1");
        assert_eq!(back.ticket_summary, "Snapshot test");
        assert_eq!(back.ticket_description, "description");
        assert_eq!(back.ticket_type, "Story");
        assert!(matches!(
            back.state,
            WorkflowState::AddressingTicket { pass: 1 }
        ));
        assert_eq!(back.branch_name, "feat/snap-1");
        assert_eq!(
            back.worktree_path,
            Some(PathBuf::from("/workspace/worktrees/snap-1"))
        );
        assert_eq!(
            back.pr_url.as_deref(),
            Some("https://github.com/org/repo/pull/99")
        );
        assert!(back.pr_merged);
        assert_eq!(back.terminal_lines.len(), 2);
        assert_eq!(back.terminal_lines[0].text, "stdout line");
        assert_eq!(back.terminal_lines[1].stream, "stderr");
        assert_eq!(
            back.current_step_label.as_deref(),
            Some("Implement (cycle 1/2, run 1/1)")
        );
        assert!(back.started_manually);
        assert!(!back.jira_available);
        assert_eq!(back.last_session_id.as_deref(), Some("session-123"));
        assert_eq!(back.description_session_id.as_deref(), Some("desc-456"));
        assert_eq!(back.ticketing_system, TicketingSystem::GitHub);
        assert!(back.driver_started);
        assert_eq!(
            back.workflow_def_runs.get("implement_ticket"),
            Some(&WorkflowDefRunState::Completed)
        );
        assert_eq!(
            back.workflow_def_runs.get("address_pr_comments"),
            Some(&WorkflowDefRunState::Idle)
        );
        assert!(back.worktree_bootstrapped);
        assert_eq!(back.steps_log.len(), 1);
        assert_eq!(back.steps_log[0].step_name, "Implement");
        assert_eq!(back.steps_log[0].status, StepStatus::Success);
    }

    #[test]
    fn snapshot_record_defaults_for_missing_fields() {
        // Simulate an old snapshot missing newer fields
        let json = r#"{
            "id": "old",
            "ticket_key": "OLD-1",
            "ticket_summary": "s",
            "ticket_description": "",
            "ticket_type": "Task",
            "state": "Done",
            "started_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "steps_log": [],
            "branch_name": "",
            "worktree_path": null,
            "pr_url": null,
            "pr_merged": false,
            "terminal_lines": [],
            "current_step_label": null,
            "started_manually": false,
            "jira_available": true
        }"#;
        let rec: PersistedWorkflowRecord = serde_json::from_str(json)
            .expect("old snapshot without newer fields should deserialize");
        assert!(rec.driver_started, "driver_started should default to true");
        assert!(rec.workflow_def_runs.is_empty());
        assert!(!rec.pr_merged);
        assert!(rec.last_session_id.is_none());
        assert!(rec.description_session_id.is_none());
        assert_eq!(rec.ticketing_system, TicketingSystem::None);
        assert!(!rec.worktree_bootstrapped);
    }

    // -----------------------------------------------------------------------
    // 8. WorkflowEngine::new() — constructs without panicking
    // -----------------------------------------------------------------------

    #[test]
    fn workflow_engine_new_does_not_panic() {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new(
                PathBuf::from("/workspace"),
                "origin".into(),
                None,
            ),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let engine = WorkflowEngine::new(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::None,
            PathBuf::from("workflows"),
        );
        // Verify basic fields are set correctly
        assert_eq!(engine.ticketing_system, TicketingSystem::None);
        assert!(!engine.jira_available.load(Ordering::SeqCst));
        assert_eq!(engine.workflows_dir, PathBuf::from("workflows"));
    }

    #[test]
    fn workflow_engine_new_with_higher_concurrency() {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new(
                PathBuf::from("/workspace"),
                "origin".into(),
                None,
            ),
        );
        let jira_available = Arc::new(AtomicBool::new(true));
        let engine = WorkflowEngine::new(
            config,
            actions,
            5,
            jira_available,
            TicketingSystem::Jira,
            PathBuf::from("/etc/maestro/workflows"),
        );
        assert_eq!(engine.ticketing_system, TicketingSystem::Jira);
        assert!(engine.jira_available.load(Ordering::SeqCst));
    }
}
