// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::actions::traits::ExternalActions;
use crate::config::Config;
use crate::error::Result;

use crate::workflow::snapshot::{
    self, PersistedWorkflowRecord, read_all_workspace_snapshots, read_workflow_snapshot,
    workspace_name_from_repo_path, write_all_workspace_snapshots,
};
use crate::workflow::state::WorkflowState;

use super::driver::drive_workflow_def;
use super::event_bus::WorkflowEventBus;
use super::repository::WorkflowRepository;
use super::types::{Workflow, workflow_to_persisted_record};

pub(crate) struct WorkflowPersistence {
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub(crate) suppress_cancelled_as_error: Arc<AtomicBool>,
    pub(crate) actions: Arc<dyn ExternalActions>,
}

impl WorkflowPersistence {
    pub fn new(
        repository: Arc<WorkflowRepository>,
        config: Arc<RwLock<Config>>,
        event_bus: Arc<WorkflowEventBus>,
        suppress_cancelled_as_error: Arc<AtomicBool>,
        actions: Arc<dyn ExternalActions>,
    ) -> Self {
        Self {
            repository,
            config,
            event_bus,
            suppress_cancelled_as_error,
            actions,
        }
    }

    /// Periodic snapshot sync during normal operation (called by background task).
    pub async fn sync(&self) -> Result<()> {
        self.sync_from_map().await
    }

    /// Load snapshots from ALL workspaces, insert workflows, spawn drivers for in-progress ones.
    pub async fn restore(
        &self,
        workflows_dir: &std::path::Path,
        agent_run_semaphore: Arc<tokio::sync::Semaphore>,
        suppress_cancelled_as_error: Arc<AtomicBool>,
    ) -> Result<usize> {
        let repo_path = {
            let c = self.config.read().await;
            PathBuf::from(&c.git.repo_path)
        };
        let data_dir = snapshot::resolve_snapshot_dir(&repo_path);
        let default_ws_name = workspace_name_from_repo_path(&repo_path);

        info!(
            data_dir = %data_dir.display(),
            "Loading workflow snapshots from all workspaces"
        );

        // Try multi-workspace read first; fall back to single-workspace read for first-time migration.
        let mut records = read_all_workspace_snapshots(&data_dir)?;
        if records.is_empty() {
            // Try single-workspace read (handles legacy + global migration).
            if let Some(file) = read_workflow_snapshot(&repo_path)? {
                if file.version == snapshot::SNAPSHOT_VERSION {
                    records = file.workflows;
                }
            }
        }

        // Backfill workspace_name for any legacy records.
        for rec in &mut records {
            if rec.workspace_name.is_empty() {
                rec.workspace_name = default_ws_name.clone();
            }
        }
        records.sort_by_key(|r| r.started_at);

        let n = records.len();
        for rec in records {
            let ticket_key = rec.ticket_key.clone();
            let is_done = matches!(rec.state, WorkflowState::Done);
            let is_terminal = is_done
                || matches!(
                    rec.state,
                    WorkflowState::Stopped | WorkflowState::Error { .. }
                );
            let state_display = rec.state.to_string();
            let is_unstarted_pending =
                matches!(rec.state, WorkflowState::Pending) && !rec.driver_started;
            let wf = Workflow::from_persisted_record(rec);
            let cancel_token = wf.cancel_token.clone();

            self.repository
                .inner_arc()
                .write()
                .await
                .insert(ticket_key.clone(), wf);

            // Terminal workflows (Done, Stopped, Error) are restored for dashboard visibility
            // but don't need a driver — they're idle until the user clicks an action (retry, delete, etc.).
            if is_terminal {
                info!(ticket = %ticket_key, state = %state_display, "Restored terminal workflow (no driver)");
                continue;
            }

            // Unstarted Pending workflows (added to dashboard but never started) are restored
            // without a driver, like terminal states. The user must click "Start" on the dashboard.
            if is_unstarted_pending {
                info!(ticket = %ticket_key, "Restored unstarted Pending workflow (no driver)");
                continue;
            }

            let engine_config = self.config.clone();
            let engine_workflows = self.repository.inner_arc();
            let engine_actions = self.actions.clone();
            let engine_event_tx = self.event_bus.sender().clone();
            let agent_sem = agent_run_semaphore.clone();
            let suppress = suppress_cancelled_as_error.clone();

            {
                use crate::workflow::definitions::{WorkflowDefRunState, discover_workflows};

                // Find defs that were running when the server stopped, and re-spawn their drivers.
                let (running_def_names, wt, ts, td, tt) = {
                    let wf_arc = self.repository.inner_arc();
                    let wf_map = wf_arc.read().await;
                    let w = wf_map.get(&ticket_key);
                    let running: Vec<String> = w
                        .map(|w| {
                            w.workflow_def_runs
                                .iter()
                                .filter(|(_, s)| matches!(s, WorkflowDefRunState::Running))
                                .map(|(n, _)| n.clone())
                                .collect()
                        })
                        .unwrap_or_default();
                    let worktree = w
                        .and_then(|w| w.worktree_path.clone())
                        .filter(|p| p.exists());
                    let (ts, td, tt) = w
                        .map(|w| {
                            (
                                w.ticket_summary.clone(),
                                w.ticket_description.clone(),
                                w.ticket_type.clone(),
                            )
                        })
                        .unwrap_or_default();
                    (running, worktree, ts, td, tt)
                };

                if running_def_names.is_empty() {
                    info!(ticket = %ticket_key, "Restored workflow with no running defs (no driver spawned)");
                    continue;
                }

                let discovery = discover_workflows(workflows_dir);

                for def_name in running_def_names {
                    if let Some(def) = discovery.workflows.iter().find(|d| d.filename == def_name) {
                        if wt.is_none() {
                            warn!(
                                ticket = %ticket_key,
                                def = %def_name,
                                "Worktree missing after restart — marking def as error"
                            );
                            let wf_arc = self.repository.inner_arc();
                            let mut wf_map = wf_arc.write().await;
                            if let Some(w) = wf_map.get_mut(&ticket_key) {
                                w.workflow_def_runs.insert(
                                    def_name.clone(),
                                    WorkflowDefRunState::Error {
                                        message: "Worktree missing after restart; use retry button"
                                            .into(),
                                    },
                                );
                            }
                            continue;
                        }

                        let steps = def.steps.clone();
                        let def_owned = def_name.clone();
                        let ticket = ticket_key.clone();
                        let worktree = wt.clone();
                        let ticket_summary = ts.clone();
                        let ticket_description = td.clone();
                        let ticket_type = tt.clone();
                        let ec = engine_config.clone();
                        let ew = engine_workflows.clone();
                        let ea = engine_actions.clone();
                        let et = engine_event_tx.clone();
                        let as_ = agent_sem.clone();
                        let su = suppress.clone();
                        let ct = cancel_token.clone();

                        tokio::spawn(async move {
                            drive_workflow_def(
                                ticket,
                                def_owned,
                                steps,
                                worktree,
                                ticket_summary,
                                ticket_description,
                                ticket_type,
                                ec,
                                ew,
                                ea,
                                et,
                                ct,
                                as_,
                                su,
                            )
                            .await;
                        });
                    } else {
                        warn!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Running def not found in workflows dir after restart"
                        );
                        let wf_arc = self.repository.inner_arc();
                        let mut wf_map = wf_arc.write().await;
                        if let Some(w) = wf_map.get_mut(&ticket_key) {
                            w.workflow_def_runs.insert(
                                def_name.clone(),
                                WorkflowDefRunState::Error {
                                    message: "Def file not found after restart".into(),
                                },
                            );
                        }
                    }
                }
            }
        }

