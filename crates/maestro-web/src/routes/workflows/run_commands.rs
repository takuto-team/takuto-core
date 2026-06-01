// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! User-defined run-command endpoints (`list_run_commands` /
//! `start_run_command` / `stop_run_command`).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use maestro_core::container::{self, ContainerRunner};

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EditorState, EngineState, RunCommandState};

use super::dto::{RunCommandStatus, build_run_commands_status};
use super::port_tracking::run_command_port_tracker;
use super::{build_editor_or_run_command_bundle, require_workflow_access};

/// Response for `GET /api/workflows/{id}/run-commands`.
#[derive(Serialize)]
pub struct RunCommandsStatusResponse {
    pub commands: Vec<RunCommandStatus>,
}

/// Request body for `POST /api/workflows/{id}/run-commands/{index}/start`.
#[derive(Deserialize)]
pub struct StartRunCommandRequest {}

/// Response for `POST /api/workflows/{id}/run-commands/{index}/start`.
#[derive(Serialize)]
pub struct StartRunCommandResponse {
    pub index: usize,
    pub name: String,
}

/// List the status of all configured run commands for a workflow.
pub async fn list_run_commands(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
) -> Result<Json<RunCommandsStatusResponse>, (StatusCode, String)> {
    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
    let owner_user_id = w.user_id.clone();
    let workspace_name = w.workspace_name.clone();
    drop(workflows);

    // Per-user-per-workspace lookup (plan-09). Owner-less workflows return
    // an empty list.
    let configured: Vec<maestro_core::db::user_worktree_commands::RunCommand> =
        match (owner_user_id.as_deref(), auth_state.db.as_ref()) {
            (Some(uid), Some(database)) => {
                // Plan-11 step 3: user_worktree_commands::get migrated to
                // the agnostic adapter. Direct async call from this handler.
                maestro_core::db::user_worktree_commands::get(
                    database.adapter(),
                    uid,
                    &workspace_name,
                )
                .await
                .ok()
                .flatten()
                .map(|r| r.run_commands)
                .unwrap_or_default()
            }
            _ => Vec::new(),
        };

    let run_cmds_state = run_command.run_commands.read().await;
    let commands = build_run_commands_status(&configured, run_cmds_state.get(&id));

    Ok(Json(RunCommandsStatusResponse { commands }))
}

