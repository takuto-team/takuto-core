// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Workflow-definition driver entry point + transition glue. The heavier
//! sub-flows (resolve, bootstrap, auth pinning, step execution) live in
//! sibling modules under [`super`]; this file re-exports the items they
//! own so existing `super::driver::*` paths inside `engine/` keep
//! compiling unchanged.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tokio::sync::{RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{error, info};

use crate::actions::traits::ExternalActions;
use crate::config::Config;
use crate::container::ContainerRunner;
use crate::db::Database;
use crate::error::{MaestroError, Result};
use crate::workflow::log_writer::WorkflowLogWriter;
use crate::workflow::state::WorkflowState;
use crate::workflow::step::StepLog;

use super::bootstrap::bootstrap_new_workflow;
use super::step_runner::run_workflow_def_steps;
use super::types::{Workflow, WorkflowEvent};

// ---------------------------------------------------------------------------
// Re-exports — keep `super::driver::xxx` paths used by sibling engine modules
// compiling unchanged after the split.
// ---------------------------------------------------------------------------

pub(super) use super::bootstrap::prepare_worktree_for_ticket;
pub use super::resolve::resolve_worktree_init_commands;
pub(super) use super::resolve::scan_definitions_dir;
pub(crate) use super::resolve::{resolve_repo_for_ticket, resolve_workspace_name};
pub(super) use super::step_runner::close_github_issue;

