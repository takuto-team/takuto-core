// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::http::StatusCode;

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

mod definitions;
mod dto;
mod editor;
mod lifecycle;
mod list;
mod log;
mod manual;
mod port_tracking;
mod run_commands;
mod steps;

pub use definitions::{list_workflow_definitions, retry_workflow_def, run_workflow_def};
pub use list::{
    WorkflowReportResponse, get_workflow, get_workflow_report, list_workflows, workflow_counts,
};
pub use dto::{
    RunCommandStatus, TerminalLineDto, WorkflowCountsResponse, WorkflowSummary,
};
pub use editor::{
    OpenEditorResponse, OpenTerminalResponse, close_editor, close_terminal, open_editor,
    open_terminal,
};
pub use lifecycle::{
    delete_workflow, mark_work_done, pause_workflow, resume_from_error, resume_workflow,
    retry_workflow, stop_workflow,
};
pub use manual::{
    StartManualWorkflowBody, StartManualWorkflowResponse, start_manual_workflow,
};
pub use port_tracking::track_port_forwards;
pub use run_commands::{
    RunCommandsStatusResponse, StartRunCommandRequest, StartRunCommandResponse, list_run_commands,
    start_run_command, stop_run_command,
};
pub use log::{LogLineDto, LogQuery, get_log};
pub use steps::{StepDto, get_steps};

/// Check whether the authenticated user may act on the workflow with the given ticket key.
/// Users can only act on workflows they created.
///
/// Exposed `pub(crate)` so the ticket-action endpoints (`routes/tickets.rs`)
/// can reuse the same NOT_FOUND-on-mismatch convention (AC-2).
///
/// **Plan-07 slice 11 — DB is now the primary read source.** When a
/// `work_items` row matches the ticket key we run the
/// (user_id + repo association) check against the row directly,
/// bypassing the in-memory `HashMap` entirely. The HashMap is
/// consulted only as a transition fallback for workflows that
/// existed before the shadow-write shipped and haven't been
/// backfilled yet (plan-07 step 6). The legacy "no DB attached"
/// short-circuit is preserved for the few test paths that build a
/// state without a database.
pub(crate) async fn require_workflow_access(
    engine: &EngineState,
    auth_state: &AuthState,
    auth: &AuthenticatedUser,
    ticket_key: &str,
) -> Result<(), StatusCode> {
    // ── DB-first path ───────────────────────────────────────────
    if let Some(database) = auth_state.db.as_ref() {
        match maestro_core::db::work_items::get_access_fields_by_ticket_key(
            database.adapter(),
            ticket_key,
        )
        .await
        {
            Ok(Some((owner, repo_id, workspace))) => {
                if owner.as_deref() != Some(&auth.user_id) {
                    return Err(StatusCode::NOT_FOUND);
                }
                let repos = match maestro_core::db::repositories::list_for_user(
                    database.adapter(),
                    &auth.user_id,
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
                };
                let has_access = if let Some(ref repo_id) = repo_id {
                    repos.iter().any(|r| &r.id == repo_id)
                } else {
                    !workspace.is_empty() && repos.iter().any(|r| r.name == workspace)
                };
                return if has_access {
                    Ok(())
                } else {
                    Err(StatusCode::NOT_FOUND)
                };
            }
            Ok(None) => {
                // Row absent — fall through to HashMap. Pre-plan-07
                // workflows live only in the in-memory map until
                // step-6 backfills them. Remove this fallback after
                // backfill ships and one release cycle confirms the
                // logs are clean.
            }
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    // ── Legacy HashMap fallback ────────────────────────────────
    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows.get(ticket_key).ok_or(StatusCode::NOT_FOUND)?;
    if w.user_id.as_deref() != Some(&auth.user_id) {
        return Err(StatusCode::NOT_FOUND);
    }
    let Some(database) = auth_state.db.as_ref() else {
        return Ok(());
    };
    let workflow_repo_id = w.repository_id.clone();
    let workflow_workspace = w.workspace_name.clone();
    drop(workflows);

    let repos =
        match maestro_core::db::repositories::list_for_user(database.adapter(), &auth.user_id)
            .await
        {
            Ok(r) => r,
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        };
    let has_access = if let Some(ref repo_id) = workflow_repo_id {
        repos.iter().any(|r| &r.id == repo_id)
    } else {
        !workflow_workspace.is_empty() && repos.iter().any(|r| r.name == workflow_workspace)
    };
    if has_access {
        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}




/// Phase 2b.3.x: try to build a `WorkerSecretsBundle` for a side-channel
/// container (browser editor, dev-server run command) tied to a workflow.
/// Returns `None` whenever any precondition for the bundle isn't met (no
/// resolver / no DB / no master key / no per-user credential and no
/// shared-default fallback). The caller falls back to the legacy
/// PASSTHROUGH path on `None` — this is a "best-effort attach" because
/// these containers are user-interactive, not agent-driven, and partial
/// credentials should not block the user from opening the editor.
///
/// When the workflow already has an `auth_pin` (the agent path has run),
/// the bundle reuses the pinned credential row by routing through the
/// same [`auth::bundle::build`] path. Otherwise it falls back to
/// `build_for_endpoint`, which looks at the user's current credentials.
pub(super) async fn build_editor_or_run_command_bundle(
    engine: &EngineState,
    auth_state: &AuthState,
    cfg: &ConfigState,
    workflow_id_or_ticket_key: &str,
    user_id: &str,
) -> Option<std::sync::Arc<maestro_core::auth::WorkerSecretsBundle>> {
    let resolver = auth_state.git_auth_resolver.as_ref()?;
    let db = auth_state.db.as_ref()?;
    db.master_key()?;
    let cfg_snapshot = cfg.config.read().await.clone();

    // If the workflow already pinned its credentials, prefer that pin so
    // the editor sees the same row the agent path used.
    let pin = {
        let wf_arc = engine.engine.workflows_arc();
        let wf = wf_arc.read().await;
        wf.get(workflow_id_or_ticket_key)
            .and_then(|w| w.auth_pin.clone())
    };
    let result = match pin {
        Some(pin) => {
            maestro_core::auth::bundle::build(&cfg_snapshot, db, resolver, &pin, user_id).await
        }
        None => {
            maestro_core::auth::bundle::build_for_endpoint(&cfg_snapshot, db, resolver, user_id)
                .await
        }
    };
    match result {
        Ok(b) => Some(std::sync::Arc::new(b)),
        Err(e) => {
            tracing::info!(
                user_id = %user_id,
                workflow = %workflow_id_or_ticket_key,
                error = %e,
                "Bundle build skipped for editor/run-command — falling back to legacy passthrough"
            );
            None
        }
    }
}

