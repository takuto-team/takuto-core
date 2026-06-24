// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Lifecycle endpoints: pause / resume / resume_from_error / retry / stop /
//! mark_work_done / delete. All thin wrappers over `WorkflowEngine` that
//! also tear down run-command containers and bundle Arcs as appropriate.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use takuto_core::container;
use takuto_core::workflow::engine::MarkDoneOutcome;

use crate::auth::AuthenticatedUser;
use crate::routes::jira::JiraRouteError;
use crate::state::{AuthState, EditorState, EngineState, RunCommandState};

use super::require_workflow_access;

/// Pause a running workflow. Delegates to WorkflowEngine::pause_workflow
/// which sets Paused state and broadcasts a WebSocket event.
pub async fn pause_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    engine
        .engine
        .pause_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Resume a paused workflow. Delegates to WorkflowEngine::resume_workflow
/// which restores the source state and broadcasts a WebSocket event.
/// The drive_workflow loop's wait_if_paused will detect the un-pause and continue.
pub async fn resume_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    engine
        .engine
        .resume_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Resume a failed/stopped workflow from the last failed step, reusing the existing worktree and
/// skipping already-succeeded steps. The worktree must still exist on disk.
pub async fn resume_from_error(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&editor, &run_command, &id).await;

    engine
        .engine
        .resume_from_error(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Retry a failed/stopped/completed workflow. Removes the old workflow and starts fresh.
pub async fn retry_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&editor, &run_command, &id).await;

    engine
        .engine
        .retry_workflow(&id)
        .await
        .map(|_| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Stop a workflow. Delegates to WorkflowEngine::stop_workflow which:
/// - Cancels the CancellationToken (killing running processes)
/// - Sets Stopped state
/// - Force-removes worker containers for this ticket (`ContainerRunner::cleanup_for_ticket`)
/// - Spawns a fire-and-forget task to unassign the Jira ticket and move it back to "To Do"
/// - Broadcasts a WebSocket event
pub async fn stop_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    // stop_workflow tears the worker container down too; drop its
    // bundle Arcs so the TempDir RAII fires once the engine's clones
    // (if any) also release. We don't go through cleanup_run_commands
    // here because stop_workflow targets the agent worker, not editors
    // or user-started run-commands — those have their own stop
    // endpoints. But if the workflow was deleted/abandoned without
    // close_editor firing, the editor_bundles entry can leak; clean it
    // up defensively. Same for run_command_bundles.
    {
        let mut eb = editor.editor_bundles.write().await;
        eb.remove(&id);
    }
    {
        let mut rcb = run_command.run_command_bundles.write().await;
        rcb.retain(|(tk, _idx), _| tk != &id);
    }
    engine
        .engine
        .stop_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Jira transition to configured **Done** status and remove worktree; removes the workflow on full success.
pub async fn mark_work_done(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<MarkDoneOutcome>, JiraRouteError> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| JiraRouteError::Plain(s, "Workflow not found".into()))?;
    cleanup_run_commands(&editor, &run_command, &id).await;

    let outcome = engine
        .engine
        .mark_work_done(&id)
        .await
        .map_err(|e| JiraRouteError::Plain(StatusCode::CONFLICT, e.to_string()))?;

    // The Jira Done-transition failed because the owner's per-user REST token
    // was rejected → drive the global "Jira authentication failed" modal
    // instead of returning a normal outcome. (One code covers 401/403.)
    if outcome.jira_credential_rejected {
        return Err(JiraRouteError::CredentialInvalid { status: 401 });
    }
    Ok(Json(outcome))
}

/// Remove workflow from the map (not **running**), best-effort worktree cleanup, no Jira changes.
pub async fn delete_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&editor, &run_command, &id).await;

    engine
        .engine
        .delete_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Stop all run commands and clean up state for a workflow.
pub(super) async fn cleanup_run_commands(
    editor: &EditorState,
    run_command: &RunCommandState,
    ticket_key: &str,
) {
    let mut run_cmds = run_command.run_commands.write().await;
    if let Some(cmds) = run_cmds.remove(ticket_key) {
        for cmd in &cmds {
            cmd.scanner_cancel.cancel();
        }
        drop(run_cmds);
        container::stop_all_run_commands(ticket_key).await;
        // Drop every bundle Arc for this ticket's run-commands. Each
        // entry's last strong reference here fires the bundle's TempDir
        // RAII cleanup. Done AFTER the container stop so the mounted
        // secret files survive the container's last read.
        let mut bundles = run_command.run_command_bundles.write().await;
        bundles.retain(|(tk, _idx), _| tk != ticket_key);
    }
    // Also drop the editor bundle for this ticket — delete / mark-done
    // tear down both the editor and all run-commands at once.
    editor.editor_bundles.write().await.remove(ticket_key);
}