/// Start a run command for a workflow.
pub async fn start_run_command(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg): State<ConfigState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path((id, index)): Path<(String, usize)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<StartRunCommandResponse>, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;

    // Resolve owner + workspace before opening the DB to keep the workflow
    // read-lock short.
    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
    let owner_user_id = w.user_id.clone();
    let workspace_name = w.workspace_name.clone();
    drop(workflows);

    let configured: Vec<maestro_core::db::user_worktree_commands::RunCommand> =
        match (owner_user_id.as_deref(), auth_state.db.as_ref()) {
            (Some(uid), Some(database)) => {
                // Plan-11 step 3: user_worktree_commands::get migrated to
                // the agnostic adapter. Direct async call from this handler.
                maestro_core::db::user_worktree_commands::get(
                    database.adapter(),
                    uid,
                    &workspace_name,
                )
                .await
                .ok()
                .flatten()
                .map(|r| r.run_commands)
                .unwrap_or_default()
            }
            _ => Vec::new(),
        };

    let rc = configured.get(index).ok_or((
        StatusCode::BAD_REQUEST,
        format!(
            "Run command index {index} out of range (max {})",
            configured.len()
        ),
    ))?;
    let rc_name = rc.name.clone();
    let rc_command = rc.command.clone();
    let dynamic_ports = {
        let cfg_guard = cfg.config.read().await;
        cfg_guard.editor.dynamic_ports
    };

    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;

    // Run commands only allowed when the workflow is not active (same as editor)
    if w.state.is_active() {
        return Err((
            StatusCode::CONFLICT,
            "Cannot start run command while workflow is active".into(),
        ));
    }

    let worktree = w
        .worktree_path
        .as_ref()
        .ok_or((StatusCode::CONFLICT, "No worktree path".into()))?
        .clone();

    if !worktree.exists() {
        return Err((
            StatusCode::CONFLICT,
            "Worktree does not exist on disk".into(),
        ));
    }

    let ticket_key = w.ticket_key.clone();
    drop(workflows);

    // Check if already running
    {
        let run_cmds = run_command.run_commands.read().await;
        if let Some(active) = run_cmds.get(&ticket_key)
            && active.iter().any(|c| c.cmd_index == index)
        {
            return Err((
                StatusCode::CONFLICT,
                format!("Run command '{}' is already running", rc_name),
            ));
        }
    }

    if !ContainerRunner::is_available() {
        return Err((
            StatusCode::CONFLICT,
            "Docker is not available — cannot start run command container".into(),
        ));
    }

    let image = ContainerRunner::discover_worker_image()
        .await
        .unwrap_or_else(|| "maestro:latest".to_string());

    // Generate the proxy token upfront so MAESTRO_PROXY_BASE can be passed
    // to the container. The token is NOT registered yet (host_port unknown) —
    // `register_with_token` is called by the tracker when the port is detected.
    let reserved_token = maestro_core::container::generate_session_path_token();
    let proxy_base = maestro_core::container::build_session_dynamic_port_url(&reserved_token);

    // Phase 2b.3.x: same per-workflow bundle the editor uses — run-commands
    // often `git push` / `gh` to publish preview deploys, so the GitHub
    // side of the bundle is the value-add here.
    let secrets_bundle: Option<std::sync::Arc<maestro_core::auth::WorkerSecretsBundle>> =
        build_editor_or_run_command_bundle(&engine, &auth_state, &cfg, &id, &auth.user_id).await;

    // Task #42: stash the bundle Arc keyed by (ticket, cmd_index). Same
    // rationale as the editor branch: the run-command container is
    // detached, so the route handler's stack scope can't be the sole
    // owner of the bundle's `TempDir` lifetime.
    if let Some(ref b) = secrets_bundle {
        let mut map = run_command.run_command_bundles.write().await;
        map.insert((ticket_key.clone(), index), b.clone());
    }

    let spare_ports = container::start_run_command(
        &ticket_key,
        &worktree,
        &image,
        &rc_command,
        index,
        dynamic_ports,
        true, // isolate_workspace: restrict container to this issue's worktree
        &[("MAESTRO_PROXY_BASE", &proxy_base)],
        secrets_bundle.as_deref(),
    )
    .await
    .map_err(|e| {
        // Spawn failed → drop the stashed Arc.
        let run_command_clone = run_command.clone();
        let key = (ticket_key.clone(), index);
        tokio::spawn(async move {
            run_command_clone
                .run_command_bundles
                .write()
                .await
                .remove(&key);
        });
        (StatusCode::INTERNAL_SERVER_ERROR, e)
    })?;

    // Register in state BEFORE spawning background tasks so that events
    // emitted by the scanner/tracker always find an existing map entry
    // (avoids a race where a fast container exit leaves a stale entry).
    let cancel = CancellationToken::new();
    let scanner_cancel = cancel.clone();
    let tracker_cancel = cancel.clone();
    {
        let mut run_cmds = run_command.run_commands.write().await;
        let entry = run_cmds.entry(ticket_key.clone()).or_default();
        entry.push(crate::state::ActiveRunCommand {
            cmd_index: index,
            name: rc_name.clone(),
            scanner_cancel: cancel,
            forwarded_port: None,
        });
    }

    // Start background port scanner for this run command
    let event_tx = engine.engine.event_sender();
    let ticket_for_scanner = ticket_key.clone();

    let run_cmds_map = run_command.run_commands.clone();
    let ticket_for_tracker = ticket_key.clone();

    // Spawn port scanner
    tokio::spawn({
        let spare = spare_ports.clone();
        let scanner_owner = Some(auth.user_id.clone());
        async move {
            container::run_run_command_port_scanner(
                &ticket_for_scanner,
                index,
                spare,
                event_tx,
                scanner_cancel,
                scanner_owner,
            )
            .await;
        }
    });

    tokio::spawn(run_command_port_tracker(
        ticket_for_tracker,
        index,
        auth.user_id.clone(),
        reserved_token,
        proxy_base,
        run_cmds_map,
        editor.path_token_registry.clone(),
        engine.engine.subscribe(),
        tracker_cancel,
    ));

    // Plan-07 step 4 slice 5: shadow-write the run-command lifecycle.
    // `id` is the work_item id (same as workflow.id); the container
    // name is deterministic (`run_command_container_name`) so we
    // record it as the `container_id` for cross-restart visibility.
    let container_name =
        maestro_core::container::run_command_container_name(&ticket_key, index);
    maestro_core::db::work_items::shadow_start_run_command_row(
        engine.engine.db(),
        &id,
        index as i32,
        &rc_name,
        Some(&container_name),
        chrono::Utc::now().timestamp(),
    )
    .await;

    Ok(Json(StartRunCommandResponse {
        index,
        name: rc_name,
    }))
}

/// Stop a running run command.
pub async fn stop_run_command(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path((id, index)): Path<(String, usize)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
    let ticket_key = w.ticket_key.clone();
    drop(workflows);

    // Cancel scanner, deregister proxy token, and remove from state
    {
        let mut run_cmds = run_command.run_commands.write().await;
        if let Some(cmds) = run_cmds.get_mut(&ticket_key) {
            if let Some(pos) = cmds.iter().position(|c| c.cmd_index == index) {
                cmds[pos].scanner_cancel.cancel();
                if let Some(ref fwd) = cmds[pos].forwarded_port {
                    editor.path_token_registry.remove(&fwd.path_token).await;
                }
                cmds.remove(pos);
            }
            if cmds.is_empty() {
                run_cmds.remove(&ticket_key);
            }
        }
    }

    // Stop the container
    container::stop_run_command(&ticket_key, index).await;
    // Task #42: drop the bundle Arc — last strong reference fires the
    // TempDir RAII cleanup. Done AFTER stop_run_command so the secret
    // files stay on disk for the container's final teardown read.
    run_command
        .run_command_bundles
        .write()
        .await
        .remove(&(ticket_key.clone(), index));

    // Plan-07 step 4 slice 5: shadow-write the run-command stop.
    // UPDATE-only — preserves the `started_at` set at start. A row
    // that never landed (race / shadow-write failure at start) stays
    // absent and this call no-ops.
    maestro_core::db::work_items::shadow_finish_run_command_row(
        engine.engine.db(),
        &id,
        index as i32,
        chrono::Utc::now().timestamp(),
    )
    .await;

    Ok(StatusCode::OK)
}