        info!(
            count = n,
            data_dir = %data_dir.display(),
            "Restored workflows from snapshots"
        );

        Ok(n)
    }

    /// Write per-workspace snapshots and cancel drivers so processes stop, without Jira unassign / **Stopped** (for container restart).
    pub async fn persist_interrupt(&self) -> Result<()> {
        self.suppress_cancelled_as_error
            .store(true, Ordering::SeqCst);

        let repo_path = {
            let c = self.config.read().await;
            PathBuf::from(&c.git.repo_path)
        };
        let data_dir = snapshot::resolve_snapshot_dir(&repo_path);

        let records: Vec<PersistedWorkflowRecord> = {
            let wf_arc = self.repository.inner_arc();
            let map = wf_arc.read().await;
            // Persist ALL workflows — they remain on the dashboard until explicitly deleted or marked done.
            let mut v: Vec<_> = map.values().map(workflow_to_persisted_record).collect();
            v.sort_by_key(|r| r.started_at);
            v
        };

        if !records.is_empty() {
            write_all_workspace_snapshots(&data_dir, &records)?;
            info!(
                count = records.len(),
                data_dir = %data_dir.display(),
                "Wrote per-workspace snapshots for resume after restart"
            );
        }

        let keys: Vec<String> = records.iter().map(|r| r.ticket_key.clone()).collect();
        for key in &keys {
            crate::container::ContainerRunner::cleanup_for_ticket(key).await;
        }
        for key in keys {
            let token = {
                let wf_arc = self.repository.inner_arc();
                let map = wf_arc.read().await;
                map.get(&key).map(|w| w.cancel_token.clone())
            };
            if let Some(t) = token {
                t.cancel();
            }
        }

        Ok(())
    }

    /// Rewrite per-workspace snapshots from the current in-memory map (best-effort).
    async fn sync_from_map(&self) -> Result<()> {
        let repo_path = {
            let c = self.config.read().await;
            PathBuf::from(&c.git.repo_path)
        };
        let data_dir = snapshot::resolve_snapshot_dir(&repo_path);

        let records: Vec<PersistedWorkflowRecord> = {
            let map = self.repository.inner_arc();
            let map = map.read().await;
            let mut v: Vec<_> = map.values().map(workflow_to_persisted_record).collect();
            v.sort_by_key(|r| r.started_at);
            v
        };

        write_all_workspace_snapshots(&data_dir, &records)?;
        Ok(())
    }

    pub async fn git_worktree_prune(&self) {
        let repo_path = {
            let c = self.config.read().await;
            PathBuf::from(&c.git.repo_path)
        };
        match self
            .actions
            .run_command("git worktree prune", &repo_path)
            .await
        {
            Ok(o) if o.success() => {}
            Ok(o) => warn!(
                stderr = %o.stderr,
                "git worktree prune finished with non-zero status"
            ),
            Err(e) => warn!(error = %e, "git worktree prune failed"),
        }
    }
}
