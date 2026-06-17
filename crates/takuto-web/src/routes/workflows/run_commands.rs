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

use takuto_core::container;

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

    // Per-user-per-workspace lookup. Owner-less workflows return an
    // empty list.
    let configured: Vec<takuto_core::db::user_worktree_commands::RunCommand> =
        match (owner_user_id.as_deref(), auth_state.db.as_ref()) {
            (Some(uid), Some(database)) => takuto_core::db::user_worktree_commands::get(
                database.adapter(),
                uid,
                &workspace_name,
            )
            .await
            .ok()
            .flatten()
            .map(|r| r.run_commands)
            .unwrap_or_default(),
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

    let configured: Vec<takuto_core::db::user_worktree_commands::RunCommand> =
        match (owner_user_id.as_deref(), auth_state.db.as_ref()) {
            (Some(uid), Some(database)) => takuto_core::db::user_worktree_commands::get(
                database.adapter(),
                uid,
                &workspace_name,
            )
            .await
            .ok()
            .flatten()
            .map(|r| r.run_commands)
            .unwrap_or_default(),
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

    if !run_command.spawner.is_available() {
        return Err((
            StatusCode::CONFLICT,
            "Docker is not available — cannot start run command container".into(),
        ));
    }

    let image = run_command
        .spawner
        .discover_worker_image()
        .await
        .unwrap_or_else(|| "takuto:latest".to_string());

    // Generate the proxy token upfront so TAKUTO_PROXY_BASE can be passed
    // to the container. The token is NOT registered yet (host_port unknown) —
    // `register_with_token` is called by the tracker when the port is detected.
    let reserved_token = takuto_core::container::generate_session_path_token();
    let proxy_base = takuto_core::container::build_session_dynamic_port_url(&reserved_token);

    // Same per-workflow bundle the editor uses — run-commands often
    // `git push` / `gh` to publish preview deploys, so the GitHub side
    // of the bundle is the value-add here.
    let secrets_bundle: Option<std::sync::Arc<takuto_core::auth::WorkerSecretsBundle>> =
        build_editor_or_run_command_bundle(&engine, &auth_state, &cfg, &id, &auth.user_id).await;

    // Stash the bundle Arc keyed by (ticket, cmd_index). Same rationale
    // as the editor branch: the run-command container is detached, so
    // the route handler's stack scope can't be the sole owner of the
    // bundle's `TempDir` lifetime.
    if let Some(ref b) = secrets_bundle {
        let mut map = run_command.run_command_bundles.write().await;
        map.insert((ticket_key.clone(), index), b.clone());
    }

    let spare_ports = run_command
        .spawner
        .start_run_command(
            &ticket_key,
            &worktree,
            &image,
            &rc_command,
            index,
            dynamic_ports,
            true, // isolate_workspace: restrict container to this issue's worktree
            &[("TAKUTO_PROXY_BASE".to_string(), proxy_base.clone())],
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

    // Clone the work_item_id + db so the spawned tracker can
    // shadow-upsert RunCommand port rows alongside the in-memory
    // registry mutation.
    let tracker_wi = id.clone();
    let tracker_db = engine.engine.db().cloned();
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
        Some(tracker_wi),
        tracker_db,
    ));

    // Shadow-write the run-command lifecycle. `id` is the work_item id
    // (same as workflow.id); the container name is deterministic
    // (`run_command_container_name`) so we record it as the
    // `container_id` for cross-restart visibility.
    let container_name = takuto_core::container::run_command_container_name(&ticket_key, index);
    takuto_core::db::work_items::shadow_start_run_command_row(
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
    run_command
        .spawner
        .stop_run_command(&ticket_key, index)
        .await;
    // Drop the bundle Arc — last strong reference fires the TempDir
    // RAII cleanup. Done AFTER stop_run_command so the secret files
    // stay on disk for the container's final teardown read.
    run_command
        .run_command_bundles
        .write()
        .await
        .remove(&(ticket_key.clone(), index));

    // Shadow-write the run-command stop. UPDATE-only — preserves the
    // `started_at` set at start. A row that never landed (race /
    // shadow-write failure at start) stays absent and this call
    // no-ops.
    takuto_core::db::work_items::shadow_finish_run_command_row(
        engine.engine.db(),
        &id,
        index as i32,
        chrono::Utc::now().timestamp(),
    )
    .await;

    Ok(StatusCode::OK)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

    use takuto_core::db::user_worktree_commands::RunCommand;
    use takuto_core::db::{repositories, user_worktree_commands};
    use takuto_core::workflow::state::WorkflowState;

    use crate::server::build_router;
    use crate::state::{ActiveRunCommand, AppState};
    use crate::test_helpers::{
        FakeSpawner, TEST_ORIGIN, register_and_login, temp_db, test_state_with_db,
        test_state_with_db_and_spawner,
    };

    const WS: &str = "rc-ws";

    async fn user_id_for(state: &AppState, username: &str) -> String {
        let db = state.auth.db.as_ref().expect("db");
        takuto_core::db::users::get_user_by_username(db.adapter(), username)
            .await
            .expect("query user")
            .expect("user exists")
            .id
    }

    /// Seed a workflow the caller can access: a repository linked to the user,
    /// plus an in-memory workflow carrying that repo id. `require_workflow_access`
    /// then passes (repo-id match), letting tests reach the run-command guards.
    /// The workflow is left in `Pending` (active) — flip `.state` per test.
    async fn seed_accessible_workflow(state: &AppState, ticket_key: &str, owner_id: &str) {
        let db = state.auth.db.as_ref().expect("db");
        let repo_id = repositories::upsert(
            db.adapter(),
            WS,
            None,
            &format!("/tmp/takuto-test-{ticket_key}-repo"),
            "main",
            Some(owner_id),
        )
        .await
        .expect("upsert repo");
        repositories::add_for_user(db.adapter(), owner_id, &repo_id)
            .await
            .expect("link repo to user");

        state
            .engine
            .engine
            .start_workflow(
                ticket_key.to_string(),
                "run-command fixture".to_string(),
                true,
                None,
                None,
                Some(owner_id.to_string()),
                Some(repo_id),
            )
            .await
            .expect("seed start_workflow");

        // The run-command DB lookup keys on the workflow's workspace name.
        let arc = state.engine.engine.workflows_arc();
        let mut map = arc.write().await;
        let w = map.get_mut(ticket_key).expect("seeded workflow present");
        w.workspace_name = WS.to_string();
    }

    /// Overwrite the seeded workflow's state + worktree so a specific guard is reached.
    async fn set_state_and_worktree(
        state: &AppState,
        ticket_key: &str,
        new_state: WorkflowState,
        worktree: Option<PathBuf>,
    ) {
        let arc = state.engine.engine.workflows_arc();
        let mut map = arc.write().await;
        let w = map.get_mut(ticket_key).expect("seeded workflow present");
        w.state = new_state;
        w.worktree_path = worktree;
    }

    async fn seed_one_run_command(state: &AppState, owner_id: &str) {
        let db = state.auth.db.as_ref().expect("db");
        user_worktree_commands::upsert(
            db.adapter(),
            owner_id,
            WS,
            &[],
            &[RunCommand {
                name: "dev".into(),
                command: "npm run dev".into(),
            }],
        )
        .await
        .expect("seed run command");
    }

    // ── list_run_commands ─────────────────────────────────────────────────

    #[tokio::test]
    async fn list_run_commands_404_when_workflow_absent() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/workflows/NOPE-1/run-commands")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn list_run_commands_empty_for_owned_workflow_without_configured_commands() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/api/workflows/RC-1/run-commands")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["commands"].as_array().unwrap().len(), 0);
    }

    // ── start_run_command guards (all return before any container spawn) ───

    fn start_req(ticket: &str, index: usize, cookie: &str) -> Request<Body> {
        Request::post(format!(
            "/api/workflows/{ticket}/run-commands/{index}/start"
        ))
        .header("Origin", TEST_ORIGIN)
        .header("Cookie", cookie)
        .body(Body::empty())
        .unwrap()
    }

    #[tokio::test]
    async fn start_run_command_404_when_workflow_absent() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);
        let resp = app.oneshot(start_req("NOPE-1", 0, &cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn start_run_command_400_when_index_out_of_range() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        // No configured run commands → index 0 is out of range.
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn start_run_command_409_when_workflow_active() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await; // Pending ⇒ active
        seed_one_run_command(&state, &uid).await;
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn start_run_command_409_when_no_worktree() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        seed_one_run_command(&state, &uid).await;
        set_state_and_worktree(&state, "RC-1", WorkflowState::Done, None).await;
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn start_run_command_409_when_worktree_missing_on_disk() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        seed_one_run_command(&state, &uid).await;
        set_state_and_worktree(
            &state,
            "RC-1",
            WorkflowState::Done,
            Some(PathBuf::from("/tmp/takuto-test-does-not-exist-xyz")),
        )
        .await;
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    /// The spawn-success path: all guards pass and an injected [`FakeSpawner`]
    /// stands in for Docker, so the handler runs image discovery → spawn →
    /// register the active run command, with no daemon and no real container.
    #[tokio::test]
    async fn start_run_command_200_spawns_via_fake_and_registers() {
        let fake = std::sync::Arc::new(FakeSpawner::ready());
        let state = test_state_with_db_and_spawner(temp_db(), fake.clone());
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        seed_one_run_command(&state, &uid).await;
        let tmp = std::env::temp_dir().join(format!("takuto-test-wt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        set_state_and_worktree(&state, "RC-1", WorkflowState::Done, Some(tmp.clone())).await;

        let rc_state = state.run_command.run_commands.clone();
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["index"], 0);
        assert_eq!(json["name"], "dev");

        // The fake recorded exactly one spawn for (ticket, index) …
        assert_eq!(
            fake.started.lock().unwrap().as_slice(),
            &[("RC-1".to_string(), 0)]
        );
        // … and the handler registered the active run command in state.
        assert!(
            rc_state.read().await.get("RC-1").is_some(),
            "a successful spawn must register the run command"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// The spawn-failure branch: all guards pass but the spawner returns `Err`,
    /// so the handler answers 500 and does not register an active run command.
    #[tokio::test]
    async fn start_run_command_500_when_spawn_fails() {
        let fake = std::sync::Arc::new(FakeSpawner::failing("boom: no ports"));
        let state = test_state_with_db_and_spawner(temp_db(), fake.clone());
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        seed_one_run_command(&state, &uid).await;
        let tmp = std::env::temp_dir().join(format!("takuto-test-wt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        set_state_and_worktree(&state, "RC-1", WorkflowState::Done, Some(tmp.clone())).await;

        let rc_state = state.run_command.run_commands.clone();
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        // The spawn was attempted …
        assert_eq!(
            fake.started.lock().unwrap().as_slice(),
            &[("RC-1".to_string(), 0)]
        );
        // … but a failed spawn must not leave a registered run command.
        assert!(
            rc_state.read().await.get("RC-1").is_none(),
            "a failed spawn must not register the run command"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn start_run_command_409_when_already_running() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        seed_one_run_command(&state, &uid).await;
        let tmp = std::env::temp_dir().join(format!("takuto-test-wt-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        set_state_and_worktree(&state, "RC-1", WorkflowState::Done, Some(tmp.clone())).await;
        // Pre-register an active run command at index 0 keyed by ticket_key.
        {
            let mut rc = state.run_command.run_commands.write().await;
            rc.entry("RC-1".to_string())
                .or_default()
                .push(ActiveRunCommand {
                    cmd_index: 0,
                    name: "dev".into(),
                    scanner_cancel: CancellationToken::new(),
                    forwarded_port: None,
                });
        }
        let app = build_router(state);
        let resp = app.oneshot(start_req("RC-1", 0, &cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── stop_run_command ──────────────────────────────────────────────────

    #[tokio::test]
    async fn stop_run_command_404_when_workflow_absent() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/workflows/NOPE-1/run-commands/0/stop")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn stop_run_command_removes_active_entry_and_returns_200() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let uid = user_id_for(&state, "admin").await;
        seed_accessible_workflow(&state, "RC-1", &uid).await;
        {
            let mut rc = state.run_command.run_commands.write().await;
            rc.entry("RC-1".to_string())
                .or_default()
                .push(ActiveRunCommand {
                    cmd_index: 0,
                    name: "dev".into(),
                    scanner_cancel: CancellationToken::new(),
                    forwarded_port: None,
                });
        }
        let rc_state = state.run_command.run_commands.clone();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/workflows/RC-1/run-commands/0/stop")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        // The active entry for the ticket was removed.
        assert!(
            rc_state.read().await.get("RC-1").is_none(),
            "stopping the only run command should drop the ticket's entry"
        );
    }
}
