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
#[cfg(test)]
mod test_support;
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
use crate::container::{ContainerRuntime, DockerRuntime};
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
    /// When set, workflow drivers that exit with [`TakutoError::Cancelled`] do not move the workflow to Error (graceful container shutdown + snapshot).
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
    /// Docker boundary used by the driver/step path: availability probe,
    /// worker-image discovery, container cleanup. Defaults to [`DockerRuntime`];
    /// tests inject a fake via [`Self::with_container_runtime`] so the full step
    /// loop runs without a daemon.
    pub(crate) container_runtime: Arc<dyn ContainerRuntime>,
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
            container_runtime: Arc::new(DockerRuntime),
            persistence,
            lifecycle,
            transitions,
            definitions,
        }
    }

    /// Override the container runtime (production uses `DockerRuntime`; tests
    /// inject a `FakeContainerRuntime`). Propagates to the three service structs
    /// that spawn driver tasks so the whole step path uses the same boundary.
    pub fn with_container_runtime(mut self, rt: Arc<dyn ContainerRuntime>) -> Self {
        self.container_runtime = rt.clone();
        self.definitions.set_container_runtime(rt.clone());
        self.persistence.set_container_runtime(rt.clone());
        self.transitions.set_container_runtime(rt);
        self
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
        self.transitions.set_git_auth_resolver(resolver.clone());
        self.lifecycle.set_git_auth_resolver(resolver);
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

    /// Resolve `(repo_path, default_branch)` for an existing workflow.
    ///
    /// Looks up the workflow's `repository_id` / `workspace_name` against the
    /// `repositories` table, falling back to `config.git`. Exposed for callers
    /// outside the engine (e.g. the editor endpoint's on-demand worktree
    /// recreation) that need the clone path and base branch for a ticket.
    pub async fn resolve_repo_for_ticket(&self, ticket_key: &str) -> (PathBuf, String) {
        resolve::resolve_repo_for_ticket(
            ticket_key,
            &self.workflows_arc(),
            &self.config(),
            self.db(),
        )
        .await
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
    /// Used by external callers (e.g. `takuto-web` routes) that still need direct
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

    /// Count workflows occupying a concurrency slot, optionally scoped to a
    /// single owner. Used to enforce `[polling] max_parallel_items` (global
    /// when `user_id = None`, per-user when `Some(id)`).
    pub async fn active_item_count(&self, user_id: Option<&str>) -> usize {
        self.repository.count_occupying_slot_for_user(user_id).await
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
        self.repository.reassign_orphans_to_owner(owner_id).await
    }

    /// Write `workflow_snapshot.json` and cancel drivers so processes stop, without Jira unassign / **Stopped** (for container restart).
    pub async fn persist_interrupt_for_restart(&self) -> Result<()> {
        self.persistence.persist_interrupt().await
    }

    /// Load snapshot from disk, insert workflows, spawn drivers. Removes the snapshot file on success.
    pub async fn restore_persisted_workflows(&self) -> Result<usize> {
        let restored = self
            .persistence
            .restore(
                &self.workflows_dir,
                self.agent_run_semaphore.clone(),
                self.suppress_cancelled_as_error.clone(),
                self.db.clone(),
            )
            .await?;

        // Reap workspace containers orphaned by an item deletion that raced with
        // a restart: keep only those whose work item is still live.
        let live: std::collections::HashSet<String> = self
            .workflows_arc()
            .read()
            .await
            .keys()
            .map(|k| crate::container::workspace::workspace_container_name(k))
            .collect();
        crate::container::workspace::sweep_orphan_workspaces(&live).await;

        Ok(restored)
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
        let engine_db = self.db.clone();
        self.lifecycle
            .mark_work_done(ticket_key, &self.persistence, move |mut event| {
                let workflows = engine_workflows.clone();
                let config = engine_config.clone();
                let ticket = ticket_key_owned.clone();
                let tx2 = tx.clone();
                let db = engine_db.clone();
                tokio::spawn(async move {
                    if let Some((pct, total)) =
                        driver::progress_dashboard_fields_for_ticket_with_db(
                            &workflows,
                            &config,
                            &ticket,
                            db.as_ref(),
                        )
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
        let db = self.db.clone();
        tokio::spawn(async move {
            if let Some((pct, total)) = driver::progress_dashboard_fields_for_ticket_with_db(
                &workflows,
                &config,
                &ticket_key,
                db.as_ref(),
            )
            .await
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
    use super::test_support::{errored, insert, paused, seed_workflow, test_engine};
    use crate::workflow::state::WorkflowState;

    /// Seed a representative mix: 2 alice (active + done), 1 bob (active),
    /// 1 orphan paused, 1 orphan errored.
    async fn seeded() -> (super::WorkflowEngine, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let (engine, _db) = test_engine(dir.path());
        insert(
            &engine,
            seed_workflow(WorkflowState::Pending, "A-1", Some("alice")),
        )
        .await;
        insert(
            &engine,
            seed_workflow(WorkflowState::Done, "A-2", Some("alice")),
        )
        .await;
        insert(
            &engine,
            seed_workflow(WorkflowState::Pending, "B-1", Some("bob")),
        )
        .await;
        insert(&engine, seed_workflow(paused(), "C-1", None)).await;
        insert(&engine, seed_workflow(errored(), "D-1", None)).await;
        (engine, dir)
    }

    #[tokio::test]
    async fn counts_reflect_workflow_states() {
        let (engine, _dir) = seeded().await;
        // Every row is on the dashboard.
        assert_eq!(engine.dashboard_workflow_count().await, 5);
        // Slots: everything except Done/Stopped/Error → Pending×2 + Paused.
        assert_eq!(engine.concurrency_slots_in_use().await, 3);
        // Active (not terminal, not paused, not error) → the two Pending.
        assert_eq!(engine.active_workflow_count().await, 2);
        assert_eq!(engine.get_workflow_ids().await.len(), 5);
    }

    #[tokio::test]
    async fn active_item_count_scopes_to_user() {
        let (engine, _dir) = seeded().await;
        // Global occupying-slot count.
        assert_eq!(engine.active_item_count(None).await, 3);
        // Alice has one occupying (A-1 Pending); A-2 is Done.
        assert_eq!(engine.active_item_count(Some("alice")).await, 1);
        assert_eq!(engine.active_item_count(Some("bob")).await, 1);
        assert_eq!(engine.active_item_count(Some("nobody")).await, 0);
    }

    #[tokio::test]
    async fn migrate_orphans_to_owner_is_idempotent() {
        let (engine, _dir) = seeded().await;
        // C-1 + D-1 have user_id None.
        assert_eq!(engine.migrate_orphan_workflows_to_owner("admin").await, 2);
        // Re-running touches nothing.
        assert_eq!(engine.migrate_orphan_workflows_to_owner("admin").await, 0);
        // The previously-orphan workflows now belong to admin.
        let arc = engine.workflows_arc();
        let map = arc.read().await;
        assert_eq!(map["C-1"].user_id.as_deref(), Some("admin"));
        assert_eq!(map["D-1"].user_id.as_deref(), Some("admin"));
    }

    #[tokio::test]
    async fn add_to_dashboard_inserts_without_driver() {
        let dir = tempfile::tempdir().unwrap();
        let (engine, _db) = test_engine(dir.path());
        let id = engine
            .add_to_dashboard(
                "NEW-1".to_string(),
                "new item".to_string(),
                true,
                None,
                None,
                None,
                None,
            )
            .await
            .expect("add_to_dashboard");
        assert!(!id.is_empty());
        assert_eq!(engine.dashboard_workflow_count().await, 1);
        assert!(
            engine
                .get_workflow_ids()
                .await
                .contains(&"NEW-1".to_string())
        );
        // No driver was spawned — the workflow is parked on the dashboard.
        let arc = engine.workflows_arc();
        assert!(!arc.read().await["NEW-1"].driver_started);
    }
}
