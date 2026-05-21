// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Editor + web terminal endpoints (`open_editor` / `close_editor` /
//! `open_terminal` / `close_terminal`).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use maestro_core::container::{self, ContainerRunner};

use crate::auth::AuthenticatedUser;
use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::{AppState, DynamicPortForward};

use super::dto::can_open_editor;
use super::port_tracking::track_port_forwards;
use super::{build_editor_or_run_command_bundle, require_workflow_access};

#[derive(Serialize)]
pub struct OpenEditorResponse {
    /// Browser URL — `/s/<path-token>/?tkn=<connection-token>&folder=<...>`
    /// when the shared-port proxy is in use (GH-45).
    pub url: String,
    /// Connection token for openvscode-server authentication.
    pub connection_token: String,
    pub vscode_port: u16,
    pub port_mappings: Vec<(u16, u16)>,
    /// 32-char hex CSPRNG path token registered in the shared-port proxy
    /// registry so `/s/<path_token>/...` routes to this editor's loopback
    /// listener (GH-45 acceptance criterion #1, #5).
    pub path_token: String,
}

/// Start a browser VS Code editor container for a workflow.
pub async fn open_editor(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<OpenEditorResponse>, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    let cfg = state.config.read().await;
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;

    if !can_open_editor(w) {
        return Err((
            StatusCode::CONFLICT,
            "Cannot open editor: workflow is active, worktree missing, or Docker unavailable"
                .into(),
        ));
    }

    let worktree = w
        .worktree_path
        .as_ref()
        .ok_or((StatusCode::CONFLICT, "No worktree path".into()))?
        .clone();
    let ticket_key = w.ticket_key.clone();
    let app_ports = cfg.editor.ports.clone();
    let dynamic_ports = cfg.editor.dynamic_ports;
    let theme = cfg.editor.theme.clone();
    let extensions = cfg.editor.extensions.clone();
    let settings = cfg.editor.settings.clone();
    let setup_commands = cfg.terminal.setup_commands.clone();
    let startup_commands = cfg.terminal.startup_commands.clone();
    let git_editor = cfg.terminal.git_editor.clone();
    drop(workflows);
    drop(cfg);

    let image = ContainerRunner::discover_worker_image()
        .await
        .unwrap_or_else(|| "maestro:latest".to_string());

    // Phase 2b.3.x: try to build a per-workflow secrets bundle so the
    // browser editor's in-terminal `claude`/`cursor`/`gh` invocations see
    // the same per-user credentials an agent step would. Falls back to the
    // legacy passthrough silently when the resolver / DB / master key /
    // credential aren't available — the editor still works, just without
    // the per-user secret mount.
    let secrets_bundle: Option<std::sync::Arc<maestro_core::auth::WorkerSecretsBundle>> =
        build_editor_or_run_command_bundle(&state, &id, &auth.user_id).await;

    // Task #42: persist the bundle Arc for the editor container's lifetime
    // BEFORE we call into `start_editor`. The bind-mount on
    // `/run/maestro-secrets/` points at the bundle's `TempDir`; when the
    // `Arc` count hits zero the RAII fires and the host dir gets
    // `rm -rf`'d, leaving the still-running detached container pointing
    // at an empty directory. We clone the Arc into `state.editor_bundles`
    // here so the route-handler stack scope is no longer the sole owner.
    // Cleared in `close_editor` (and workflow teardown).
    if let Some(ref b) = secrets_bundle {
        let mut map = state.editor_bundles.write().await;
        // Replace any prior entry (open-editor → close → open again).
        map.insert(ticket_key.clone(), b.clone());
    }

    let info = container::start_editor(
        &ticket_key,
        &worktree,
        &image,
        &app_ports,
        dynamic_ports,
        &theme,
        &extensions,
        &settings,
        &setup_commands,
        &startup_commands,
        &git_editor,
        true, // isolate_workspace: restrict container to this issue's worktree
        secrets_bundle.as_deref(),
    )
    .await
    .map_err(|e| {
        // start_editor failed → no detached container was spawned. Drop
        // the bundle entry we just stashed so the TempDir RAII fires now
        // instead of leaking until process exit.
        let st = state.clone();
        let tk = ticket_key.clone();
        tokio::spawn(async move {
            st.editor_bundles.write().await.remove(&tk);
        });
        (StatusCode::INTERNAL_SERVER_ERROR, e)
    })?;

    // Seed the server-side dynamic-forwards map with the static (Docker -p) port
    // mappings so that `GET /api/workflows` returns them immediately (no need to
    // wait for the port scanner or call get_editor_info per-workflow).
    // Each port gets a proxy token so the frontend uses `/s/{token}/` URLs.
    {
        let mut entries = Vec::new();
        for (cp, hp) in &info.port_mappings {
            let path_token = state.path_token_registry.register(SessionRoute {
                kind: SessionRouteKind::DynamicPort,
                host_port: *hp,
                ticket_key: ticket_key.clone(),
                user_id: auth.user_id.clone(),
            }).await;
            let proxy_url = container::build_session_dynamic_port_url(&path_token);
            entries.push(DynamicPortForward {
                container_port: *cp,
                host_port: *hp,
                proxy_url,
                path_token,
            });
        }
        let mut fwd = state.dynamic_forwards.write().await;
        fwd.insert(ticket_key.clone(), entries);
    }

    // Spawn background port scanner if dynamic ports are available.
    if !info.spare_ports.is_empty() {
        let scanner_ticket = ticket_key.clone();
        let scanner_spare = info.spare_ports.clone();
        let scanner_vscode = info.vscode_port;
        let scanner_event_tx = state.engine.event_sender();
        let scanner_cancel = tokio_util::sync::CancellationToken::new();
        let scanner_cancel_clone = scanner_cancel.clone();

        // Cancel any prior scanner for this ticket so we don't end up with two
        // scanners racing to grab spare ports.
        {
            let mut scanners = state.editor_scanners.write().await;
            if let Some(old) = scanners.insert(ticket_key.clone(), scanner_cancel.clone()) {
                old.cancel();
            }
        }

        let scanner_owner = Some(auth.user_id.clone());
        tokio::spawn(async move {
            container::run_port_scanner(
                &scanner_ticket,
                scanner_vscode,
                scanner_spare,
                scanner_event_tx,
                scanner_cancel_clone,
                scanner_owner,
            )
            .await;
        });

        // Spawn a companion task that subscribes to broadcast events and keeps
        // `dynamic_forwards` in sync with the port scanner's forwarded/unforwarded
        // events.  This allows the list endpoint to return current port data without
        // per-workflow Docker calls.
        let dyn_fwd = state.dynamic_forwards.clone();
        let rx = state.engine.subscribe();
        let tracker_ticket = ticket_key.clone();
        let tracker_cancel = {
            let scanners = state.editor_scanners.read().await;
            scanners.get(&ticket_key).cloned()
        };
        if let Some(cancel_tok) = tracker_cancel {
            let registry = state.path_token_registry.clone();
            let tracker_user_id = auth.user_id.clone();
            tokio::spawn(track_port_forwards(tracker_ticket, tracker_user_id, dyn_fwd, registry, rx, cancel_tok));
        }
    }

    // GH-45: the editor container owns the path token (stored as a label and
    // used in `--server-base-path`). Register it in the in-memory proxy
    // registry so the reverse proxy can route `/s/<path-token>/...` requests.
    // `register_with_token` is idempotent — returns false if already present
    // (e.g. from a previous `open_editor` call for a still-running container).
    // Guard: pre-GH-45 containers lack the `maestro.path_token` label and
    // return an empty string — skip registration to avoid a phantom entry.
    let path_token = info.path_token.clone();
    if !path_token.is_empty() {
        let _ = state
            .path_token_registry
            .register_with_token(
                path_token.clone(),
                SessionRoute {
                    kind: SessionRouteKind::Editor,
                    host_port: info.vscode_port,
                    ticket_key: ticket_key.clone(),
                    user_id: auth.user_id.clone(),
                },
            )
            .await;
    }
    // Use the structured `folder` field from `EditorInfo` directly so the
    // path-prefixed proxy URL points at the same worktree path the editor
    // container was launched against. `EditorInfo::url` is intentionally NOT
    // re-parsed here — that would silently break if `build_editor_url`'s
    // query-string layout ever changed.
    let folder = if info.folder.is_empty() {
        "/".to_string()
    } else {
        info.folder.clone()
    };
    let proxy_url =
        container::build_session_editor_url(&path_token, &info.connection_token, &folder);

    Ok(Json(OpenEditorResponse {
        url: proxy_url,
        connection_token: info.connection_token,
        vscode_port: info.vscode_port,
        port_mappings: info.port_mappings,
        path_token,
    }))
}

