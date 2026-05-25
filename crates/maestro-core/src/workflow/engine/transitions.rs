// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::actions::traits::ExternalActions;
use crate::config::{Config, ConfigError};
use crate::container::ContainerRunner;
use crate::db::Database;
use crate::error::Result;

use crate::workflow::state::WorkflowState;
use crate::workflow::step::StepStatus;

use super::definitions::WorkflowDefinitionManager;
use super::event_bus::WorkflowEventBus;
use super::repository::WorkflowRepository;
use super::types::WorkflowEvent;

pub(crate) struct WorkflowTransitions {
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub(crate) actions: Arc<dyn ExternalActions>,
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) agent_run_semaphore: Arc<Semaphore>,
    pub(crate) suppress_cancelled_as_error: Arc<AtomicBool>,
    pub(crate) jira_available: Arc<AtomicBool>,
    pub(crate) workflows_dir: PathBuf,
    pub(crate) db: Option<Database>,
    /// Phase 2b.3: resolver for pin + bundle build on resume-after-pause.
    pub(crate) git_auth_resolver:
        Option<Arc<crate::github::auth_resolver::GitAuthResolver>>,
    /// Phase 2b.3.x: GhClient for at-resume PAT revalidation.
    pub(crate) gh_client: Option<Arc<dyn crate::auth::GhClient>>,
}

