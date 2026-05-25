// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::Utc;
use tokio::sync::{RwLock, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::info;

use crate::actions::traits::ExternalActions;
use crate::config::{Config, ConfigError};
use crate::db::Database;
use crate::error::Result;

use super::event_bus::WorkflowEventBus;
use super::repository::WorkflowRepository;
use super::types::WorkflowEvent;

pub(crate) struct WorkflowDefinitionManager {
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) actions: Arc<dyn ExternalActions>,
    pub(crate) agent_run_semaphore: Arc<Semaphore>,
    pub(crate) suppress_cancelled_as_error: Arc<AtomicBool>,
    pub(crate) workflows_dir: PathBuf,
    /// Optional DB handle used by the bootstrap driver for per-workspace overrides.
    pub(crate) db: Option<Database>,
    /// Phase 2b.3: resolver for pin + bundle build. Set via
    /// [`WorkflowEngine::with_git_auth_resolver`].
    pub(crate) git_auth_resolver:
        Option<Arc<crate::github::auth_resolver::GitAuthResolver>>,
}

impl WorkflowDefinitionManager {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repository: Arc<WorkflowRepository>,
        event_bus: Arc<WorkflowEventBus>,
        config: Arc<RwLock<Config>>,
        actions: Arc<dyn ExternalActions>,
        agent_run_semaphore: Arc<Semaphore>,
        suppress_cancelled_as_error: Arc<AtomicBool>,
        workflows_dir: PathBuf,
        db: Option<Database>,
    ) -> Self {
        Self {
            repository,
            event_bus,
            config,
            actions,
            agent_run_semaphore,
            suppress_cancelled_as_error,
            workflows_dir,
            db,
            git_auth_resolver: None,
        }
    }

    pub(crate) fn set_git_auth_resolver(
        &mut self,
        resolver: Arc<crate::github::auth_resolver::GitAuthResolver>,
    ) {
        self.git_auth_resolver = Some(resolver);
    }

    /// Start running a specific workflow definition for a ticket.
    pub async fn start_workflow_def(&self, ticket_key: &str, def_name: &str) -> Result<()> {
        use crate::workflow::definitions::{
            WorkflowDefRunState, are_dependencies_met, discover_workflows,
        };

        // Discover workflow definitions from the workflows directory
        let discovery = discover_workflows(&self.workflows_dir);
        let def = discovery
            .workflows
            .iter()
            .find(|w| w.filename == def_name)
            .ok_or_else(|| ConfigError::DefinitionNotFound {
                def_name: def_name.to_string(),
                dir: self.workflows_dir.clone(),
            })?;

        if !def.valid {
            return Err(ConfigError::DefinitionInvalid {
                def_name: def_name.to_string(),
                reason: def.error.as_deref().unwrap_or("unknown error").to_string(),
            }
            .into());
        }

        // Extract needed data under read lock, then release
        let (workflow_id, maybe_wt, ticket_summary, ticket_description, ticket_type, run_states) = {
            let wf_arc = self.repository.inner_arc();
            let wf_map = wf_arc.read().await;
            let w = wf_map
                .get(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            // Only skip bootstrap entirely (resume path) when the full bootstrap — including
            // mise install and hooks — has already completed.  A pre-created worktree (created
            // at ticket-add time, worktree_bootstrapped == false) still needs bootstrap to run
            // mise install and project hooks; it just skips the git-worktree-add step.
            let maybe_wt = if w.worktree_bootstrapped {
                w.worktree_path.as_ref().filter(|p| p.exists()).cloned()
            } else {
                None
            };

            // Check if already running this definition
            if let Some(state) = w.workflow_def_runs.get(def_name)
                && matches!(state, WorkflowDefRunState::Running)
            {
                return Err(ConfigError::DefinitionAlreadyRunning {
                    def_name: def_name.to_string(),
                    ticket_key: ticket_key.to_string(),
                }
                .into());
            }

            (
                w.id.clone(),
                maybe_wt,
                w.ticket_summary.clone(),
                w.ticket_description.clone(),
                w.ticket_type.clone(),
                w.workflow_def_runs.clone(),
            )
        };

        // Check dependencies
        if !are_dependencies_met(def_name, &discovery.workflows, &run_states) {
            return Err(ConfigError::DefinitionDependenciesNotMet {
                def_name: def_name.to_string(),
            }
            .into());
        }

        // Set the run state to Running under write lock and assign a fresh cancel token.
        // CancellationToken never un-cancels, so a prior stop/interrupt/shutdown would make
        // the definition driver exit instantly at `check_cancelled` even though the parent
        // workflow may now allow this action.
        let (display, cancel_token, owner_user_id) = {
            let wf_arc = self.repository.inner_arc();
            let mut wf_map = wf_arc.write().await;
            let w = wf_map
                .get_mut(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;
            w.cancel_token = CancellationToken::new();
            w.driver_started = true;
            w.workflow_def_runs
                .insert(def_name.to_string(), WorkflowDefRunState::Running);
            w.updated_at = Utc::now();
            (
                w.status_display(),
                w.cancel_token.clone(),
                w.user_id.clone(),
            )
        };

        // Broadcast update event
        self.event_bus.send(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id: workflow_id.clone(),
            ticket_key: ticket_key.to_string(),
            state: display,
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

        // Clone values for the spawned task
        let engine_config = self.config.clone();
        let engine_workflows = self.repository.inner_arc();
        let engine_actions = self.actions.clone();
        let engine_event_tx = self.event_bus.sender().clone();
        let agent_sem = self.agent_run_semaphore.clone();
        let suppress = self.suppress_cancelled_as_error.clone();
        let ticket = ticket_key.to_string();
        let def_name_owned = def_name.to_string();
        let steps = def.steps.clone();
        let db = self.db.clone();
        // Phase 2b.3: thread the resolver into the spawned driver task.
        let resolver = self.git_auth_resolver.clone();

        tokio::spawn(async move {
            super::driver::drive_workflow_def(
                ticket,
                def_name_owned,
                steps,
                maybe_wt,
                ticket_summary,
                ticket_description,
                ticket_type,
                engine_config,
                engine_workflows,
                engine_actions,
                engine_event_tx,
                cancel_token,
                agent_sem,
                suppress,
                db,
                resolver,
            )
            .await;
        });

        Ok(())
    }

    /// Reset a workflow definition run from Error to Idle and start it again.
    pub async fn retry_workflow_def(&self, ticket_key: &str, def_name: &str) -> Result<()> {
        use crate::workflow::definitions::WorkflowDefRunState;

        // Reset the state from Error to Idle
        {
            let wf_arc = self.repository.inner_arc();
            let mut wf_map = wf_arc.write().await;
            let w = wf_map
                .get_mut(ticket_key)
                .ok_or_else(|| ConfigError::WorkflowNotFound {
                    ticket_key: ticket_key.to_string(),
                })?;

            match w.workflow_def_runs.get(def_name) {
                Some(WorkflowDefRunState::Error { .. }) => {
                    w.workflow_def_runs
                        .insert(def_name.to_string(), WorkflowDefRunState::Idle);
                }
                Some(state) => {
                    return Err(ConfigError::DefinitionRetryWrongState {
                        def_name: def_name.to_string(),
                        current_state: state.display_name().to_string(),
                    }
                    .into());
                }
                None => {
                    return Err(ConfigError::DefinitionNoRunState {
                        def_name: def_name.to_string(),
                        ticket_key: ticket_key.to_string(),
                    }
                    .into());
                }
            }
        }

        self.start_workflow_def(ticket_key, def_name).await
    }

    /// Start a background task that periodically scans the workflows directory for changes
    /// and broadcasts a `workflow_definitions_changed` event when the file list changes.
    pub fn start_definitions_watcher(&self, cancel_token: CancellationToken) {
        let workflows_dir = self.workflows_dir.clone();
        let event_tx = self.event_bus.sender().clone();

        tokio::spawn(async move {
            let mut last_snapshot: Option<Vec<(String, std::time::SystemTime)>> = None;
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));

            loop {
                tokio::select! {
                    _ = cancel_token.cancelled() => break,
                    _ = interval.tick() => {
                        let current = super::driver::scan_definitions_dir(&workflows_dir);
                        let changed = match &last_snapshot {
                            None => true,
                            Some(prev) => prev != &current,
                        };
                        if changed {
                            if last_snapshot.is_some() {
                                // Only broadcast after the first scan (skip initial)
                                let _ = event_tx.send(WorkflowEvent {
                                    event_type: "workflow_definitions_changed".to_string(),
                                    workflow_id: String::new(),
                                    ticket_key: String::new(),
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
                                    // Broadcast: no specific user — visible to all subscribers.
                                    user_id: None,
                                    ..Default::default()
                                });
                                info!("Workflow definitions directory changed, notified clients");
                            }
                            last_snapshot = Some(current);
                        }
                    }
                }
            }
        });
    }
}
