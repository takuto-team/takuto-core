// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::actions::traits::ExternalActions;
use crate::config::Config;
use crate::db::Database;
use crate::error::Result;

use crate::workflow::snapshot::{
    self, PersistedWorkflowRecord, cleanup_legacy_global_snapshot, read_all_workspace_snapshots,
    read_workflow_snapshot, resolve_data_dir, workspace_name_from_repo_path,
    write_all_workspace_snapshots,
};
use crate::workflow::state::WorkflowState;

use super::driver::drive_workflow_def;
use super::event_bus::WorkflowEventBus;
use super::repository::WorkflowRepository;
use super::types::{TerminalLine, Workflow, workflow_to_persisted_record};

pub(crate) struct WorkflowPersistence {
    pub(crate) repository: Arc<WorkflowRepository>,
    pub(crate) config: Arc<RwLock<Config>>,
    pub(crate) event_bus: Arc<WorkflowEventBus>,
    pub(crate) suppress_cancelled_as_error: Arc<AtomicBool>,
    pub(crate) actions: Arc<dyn ExternalActions>,
    /// Resolver for pin + bundle build on snapshot-restore.
    pub(crate) git_auth_resolver: Option<Arc<crate::github::auth_resolver::GitAuthResolver>>,
    /// GhClient for at-restore PAT revalidation. Defaults to
    /// `None`; set by `WorkflowEngine::with_gh_client` (so tests can inject
    /// a mock without going through the real `gh` binary).
    pub(crate) gh_client: Option<Arc<dyn crate::auth::GhClient>>,
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
        db: Option<Database>,
    ) -> Result<usize> {
        // Snapshots live under `{data_dir}/workspaces/<name>/` and resolving
        // the data dir no longer needs a repo path. The legacy
        // `cfg.git.repo_path` fallback survives in `resolve_snapshot_dir` for
        // the back-compat single-workspace read path below.
        let legacy_repo_path = {
            let c = self.config.read().await;
            PathBuf::from(&c.git.repo_path)
        };
        let data_dir =
            resolve_data_dir().unwrap_or_else(|| snapshot::resolve_snapshot_dir(&legacy_repo_path));

        info!(
            data_dir = %data_dir.display(),
            "Restoring workflows: DB-first, snapshot fallback"
        );

        // Phase A — DB-first restore (cutover invariant I3). Rebuild the
        // in-memory cache from the authoritative `work_items` table; runtime
        // handles (cancel_token, worktree_lock) are created fresh by
        // `from_work_item_row`. `from_db` records the ticket_keys the DB owns
        // so the snapshot pass below can apply DB-wins precedence.
        let mut from_db: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        if let Some(database) = db.as_ref() {
            match crate::db::work_items::list_all_for_restore(database.adapter()).await {
                Ok(rows) => {
                    for row in rows {
                        let ticket_key = row.ticket_key.clone();
                        let row_id = row.id.clone();
                        let def_runs = crate::db::work_items::list_definition_runs(
                            database.adapter(),
                            &row.id,
                        )
                        .await
                        .unwrap_or_default();
                        let wf = Workflow::from_work_item_row(row, def_runs);
                        // Rows arrive started_at-ASC, so a later row for the same
                        // ticket_key (cross-workspace) is the more recent one and
                        // wins; name the dropped row so the clobber is observable.
                        if let Some(prev) = from_db.insert(ticket_key.clone(), row_id.clone()) {
                            warn!(
                                ticket = %ticket_key,
                                dropped = %prev,
                                kept = %row_id,
                                "Multiple live work_items rows share this ticket_key (cross-workspace); most-recently-started wins in the cache"
                            );
                        }
                        self.repository
                            .inner_arc()
                            .write()
                            .await
                            .insert(ticket_key, wf);
                    }
                }
                Err(e) => warn!(
                    error = %e,
                    "DB-first restore query failed; falling back to snapshot-only restore"
                ),
            }
        }

        // Try multi-workspace read first; fall back to single-workspace read for first-time migration.
        let mut records = read_all_workspace_snapshots(&data_dir)?;
        if records.is_empty()
            && let Some(file) = read_workflow_snapshot(&legacy_repo_path)?
            && file.version == snapshot::SNAPSHOT_VERSION
        {
            records = file.workflows;
        }

        // Back-fill `workspace_name` from the registered `repositories` row
        // when the snapshot lacks it. Records without a `repository_id` and
        // without a `workspace_name` fall back to the (deprecated) global
        // `cfg.git.repo_path` derivation purely so they retain some identity
        // for the dashboard filter to skip over.
        let legacy_default_ws_name = workspace_name_from_repo_path(&legacy_repo_path);
        for rec in &mut records {
            if rec.workspace_name.is_empty() {
                if let (Some(repo_id), Some(database)) = (rec.repository_id.as_deref(), db.as_ref())
                    && let Ok(Some(row)) =
                        crate::db::repositories::get(database.adapter(), repo_id).await
                {
                    rec.workspace_name = row.name;
                    continue;
                }
                rec.workspace_name = legacy_default_ws_name.clone();
            }
        }
        // Clean up legacy global snapshot after successful migration.
        cleanup_legacy_global_snapshot(&data_dir);

        records.sort_by_key(|r| r.started_at);

        // Phase B — merge the snapshot with DB-wins precedence. For a
        // ticket_key the DB already loaded (Phase A), the snapshot only
        // SUPPLEMENTS the fields the DB cannot store, and only while still
        // unset; durable fields are never overwritten and the DB is not
        // backfilled. For a ticket_key the DB lacks, the snapshot is the
        // source and is backfilled into the DB.
        for rec in records {
            let ticket_key = rec.ticket_key.clone();
            if from_db.contains_key(&ticket_key) {
                let wf_arc = self.repository.inner_arc();
                let mut map = wf_arc.write().await;
                if let Some(w) = map.get_mut(&ticket_key) {
                    if w.terminal_lines.is_empty() && !rec.terminal_lines.is_empty() {
                        w.terminal_lines = rec
                            .terminal_lines
                            .iter()
                            .map(|l| TerminalLine {
                                text: l.text.clone(),
                                stream: l.stream.clone(),
                            })
                            .collect();
                    }
                    if w.auth_pin.is_none() {
                        w.auth_pin = rec.auth_pin.clone();
                    }
                    if w.description_session_id.is_none() {
                        w.description_session_id = rec.description_session_id.clone();
                    }
                    if !w.worktree_bootstrapped {
                        w.worktree_bootstrapped = rec.worktree_bootstrapped;
                    }
                    // The DB only approximates ticketing_system via
                    // jira_available; a github-mode snapshot carries the truth.
                    if w.ticketing_system == crate::config::TicketingSystem::None
                        && rec.ticketing_system != crate::config::TicketingSystem::None
                    {
                        w.ticketing_system = rec.ticketing_system;
                        w.ticketing_available = true;
                    }
                }
                continue;
            }

            let wf = Workflow::from_persisted_record(rec);
            self.repository
                .inner_arc()
                .write()
                .await
                .insert(ticket_key.clone(), wf);

            // Backfill the DB from the snapshot only for rows the DB lacked
            // (best-effort, idempotent). Never overwrites a DB-won row.
            {
                let wf_arc = self.repository.inner_arc();
                let wf_map = wf_arc.read().await;
                if let Some(w) = wf_map.get(&ticket_key) {
                    super::lifecycle::shadow_persist_work_item(db.as_ref(), w).await;
                }
            }
        }

        // Phase C — respawn drivers for every in-progress entry in the merged
        // map (DB- or snapshot-sourced), so DB-only rows are respawned too.
        let n = self.repository.inner_arc().read().await.len();
        let restore_keys: Vec<String> = self
            .repository
            .inner_arc()
            .read()
            .await
            .keys()
            .cloned()
            .collect();
        for ticket_key in restore_keys {
            let (cancel_token, state_for_checks, driver_started) = {
                let wf_arc = self.repository.inner_arc();
                let map = wf_arc.read().await;
                match map.get(&ticket_key) {
                    Some(w) => (w.cancel_token.clone(), w.state.clone(), w.driver_started),
                    None => continue,
                }
            };
            let is_terminal = state_for_checks.is_terminal();
            let state_display = state_for_checks.to_string();
            let is_unstarted_pending =
                matches!(state_for_checks, WorkflowState::Pending) && !driver_started;

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

            // Paused workflows wait for an explicit Resume click — re-spawning a
            // driver here would race with the one `resume_workflow` spawns when
            // the user clicks Resume, since both share the workflow's
            // cancel_token and both would compete for the worktree.
            let is_paused = {
                let wf_arc = self.repository.inner_arc();
                let wf_map = wf_arc.read().await;
                wf_map
                    .get(&ticket_key)
                    .is_some_and(|w| matches!(w.state, WorkflowState::Paused { .. }))
            };
            if is_paused {
                info!(ticket = %ticket_key, "Restored Paused workflow (no driver — awaiting Resume)");
                continue;
            }

            let engine_config = self.config.clone();
            let engine_workflows = self.repository.inner_arc();
            let engine_actions = self.actions.clone();
            let engine_event_tx = self.event_bus.sender().clone();
            let agent_sem = agent_run_semaphore.clone();
            let suppress = suppress_cancelled_as_error.clone();

            {
                use crate::workflow::definitions::WorkflowDefRunState;

                // Find defs that were running when the server stopped, and re-spawn their drivers.
                let (running_def_names, wt, ts, td, tt, wf_user_id, wf_workspace) = {
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
                    let user_id = w.and_then(|w| w.user_id.clone());
                    let workspace = w.map(|w| w.workspace_name.clone()).unwrap_or_default();
                    (running, worktree, ts, td, tt, user_id, workspace)
                };

                if running_def_names.is_empty() {
                    info!(ticket = %ticket_key, "Restored workflow with no running defs (no driver spawned)");
                    continue;
                }

                // Re-resolve the owner's flows for this workspace. When a DB +
                // user identity is available the running def slugs are matched
                // against the user's current flows, so a since-renamed/deleted
                // flow surfaces a typed `flow_no_longer_exists` error below.
                // Without a DB/user we fall back to TOML discovery.
                let resolved_from_user_flows =
                    db.is_some() && wf_user_id.is_some() && !wf_workspace.is_empty();
                let defs: Vec<crate::workflow::definitions::DiscoveredWorkflow> =
                    match (db.as_ref(), wf_user_id.as_deref()) {
                        (Some(database), Some(uid)) if !wf_workspace.is_empty() => {
                            let defaults =
                                crate::workflow::definitions::default_flows_from_dir(workflows_dir);
                            crate::workflow::definitions::resolve_user_flows(
                                database,
                                uid,
                                &wf_workspace,
                                &defaults,
                            )
                            .await
                        }
                        _ => {
                            crate::workflow::definitions::discover_workflows(workflows_dir)
                                .workflows
                        }
                    };

                for def_name in running_def_names {
                    if let Some(def) = defs.iter().find(|d| d.filename == def_name) {
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
                        let db_clone = db.clone();
                        // Revalidate the user's PAT at restore. Best-effort:
                        // failure broadcasts an AuthWarning event but does
                        // NOT block re-spawn (the workflow's next git action
                        // will still fail loudly). Done BEFORE binding the
                        // owned `resolver` for the driver spawn — otherwise
                        // we'd borrow-then-move.
                        if let (Some(r), Some(gh)) =
                            (self.git_auth_resolver.as_ref(), self.gh_client.as_ref())
                        {
                            let wf_arc = self.repository.inner_arc();
                            let pin_info = {
                                let wf = wf_arc.read().await;
                                wf.get(&ticket_key).and_then(|w| {
                                    w.auth_pin.as_ref().and_then(|p| {
                                        let uid = w.user_id.clone()?;
                                        if p.github_credential_row_id.is_some() {
                                            Some(uid)
                                        } else {
                                            None
                                        }
                                    })
                                })
                            };
                            if let Some(uid) = pin_info {
                                let r_clone: Arc<crate::github::auth_resolver::GitAuthResolver> =
                                    r.clone();
                                let gh_clone: Arc<dyn crate::auth::GhClient> = gh.clone();
                                let event_tx = engine_event_tx.clone();
                                let ticket_for_event = ticket_key.clone();
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
                                            "PAT revalidation failed at restore — emitting AuthWarning"
                                        );
                                        let _ =
                                            event_tx.send(crate::workflow::engine::WorkflowEvent {
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

                        // Thread the resolver to the spawned driver.
                        let resolver = self.git_auth_resolver.clone();
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
                                db_clone,
                                resolver,
                                None, // restart restore — fresh start, not a resume
                            )
                            .await;
                        });
                    } else {
                        // The running def's slug no longer resolves to a flow.
                        // When resolved from the user's flow list this means the
                        // flow was renamed or deleted — surface the typed
                        // `flow_no_longer_exists` reason so the card shows a
                        // clear error and the user can trigger a fresh run.
                        let message = if resolved_from_user_flows {
                            "flow_no_longer_exists: this flow was renamed or deleted; \
                             trigger it again from the work-item card to start a fresh run"
                        } else {
                            "Def file not found after restart"
                        };
                        warn!(
                            ticket = %ticket_key,
                            def = %def_name,
                            resolved_from_user_flows,
                            "Running def no longer resolves after restart"
                        );
                        let wf_arc = self.repository.inner_arc();
                        let mut wf_map = wf_arc.write().await;
                        if let Some(w) = wf_map.get_mut(&ticket_key) {
                            w.workflow_def_runs.insert(
                                def_name.clone(),
                                WorkflowDefRunState::Error {
                                    message: message.to_string(),
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

        let data_dir = match resolve_data_dir() {
            Some(d) => d,
            None => {
                let c = self.config.read().await;
                snapshot::resolve_snapshot_dir(std::path::Path::new(&c.git.repo_path))
            }
        };

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
        let data_dir = match resolve_data_dir() {
            Some(d) => d,
            None => {
                let c = self.config.read().await;
                snapshot::resolve_snapshot_dir(std::path::Path::new(&c.git.repo_path))
            }
        };

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
        // In the per-repo model each workflow owns its repository path, and
        // `mark_work_done`/`delete_workflow` prune via the per-workflow repo
        // path. This blanket prune (called after the per-action remove) is
        // informational and uses the deprecated global config path so it
        // keeps working on legacy single-repo deployments. Best-effort.
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

#[cfg(test)]
#[allow(unsafe_code)]
mod tests {
    use std::collections::HashMap;

    use crate::actions::dry_run::DryRunActions;
    use crate::actions::traits::ExternalActions;
    use crate::config::{Config, TicketingSystem};
    use crate::db::Database;
    use crate::db::user_work_item_flows::{self, UserFlow, UserFlowStep};
    use crate::workflow::definitions::WorkflowDefRunState;
    use crate::workflow::engine::WorkflowEngine;
    use crate::workflow::snapshot::{PersistedWorkflowRecord, write_all_workspace_snapshots};
    use crate::workflow::state::WorkflowState;

    /// Serializes the few tests that mutate `MAESTRO_DATA_DIR`, since the env
    /// is process-global and `resolve_data_dir()` reads it.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    fn one_flow(name: &str) -> UserFlow {
        UserFlow {
            name: name.to_string(),
            depends_on: Vec::new(),
            steps: vec![UserFlowStep {
                name: "step".to_string(),
                prompt: "do it".to_string(),
                skills: Vec::new(),
            }],
        }
    }

    /// A workflow whose snapshot recorded a `Running` def under the slug
    /// `implement-ticket`, restored after the user renamed that flow away,
    /// surfaces `flow_no_longer_exists` on the card instead of resuming.
    #[tokio::test]
    async fn restore_marks_renamed_flow_as_no_longer_existing() {
        let _guard = ENV_LOCK.lock().await;

        let data_dir = tempfile::tempdir().expect("data dir");
        let workflows_dir = tempfile::tempdir().expect("workflows dir");
        // SAFETY: env mutation is serialized by ENV_LOCK; this is the only
        // test in the crate that sets MAESTRO_DATA_DIR.
        unsafe {
            std::env::set_var("MAESTRO_DATA_DIR", data_dir.path());
        }

        let db = Database::open_in_memory().expect("in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u1', 'alice', 'user')",
                vec![],
            )
            .await
            .expect("seed user");
        // The user's current flow list no longer contains "Implement Ticket"
        // (it was renamed to "Implement Feature").
        user_work_item_flows::set(db.adapter(), "u1", "ws", &[one_flow("Implement Feature")])
            .await
            .expect("set flows");

        // Snapshot a workflow that had `implement-ticket` running at shutdown.
        let now = chrono::Utc::now();
        let record = PersistedWorkflowRecord {
            id: "wf-1".to_string(),
            ticket_key: "TICK-1".to_string(),
            ticket_summary: "summary".to_string(),
            ticket_description: "desc".to_string(),
            ticket_type: "Task".to_string(),
            state: WorkflowState::Reviewing,
            started_at: now,
            updated_at: now,
            steps_log: Vec::new(),
            branch_name: "feature/tick-1".to_string(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually: false,
            jira_available: false,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: TicketingSystem::None,
            ticket_url: None,
            driver_started: true,
            workflow_def_runs: HashMap::from([(
                "implement-ticket".to_string(),
                WorkflowDefRunState::Running,
            )]),
            worktree_bootstrapped: true,
            workspace_name: "ws".to_string(),
            repository_id: None,
            user_id: Some("u1".to_string()),
            auth_pin: None,
        };
        write_all_workspace_snapshots(data_dir.path(), &[record]).expect("write snapshot");

        let config = std::sync::Arc::new(tokio::sync::RwLock::new(Config::default()));
        let actions: std::sync::Arc<dyn ExternalActions> =
            std::sync::Arc::new(DryRunActions::new("origin".to_string(), None));
        let engine = WorkflowEngine::new_with_db(
            config,
            actions,
            1,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            TicketingSystem::None,
            workflows_dir.path().to_path_buf(),
            Some(db.clone()),
        );

        engine.restore_persisted_workflows().await.expect("restore");

        let state = {
            let wf_arc = engine.workflows_arc();
            let map = wf_arc.read().await;
            map.get("TICK-1")
                .and_then(|w| w.workflow_def_runs.get("implement-ticket").cloned())
                .expect("def-run state present")
        };

        match state {
            WorkflowDefRunState::Error { message } => {
                assert!(
                    message.contains("flow_no_longer_exists"),
                    "expected flow_no_longer_exists, got: {message}"
                );
            }
            other => panic!("expected Error state, got {other:?}"),
        }

        // SAFETY: serialized by ENV_LOCK (still held).
        unsafe {
            std::env::remove_var("MAESTRO_DATA_DIR");
        }
    }

    // ── DB-first restore (cutover invariants I3 / AC-2 / AC-3) ──────────

    /// Build a DryRun engine backed by an in-memory DB with a seeded user.
    async fn engine_with_db(workflows_dir: &std::path::Path) -> (WorkflowEngine, Database) {
        let db = Database::open_in_memory().expect("in-memory db");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u1', 'alice', 'user')",
                vec![],
            )
            .await
            .expect("seed user");
        let config = std::sync::Arc::new(tokio::sync::RwLock::new(Config::default()));
        let actions: std::sync::Arc<dyn ExternalActions> =
            std::sync::Arc::new(DryRunActions::new("origin".to_string(), None));
        let engine = WorkflowEngine::new_with_db(
            config,
            actions,
            1,
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            TicketingSystem::None,
            workflows_dir.to_path_buf(),
            Some(db.clone()),
        );
        (engine, db)
    }

    /// Insert a work_items row for `ticket_key` in `state` (owner `u1`).
    async fn seed_db_row(db: &Database, id: &str, ticket_key: &str, state: WorkflowState) {
        let mut wf = crate::workflow::engine::types::Workflow::new(
            ticket_key.to_string(),
            "summary".to_string(),
            false,
            false,
            TicketingSystem::None,
            None,
            "ws".to_string(),
        );
        wf.id = id.to_string();
        wf.user_id = Some("u1".to_string());
        wf.state = state;
        crate::db::work_items::insert_work_item(db.adapter(), &wf.to_work_item_row())
            .await
            .expect("insert work_items row");
    }

    /// A minimal snapshot record for `ticket_key` in `state`.
    fn sample_record(id: &str, ticket_key: &str, state: WorkflowState) -> PersistedWorkflowRecord {
        let now = chrono::Utc::now();
        PersistedWorkflowRecord {
            id: id.to_string(),
            ticket_key: ticket_key.to_string(),
            ticket_summary: "summary".to_string(),
            ticket_description: "desc".to_string(),
            ticket_type: "Task".to_string(),
            state,
            started_at: now,
            updated_at: now,
            steps_log: Vec::new(),
            branch_name: "feature/x".to_string(),
            worktree_path: None,
            pr_url: None,
            pr_merged: false,
            terminal_lines: Vec::new(),
            current_step_label: None,
            started_manually: false,
            jira_available: false,
            last_session_id: None,
            description_session_id: None,
            ticketing_system: TicketingSystem::None,
            ticket_url: None,
            driver_started: true,
            workflow_def_runs: HashMap::new(),
            worktree_bootstrapped: true,
            workspace_name: "ws".to_string(),
            repository_id: None,
            user_id: Some("u1".to_string()),
            auth_pin: None,
        }
    }

    /// T1 — on a `(ticket_key)` present in both the DB and the snapshot, the
    /// DB row wins and the snapshot's (conflicting) state is discarded.
    #[tokio::test]
    async fn restore_db_row_wins_over_conflicting_snapshot() {
        let _guard = ENV_LOCK.lock().await;
        let data_dir = tempfile::tempdir().expect("data dir");
        let workflows_dir = tempfile::tempdir().expect("workflows dir");
        // SAFETY: env mutation serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("MAESTRO_DATA_DIR", data_dir.path());
        }

        let (engine, db) = engine_with_db(workflows_dir.path()).await;
        // Authoritative DB row says Done; stale snapshot says AddressingTicket.
        seed_db_row(&db, "wf-db", "TICK-1", WorkflowState::Done).await;
        let rec = sample_record("wf-snap", "TICK-1", WorkflowState::AddressingTicket { pass: 1 });
        write_all_workspace_snapshots(data_dir.path(), &[rec]).expect("write snapshot");

        engine.restore_persisted_workflows().await.expect("restore");

        let state = {
            let m = engine.workflows_arc();
            let g = m.read().await;
            g.get("TICK-1").map(|w| w.state.clone()).expect("present")
        };
        assert!(
            matches!(state, WorkflowState::Done),
            "DB row must win on conflict, got {state:?}"
        );

        // SAFETY: serialized by ENV_LOCK (still held).
        unsafe {
            std::env::remove_var("MAESTRO_DATA_DIR");
        }
    }

    /// T2 — a snapshot record with NO matching DB row is restored AND
    /// backfilled into the DB (so the DB-first read path can serve it).
    #[tokio::test]
    async fn restore_snapshot_only_row_backfills_db() {
        let _guard = ENV_LOCK.lock().await;
        let data_dir = tempfile::tempdir().expect("data dir");
        let workflows_dir = tempfile::tempdir().expect("workflows dir");
        // SAFETY: env mutation serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("MAESTRO_DATA_DIR", data_dir.path());
        }

        let (engine, db) = engine_with_db(workflows_dir.path()).await;
        // Terminal so no driver is spawned; only the merge+backfill matters.
        let rec = sample_record("wf-snap2", "TICK-2", WorkflowState::Done);
        write_all_workspace_snapshots(data_dir.path(), &[rec]).expect("write snapshot");

        engine.restore_persisted_workflows().await.expect("restore");

        let in_map = {
            let m = engine.workflows_arc();
            let g = m.read().await;
            g.contains_key("TICK-2")
        };
        assert!(in_map, "snapshot-only row must land in the map");

        let row = crate::db::work_items::get_work_item_by_ticket_key(db.adapter(), "TICK-2")
            .await
            .expect("query work_items");
        assert!(
            row.is_some(),
            "snapshot-only row must be backfilled into the DB"
        );
    }

    /// T6 — with the DB populated and NO snapshot file present, restore
    /// rebuilds the dashboard from the DB alone (restore is DB-independent of
    /// the snapshot).
    #[tokio::test]
    async fn restore_rebuilds_from_db_with_no_snapshot_file() {
        let _guard = ENV_LOCK.lock().await;
        let data_dir = tempfile::tempdir().expect("data dir");
        let workflows_dir = tempfile::tempdir().expect("workflows dir");
        // SAFETY: env mutation serialized by ENV_LOCK.
        unsafe {
            std::env::set_var("MAESTRO_DATA_DIR", data_dir.path());
        }

        let (engine, db) = engine_with_db(workflows_dir.path()).await;
        seed_db_row(&db, "wf-db3", "TICK-3", WorkflowState::Done).await;
        // No snapshot file is written.

        engine.restore_persisted_workflows().await.expect("restore");

        let in_map = {
            let m = engine.workflows_arc();
            let g = m.read().await;
            g.contains_key("TICK-3")
        };
        assert!(in_map, "must rebuild TICK-3 from the DB with no snapshot file");

        // SAFETY: serialized by ENV_LOCK (still held).
        unsafe {
            std::env::remove_var("MAESTRO_DATA_DIR");
        }
    }
}
