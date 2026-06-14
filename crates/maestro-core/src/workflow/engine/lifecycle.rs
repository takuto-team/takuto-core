// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tokio::sync::RwLock;
use tracing::{error, warn};

use crate::actions::traits::ExternalActions;
use crate::config::{Config, ConfigError, TicketingSystem};
use crate::container::ContainerRunner;
use crate::db::Database;
use crate::error::Result;

use crate::workflow::state::WorkflowState;

use super::definitions::WorkflowDefinitionManager;
use super::driver::resolve_workspace_name;
use super::event_bus::WorkflowEventBus;
use super::persistence::WorkflowPersistence;
use super::repository::WorkflowRepository;
use super::types::{MarkDoneOutcome, Workflow, WorkflowEvent};

pub(crate) struct WorkflowLifecycle {
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub(crate) actions: Arc<dyn ExternalActions>,
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) jira_available: Arc<AtomicBool>,
    pub(crate) ticketing_system: TicketingSystem,
    /// Used to look up `repositories.local_path` from a workflow's
    /// `repository_id`. Optional only because some unit-test paths construct
    /// the engine without a DB.
    pub(crate) db: Option<Database>,
}

impl WorkflowLifecycle {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository: Arc<WorkflowRepository>,
        event_bus: Arc<WorkflowEventBus>,
        actions: Arc<dyn ExternalActions>,
        config: Arc<RwLock<Config>>,
        jira_available: Arc<AtomicBool>,
        ticketing_system: TicketingSystem,
        db: Option<Database>,
    ) -> Self {
        Self {
            repository,
            event_bus,
            actions,
            config,
            jira_available,
            ticketing_system,
            db,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn start_workflow(
        &self,
        ticket_key: String,
        ticket_summary: String,
        started_manually: bool,
        ticket_description: Option<String>,
        ticket_url: Option<String>,
        definitions: &WorkflowDefinitionManager,
        user_id: Option<String>,
        repository_id: Option<String>,
    ) -> Result<String> {
        let jira = self.jira_available.load(Ordering::Relaxed);
        // Resolve workspace_name from the registered `repositories` row when
        // `repository_id` is provided (the canonical path). Fall back to
        // deriving it from `cfg.git.repo_path` for tests/dry-mode paths that
        // construct workflows without a DB or a repo association.
        let ws_name =
            resolve_workspace_name(repository_id.as_deref(), self.db.as_ref(), &self.config).await;
        let mut workflow = Workflow::new(
            ticket_key.clone(),
            ticket_summary,
            started_manually,
            jira,
            self.ticketing_system,
            ticket_url,
            ws_name,
        );
        workflow.user_id = user_id;
        workflow.repository_id = repository_id;
        if let Some(desc) = ticket_description {
            workflow.ticket_description = desc;
        }
        // driver_started stays false until a def is started
        let id = workflow.id.clone();

        // Durably persist the work_items row BEFORE we hand the Workflow off
        // to the in-memory cache. The DB is the source of truth, so a failed
        // insert is propagated (fail-loud) rather than swallowed — we do not
        // want a cache entry the DB never recorded.
        persist_new_work_item(self.db.as_ref(), &workflow).await?;

        self.repository
            .inner_arc()
            .write()
            .await
            .insert(ticket_key.clone(), workflow);

        // Auto-start workflow definitions. The candidate set is the owner's
        // dep-free per-workspace flows (resolved from the just-inserted
        // workflow's user_id + workspace), falling back to TOML discovery when
        // no DB/user is available.
        let all_dep_free: Vec<String> = definitions
            .resolve_definitions(&ticket_key, None)
            .await
            .iter()
            .filter(|d| d.valid && d.depends_on.is_empty())
            .map(|d| d.filename.clone())
            .collect();

        // `[polling] auto_start_flow` narrows the set: empty slug → all dep-free
        // (legacy); a slug present in the set → just that one; a slug that is
        // absent → start nothing (the row still lands on the dashboard so the
        // admin notices the misconfiguration — no silent fallback to "all").
        let auto_start_flow = {
            let cfg = self.config.read().await;
            cfg.polling.auto_start_flow.clone()
        };
        let defs_to_start: Vec<String> = if auto_start_flow.is_empty() {
            all_dep_free
        } else if all_dep_free.contains(&auto_start_flow) {
            vec![auto_start_flow]
        } else {
            warn!(
                ticket = %ticket_key,
                slug = %auto_start_flow,
                "polling.auto_start_flow not found among dep-free flows; starting nothing"
            );
            Vec::new()
        };

        for def_name in defs_to_start {
            if let Err(e) = definitions
                .start_workflow_def(&ticket_key, &def_name, None)
                .await
            {
                warn!(
                    ticket = %ticket_key,
                    def = %def_name,
                    error = %e,
                    "Failed to auto-start dep-free workflow definition"
                );
            }
        }

        Ok(id)
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
        let jira = self.jira_available.load(Ordering::Relaxed);
        let ws_name =
            resolve_workspace_name(repository_id.as_deref(), self.db.as_ref(), &self.config).await;
        let mut workflow = Workflow::new(
            ticket_key.clone(),
            ticket_summary,
            started_manually,
            jira,
            self.ticketing_system,
            ticket_url,
            ws_name,
        );
        workflow.user_id = user_id;
        workflow.repository_id = repository_id;
        if let Some(desc) = ticket_description {
            workflow.ticket_description = desc;
        }
        // driver_started stays false (set by Workflow::new)
        let id = workflow.id.clone();
        let user_id_for_emit = workflow.user_id.clone();

        // Durably persist the work_items row BEFORE we hand the Workflow off
        // to the in-memory cache. Fail-loud — see `start_workflow`.
        persist_new_work_item(self.db.as_ref(), &workflow).await?;

        self.repository
            .inner_arc()
            .write()
            .await
            .insert(ticket_key.clone(), workflow);

        // Best-effort Jira assign + transition (same as the driver does, but earlier)
        if jira {
            let actions = self.actions.clone();
            let key = ticket_key.clone();
            // Resolve the workflow's repo path up-front so the spawned task is
            // self-contained and doesn't need to re-acquire DB locks.
            let (repo_path, _base_branch) = super::driver::resolve_repo_for_ticket(
                &ticket_key,
                &self.repository.inner_arc(),
                &self.config,
                self.db.as_ref(),
            )
            .await;
            tokio::spawn(async move {
                if let Err(e) = actions.assign_ticket(&repo_path, &key).await {
                    warn!(ticket = %key, error = ?e, "Failed to assign ticket at add-to-dashboard (best-effort)");
                }
                if let Err(e) = actions
                    .transition_ticket(&repo_path, &key, "In Progress")
                    .await
                {
                    warn!(ticket = %key, error = ?e, "Failed to transition ticket at add-to-dashboard (best-effort)");
                }
            });
        }

        // Broadcast event so the dashboard updates
        self.event_bus.send(WorkflowEvent {
            event_type: "work_item_updated".to_string(),
            workflow_id: id.clone(),
            ticket_key: ticket_key.clone(),
            state: "Pending".to_string(),
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: Some(0),
            progress_steps_total: None,
            forwarded_port: None,
            pr_merged: None,
            user_id: user_id_for_emit.clone(),
            ..Default::default()
        });

        // Pre-create the git worktree in the background so it is ready before the user
        // starts a workflow def.  Failure is non-fatal — bootstrap will create it on first run.
        {
            let actions = self.actions.clone();
            let config = self.config.clone();
            let workflows = self.repository.inner_arc();
            let event_tx = self.event_bus.sender().clone();
            let key = ticket_key.clone();
            let db = self.db.clone();
            tokio::spawn(async move {
                super::driver::prepare_worktree_for_ticket(
                    &key,
                    &config,
                    &workflows,
                    &actions,
                    &event_tx,
                    db.as_ref(),
                )
                .await;
            });
        }

        Ok(id)
    }

    /// Remove a workflow from the dashboard when it is not **running** (see [`WorkflowState::is_active`]).
    /// Best-effort worktree removal; no Jira transitions. Cancels the driver token if a paused task is still attached.
    pub async fn delete_workflow(
        &self,
        ticket_key: &str,
        persistence: &WorkflowPersistence,
    ) -> Result<()> {
        let (
            workflow_id,
            worktree_path,
            cancel_token,
            branch_name,
            jira_available,
            driver_started,
            owner_user_id,
        ) = {
            let wf_arc = self.repository.inner_arc();
            let map = wf_arc.read().await;
            let w = map
                .get(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;
            if w.state.is_active() && w.driver_started {
                return Err(ConfigError::InvalidWorkflowState {
                    op: "delete",
                    current_state: w.state.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }
            (
                w.id.clone(),
                w.worktree_path.clone(),
                w.cancel_token.clone(),
                w.branch_name.clone(),
                w.jira_available,
                w.driver_started,
                w.user_id.clone(),
            )
        };

        cancel_token.cancel();
        ContainerRunner::cleanup_for_ticket(ticket_key).await;

        // Resolve the workflow's repository path so worktree / branch
        // operations target the right clone.
        let (repo_path, _base_branch) = super::driver::resolve_repo_for_ticket(
            ticket_key,
            &self.repository.inner_arc(),
            &self.config,
            self.db.as_ref(),
        )
        .await;

        if let Some(ref path) = worktree_path
            && path.exists()
            && let Err(e) = self.actions.remove_worktree(&repo_path, path).await
        {
            warn!(
                ticket = %ticket_key,
                path = %path.display(),
                error = %e,
                "Failed to remove worktree on delete (workflow row still removed)"
            );
        }

        if !branch_name.trim().is_empty()
            && let Err(e) = self
                .actions
                .delete_local_branch(&repo_path, &branch_name)
                .await
        {
            warn!(
                ticket = %ticket_key,
                branch = %branch_name,
                error = %e,
                "Failed to delete local branch on delete (best-effort)"
            );
        }

        persistence.git_worktree_prune().await;

        // Unstarted workflows had Jira assign+transition at add-to-dashboard time.
        // Revert: unassign and move back to To Do.
        if jira_available && !driver_started {
            let actions = self.actions.clone();
            let key = ticket_key.to_string();
            let repo_path_clone = repo_path.clone();
            if let Err(e) = actions.unassign_ticket(&repo_path_clone, &key).await {
                warn!(ticket = %key, error = ?e, "Failed to unassign ticket on delete (best-effort)");
            }
            if let Err(e) = actions
                .transition_ticket(&repo_path_clone, &key, "To Do")
                .await
            {
                warn!(ticket = %key, error = ?e, "Failed to transition ticket back to To Do on delete (best-effort)");
            }
        }

        self.repository.inner_arc().write().await.remove(ticket_key);

        // Soft-delete the DB row: stamp `deleted_at` so the run survives as
        // history but drops out of every live query. The row used to stay
        // fully live, so the UNIQUE (workspace_name, ticket_key) index made
        // re-adding the same ticket collide on INSERT and the dashboard kept
        // serving the stale run. With soft-delete + the dropped unique index,
        // a re-add is a brand-new row / fresh run and the old run is retained,
        // flagged deleted.
        if let Some(db) = self.db.as_ref()
            && let Err(e) = crate::db::work_items::soft_delete_work_item(
                db.adapter(),
                &workflow_id,
                Utc::now().timestamp(),
            )
            .await
        {
            warn!(
                ticket = %ticket_key,
                work_item_id = %workflow_id,
                error = %e,
                "Failed to soft-delete work_items row on delete (re-add may collide)"
            );
        }

        if let Err(e) = persistence.sync().await {
            warn!(ticket = %ticket_key, error = %e, "Failed to sync workflow snapshot after delete");
        }

        self.event_bus.send(WorkflowEvent {
            event_type: "work_item_removed".to_string(),
            workflow_id: String::new(),
            ticket_key: ticket_key.to_string(),
            state: String::new(),
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: None,
            progress_steps_total: None,
            forwarded_port: None,
            pr_merged: None,
            user_id: owner_user_id,
            ..Default::default()
        });

        Ok(())
    }

    /// Jira **Done** transition (configured status name) and remove worktree; remove workflow from the map only if both succeed.
    pub async fn mark_work_done(
        &self,
        ticket_key: &str,
        persistence: &WorkflowPersistence,
        broadcast_event: impl Fn(WorkflowEvent),
    ) -> Result<MarkDoneOutcome> {
        let (done_status, remote, ticketing_system) = {
            let c = self.config.read().await;
            (
                c.jira.done_status.clone(),
                c.git.remote.clone(),
                c.general.ticketing_system,
            )
        };

        let (workflow_id, worktree_path, branch_name, owner_user_id) = {
            let wf_arc = self.repository.inner_arc();
            let wf = wf_arc.read().await;
            let w = wf
                .get(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;
            if !matches!(w.state, WorkflowState::Done) {
                return Err(ConfigError::InvalidWorkflowState {
                    op: "mark-as-done",
                    current_state: w.state.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }
            (
                w.id.clone(),
                w.worktree_path.clone(),
                w.branch_name.clone(),
                w.user_id.clone(),
            )
        };

        // Resolve the per-workflow repo path.
        let (repo_path, _base_branch) = super::driver::resolve_repo_for_ticket(
            ticket_key,
            &self.repository.inner_arc(),
            &self.config,
            self.db.as_ref(),
        )
        .await;

        let mut jira_ok = true;
        let mut jira_error = None;
        if self.jira_available.load(Ordering::Relaxed) {
            // Jira mode: transition ticket to the configured done status.
            if let Err(e) = self
                .actions
                .transition_ticket(&repo_path, ticket_key, done_status.trim())
                .await
            {
                jira_ok = false;
                jira_error = Some(e.to_string());
                warn!(ticket = %ticket_key, error = %e, "Jira transition to Done failed");
            }
        } else if ticketing_system == TicketingSystem::GitHub {
            // GitHub mode: close the corresponding issue via `gh api`.
            let cwd = worktree_path
                .as_deref()
                .filter(|p| p.exists())
                .unwrap_or(repo_path.as_path());
            match crate::git::remote::resolve_remote_url(&repo_path, &remote).await {
                Ok(remote_url) => {
                    if let Err(e) = super::driver::close_github_issue(
                        ticket_key,
                        &remote_url,
                        cwd,
                        self.actions.as_ref(),
                    )
                    .await
                    {
                        jira_ok = false;
                        jira_error = Some(e.to_string());
                        warn!(ticket = %ticket_key, error = %e, "GitHub issue close failed");
                    }
                }
                Err(e) => {
                    jira_ok = false;
                    jira_error = Some(format!("Cannot resolve git remote URL: {e}"));
                    warn!(ticket = %ticket_key, error = %e, "GitHub issue close: failed to resolve remote URL");
                }
            }
        }

        // Clean up any worker containers for this workflow
        ContainerRunner::cleanup_for_ticket(ticket_key).await;

        let mut worktree_ok = true;
        let mut worktree_error = None;
        if let Some(ref path) = worktree_path
            && path.exists()
            && let Err(e) = self.actions.remove_worktree(&repo_path, path).await
        {
            worktree_ok = false;
            worktree_error = Some(e.to_string());
            warn!(ticket = %ticket_key, path = %path.display(), error = %e, "Failed to remove worktree");
        }

        if worktree_ok
            && !branch_name.trim().is_empty()
            && let Err(e) = self
                .actions
                .delete_local_branch(&repo_path, &branch_name)
                .await
        {
            warn!(
                ticket = %ticket_key,
                branch = %branch_name,
                error = %e,
                "Failed to delete local branch after mark-done (best-effort)"
            );
        }

        if worktree_ok {
            persistence.git_worktree_prune().await;
        }

        let workflow_removed = jira_ok && worktree_ok;
        if workflow_removed {
            self.repository.inner_arc().write().await.remove(ticket_key);

            // Soft-delete the DB row so a later re-add of the same ticket is
            // a fresh run while this completed run is retained as history.
            // Mirrors delete_workflow.
            if let Some(db) = self.db.as_ref()
                && let Err(e) = crate::db::work_items::soft_delete_work_item(
                    db.adapter(),
                    &workflow_id,
                    Utc::now().timestamp(),
                )
                .await
            {
                warn!(
                    ticket = %ticket_key,
                    work_item_id = %workflow_id,
                    error = %e,
                    "Failed to soft-delete work_items row on mark-done (re-add may collide)"
                );
            }
            broadcast_event(WorkflowEvent {
                event_type: "work_item_removed".to_string(),
                workflow_id: String::new(),
                ticket_key: ticket_key.to_string(),
                state: String::new(),
                timestamp: Utc::now(),
                error: None,
                step_name: None,
                output_line: None,
                stream: None,
                progress_percent: None,
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
                user_id: owner_user_id.clone(),
                ..Default::default()
            });
        }

        Ok(MarkDoneOutcome {
            jira_ok,
            jira_error,
            worktree_ok,
            worktree_error,
            workflow_removed,
        })
    }

    pub async fn stop_all_workflows(&self, transitions: &super::transitions::WorkflowTransitions) {
        let keys: Vec<String> = {
            let wf_arc = self.repository.inner_arc();
            let workflows = wf_arc.read().await;
            workflows
                .iter()
                .filter(|(_, w)| !w.state.is_terminal())
                .map(|(k, _)| k.clone())
                .collect()
        };

        for key in keys {
            if let Err(e) = transitions.stop_workflow(&key).await {
                warn!(ticket = %key, error = %e, "Failed to stop workflow during shutdown");
            }
        }
    }
}

/// Fail-loud durable insert of a fresh `Workflow` into the authoritative
/// `work_items` table, used by the workflow-start paths.
///
/// The DB row is the source of truth (cutover invariant I1/I4), so the
/// initial insert is retried and, on persistent failure, the error is
/// **propagated** to the caller rather than swallowed — a workflow that
/// could not be durably recorded must not silently land in the in-memory
/// cache as if it had been. When `db` is `None` (dry-mode / no-DB
/// deployments) this is a no-op success.
pub(super) async fn persist_new_work_item(db: Option<&Database>, workflow: &Workflow) -> Result<()> {
    let Some(db) = db else { return Ok(()) };
    let row = workflow.to_work_item_row();
    super::driver::retry_durable_write(|| crate::db::work_items::insert_work_item(db.adapter(), &row))
        .await
        .inspect_err(|e| {
            error!(
                work_item_id = %workflow.id,
                ticket_key = %workflow.ticket_key,
                error = %e,
                "Durable insert of work_items row failed after retries — \
                 refusing to add the work item to the in-memory cache"
            );
        })
}

/// Best-effort shadow-write of a `Workflow` into the `work_items` table.
///
/// Used by the snapshot-restore backfill path, where the DB row may already
/// exist (an idempotent re-assert) and a duplicate is expected, not an
/// error. Failures are logged at WARN and swallowed so a flaky DB cannot
/// block restore. The start paths use [`persist_new_work_item`] instead,
/// which is fail-loud.
pub(super) async fn shadow_persist_work_item(db: Option<&Database>, workflow: &Workflow) {
    let Some(db) = db else { return };
    let row = workflow.to_work_item_row();
    if let Err(e) = crate::db::work_items::insert_work_item(db.adapter(), &row).await {
        warn!(
            work_item_id = %workflow.id,
            ticket_key = %workflow.ticket_key,
            error = %e,
            "Shadow-write of work_items row failed during restore backfill (in-memory state is unaffected)"
        );
    }
}

#[cfg(test)]
mod facade_engine_tests {
    use crate::actions::traits::ExternalActions;
    use crate::config::{Config, TicketingSystem};
    use crate::db::Database;
    use crate::workflow::engine::{Workflow, WorkflowEngine};
    use crate::workflow::state::WorkflowState;
    use crate::workflow::step::StepLog;
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::sync::RwLock;
    use tokio_util::sync::CancellationToken;

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

    /// `active_item_count` underpins the `[polling] max_parallel_items` cap.
    /// It must count only slot-occupying workflows (excluding terminal states)
    /// and honor the optional per-user scope.
    #[tokio::test]
    async fn active_item_count_scopes_by_user_and_excludes_terminal() {
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

        let mut alice_active = wf_with(WorkflowState::Pending, vec![], None);
        alice_active.ticket_key = "A-1".into();
        alice_active.user_id = Some("alice".into());

        let mut alice_done = wf_with(WorkflowState::Done, vec![], None);
        alice_done.ticket_key = "A-2".into();
        alice_done.user_id = Some("alice".into());

        let mut bob_active = wf_with(WorkflowState::Pending, vec![], None);
        bob_active.ticket_key = "B-1".into();
        bob_active.user_id = Some("bob".into());

        {
            let wf_arc = engine.workflows_arc();
            let mut map = wf_arc.write().await;
            map.insert("A-1".into(), alice_active);
            map.insert("A-2".into(), alice_done);
            map.insert("B-1".into(), bob_active);
        }

        // Global: two slot-occupying items (A-1, B-1); the Done row (A-2) is excluded.
        assert_eq!(engine.active_item_count(None).await, 2);
        // Per-user scoping.
        assert_eq!(engine.active_item_count(Some("alice")).await, 1);
        assert_eq!(engine.active_item_count(Some("bob")).await, 1);
        assert_eq!(engine.active_item_count(Some("carol")).await, 0);
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
        let step_id = crate::workflow::engine::driver::shadow_record_step_start(
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
        crate::workflow::engine::driver::shadow_record_step_end(
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
        crate::workflow::engine::driver::shadow_start_def_run(Some(&db), "wf-def-1", "ship.toml", 200).await;

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
        crate::workflow::engine::driver::shadow_finish_def_run(
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
        crate::workflow::engine::driver::shadow_finish_def_run(
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
        crate::workflow::engine::driver::shadow_start_def_run(Some(&db), "wf-def-1", "ship.toml", 900).await;
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
        crate::workflow::engine::driver::shadow_finish_def_run(
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
        crate::workflow::engine::driver::shadow_record_step_end(
            Some(&db),
            None,
            crate::db::work_items::StepStatus::Success,
            None,
            None,
            0,
        )
        .await;
        crate::workflow::engine::driver::shadow_record_step_end(
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
