// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
#![allow(deprecated)] // Transitional: ConfigStr sites rewritten to ConfigError variants in C2.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tokio::sync::RwLock;
use tracing::warn;

use crate::actions::traits::ExternalActions;
use crate::config::{Config, TicketingSystem};
use crate::container::ContainerRunner;
use crate::db::Database;
use crate::error::{MaestroError, Result};

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
    pub(crate) workflows_dir: PathBuf,
    /// Plan-10: used to look up `repositories.local_path` from a workflow's
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
        workflows_dir: PathBuf,
        db: Option<Database>,
    ) -> Self {
        Self {
            repository,
            event_bus,
            actions,
            config,
            jira_available,
            ticketing_system,
            workflows_dir,
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
        // Plan-10: resolve workspace_name from the registered `repositories` row
        // when `repository_id` is provided (the canonical path). Fall back to
        // deriving it from `cfg.git.repo_path` for tests/dry-mode paths that
        // construct workflows without a DB or a repo association.
        let ws_name = resolve_workspace_name(
            repository_id.as_deref(),
            self.db.as_ref(),
            &self.config,
        )
        .await;
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

        self.repository
            .inner_arc()
            .write()
            .await
            .insert(ticket_key.clone(), workflow);

        // Auto-start all dep-free dynamic workflow definitions
        let discovery = crate::workflow::definitions::discover_workflows(&self.workflows_dir);
        let dep_free_defs: Vec<String> = discovery
            .workflows
            .iter()
            .filter(|d| d.valid && d.depends_on.is_empty())
            .map(|d| d.filename.clone())
            .collect();

        for def_name in dep_free_defs {
            if let Err(e) = definitions.start_workflow_def(&ticket_key, &def_name).await {
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
        let ws_name = resolve_workspace_name(
            repository_id.as_deref(),
            self.db.as_ref(),
            &self.config,
        )
        .await;
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
            event_type: "workflow_updated".to_string(),
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
                .ok_or_else(|| MaestroError::ConfigStr(format!("Workflow not found: {ticket_key}")))?;
            if w.state.is_active() && w.driver_started {
                return Err(MaestroError::ConfigStr(format!(
                    "Cannot delete workflow while it is running (current: {})",
                    w.state
                )));
            }
            (
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

        // Plan-10: resolve the workflow's repository path so worktree / branch
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

        if let Err(e) = persistence.sync().await {
            warn!(ticket = %ticket_key, error = %e, "Failed to sync workflow snapshot after delete");
        }

        self.event_bus.send(WorkflowEvent {
            event_type: "workflow_removed".to_string(),
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

        let (worktree_path, branch_name, owner_user_id) = {
            let wf_arc = self.repository.inner_arc();
            let wf = wf_arc.read().await;
            let w = wf
                .get(ticket_key)
                .ok_or_else(|| MaestroError::ConfigStr(format!("Workflow not found: {ticket_key}")))?;
            if !matches!(w.state, WorkflowState::Done) {
                return Err(MaestroError::ConfigStr(format!(
                    "Mark as Done is only available when the workflow is Done (current: {})",
                    w.state
                )));
            }
            (
                w.worktree_path.clone(),
                w.branch_name.clone(),
                w.user_id.clone(),
            )
        };

        // Plan-10: resolve the per-workflow repo path.
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
            broadcast_event(WorkflowEvent {
                event_type: "workflow_removed".to_string(),
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