/// Stop and remove the editor container for a workflow.
pub async fn close_editor(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> StatusCode {
    if require_workflow_access(&state, &auth, &id).await.is_err() {
        return StatusCode::NOT_FOUND;
    }
    // GH-45 AC #9: drop the path-token mapping BEFORE the port is torn down
    // so any in-flight `/s/<token>/...` request gets a clean 404 instead of
    // a hung connection or — worse — a successful upgrade right as the
    // backend dies. Both editor and terminal entries for this ticket are
    // removed because closing the editor implicitly tears down the terminal.
    let _ = state.path_token_registry.remove_for_ticket(&id).await;
    // Cancel port scanner first so it doesn't try to scan a dying container.
    if let Some(token) = state.editor_scanners.write().await.remove(&id) {
        token.cancel();
    }
    // Clean up dynamic forward tracking and terminal state.
    state.dynamic_forwards.write().await.remove(&id);
    state.terminal_ports.write().await.remove(&id);
    // Task #42: drop the bundle Arc — last strong reference triggers the
    // TempDir RAII cleanup. Done AFTER stop_editor below so the secret
    // files stay on disk for the container's final teardown read.
    container::stop_editor(&id).await;
    state.editor_bundles.write().await.remove(&id);
    StatusCode::OK
}

#[derive(Serialize)]
pub struct OpenTerminalResponse {
    /// Browser URL — `/s/<path-token>/<ttyd-token>/` when the shared-port
    /// proxy is in use (GH-45).
    pub url: String,
    /// The raw authentication token (same value embedded in the URL path).
    /// Provided separately so programmatic consumers can use it independently.
    pub credential: String,
    /// 32-char hex CSPRNG path token registered in the shared-port proxy
    /// registry so `/s/<path_token>/<ttyd-token>/` routes to this terminal's
    /// loopback listener (GH-45 acceptance criterion #1, #5).
    pub path_token: String,
}

/// Start a web terminal (ttyd) inside the running editor container.
/// The editor container must already be running (use open-editor first).
pub async fn open_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<OpenTerminalResponse>, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    // Reuse existing terminal if already recorded in the in-memory map.
    if let Some((port, token)) = state.terminal_ports.read().await.get(&id).cloned() {
        // GH-45: re-use the existing path token if one is already registered
        // for this terminal; otherwise register one now (covers the case of a
        // terminal that was started before the proxy registry shipped).
        let path_token = match state
            .path_token_registry
            .find_token_for(&id, SessionRouteKind::Terminal)
            .await
        {
            Some(t) => t,
            None => {
                state
                    .path_token_registry
                    .register(SessionRoute {
                        kind: SessionRouteKind::Terminal,
                        host_port: port,
                        ticket_key: id.clone(),
                        user_id: auth.user_id.clone(),
                    })
                    .await
            }
        };
        let url = container::build_session_terminal_url(&path_token, &token);
        return Ok(Json(OpenTerminalResponse {
            url,
            credential: token,
            path_token,
        }));
    }

    // Editor container must be running.
    let _info = container::get_editor_info(&id).await.ok_or((
        StatusCode::CONFLICT,
        "Editor container is not running — open the editor first.".into(),
    ))?;

    // Recover from a server restart: ttyd may already be running from a previous session.
    // Ask the container for the actual port and token (via pgrep) rather than trusting the now-empty map.
    if let Some((port, token)) = container::find_running_terminal(&id).await {
        state
            .terminal_ports
            .write()
            .await
            .insert(id.clone(), (port, token.clone()));
        let path_token = state
            .path_token_registry
            .register(SessionRoute {
                kind: SessionRouteKind::Terminal,
                host_port: port,
                ticket_key: id.clone(),
                user_id: auth.user_id.clone(),
            })
            .await;
        let url = container::build_session_terminal_url(&path_token, &token);
        return Ok(Json(OpenTerminalResponse {
            url,
            credential: token,
            path_token,
        }));
    }

    // Allocate a single port for ttyd from the shared editor port range.
    let ports = container::allocate_single_port().await.ok_or((
        StatusCode::CONFLICT,
        "No free ports available for terminal.".into(),
    ))?;
    let port = ports;

    let (_legacy_url, token) = container::start_terminal(&id, port)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    state
        .terminal_ports
        .write()
        .await
        .insert(id.clone(), (port, token.clone()));

    // GH-45: register a fresh CSPRNG path token so the terminal is reachable
    // only via `/s/<path-token>/<ttyd-token>/` on the dashboard origin.
    let path_token = state
        .path_token_registry
        .register(SessionRoute {
            kind: SessionRouteKind::Terminal,
            host_port: port,
            ticket_key: id.clone(),
            user_id: auth.user_id.clone(),
        })
        .await;
    let url = container::build_session_terminal_url(&path_token, &token);

    tracing::info!(workflow = %id, port, "Terminal started on port");

    Ok(Json(OpenTerminalResponse {
        url,
        credential: token,
        path_token,
    }))
}

/// Stop the web terminal for a workflow's editor container.
pub async fn close_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> StatusCode {
    if require_workflow_access(&state, &auth, &id).await.is_err() {
        return StatusCode::NOT_FOUND;
    }
    // GH-45 AC #9: drop the terminal's path-token mapping BEFORE we tear
    // down the listener, so any `/s/<token>/...` request mid-flight gets a
    // clean 404 instead of a hung connection. Editor entries for the same
    // ticket are intentionally left alone so closing the terminal doesn't
    // also break the editor.
    let _ = state
        .path_token_registry
        .remove_for_ticket_kind(&id, SessionRouteKind::Terminal)
        .await;
    state.terminal_ports.write().await.remove(&id);
    container::stop_terminal(&id).await;
    StatusCode::OK
}
