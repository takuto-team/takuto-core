// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

mod auth_pin;
mod bootstrap;
mod context;
mod definitions;
mod driver;
mod event_bus;
mod lifecycle;
mod persistence;
mod repository;
mod resolve;
mod step_runner;
mod transitions;
mod types;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::{RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;

use definitions::WorkflowDefinitionManager;
use event_bus::WorkflowEventBus;
use lifecycle::WorkflowLifecycle;
use persistence::WorkflowPersistence;
use repository::WorkflowRepository;
use transitions::WorkflowTransitions;

use crate::actions::traits::ExternalActions;
use crate::config::{Config, TicketingSystem};
use crate::db::Database;
use crate::error::Result;

pub use driver::resolve_worktree_init_commands;
pub use types::{MarkDoneOutcome, TerminalLine, Workflow, WorkflowEvent};

pub struct WorkflowEngine {
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub(crate) actions: Arc<dyn ExternalActions>,
    /// Limits concurrent heavy work (mise/install/agent sessions) across workflows. Paused workflows do not hold a permit.
    agent_run_semaphore: Arc<Semaphore>,
    /// When set, workflow drivers that exit with [`MaestroError::Cancelled`] do not move the workflow to Error (graceful container shutdown + snapshot).
    pub(crate) suppress_cancelled_as_error: Arc<AtomicBool>,
    /// `true` when acli (Atlassian CLI) is authenticated. When `false`, workflows skip all Jira
    /// operations (assign, transition, retrieve details) and the poller is not started.
    pub(crate) jira_available: Arc<AtomicBool>,
    /// Which ticketing system is configured for this engine run.
    pub(crate) ticketing_system: TicketingSystem,
    /// Directory containing dynamic workflow definition YAML files, resolved at construction time.
    pub(crate) workflows_dir: PathBuf,
    /// Optional SQLite handle used by the bootstrap driver to look up per-workspace
    /// `worktree_init_commands` overrides. `None` when running without a DB (e.g.
    /// some test paths) — the driver then falls back to the global config.
    pub(crate) db: Option<Database>,
    /// Optional `GitAuthResolver` used to pin credentials at workflow
    /// start and build per-step `WorkerSecretsBundle`s. `None` when
    /// running without the resolver (legacy poller / single-tenant) —
    /// the worker falls back to the ambient-env `PASSTHROUGH_ENV` path.
    pub(crate) git_auth_resolver: Option<Arc<crate::github::auth_resolver::GitAuthResolver>>,
    /// `GhClient` used by the engine for at-resume PAT revalidation.
    /// Defaults to a `RealGhClient`. Test fixtures override via
    /// [`Self::with_gh_client`] to inject a mock.
    pub(crate) gh_client: Arc<dyn crate::auth::GhClient>,
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
        Self::new_with_db(
            config,
            actions,
            max_concurrent_workflows,
            jira_available,
            ticketing_system,
            workflows_dir,
            None,
        )
    }