#[allow(clippy::too_many_arguments)]
pub(super) async fn drive_workflow_def(
    ticket_key: String,
    def_name: String,
    steps: Vec<crate::config::AgentStepConfig>,
    worktree_path: Option<PathBuf>,
    ticket_summary: String,
    ticket_description: String,
    ticket_type: String,
    config: Arc<RwLock<Config>>,
    workflows: Arc<RwLock<HashMap<String, Workflow>>>,
    actions: Arc<dyn ExternalActions>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel_token: CancellationToken,
    agent_run_semaphore: Arc<Semaphore>,
    suppress_cancelled_as_error: Arc<AtomicBool>,
    db: Option<Database>,
    // Resolver passed through from the engine so the bootstrap step can pin
    // credentials + build a `WorkerSecretsBundle`. `None` preserves the
    // legacy `PASSTHROUGH_ENV` behaviour.
    git_auth_resolver: Option<Arc<crate::github::auth_resolver::GitAuthResolver>>,
    // When `Some`, the run is a resume: the first agent step gets a
    // built-in resume prompt and (if known) the recorded session id.
    resume: Option<super::step_runner::ResumeContext>,
) {
    use crate::workflow::definitions::WorkflowDefRunState;

    info!(ticket = %ticket_key, def = %def_name, "Workflow definition driver started");

    // Lock the dashboard progress denominator for this run. Without this,
    // `dashboard_progress::estimated_step_total` infers the total from
    // `steps_log.len() + 2`, so the displayed "k/N" denominator grows as
    // the run progresses. Cache the def's actual step count plus a
    // bootstrap estimate (which we cannot resolve exactly here — init
    // commands etc. live behind a DB lookup — but a sensible upper bound
    // keeps the bar stable for the whole run).
    {
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(&ticket_key) {
            let bootstrap_est: u32 = if workflow.jira_available { 3 } else { 1 };
            let mise_est: u32 = 1;
            let agent_steps = u32::try_from(steps.len()).unwrap_or(u32::MAX);
            workflow.current_def_total_steps = Some(bootstrap_est + mise_est + agent_steps + 1);
        }
    }

    // Workflow logs live under `{data_dir}/logs/<TICKET>.log` so they are
    // independent of the repository (there is no global active repo any
    // more). Fall back to a writable temp dir if the data dir cannot be
    // resolved, mirroring the existing best-effort log writer behaviour.
    let log_dir = crate::workflow::snapshot::resolve_data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("logs");

    // Spawn a log batcher scoped to this drive when DB is attached. The
    // sink is held in a local variable so when drive_workflow_def returns,
    // the sink + writer drop, the batcher flushes its remaining buffer,
    // and the task exits. Looking up work_item_id by ticket_key works
    // because the engine shadow-writes the row at start_workflow BEFORE
    // spawning the driver.
    let (log_sink, work_item_id_for_log) = match db.as_ref() {
        Some(database) => {
            let resolved_id = match crate::db::work_items::get_work_item_by_ticket_key(
                database.adapter(),
                &ticket_key,
            )
            .await
            {
                Ok(Some(row)) => Some(row.id),
                _ => None,
            };
            (
                resolved_id
                    .as_ref()
                    .map(|_| crate::workflow::log_sink::spawn_batcher(database.clone())),
                resolved_id,
            )
        }
        None => (None, None),
    };
    let log_writer = Arc::new(
        WorkflowLogWriter::with_sink(
            &log_dir,
            &ticket_key,
            log_sink.clone(),
            work_item_id_for_log,
        )
        .await,
    );

    let result = async {
        // Bootstrap if no worktree exists yet (Pending workflow, first run).
        let (resolved_wt, ts, td, tt) = match worktree_path {
            Some(p) => (p, ticket_summary, ticket_description, ticket_type),
            None => {
                let (wt, ticket_detail) = bootstrap_new_workflow(
                    &ticket_key,
                    &config,
                    &workflows,
                    &actions,
                    &event_tx,
                    &cancel_token,
                    &log_writer,
                    &agent_run_semaphore,
                    db.as_ref(),
                    git_auth_resolver.as_ref(),
                )
                .await?;
                (
                    wt,
                    ticket_detail.summary,
                    ticket_detail.description,
                    ticket_detail.item_type,
                )
            }
        };
        run_workflow_def_steps(
            &ticket_key,
            &def_name,
            &steps,
            &resolved_wt,
            &ts,
            &td,
            &tt,
            &config,
            &workflows,
            &event_tx,
            &cancel_token,
            &log_writer,
            &agent_run_semaphore,
            db.as_ref(),
            git_auth_resolver.as_ref(),
            resume.clone(),
        )
        .await
    }
    .await;

    // Always clean up worker containers regardless of success/failure
    ContainerRunner::cleanup_for_ticket(&ticket_key).await;

    let (workflow_id, workflow_user_id) = {
        let wf = workflows.read().await;
        wf.get(&ticket_key)
            .map(|w| (w.id.clone(), w.user_id.clone()))
            .unwrap_or_default()
    };

    match result {
        Ok(()) => {
            // Set state to Completed
            {
                let mut wf_map = workflows.write().await;
                if let Some(w) = wf_map.get_mut(&ticket_key) {
                    w.workflow_def_runs
                        .insert(def_name.clone(), WorkflowDefRunState::Completed);
                    w.updated_at = Utc::now();
                }
            }
            // Shadow-write Completed. UPDATE-only so we preserve the
            // started_at written by start_workflow_def.
            shadow_finish_def_run(
                db.as_ref(),
                &workflow_id,
                &def_name,
                crate::db::work_items::DefRunState::Completed,
                None,
                Utc::now().timestamp(),
            )
            .await;

            info!(ticket = %ticket_key, def = %def_name, "Workflow definition completed");

            let _ = event_tx.send(WorkflowEvent {
                event_type: "work_item_updated".to_string(),
                workflow_id,
                ticket_key: ticket_key.clone(),
                state: {
                    let wf = workflows.read().await;
                    wf.get(&ticket_key)
                        .map(|w| w.status_display())
                        .unwrap_or_default()
                },
                timestamp: Utc::now(),
                error: None,
                step_name: None,
                output_line: None,
                stream: None,
                progress_percent: None,
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
                user_id: workflow_user_id.clone(),
                ..Default::default()
            });
        }
        Err(e) => {
            if matches!(e, MaestroError::Cancelled)
                && suppress_cancelled_as_error.load(Ordering::SeqCst)
            {
                info!(
                    ticket = %ticket_key,
                    def = %def_name,
                    "Workflow def driver cancelled during shutdown; state preserved for resume"
                );
                return;
            }

            // When the user explicitly stops a workflow, the cancel token fires and
            // the parent workflow state transitions to Stopped before the driver
            // processes the cancellation. Do not overwrite the def run state with
            // Error when the workflow was intentionally stopped or removed.
            if matches!(e, MaestroError::Cancelled) {
                let snapshot = {
                    let wf = workflows.read().await;
                    wf.get(&ticket_key).map(|w| w.state.clone())
                };
                match snapshot {
                    None => {
                        info!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Workflow def driver cancelled; row no longer in map"
                        );
                        return;
                    }
                    Some(WorkflowState::Stopped) => {
                        info!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Workflow def driver cancelled; left in Stopped (operator stop)"
                        );
                        return;
                    }
                    Some(WorkflowState::Paused { .. }) => {
                        info!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Workflow def driver cancelled; left in Paused (resume will spawn a new driver)"
                        );
                        return;
                    }
                    _ => {}
                }
            }

            error!(ticket = %ticket_key, def = %def_name, error = %e, "Workflow definition failed");
            log_writer
                .write(&format!("WORKFLOW DEF '{}' FAILED: {e}", def_name))
                .await;

            {
                let mut wf_map = workflows.write().await;
                if let Some(w) = wf_map.get_mut(&ticket_key) {
                    w.workflow_def_runs.insert(
                        def_name.clone(),
                        WorkflowDefRunState::Error {
                            message: e.to_string(),
                        },
                    );
                    // Also move the workflow's own state to Error so the
                    // dashboard card badge reflects the failure. Without
                    // this the card stays on its last in-progress label
                    // (e.g. "Running agent steps") even though the
                    // definition run has failed — only the per-definition
                    // button goes red. Don't clobber a state that is
                    // already terminal (a concurrent stop/done wins).
                    if !w.state.is_terminal() {
                        let source = w.state.clone();
                        w.state = WorkflowState::Error {
                            source_state: Box::new(source),
                            message: e.to_string(),
                        };
                    }
                    w.updated_at = Utc::now();
                }
            }
            // Shadow-write Error with the message.
            shadow_finish_def_run(
                db.as_ref(),
                &workflow_id,
                &def_name,
                crate::db::work_items::DefRunState::Error,
                Some(&e.to_string()),
                Utc::now().timestamp(),
            )
            .await;

            let _ = event_tx.send(WorkflowEvent {
                event_type: "work_item_updated".to_string(),
                workflow_id,
                ticket_key: ticket_key.clone(),
                state: {
                    let wf = workflows.read().await;
                    wf.get(&ticket_key)
                        .map(|w| w.status_display())
                        .unwrap_or_default()
                },
                timestamp: Utc::now(),
                error: Some(e.to_string()),
                step_name: None,
                output_line: None,
                stream: None,
                progress_percent: None,
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
                user_id: workflow_user_id.clone(),
                ..Default::default()
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Transition glue — small helpers shared by every driver sub-module
// ---------------------------------------------------------------------------

/// Read the owning `user_id` from the workflow map for the given ticket key.
///
/// Returns `None` if the workflow has been removed or has no associated user
/// (legacy snapshots / poller workflows pre-migration). The WS filter delivers
/// `None` events to every authenticated subscriber, matching broadcast semantics.
pub(super) async fn user_id_for_ticket(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
) -> Option<String> {
    let wf = workflows.read().await;
    wf.get(ticket_key).and_then(|w| w.user_id.clone())
}

pub(super) async fn progress_dashboard_fields_for_ticket(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    config: &Arc<RwLock<Config>>,
    ticket_key: &str,
) -> Option<(u8, u32)> {
    let cfg = config.read().await;
    let wf = workflows.read().await;
    wf.get(ticket_key).map(|w| {
        (
            crate::workflow::dashboard_progress::workflow_progress_percent(w, &cfg),
            crate::workflow::dashboard_progress::estimated_step_total(w, &cfg),
        )
    })
}

pub(super) async fn transition_to_agent_step(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    pass: u8,
    step_label: &str,
    config: &Arc<RwLock<Config>>,
    db: Option<&Database>,
) {
    info!(
        ticket = %ticket_key,
        pass,
        step = %step_label,
        "Agent step (state + dashboard label)"
    );

    // Snapshot the post-mutation Workflow shape for the shadow-persist
    // pass — done inside the same lock-guard so we don't observe a
    // mid-flight torn state between the in-memory write and the DB
    // write.
    let updated = {
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(ticket_key) {
            workflow.state = WorkflowState::AddressingTicket { pass };
            workflow.current_step_label = Some(step_label.to_string());
            workflow.updated_at = Utc::now();
            Some((
                workflow.id.clone(),
                workflow.status_display(),
                workflow.user_id.clone(),
                // Shadow-write inputs.
                workflow.state.clone(),
                workflow.current_step_label.clone(),
                workflow.updated_at.timestamp(),
            ))
        } else {
            None
        }
    };
    if let Some((id, display, owner_user_id, state, label, updated_at)) = updated {
        shadow_persist_state_change(db, &id, &state, label.as_deref(), updated_at).await;
        // `display` and `owner_user_id` flow into the WS event below.
        let dash = progress_dashboard_fields_for_ticket(workflows, config, ticket_key).await;
        let _ = event_tx.send(WorkflowEvent {
            event_type: "work_item_updated".to_string(),
            workflow_id: id,
            ticket_key: ticket_key.to_string(),
            state: display,
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: dash.map(|(p, _)| p),
            progress_steps_total: dash.map(|(_, t)| t),
            forwarded_port: None,
            pr_merged: None,
            user_id: owner_user_id,
            ..Default::default()
        });
    }
}

pub(super) async fn transition(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    new_state: WorkflowState,
    config: &Arc<RwLock<Config>>,
    db: Option<&Database>,
) {
    let state_name = new_state.display_name();
    info!(ticket = ticket_key, state = %state_name, "Transitioning workflow");

    let updated = {
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(ticket_key) {
            workflow.current_step_label = None;
            workflow.state = new_state;
            workflow.updated_at = Utc::now();
            Some((
                workflow.id.clone(),
                workflow.status_display(),
                workflow.user_id.clone(),
                // Shadow-write inputs.
                workflow.state.clone(),
                workflow.updated_at.timestamp(),
            ))
        } else {
            None
        }
    };
    if let Some((id, display, owner_user_id, state, updated_at)) = updated {
        // `transition()` clears the step label, so pass None.
        shadow_persist_state_change(db, &id, &state, None, updated_at).await;
        let dash = progress_dashboard_fields_for_ticket(workflows, config, ticket_key).await;
        let _ = event_tx.send(WorkflowEvent {
            event_type: "work_item_updated".to_string(),
            workflow_id: id,
            ticket_key: ticket_key.to_string(),
            state: display,
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: dash.map(|(p, _)| p),
            progress_steps_total: dash.map(|(_, t)| t),
            forwarded_port: None,
            pr_merged: None,
            user_id: owner_user_id,
            ..Default::default()
        });
    }
}

/// Best-effort shadow-write of a state transition into the `work_items`
/// row.
///
/// Truth-of-record is the in-memory `HashMap<String, Workflow>` (the
/// caller has already mutated it). This call exists so the DB row's
/// `state_kind`, `state_payload`, `current_step_label`, and
/// `updated_at` track the in-memory state.
///
/// Failures log at WARN and are swallowed — a flaky DB must not stall
/// engine progress. The map mutation already happened; the row will
/// catch up on the next transition.
pub(crate) async fn shadow_persist_state_change(
    db: Option<&Database>,
    work_item_id: &str,
    new_state: &WorkflowState,
    current_step_label: Option<&str>,
    updated_at_unix: i64,
) {
    let Some(db) = db else { return };
    let (state_kind, state_payload) =
        crate::workflow::engine::types::state_to_kind_and_payload(new_state);
    if let Err(e) = crate::db::work_items::update_work_item_state(
        db.adapter(),
        work_item_id,
        state_kind,
        state_payload.as_deref(),
        current_step_label,
        updated_at_unix,
    )
    .await
    {
        tracing::warn!(
            work_item_id,
            state_kind = %state_kind.as_str(),
            error = %e,
            "Ushadow-write of work_items state failed (in-memory state is unaffected)"
        );
    }
}

/// Shadow-write a step's start into `work_item_steps`. Returns the row's
/// auto-increment id so the matching end-write can target the right row.
///
/// Failures (and a `None` `db`) log at WARN and yield `None`. A
/// missing id later turns the end-write into a no-op — the engine
/// must never stall on the secondary store.
pub(crate) async fn shadow_record_step_start(
    db: Option<&Database>,
    work_item_id: &str,
    step_name: &str,
    definition_filename: Option<&str>,
    started_at_unix: i64,
) -> Option<i64> {
    let db = db?;
    match crate::db::work_items::record_step_start(
        db.adapter(),
        work_item_id,
        step_name,
        definition_filename,
        started_at_unix,
    )
    .await
    {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(
                work_item_id,
                step_name,
                error = %e,
                "Ushadow-write of step start failed (engine progress unaffected)"
            );
            None
        }
    }
}

/// Shadow-write a step's end into `work_item_steps`. No-op when either
/// `db` is `None` or the matching start-write didn't return a row id.
pub(crate) async fn shadow_record_step_end(
    db: Option<&Database>,
    step_db_id: Option<i64>,
    status: crate::db::work_items::StepStatus,
    exit_code: Option<i32>,
    error_message: Option<&str>,
    ended_at_unix: i64,
) {
    let (Some(db), Some(step_db_id)) = (db, step_db_id) else {
        return;
    };
    if let Err(e) = crate::db::work_items::record_step_end(
        db.adapter(),
        step_db_id,
        status,
        exit_code,
        error_message,
        ended_at_unix,
    )
    .await
    {
        tracing::warn!(
            step_db_id,
            error = %e,
            "Ushadow-write of step end failed (engine progress unaffected)"
        );
    }
}

/// Shadow-write the start of a definition run. Marks the (work_item,
/// definition) row as Running with `started_at` set; clears any prior
/// error / ended_at so retries look fresh. Failures (and `None` `db`)
/// log at WARN and never propagate — the in-memory map remains the
/// truth-of-record.
pub(crate) async fn shadow_start_def_run(
    db: Option<&Database>,
    work_item_id: &str,
    def_filename: &str,
    started_at_unix: i64,
) {
    let Some(db) = db else { return };
    if let Err(e) = crate::db::work_items::start_definition_run(
        db.adapter(),
        work_item_id,
        def_filename,
        started_at_unix,
    )
    .await
    {
        tracing::warn!(
            work_item_id,
            def_filename,
            error = %e,
            "Ushadow-write of def-run start failed (engine progress unaffected)"
        );
    }
}

/// Shadow-write the terminal state of a definition run. UPDATE-only:
/// a missing row is a silent no-op so hot-start engines (where the
/// start-write hadn't completed yet) can't surface this as an error
/// path.
pub(crate) async fn shadow_finish_def_run(
    db: Option<&Database>,
    work_item_id: &str,
    def_filename: &str,
    state: crate::db::work_items::DefRunState,
    error_message: Option<&str>,
    ended_at_unix: i64,
) {
    let Some(db) = db else { return };
    if let Err(e) = crate::db::work_items::finish_definition_run(
        db.adapter(),
        work_item_id,
        def_filename,
        state,
        error_message,
        ended_at_unix,
    )
    .await
    {
        tracing::warn!(
            work_item_id,
            def_filename,
            state = %state.as_str(),
            error = %e,
            "Ushadow-write of def-run finish failed (engine progress unaffected)"
        );
    }
}

pub(super) async fn add_step_log(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
    step_log: StepLog,
) {
    let mut wf = workflows.write().await;
    if let Some(workflow) = wf.get_mut(ticket_key) {
        workflow.steps_log.push(step_log);
    }
}

pub(super) async fn wait_if_paused(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
    cancel_token: &CancellationToken,
) -> Result<()> {
    loop {
        let is_paused = {
            let wf = workflows.read().await;
            wf.get(ticket_key)
                .is_some_and(|w| matches!(w.state, WorkflowState::Paused { .. }))
        };

        if !is_paused {
            return Ok(());
        }

        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Err(MaestroError::Cancelled);
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                // Check again
            }
        }
    }
}