impl WorkflowTransitions {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository: Arc<WorkflowRepository>,
        event_bus: Arc<WorkflowEventBus>,
        actions: Arc<dyn ExternalActions>,
        config: Arc<RwLock<Config>>,
        agent_run_semaphore: Arc<Semaphore>,
        suppress_cancelled_as_error: Arc<AtomicBool>,
        jira_available: Arc<AtomicBool>,
        workflows_dir: PathBuf,
        db: Option<Database>,
    ) -> Self {
        Self {
            repository,
            event_bus,
            actions,
            config,
            agent_run_semaphore,
            suppress_cancelled_as_error,
            jira_available,
            workflows_dir,
            db,
            git_auth_resolver: None,
            gh_client: None,
        }
    }

    pub(crate) fn set_git_auth_resolver(
        &mut self,
        resolver: Arc<crate::github::auth_resolver::GitAuthResolver>,
    ) {
        self.git_auth_resolver = Some(resolver);
    }

    pub(crate) fn set_gh_client(&mut self, gh: Arc<dyn crate::auth::GhClient>) {
        self.gh_client = Some(gh);
    }

    pub async fn pause_workflow(&self, ticket_key: &str) -> Result<()> {
        let (ticket_key_owned, workflow_id, owner_user_id) = {
            let wf_arc = self.repository.inner_arc();
            let mut workflows = wf_arc.write().await;
            let workflow = workflows
                .get_mut(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            if !workflow.state.is_active() {
                return Err(ConfigError::InvalidWorkflowState {
                    op: "pause",
                    current_state: workflow.state.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }

            // Cancel the current driver token so the running agent process is killed immediately.
            workflow.cancel_token.cancel();

            let source = Box::new(workflow.state.clone());
            workflow.state = WorkflowState::Paused {
                source_state: source,
            };
            workflow.updated_at = Utc::now();

            // Replace the cancel token with a fresh one for the resumed driver.
            workflow.cancel_token = CancellationToken::new();

            (
                ticket_key.to_string(),
                workflow.id.clone(),
                workflow.user_id.clone(),
            )
        };

        // Force-remove any worker containers for this ticket.
        ContainerRunner::cleanup_for_ticket(&ticket_key_owned).await;

        self.event_bus.send(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id,
            ticket_key: ticket_key_owned,
            state: "Paused".to_string(),
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

    pub async fn resume_workflow(&self, ticket_key: &str) -> Result<()> {
        use crate::workflow::definitions::{WorkflowDefRunState, discover_workflows};

        let (running_defs, worktree_path, cancel_token) = {
            let wf_arc = self.repository.inner_arc();
            let mut workflows = wf_arc.write().await;
            let workflow = workflows
                .get_mut(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            if let WorkflowState::Paused { source_state } = &workflow.state {
                let restored = *source_state.clone();
                workflow.state = restored;
                workflow.updated_at = Utc::now();
                // Drop Running step-log entries — interrupted steps will re-run.
                workflow
                    .steps_log
                    .retain(|s| s.status != StepStatus::Running);

                let state_line = workflow.status_display();
                self.event_bus.send(WorkflowEvent {
                    event_type: "workflow_updated".to_string(),
                    workflow_id: workflow.id.clone(),
                    ticket_key: ticket_key.to_string(),
                    state: state_line,
                    timestamp: Utc::now(),
                    error: None,
                    step_name: None,
                    output_line: None,
                    stream: None,
                    progress_percent: None,
                    progress_steps_total: None,
                    forwarded_port: None,
                    pr_merged: None,
                    user_id: workflow.user_id.clone(),
                    ..Default::default()
                });

                let running: Vec<String> = workflow
                    .workflow_def_runs
                    .iter()
                    .filter(|(_, s)| matches!(s, WorkflowDefRunState::Running))
                    .map(|(n, _)| n.clone())
                    .collect();

                let wt = workflow.worktree_path.clone().filter(|p| p.exists());
                (running, wt, workflow.cancel_token.clone())
            } else {
                return Err(ConfigError::InvalidWorkflowState {
                    op: "resume",
                    current_state: workflow.state.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }
        };

        // Re-spawn drive_workflow_def for each def that was running when paused
        if running_defs.is_empty() {
            info!(ticket = %ticket_key, "Resumed workflow has no running defs — no driver spawned");
            return Ok(());
        }

        let discovery = discover_workflows(&self.workflows_dir);
        let engine_config = self.config.clone();
        let engine_workflows = self.repository.inner_arc();
        let engine_actions = self.actions.clone();
        let engine_event_tx = self.event_bus.sender().clone();
        let agent_sem = self.agent_run_semaphore.clone();
        let suppress = self.suppress_cancelled_as_error.clone();

        for def_name in running_defs {
            if let Some(def) = discovery.workflows.iter().find(|d| d.filename == def_name) {
                let steps = def.steps.clone();
                let ticket = ticket_key.to_string();
                let def_owned = def_name.clone();
                let wt = worktree_path.clone();
                let (ts, td, tt) = {
                    let wf_arc = self.repository.inner_arc();
                    let wf = wf_arc.read().await;
                    wf.get(ticket_key)
                        .map(|w| {
                            (
                                w.ticket_summary.clone(),
                                w.ticket_description.clone(),
                                w.ticket_type.clone(),
                            )
                        })
                        .unwrap_or_default()
                };
                let ec = engine_config.clone();
                let ew = engine_workflows.clone();
                let ea = engine_actions.clone();
                let et = engine_event_tx.clone();
                let as_ = agent_sem.clone();
                let su = suppress.clone();
                let ct = cancel_token.clone();
                let db = self.db.clone();
                // Phase 2b.3.x: revalidate the user's PAT on resume —
                // their SSO session may have lapsed while paused. Done BEFORE
                // taking the owned `resolver` clone for `drive_workflow_def` so
                // we don't move-then-borrow.
                if let (Some(r), Some(gh)) =
                    (self.git_auth_resolver.as_ref(), self.gh_client.as_ref())
                {
                    let wf_arc = self.repository.inner_arc();
                    let pin_uid = {
                        let wf = wf_arc.read().await;
                        wf.get(ticket_key).and_then(|w| {
                            w.auth_pin.as_ref().and_then(|p| {
                                let uid = w.user_id.clone()?;
                                p.github_credential_row_id.map(|_| uid)
                            })
                        })
                    };
                    if let Some(uid) = pin_uid {
                        let r_clone: Arc<crate::github::auth_resolver::GitAuthResolver> =
                            r.clone();
                        let gh_clone: Arc<dyn crate::auth::GhClient> = gh.clone();
                        let event_tx = engine_event_tx.clone();
                        let ticket_for_event = ticket_key.to_string();
                        tokio::spawn(async move {
                            if let Err(e) = r_clone
                                .revalidate_pat_for_workflow(&uid, gh_clone.as_ref(), &[])
                                .await
                            {
                                let (code, message) =
                                    crate::github::auth_resolver::auth_warning_payload(&e);
                                tracing::warn!(
                                    ticket = %ticket_for_event,
                                    user_id = %uid,
                                    code = code,
                                    "PAT revalidation failed at resume — emitting AuthWarning"
                                );
                                let _ = event_tx.send(WorkflowEvent {
                                    event_type: "auth_warning".to_string(),
                                    ticket_key: ticket_for_event,
                                    timestamp: chrono::Utc::now(),
                                    user_id: Some(uid),
                                    auth_warning_code: Some(code.to_string()),
                                    auth_warning_message: Some(message),
                                    ..Default::default()
                                });
                            }
                        });
                    }
                }

                // Phase 2b.3: thread the resolver into the resumed driver task.
                let resolver = self.git_auth_resolver.clone();
                tokio::spawn(async move {
                    super::driver::drive_workflow_def(
                        ticket, def_owned, steps, wt, ts, td, tt, ec, ew, ea, et, ct, as_, su, db,
                        resolver,
                    )
                    .await;
                });
            } else {
                warn!(ticket = %ticket_key, def = %def_name, "Running def not found in workflows dir during resume");
            }
        }

        Ok(())
    }

    pub async fn stop_workflow(&self, ticket_key: &str) -> Result<()> {
        let (ticket_key_owned, workflow_id, owner_user_id) = {
            let wf_arc = self.repository.inner_arc();
            let mut workflows = wf_arc.write().await;
            let workflow = workflows
                .get_mut(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            workflow.cancel_token.cancel();
            workflow.current_step_label = None;
            workflow.state = WorkflowState::Stopped;
            workflow.updated_at = Utc::now();

            (
                ticket_key.to_string(),
                workflow.id.clone(),
                workflow.user_id.clone(),
            )
        };

        ContainerRunner::cleanup_for_ticket(&ticket_key_owned).await;

        if self.jira_available.load(Ordering::Relaxed) {
            let actions = self.actions.clone();
            let ticket_for_jira = ticket_key_owned.clone();
            let (repo_path, _base_branch) = super::driver::resolve_repo_for_ticket(
                &ticket_for_jira,
                &self.repository.inner_arc(),
                &self.config,
                self.db.as_ref(),
            )
            .await;

            tokio::spawn(async move {
                if let Err(e) = actions.unassign_ticket(&repo_path, &ticket_for_jira).await {
                    warn!(error = ?e, ticket = %ticket_for_jira, "Failed to unassign ticket on stop");
                }
                if let Err(e) = actions
                    .transition_ticket(&repo_path, &ticket_for_jira, "To Do")
                    .await
                {
                    warn!(error = ?e, ticket = %ticket_for_jira, "Failed to transition ticket back to To Do on stop");
                }
            });
        }

        self.event_bus.send(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id,
            ticket_key: ticket_key_owned,
            state: "Stopped".to_string(),
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

    pub async fn retry_workflow(
        &self,
        ticket_key: &str,
        lifecycle: &super::lifecycle::WorkflowLifecycle,
        definitions: &WorkflowDefinitionManager,
    ) -> Result<String> {
        let (ticket_summary, ticket_description, ticket_url, user_id, repository_id) = {
            let wf_arc = self.repository.inner_arc();
            let workflows = wf_arc.read().await;
            let workflow = workflows
                .get(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            if !workflow.state.is_terminal() {
                return Err(ConfigError::InvalidWorkflowState {
                    op: "retry",
                    current_state: workflow.state.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }

            (
                workflow.ticket_summary.clone(),
                if workflow.ticket_description.is_empty() {
                    None
                } else {
                    Some(workflow.ticket_description.clone())
                },
                workflow.ticket_url.clone(),
                workflow.user_id.clone(),
                workflow.repository_id.clone(),
            )
        };

        // Remove the old workflow
        self.repository.inner_arc().write().await.remove(ticket_key);

        // Start a fresh one (preserves description, ticket URL, owner, and repo for the retry)
        lifecycle
            .start_workflow(
                ticket_key.to_string(),
                ticket_summary,
                false,
                ticket_description,
                ticket_url,
                definitions,
                user_id,
                repository_id,
            )
            .await
    }

    /// Resume a failed or stopped workflow by retrying all Error-state workflow definitions.
    pub async fn resume_from_error(
        &self,
        ticket_key: &str,
        definitions: &WorkflowDefinitionManager,
    ) -> Result<()> {
        use crate::workflow::definitions::WorkflowDefRunState;

        // Collect Error defs and restore the workflow state.
        let error_defs: Vec<String> = {
            let wf_arc = self.repository.inner_arc();
            let mut workflows = wf_arc.write().await;
            let workflow = workflows
                .get_mut(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            // Require Error or Stopped state at the workflow level.
            if !matches!(
                workflow.state,
                WorkflowState::Error { .. } | WorkflowState::Stopped
            ) {
                return Err(ConfigError::InvalidWorkflowState {
                    op: "resume-from-error",
                    current_state: workflow.state.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }

            // Collect all defs that are in Error state.
            let defs: Vec<String> = workflow
                .workflow_def_runs
                .iter()
                .filter(|(_, s)| matches!(s, WorkflowDefRunState::Error { .. }))
                .map(|(n, _)| n.clone())
                .collect();

            if defs.is_empty() {
                return Err(ConfigError::Operational {
                    op: "retry workflow",
                    detail: "No failed workflow definitions to retry. Use the individual def retry buttons.".to_string(),
                }
                .into());
            }

            // Reset Error defs to Idle and clear the workflow-level error state.
            for def_name in &defs {
                workflow
                    .workflow_def_runs
                    .insert(def_name.clone(), WorkflowDefRunState::Idle);
            }
            workflow.state = WorkflowState::Pending;
            workflow.current_step_label = None;
            workflow.updated_at = Utc::now();

            defs
        };

        // Re-start each error def via start_workflow_def (handles bootstrap if needed).
        for def_name in error_defs {
            if let Err(e) = definitions.start_workflow_def(ticket_key, &def_name).await {
                warn!(ticket = %ticket_key, def = %def_name, error = %e, "Failed to restart error def");
            }
        }

        Ok(())
    }
}