    /// Like [`Self::new`] but optionally threads a `Database` handle through to
    /// the bootstrap driver so it can resolve per-workspace
    /// `worktree_init_commands` overrides from the `workspace_commands` table.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_db(
        config: Arc<RwLock<Config>>,
        actions: Arc<dyn ExternalActions>,
        max_concurrent_workflows: usize,
        jira_available: Arc<AtomicBool>,
        ticketing_system: TicketingSystem,
        workflows_dir: PathBuf,
        db: Option<Database>,
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
            db.clone(),
        );

        let lifecycle = WorkflowLifecycle::new(
            repository.clone(),
            event_bus.clone(),
            actions.clone(),
            config.clone(),
            jira_available.clone(),
            ticketing_system,
            db.clone(),
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
            db.clone(),
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
            db,
            git_auth_resolver: None,
            gh_client: Arc::new(crate::auth::RealGhClient::new()),
            persistence,
            lifecycle,
            transitions,
            definitions,
        }
    }

    /// Override the GhClient (production uses `RealGhClient`; tests
    /// inject a mock).
    pub fn with_gh_client(mut self, gh: Arc<dyn crate::auth::GhClient>) -> Self {
        self.gh_client = gh.clone();
        self.persistence.set_gh_client(gh.clone());
        self.transitions.set_gh_client(gh);
        self
    }

    /// Attach the `GitAuthResolver` so the driver can pin credentials at
    /// workflow start and build per-step worker secrets bundles.
    /// Builder-style so the existing constructor signature stays
    /// back-compatible with all test fixtures.
    ///
    /// Propagates the same `Arc` to the three service structs that spawn
    /// driver tasks (definitions / persistence / transitions). The
    /// lifecycle does not spawn drivers itself, so it doesn't need a
    /// resolver field.
    pub fn with_git_auth_resolver(
        mut self,
        resolver: Arc<crate::github::auth_resolver::GitAuthResolver>,
    ) -> Self {
        self.git_auth_resolver = Some(resolver.clone());
        self.definitions.set_git_auth_resolver(resolver.clone());
        self.persistence.set_git_auth_resolver(resolver.clone());
        self.transitions.set_git_auth_resolver(resolver);
        self
    }

    /// Shared workflow configuration. Returns a fresh `Arc` handle; the underlying
    /// `RwLock<Config>` is the same value seen by every other clone.
    pub fn config(&self) -> Arc<RwLock<Config>> {
        self.config.clone()
    }

    /// Trait object that performs all external side-effects (git, Jira, GitHub,
    /// Claude). Returns a fresh `Arc` handle to the shared instance.
    pub fn actions(&self) -> Arc<dyn ExternalActions> {
        self.actions.clone()
    }

    /// Flag flipped on graceful container shutdown so cancelled drivers do not
    /// move workflows to `Error`. Returns a fresh `Arc` handle.
    pub fn suppress_cancelled_as_error(&self) -> Arc<AtomicBool> {
        self.suppress_cancelled_as_error.clone()
    }

    /// `true` when acli (Atlassian CLI) is authenticated. Returns a fresh `Arc`
    /// handle so callers can share the live flag without holding the engine.
    pub fn jira_available(&self) -> Arc<AtomicBool> {
        self.jira_available.clone()
    }

    /// Which ticketing system is configured for this engine run.
    pub fn ticketing_system(&self) -> TicketingSystem {
        self.ticketing_system
    }

    /// Directory containing dynamic workflow definition TOML files.
    pub fn workflows_dir(&self) -> &std::path::Path {
        &self.workflows_dir
    }

    /// Optional SQLite handle used by the bootstrap driver. `None` when running
    /// without a DB (e.g. some test paths).
    pub fn db(&self) -> Option<&Database> {
        self.db.as_ref()
    }

    /// Optional `GitAuthResolver` used to pin credentials at workflow start.
    /// Returns a cloned `Option<Arc<...>>` so callers can share the resolver
    /// without holding the engine.
    pub fn git_auth_resolver(&self) -> Option<Arc<crate::github::auth_resolver::GitAuthResolver>> {
        self.git_auth_resolver.clone()
    }

    /// `GhClient` used by the engine for at-resume PAT revalidation. Returns a
    /// fresh `Arc` handle to the shared trait object.
    pub fn gh_client(&self) -> Arc<dyn crate::auth::GhClient> {
        self.gh_client.clone()
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

    /// Reassign every workflow currently in the repository whose `user_id` is
    /// `None` to `owner_id`. Returns the number of workflows that were touched.
    ///
    /// This is the in-memory half of the one-shot orphan migration. The
    /// caller is responsible for persisting via [`sync_workflow_snapshot`]
    /// once the migration is complete so it survives a crash. The helper is
    /// idempotent: re-running it on a clean repository is a no-op.
    pub async fn migrate_orphan_workflows_to_owner(&self, owner_id: &str) -> usize {
        let wf_arc = self.repository.inner_arc();
        let mut workflows = wf_arc.write().await;
        let mut migrated = 0usize;
        for w in workflows.values_mut() {
            if w.user_id.is_none() {
                tracing::warn!(
                    ticket = %w.ticket_key,
                    new_owner = %owner_id,
                    "Migrating orphan workflow to resolved poller owner"
                );
                w.user_id = Some(owner_id.to_string());
                migrated += 1;
            }
        }
        migrated
    }

    /// Write `workflow_snapshot.json` and cancel drivers so processes stop, without Jira unassign / **Stopped** (for container restart).
    pub async fn persist_interrupt_for_restart(&self) -> Result<()> {
        self.persistence.persist_interrupt().await
    }

    /// Load snapshot from disk, insert workflows, spawn drivers. Removes the snapshot file on success.
    pub async fn restore_persisted_workflows(&self) -> Result<usize> {
        self.persistence
            .restore(
                &self.workflows_dir,
                self.agent_run_semaphore.clone(),
                self.suppress_cancelled_as_error.clone(),
                self.db.clone(),
            )
            .await
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_bus.subscribe()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_workflow(
        &self,
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        ticket_description: Option<String>,
        ticket_url: Option<String>,
        user_id: Option<String>,
        repository_id: Option<String>,
    ) -> Result<String> {
        self.lifecycle
            .start_workflow(
                ticket_key,
                ticket_summary,
                started_manually,
                ticket_description,
                ticket_url,
                &self.definitions,
                user_id,
                repository_id,
            )
            .await
    }

    /// Add a workflow to the dashboard without spawning the driver.
    /// For Jira tickets, assigns the ticket and transitions to In Progress (best-effort).
    #[allow(clippy::too_many_arguments)]
    pub async fn add_to_dashboard(
        &self,
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        ticket_description: Option<String>,
        ticket_url: Option<String>,
        user_id: Option<String>,
        repository_id: Option<String>,
    ) -> Result<String> {
        self.lifecycle
            .add_to_dashboard(
                ticket_key,
                ticket_summary,
                started_manually,
                ticket_description,
                ticket_url,
                user_id,
                repository_id,
            )
            .await
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
        self.transitions
            .retry_workflow(ticket_key, &self.lifecycle, &self.definitions)
            .await
    }

    /// Resume a failed or stopped workflow by retrying all Error-state workflow definitions.
    pub async fn resume_from_error(&self, ticket_key: &str) -> Result<()> {
        self.transitions
            .resume_from_error(ticket_key, &self.definitions)
            .await
    }

    /// Manual dashboard starts with **`started_manually`** that are not **Done** / **Stopped** / **Error**.
    pub async fn manual_workflows_toward_cap_count(&self) -> usize {
        self.repository
            .inner_arc()
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
        self.lifecycle
            .delete_workflow(ticket_key, &self.persistence)
            .await
    }

    /// Jira **Done** transition (configured status name) and remove worktree; remove workflow from the map only if both succeed.
    pub async fn mark_work_done(&self, ticket_key: &str) -> Result<MarkDoneOutcome> {
        let engine_workflows = self.repository.inner_arc();
        let engine_config = Arc::clone(&self.config);
        let ticket_key_owned = ticket_key.to_string();
        let tx = self.event_bus.sender().clone();
        self.lifecycle
            .mark_work_done(ticket_key, &self.persistence, move |mut event| {
                let workflows = engine_workflows.clone();
                let config = engine_config.clone();
                let ticket = ticket_key_owned.clone();
                let tx2 = tx.clone();
                tokio::spawn(async move {
                    if let Some((pct, total)) =
                        driver::progress_dashboard_fields_for_ticket(&workflows, &config, &ticket)
                            .await
                    {
                        event.progress_percent = Some(pct);
                        event.progress_steps_total = Some(total);
                    }
                    let _ = tx2.send(event);
                });
            })
            .await
    }

    pub fn broadcast_event(&self, mut event: WorkflowEvent) {
        if event.event_type != "work_item_updated" {
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
    ///
    /// `user_id` is the authenticated caller; pass `None` from internal callers
    /// to resolve the definition set against the workflow's stored owner.
    pub async fn start_workflow_def(
        &self,
        ticket_key: &str,
        def_name: &str,
        user_id: Option<&str>,
    ) -> Result<()> {
        self.definitions
            .start_workflow_def(ticket_key, def_name, user_id)
            .await
    }

    /// Reset a workflow definition run from Error to Idle and start it again.
    pub async fn retry_workflow_def(
        &self,
        ticket_key: &str,
        def_name: &str,
        user_id: Option<&str>,
    ) -> Result<()> {
        self.definitions
            .retry_workflow_def(ticket_key, def_name, user_id)
            .await
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
            worktree_lock: std::sync::Arc::new(tokio::sync::Mutex::new(())),
            current_def_total_steps: None,
            terminal_lines: Vec::new(),
            current_step_label,
            started_manually: false,
            jira_available: true,
            ticketing_available: true,
            ticketing_system: TicketingSystem::Jira,
            ticket_url: None,
            last_session_id: None,
            description_session_id: None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
            workspace_name: "test-workspace".into(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
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
        assert_eq!(WorkflowState::Reviewing.display_name(), "Reviewing Changes");
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
        assert!(WorkflowState::AddressingTicket { pass: 1 }.occupies_concurrency_slot());
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
        assert_eq!(w.status_display(), "Implement ticket (cycle 1/3, run 1/1)");
    }

    #[test]
    fn status_display_addressing_ticket_fallback_no_label() {
        let w = wf_with(WorkflowState::AddressingTicket { pass: 1 }, vec![], None);
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
    fn make_persisted_record(
        state: WorkflowState,
        driver_started: bool,
    ) -> PersistedWorkflowRecord {
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
            ticket_url: Some("https://github.com/foo/bar/issues/1".into()),
            driver_started,
            workflow_def_runs: {
                let mut m = HashMap::new();
                m.insert("implement_ticket".into(), WorkflowDefRunState::Completed);
                m
            },
            worktree_bootstrapped: true,
            workspace_name: "test-workspace".into(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
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
        assert!(matches!(
            w.state,
            WorkflowState::AddressingTicket { pass: 1 }
        ));
        assert_eq!(w.branch_name, "feat/rec-1");
        assert_eq!(w.worktree_path, Some(PathBuf::from("/tmp/wt")));
        assert_eq!(
            w.pr_url.as_deref(),
            Some("https://github.com/foo/bar/pull/42")
        );
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
            ticket_url: None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: false,
            workspace_name: String::new(),
            repository_id: None,
            user_id: None,
            auth_pin: None,
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
            ticket_url: Some("https://github.com/org/repo/issues/42".into()),
            driver_started: true,
            workflow_def_runs: def_runs,
            worktree_bootstrapped: true,
            workspace_name: "test-workspace".into(),
            repository_id: Some("repo-uuid-99".into()),
            user_id: Some("user-abc".into()),
            auth_pin: None,
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
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
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
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
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

    // -----------------------------------------------------------------------
    // 9. start_workflow propagates user_id
    // -----------------------------------------------------------------------

    /// Verifies that `WorkflowEngine::start_workflow` stores the caller-supplied
    /// `user_id` on the created `Workflow`. Pollers pass
    /// `Some(resolved_owner_id)`, web `start-manual` passes
    /// `Some(auth.user_id)`, and the workflow should carry that owner forward
    /// so the per-user filter and dashboard list endpoint match the right
    /// user.
    #[tokio::test]
    async fn start_workflow_propagates_user_id() {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let workflows_dir =
            std::env::temp_dir().join(format!("maestro-engine-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");
        let engine = WorkflowEngine::new(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::None,
            workflows_dir,
        );

        let id = engine
            .start_workflow(
                "POLLED-1".into(),
                "Some summary".into(),
                false,
                None,
                None,
                Some("user-abc".to_string()),
                None,
            )
            .await
            .expect("start_workflow should succeed in dry mode");
        assert!(!id.is_empty());

        let wf_arc = engine.workflows_arc();
        let map = wf_arc.read().await;
        let wf = map
            .get("POLLED-1")
            .expect("workflow should be present in the repository");
        assert_eq!(
            wf.user_id.as_deref(),
            Some("user-abc"),
            "user_id passed into start_workflow must be stored on the Workflow"
        );
    }

    #[tokio::test]
    async fn start_workflow_with_none_user_id_creates_orphan() {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let workflows_dir =
            std::env::temp_dir().join(format!("maestro-engine-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");
        let engine = WorkflowEngine::new(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::None,
            workflows_dir,
        );

        engine
            .start_workflow(
                "POLLED-2".into(),
                "Other summary".into(),
                false,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("start_workflow should accept None user_id");

        let wf_arc = engine.workflows_arc();
        let map = wf_arc.read().await;
        let wf = map.get("POLLED-2").expect("workflow should exist");
        assert!(
            wf.user_id.is_none(),
            "None should leave the workflow unowned"
        );
    }

    /// Every call to `start_workflow` must shadow-write the matching
    /// `work_items` row alongside the in-memory map insert. The map
    /// remains the truth-of-record; the row in `work_items` is consumed
    /// by the dashboard reads.
    #[tokio::test]
    async fn start_workflow_shadow_writes_work_items_row() {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let workflows_dir =
            std::env::temp_dir().join(format!("maestro-engine-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");

        let db = Database::open_in_memory().expect("open in-memory db");
        // Seed the owning user — work_items.user_id has an FK to users.
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-poller', 'poller', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");

        let engine = WorkflowEngine::new_with_db(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::None,
            workflows_dir,
            Some(db.clone()),
        );

        let id = engine
            .start_workflow(
                "POLLED-3".into(),
                "Shadow-write test".into(),
                false,
                Some("a description".into()),
                None,
                Some("u-poller".to_string()),
                None,
            )
            .await
            .expect("start_workflow");

        // The map still holds the canonical record.
        {
            let wf_arc = engine.workflows_arc();
            let map = wf_arc.read().await;
            let wf = map.get("POLLED-3").expect("must be in the map");
            assert_eq!(wf.id, id);
        }

        // The DB shadow-write landed a matching row.
        let row = crate::db::work_items::get_work_item(db.adapter(), &id, None, true)
            .await
            .expect("query work_items")
            .expect("shadow-write must produce a row");
        assert_eq!(row.id, id);
        assert_eq!(row.ticket_key, "POLLED-3");
        assert_eq!(row.user_id.as_deref(), Some("u-poller"));
        assert_eq!(
            row.state_kind,
            crate::db::work_items::WorkItemStateKind::Pending
        );
        // `Pending` carries no variant data, so payload stays NULL.
        assert_eq!(row.state_payload, None);
        assert_eq!(row.ticket_description.as_deref(), Some("a description"));
        assert!(!row.driver_started);
    }

    /// `shadow_persist_work_item` must be safe to call twice on the same
    /// `Workflow`. This is the load-bearing claim of the restore-time
    /// backfill: restarting an already-backfilled install must not
    /// produce a hard error and must not create a duplicate row.
    ///
    /// The helper logs WARN on the second call (the UNIQUE index
    /// on `(workspace_name, ticket_key)` rejects the duplicate) but
    /// never panics or returns.
    #[tokio::test]
    async fn shadow_persist_work_item_is_idempotent_on_repeat_calls() {
        use crate::workflow::engine::lifecycle::shadow_persist_work_item;

        let db = Database::open_in_memory().expect("open in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");

        // Build a workflow directly — no engine needed for this test.
        let mut wf = Workflow::new(
            "BACKFILL-1".into(),
            "summary".into(),
            false,
            false,
            TicketingSystem::None,
            None,
            "ws".into(),
        );
        wf.user_id = Some("u-1".into());

        // First call — fresh insert.
        shadow_persist_work_item(Some(&db), &wf).await;

        let row1 = crate::db::work_items::get_work_item(db.adapter(), &wf.id, None, true)
            .await
            .expect("get_work_item")
            .expect("row exists after first call");
        assert_eq!(row1.ticket_key, "BACKFILL-1");

        // Second call — must not panic, must not duplicate.
        shadow_persist_work_item(Some(&db), &wf).await;

        // Still exactly one row for this (workspace, ticket_key).
        let rows = db
            .adapter()
            .query_all(
                "SELECT id FROM work_items WHERE workspace_name = 'ws' AND ticket_key = 'BACKFILL-1'",
                vec![],
            )
            .await
            .expect("query rows");
        assert_eq!(
            rows.len(),
            1,
            "second shadow_persist_work_item call must NOT create a duplicate row"
        );
    }

    /// `pause_workflow` must shadow-write the new `Paused` state to the
    /// DB row. We exercise the engine's `WorkflowTransitions` (where the
    /// inline shadow-write lives) rather than the free `driver::transition`
    /// helper to cover the path the dashboard pause button actually uses.
    #[tokio::test]
    async fn pause_workflow_shadow_writes_state_change() {
        use crate::workflow::engine::transitions::WorkflowTransitions;
        use crate::workflow::state::WorkflowState;

        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let workflows_dir =
            std::env::temp_dir().join(format!("maestro-engine-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");
        let db = Database::open_in_memory().expect("open in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");

        let engine = WorkflowEngine::new_with_db(
            config.clone(),
            actions.clone(),
            1,
            jira_available.clone(),
            TicketingSystem::None,
            workflows_dir.clone(),
            Some(db.clone()),
        );

        // Insert a workflow + put it in an active state so pause is legal.
        let id = engine
            .start_workflow(
                "PAUSE-1".into(),
                "Pause test".into(),
                false,
                None,
                None,
                Some("u-1".into()),
                None,
            )
            .await
            .expect("start_workflow");
        {
            let wf_arc = engine.workflows_arc();
            let mut map = wf_arc.write().await;
            let wf = map.get_mut("PAUSE-1").expect("present");
            wf.state = WorkflowState::AddressingTicket { pass: 1 };
        }

        // Drive a pause through the dashboard-facing API.
        let transitions = WorkflowTransitions::new(
            engine.repository.clone(),
            engine.event_bus.clone(),
            actions,
            config,
            engine.agent_run_semaphore.clone(),
            engine.suppress_cancelled_as_error.clone(),
            jira_available,
            workflows_dir,
            Some(db.clone()),
        );
        transitions
            .pause_workflow("PAUSE-1")
            .await
            .expect("pause_workflow");

        // The DB row tracks the Paused state, and the JSON payload
        // carries the `source_state` so the engine could restore on
        // resume even from a cold-start DB read.
        let row = crate::db::work_items::get_work_item(db.adapter(), &id, None, true)
            .await
            .unwrap()
            .expect("row exists");
        assert_eq!(
            row.state_kind,
            crate::db::work_items::WorkItemStateKind::Paused
        );
        let payload = row.state_payload.expect("Paused carries source_state");
        assert!(
            payload.contains("\"Paused\""),
            "payload must serialise the full WorkflowState; got: {payload}"
        );
        assert!(
            payload.contains("\"AddressingTicket\""),
            "payload must include the source state we paused from; got: {payload}"
        );
    }

    /// Resume restores the source state, picks up the recorded
    /// session id, and clears the Paused state — proving the
    /// pause→resume round-trip carries the session id the agent
    /// needs to continue where it left off.
    #[tokio::test]
    async fn resume_workflow_carries_session_id_from_pause() {
        use crate::workflow::engine::transitions::WorkflowTransitions;
        use crate::workflow::state::WorkflowState;

        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn ExternalActions> = Arc::new(
            crate::actions::dry_run::DryRunActions::new("origin".into(), None),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let workflows_dir =
            std::env::temp_dir().join(format!("maestro-engine-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");
        let db = Database::open_in_memory().expect("open in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");

        let engine = WorkflowEngine::new_with_db(
            config.clone(),
            actions.clone(),
            1,
            jira_available.clone(),
            TicketingSystem::None,
            workflows_dir.clone(),
            Some(db.clone()),
        );

        engine
            .start_workflow(
                "RES-1".into(),
                "Resume test".into(),
                false,
                None,
                None,
                Some("u-1".into()),
                None,
            )
            .await
            .expect("start_workflow");

        // Simulate the engine state at the moment of pause: workflow
        // is mid-flight (AddressingTicket), the agent has produced a
        // session id, and a def is recorded as Running.
        {
            let wf_arc = engine.workflows_arc();
            let mut map = wf_arc.write().await;
            let wf = map.get_mut("RES-1").expect("present");
            wf.state = WorkflowState::AddressingTicket { pass: 1 };
            wf.last_session_id = Some("session-abc-123".into());
            wf.workflow_def_runs.insert(
                "ship.toml".into(),
                crate::workflow::definitions::WorkflowDefRunState::Running,
            );
        }

        let transitions = WorkflowTransitions::new(
            engine.repository.clone(),
            engine.event_bus.clone(),
            actions,
            config,
            engine.agent_run_semaphore.clone(),
            engine.suppress_cancelled_as_error.clone(),
            jira_available,
            workflows_dir,
            Some(db.clone()),
        );
        transitions
            .pause_workflow("RES-1")
            .await
            .expect("pause_workflow");

        // After pause: state is Paused, but `last_session_id` MUST
        // survive — resume reads it.
        {
            let wf_arc = engine.workflows_arc();
            let map = wf_arc.read().await;
            let wf = map.get("RES-1").expect("present");
            assert!(
                matches!(wf.state, WorkflowState::Paused { .. }),
                "expected Paused after pause"
            );
            assert_eq!(
                wf.last_session_id.as_deref(),
                Some("session-abc-123"),
                "pause must preserve last_session_id so resume can pick it up"
            );
        }

        transitions
            .resume_workflow("RES-1")
            .await
            .expect("resume_workflow");

        // After resume: state has been restored from source_state,
        // and last_session_id is still there for the spawned driver
        // to pass to the agent.
        let wf_arc = engine.workflows_arc();
        let map = wf_arc.read().await;
        let wf = map.get("RES-1").expect("present");
        assert!(
            matches!(wf.state, WorkflowState::AddressingTicket { .. }),
            "resume must restore the source state; got {:?}",
            wf.state
        );
        assert_eq!(
            wf.last_session_id.as_deref(),
            Some("session-abc-123"),
            "resume does not clear last_session_id — the driver consumes it"
        );
    }

    /// Round-trip the step shadow helpers. `shadow_record_step_start`
    /// returns an id which `shadow_record_step_end` resolves to the same
    /// row; the final row reflects the end-write's status, exit code, and
    /// timestamps.
    #[tokio::test]
    async fn shadow_record_step_start_and_end_round_trip() {
        let db = Database::open_in_memory().expect("open in-memory db");
        // Seed prerequisites: a user, then a work_items row owned by them.
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");
        db.adapter()
            .execute(
                "INSERT INTO work_items (\
                    id, ticket_key, workspace_name, user_id, private, \
                    started_manually, counts_toward_manual_cap, driver_started, \
                    jira_available, state_kind, started_at, created_at, updated_at\
                 ) VALUES (\
                    'wf-step-1', 'TICK-1', 'ws', 'u-1', 0, \
                    0, 0, 0, \
                    0, 'pending', 100, 100, 100\
                 )",
                vec![],
            )
            .await
            .expect("seed work_items");

        // Start a step row.
        let step_id = super::driver::shadow_record_step_start(
            Some(&db),
            "wf-step-1",
            "build (run 1/1)",
            Some("ship.toml"),
            200,
        )
        .await
        .expect("shadow_record_step_start must return an id");

        let steps = crate::db::work_items::list_steps(db.adapter(), "wf-step-1")
            .await
            .expect("list_steps");
        assert_eq!(steps.len(), 1, "exactly one step row after start");
        assert_eq!(steps[0].id, step_id);
        assert_eq!(steps[0].name, "build (run 1/1)");
        assert_eq!(steps[0].definition_filename.as_deref(), Some("ship.toml"));
        assert_eq!(steps[0].status, crate::db::work_items::StepStatus::Running);
        assert_eq!(steps[0].started_at, 200);
        assert_eq!(steps[0].ended_at, None);

        // End it as Failed with an exit code + error.
        super::driver::shadow_record_step_end(
            Some(&db),
            Some(step_id),
            crate::db::work_items::StepStatus::Failed,
            Some(7),
            Some("cargo build returned 7"),
            350,
        )
        .await;

        let steps = crate::db::work_items::list_steps(db.adapter(), "wf-step-1")
            .await
            .expect("list_steps");
        assert_eq!(steps.len(), 1, "end must update — never insert");
        assert_eq!(steps[0].id, step_id, "same row");
        assert_eq!(steps[0].status, crate::db::work_items::StepStatus::Failed);
        assert_eq!(steps[0].exit_code, Some(7));
        assert_eq!(
            steps[0].error_message.as_deref(),
            Some("cargo build returned 7")
        );
        assert_eq!(steps[0].ended_at, Some(350));
    }

    /// Round-trip the def-run shadow helpers. The start write places the
    /// row at Running with `started_at` populated; subsequent finish
    /// writes flip the row to Completed / Error while preserving the
    /// original `started_at` (UPDATE-only contract).
    #[tokio::test]
    async fn shadow_def_run_start_and_finish_round_trip() {
        let db = Database::open_in_memory().expect("open in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");
        db.adapter()
            .execute(
                "INSERT INTO work_items (\
                    id, ticket_key, workspace_name, user_id, private, \
                    started_manually, counts_toward_manual_cap, driver_started, \
                    jira_available, state_kind, started_at, created_at, updated_at\
                 ) VALUES (\
                    'wf-def-1', 'TICK-1', 'ws', 'u-1', 0, \
                    0, 0, 0, \
                    0, 'pending', 100, 100, 100\
                 )",
                vec![],
            )
            .await
            .expect("seed work_items");

        // Start: Running, started_at=200.
        super::driver::shadow_start_def_run(Some(&db), "wf-def-1", "ship.toml", 200).await;

        let runs = crate::db::work_items::list_definition_runs(db.adapter(), "wf-def-1")
            .await
            .expect("list");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].definition_filename, "ship.toml");
        assert_eq!(runs[0].state, crate::db::work_items::DefRunState::Running);
        assert_eq!(runs[0].started_at, Some(200));
        assert_eq!(runs[0].ended_at, None);
        assert_eq!(runs[0].error_message, None);

        // Finish: Completed at t=350. started_at must be preserved.
        super::driver::shadow_finish_def_run(
            Some(&db),
            "wf-def-1",
            "ship.toml",
            crate::db::work_items::DefRunState::Completed,
            None,
            350,
        )
        .await;

        let runs = crate::db::work_items::list_definition_runs(db.adapter(), "wf-def-1")
            .await
            .expect("list");
        assert_eq!(runs.len(), 1, "UPDATE-only — never duplicates");
        assert_eq!(runs[0].state, crate::db::work_items::DefRunState::Completed);
        assert_eq!(
            runs[0].started_at,
            Some(200),
            "finish must preserve start timestamp"
        );
        assert_eq!(runs[0].ended_at, Some(350));
        assert_eq!(runs[0].error_message, None);

        // Then flip the same row to Error with a message.
        super::driver::shadow_finish_def_run(
            Some(&db),
            "wf-def-1",
            "ship.toml",
            crate::db::work_items::DefRunState::Error,
            Some("agent crashed"),
            500,
        )
        .await;

        let runs = crate::db::work_items::list_definition_runs(db.adapter(), "wf-def-1")
            .await
            .expect("list");
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].state, crate::db::work_items::DefRunState::Error);
        assert_eq!(runs[0].started_at, Some(200), "still preserved");
        assert_eq!(runs[0].ended_at, Some(500));
        assert_eq!(runs[0].error_message.as_deref(), Some("agent crashed"));

        // A second start_def_run (retry) MUST reset the row to
        // Running with fresh timing and clear the prior error so
        // observers see a clean fresh-run state.
        super::driver::shadow_start_def_run(Some(&db), "wf-def-1", "ship.toml", 900).await;
        let runs = crate::db::work_items::list_definition_runs(db.adapter(), "wf-def-1")
            .await
            .expect("list");
        assert_eq!(runs[0].state, crate::db::work_items::DefRunState::Running);
        assert_eq!(runs[0].started_at, Some(900));
        assert_eq!(runs[0].ended_at, None);
        assert_eq!(runs[0].error_message, None);
    }

    /// `shadow_finish_def_run` is a silent no-op when no prior start
    /// row exists. This is critical: the engine writes Completed/Error
    /// after the in-memory mutation, and a slow / failed start-write
    /// must not surface as an error path at finish time.
    #[tokio::test]
    async fn shadow_finish_def_run_is_silent_noop_without_prior_start() {
        let db = Database::open_in_memory().expect("open in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                vec![],
            )
            .await
            .expect("seed user");
        db.adapter()
            .execute(
                "INSERT INTO work_items (\
                    id, ticket_key, workspace_name, user_id, private, \
                    started_manually, counts_toward_manual_cap, driver_started, \
                    jira_available, state_kind, started_at, created_at, updated_at\
                 ) VALUES (\
                    'wf-orphan', 'T-1', 'ws', 'u-1', 0, \
                    0, 0, 0, \
                    0, 'pending', 100, 100, 100\
                 )",
                vec![],
            )
            .await
            .expect("seed work_items");

        // Must not panic, must not insert.
        super::driver::shadow_finish_def_run(
            Some(&db),
            "wf-orphan",
            "ship.toml",
            crate::db::work_items::DefRunState::Completed,
            None,
            42,
        )
        .await;

        let runs = crate::db::work_items::list_definition_runs(db.adapter(), "wf-orphan")
            .await
            .expect("list");
        assert!(
            runs.is_empty(),
            "finish without start must not synthesise a row"
        );
    }

    /// End-write is a no-op when `db` is `None` OR when the start-write
    /// couldn't return an id. Confirms that flaky DB conditions never
    /// throw from the engine path.
    #[tokio::test]
    async fn shadow_record_step_end_is_noop_when_id_missing() {
        let db = Database::open_in_memory().expect("open in-memory db");
        // Neither call must panic / error.
        super::driver::shadow_record_step_end(
            Some(&db),
            None,
            crate::db::work_items::StepStatus::Success,
            None,
            None,
            0,
        )
        .await;
        super::driver::shadow_record_step_end(
            None,
            Some(42),
            crate::db::work_items::StepStatus::Success,
            None,
            None,
            0,
        )
        .await;
    }
}
