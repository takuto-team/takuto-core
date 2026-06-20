// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Editor + web terminal endpoints (`open_editor` / `close_editor` /
//! `open_terminal` / `close_terminal`).

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use takuto_core::container::{self, ContainerRunner};
use takuto_core::workflow::snapshot::WORKSPACES_DIR;

use crate::auth::AuthenticatedUser;
use crate::routes::repos::{CloneGuard, do_clone, sanitize_clone_error};
use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::{AuthState, ConfigState, DynamicPortForward, EditorState, EngineState};

use super::dto::can_open_editor;
use super::port_tracking::track_port_forwards;
use super::{build_editor_or_run_command_bundle, require_workflow_access};

#[derive(Serialize)]
pub struct OpenEditorResponse {
    /// Browser URL — `/s/<path-token>/?tkn=<connection-token>&folder=<...>`
    /// when the shared-port proxy is in use.
    pub url: String,
    /// Connection token for openvscode-server authentication.
    pub connection_token: String,
    pub vscode_port: u16,
    pub port_mappings: Vec<(u16, u16)>,
    /// 32-char hex CSPRNG path token registered in the shared-port proxy
    /// registry so `/s/<path_token>/...` routes to this editor's loopback
    /// listener.
    pub path_token: String,
}

/// Start a browser VS Code editor container for a workflow.
pub async fn open_editor(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    State(editor): State<EditorState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<OpenEditorResponse>, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    let wf_arc = engine.engine.workflows_arc();
    let (existing_worktree, branch_name, worktree_lock, ticket_key, workspace_name) = {
        let workflows = wf_arc.read().await;
        let w = workflows
            .get(&id)
            .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;

        if !can_open_editor(w) {
            return Err((
                StatusCode::CONFLICT,
                "Cannot open editor: workflow is active, has no branch, or Docker is unavailable"
                    .into(),
            ));
        }

        (
            w.worktree_path.clone(),
            w.branch_name.clone(),
            w.worktree_lock.clone(),
            w.ticket_key.clone(),
            w.workspace_name.clone(),
        )
    };

    let (
        app_ports,
        dynamic_ports,
        theme,
        extensions,
        settings,
        setup_commands,
        startup_commands,
        git_editor,
    ) = {
        let cfg = cfg_state.config.read().await;
        (
            cfg.editor.ports.clone(),
            cfg.editor.dynamic_ports,
            cfg.editor.theme.clone(),
            cfg.editor.extensions.clone(),
            cfg.editor.settings.clone(),
            cfg.terminal.setup_commands.clone(),
            cfg.terminal.startup_commands.clone(),
            cfg.terminal.git_editor.clone(),
        )
    };

    // Resolve the worktree path, recreating it on demand when the directory is
    // missing (terminal workflow whose worktree was pruned). For an active
    // workflow with a live worktree this is a cheap existence check.
    let worktree = ensure_worktree(
        &engine,
        &auth_state,
        &id,
        &auth.user_id,
        existing_worktree,
        &branch_name,
        worktree_lock,
    )
    .await?;

    let image = ContainerRunner::discover_worker_image()
        .await
        .unwrap_or_else(|| "takuto:latest".to_string());

    // Try to build a per-workflow secrets bundle so the browser editor's
    // in-terminal `claude`/`cursor`/`gh` invocations see the same per-user
    // credentials an agent step would. Falls back to the legacy
    // passthrough silently when the resolver / DB / master key /
    // credential aren't available — the editor still works, just without
    // the per-user secret mount.
    let secrets_bundle: Option<std::sync::Arc<takuto_core::auth::WorkerSecretsBundle>> =
        build_editor_or_run_command_bundle(&engine, &auth_state, &cfg_state, &id, &auth.user_id)
            .await;

    // Persist the bundle Arc for the editor container's lifetime BEFORE
    // we call into `start_editor`. The bind-mount on
    // `/run/takuto-secrets/` points at the bundle's `TempDir`; when the
    // `Arc` count hits zero the RAII fires and the host dir gets
    // `rm -rf`'d, leaving the still-running detached container pointing
    // at an empty directory. We clone the Arc into `editor.editor_bundles`
    // here so the route-handler stack scope is no longer the sole owner.
    // Cleared in `close_editor` (and workflow teardown).
    if let Some(ref b) = secrets_bundle {
        let mut map = editor.editor_bundles.write().await;
        // Replace any prior entry (open-editor → close → open again).
        map.insert(ticket_key.clone(), b.clone());
    }

    // Workspace init commands run when the workspace container is brought up,
    // so the IDE (and any terminal/run-command sharing the container) starts
    // against a ready environment.
    let init_commands = takuto_core::workflow::engine::resolve_worktree_init_commands(
        Some(&auth.user_id),
        &workspace_name,
        auth_state.db.as_ref(),
    )
    .await;

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
        &init_commands,
    )
    .await
    .map_err(|e| {
        // start_editor failed → no detached container was spawned. Drop
        // the bundle entry we just stashed so the TempDir RAII fires now
        // instead of leaking until process exit.
        let editor_clone = editor.clone();
        let tk = ticket_key.clone();
        tokio::spawn(async move {
            editor_clone.editor_bundles.write().await.remove(&tk);
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
            let Some(path_token) = editor
                .path_token_registry
                .register(SessionRoute {
                    kind: SessionRouteKind::DynamicPort,
                    host_port: *hp,
                    ticket_key: ticket_key.clone(),
                    user_id: auth.user_id.clone(),
                })
                .await
            else {
                tracing::error!(
                    container_port = *cp,
                    host_port = *hp,
                    "Could not allocate a proxy token; skipping port mapping"
                );
                continue;
            };
            let proxy_url = container::build_session_dynamic_port_url(&path_token);
            // Shadow-write the static port row.
            takuto_core::db::work_items::shadow_upsert_port_mapping(
                engine.engine.db(),
                &id,
                *cp as i32,
                *hp as i32,
                &proxy_url,
                &path_token,
                takuto_core::db::work_items::PortMappingKind::Dynamic,
                None,
                chrono::Utc::now().timestamp(),
            )
            .await;
            entries.push(DynamicPortForward {
                container_port: *cp,
                host_port: *hp,
                proxy_url,
                path_token,
            });
        }
        let mut fwd = editor.dynamic_forwards.write().await;
        fwd.insert(ticket_key.clone(), entries);
    }

    // Spawn background port scanner if dynamic ports are available.
    if !info.spare_ports.is_empty() {
        let scanner_ticket = ticket_key.clone();
        let scanner_spare = info.spare_ports.clone();
        let scanner_vscode = info.vscode_port;
        let scanner_event_tx = engine.engine.event_sender();
        let scanner_cancel = tokio_util::sync::CancellationToken::new();
        let scanner_cancel_clone = scanner_cancel.clone();

        // Cancel any prior scanner for this ticket so we don't end up with two
        // scanners racing to grab spare ports.
        {
            let mut scanners = editor.editor_scanners.write().await;
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
        let dyn_fwd = editor.dynamic_forwards.clone();
        let rx = engine.engine.subscribe();
        let tracker_ticket = ticket_key.clone();
        let tracker_cancel = {
            let scanners = editor.editor_scanners.read().await;
            scanners.get(&ticket_key).cloned()
        };
        if let Some(cancel_tok) = tracker_cancel {
            let registry = editor.path_token_registry.clone();
            let tracker_user_id = auth.user_id.clone();
            // Clone the work_item_id + db into the spawned tracker so
            // it can shadow-upsert Dynamic port rows as the scanner
            // detects them.
            let tracker_wi = id.clone();
            let tracker_db = engine.engine.db().cloned();
            tokio::spawn(track_port_forwards(
                tracker_ticket,
                tracker_user_id,
                dyn_fwd,
                registry,
                rx,
                cancel_tok,
                Some(tracker_wi),
                tracker_db,
            ));
        }
    }

    // The editor container owns the path token (stored as a label and
    // used in `--server-base-path`). Register it in the in-memory proxy
    // registry so the reverse proxy can route `/s/<path-token>/...`
    // requests. `register_with_token` is idempotent — returns false if
    // already present (e.g. from a previous `open_editor` call for a
    // still-running container). Guard: legacy containers lack the
    // `takuto.path_token` label and return an empty string — skip
    // registration to avoid a phantom entry.
    let path_token = info.path_token.clone();
    if !path_token.is_empty() {
        let _ = editor
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
        // Shadow-write the editor port row.
        // The editor container is a 1:1 Docker forward so
        // `container_port == host_port == info.vscode_port`. The
        // proxy URL is the path-prefixed reverse-proxy route the
        // browser uses, NOT the localhost direct URL.
        // Store the path-prefix base URL only; the connection-
        // token and folder query string is regenerated per request
        // and not part of the routing record.
        let proxy_url = format!("/s/{path_token}/");
        takuto_core::db::work_items::shadow_upsert_port_mapping(
            engine.engine.db(),
            &id,
            info.vscode_port as i32,
            info.vscode_port as i32,
            &proxy_url,
            &path_token,
            takuto_core::db::work_items::PortMappingKind::Editor,
            None,
            chrono::Utc::now().timestamp(),
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

/// Ensure the workflow's worktree exists on disk, recreating it on demand.
///
/// For a live worktree this is a cheap existence check returning the existing
/// path. When the directory is missing (a terminal workflow whose worktree was
/// pruned), it re-clones the repository if its clone is gone, then checks out a
/// worktree for the **existing** branch and persists the new path to both the
/// in-memory `Workflow` and the `work_items` row.
///
/// Serialised per workflow via `worktree_lock` so two concurrent `open_editor`
/// calls cannot race to recreate the same worktree; the loser observes the
/// freshly-persisted path and reuses it.
async fn ensure_worktree(
    engine: &EngineState,
    auth_state: &AuthState,
    id: &str,
    user_id: &str,
    existing: Option<PathBuf>,
    branch_name: &str,
    worktree_lock: Arc<tokio::sync::Mutex<()>>,
) -> Result<PathBuf, (StatusCode, String)> {
    if let Some(p) = &existing
        && p.exists()
    {
        return Ok(p.clone());
    }

    if branch_name.is_empty() {
        return Err((
            StatusCode::CONFLICT,
            "Cannot recreate workspace: workflow has no branch to check out.".into(),
        ));
    }

    // Serialise recreation per workflow.
    let _guard = worktree_lock.lock().await;

    // A concurrent open may have recreated the worktree while we waited for the
    // lock — re-read the live path and reuse it if it now exists.
    {
        let workflows = engine.engine.workflows_arc();
        let wf = workflows.read().await;
        if let Some(w) = wf.get(id)
            && let Some(p) = &w.worktree_path
            && p.exists()
        {
            return Ok(p.clone());
        }
    }

    let (repo_path, _base_branch) = engine.engine.resolve_repo_for_ticket(id).await;

    // Re-clone the repository if its clone directory is missing.
    if !repo_path.join(".git").exists() {
        let repo_url = lookup_repo_remote_url(engine, id).await.ok_or((
            StatusCode::BAD_GATEWAY,
            "Cannot recreate workspace: repository remote URL is unknown.".into(),
        ))?;
        let full_name = parse_github_owner_repo(&repo_url).ok_or((
            StatusCode::BAD_GATEWAY,
            "Cannot recreate workspace: repository URL is not a GitHub owner/repo URL.".into(),
        ))?;

        // Hold the process-wide clone lock so we don't race a repository add.
        if engine
            .clone_in_progress
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err((
                StatusCode::CONFLICT,
                "A repository clone is already in progress; retry shortly.".into(),
            ));
        }
        let _clone_guard = CloneGuard(engine.clone_in_progress.clone());

        if let Some(parent) = repo_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let token_cwd = std::path::Path::new(WORKSPACES_DIR);

        do_clone(
            engine,
            auth_state,
            &full_name,
            token_cwd,
            &repo_path,
            Some(user_id),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!(
                    "Failed to re-clone repository: {}",
                    sanitize_clone_error(&e.to_string())
                ),
            )
        })?;
    }

    // Recreate the worktree for the EXISTING branch. The clone above already
    // fetched every branch, so this checks out `branch_name` offline — no
    // base-branch fetch (which would need a GitHub App token this deployment
    // may not have). A stale clone missing the branch falls back to an
    // authenticated fetch using the same resolver token the clone used.
    let worktree =
        worktree_add_existing_branch(engine, auth_state, &repo_path, branch_name, user_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Failed to recreate worktree: {e}"),
                )
            })?;

    // Persist the new worktree path to the in-memory workflow and the DB.
    {
        let workflows = engine.engine.workflows_arc();
        let mut wf = workflows.write().await;
        if let Some(w) = wf.get_mut(id) {
            w.worktree_path = Some(worktree.clone());
        }
    }
    let worktree_str = worktree.display().to_string();
    if let Some(db) = engine.engine.db()
        && let Err(e) = takuto_core::db::work_items::update_work_item_branch_and_worktree(
            db.adapter(),
            id,
            Some(branch_name),
            Some(&worktree_str),
            chrono::Utc::now().timestamp(),
        )
        .await
    {
        tracing::warn!(
            work_item_id = id,
            error = %e,
            "Failed to persist recreated worktree path (in-memory state is unaffected)"
        );
    }

    tracing::info!(
        work_item_id = id,
        branch = branch_name,
        worktree = %worktree_str,
        "Recreated missing worktree on demand for editor/terminal"
    );

    Ok(worktree)
}

/// Add a git worktree for an **existing** branch in an already-present clone.
///
/// Unlike `RealActions::create_worktree`, this does NOT fetch the base branch:
/// a full clone already has every branch as a remote-tracking ref, so
/// `git worktree add <path> <branch>` checks the branch out offline. Only a
/// stale clone that is missing the branch needs a network fetch — and that
/// fetch authenticates with the same `GitAuthResolver` token the clone used
/// (preferred), falling back to a GitHub App installation token. This is what
/// makes recreation work on PAT-only deployments, where the App-only
/// `gh_token_env` path returns no token and `git` would prompt for a username.
async fn worktree_add_existing_branch(
    engine: &EngineState,
    auth_state: &AuthState,
    repo_path: &std::path::Path,
    branch: &str,
    user_id: &str,
) -> Result<PathBuf, String> {
    let worktree_path = repo_path.join("worktrees").join(branch.replace('/', "-"));

    // Best-effort cleanup of any stale directory / registration at the target.
    let _ = tokio::fs::remove_dir_all(&worktree_path).await;
    let _ = takuto_core::process::run_shell_command_with_env(
        "git worktree prune",
        repo_path,
        tokio_util::sync::CancellationToken::new(),
        &[],
    )
    .await;

    let add_cmd = format!(
        "git -c core.hooksPath=/dev/null worktree add {} {branch}",
        worktree_path.display()
    );

    // Fast path: branch present locally (fresh clone) → offline checkout.
    let first = takuto_core::process::run_shell_command_with_env(
        &add_cmd,
        repo_path,
        tokio_util::sync::CancellationToken::new(),
        &[],
    )
    .await
    .map_err(|e| e.to_string())?;
    if first.success() {
        return Ok(worktree_path);
    }

    // Slow path: the branch is not present locally. Fetch it with auth, retry.
    let token = match auth_state.git_auth_resolver.as_ref() {
        Some(resolver) => resolver
            .token_for(
                takuto_core::github::auth_resolver::GitAction::Fetch,
                user_id,
            )
            .await
            .ok()
            .map(|t| t.bearer.expose().to_string()),
        None => {
            engine
                .engine
                .actions()
                .get_gh_installation_token(repo_path)
                .await
        }
    };
    if let Some(tok) = token {
        // Inline credential helper reads the token from $GH_TOKEN so it never
        // appears in argv / process listings (mirrors `do_clone`).
        let cred = "!f() { echo protocol=https; echo host=github.com; echo username=x-access-token; echo \"password=$GH_TOKEN\"; }; f";
        let _ = takuto_core::process::run_shell_command_with_env(
            &format!("git -c credential.helper='{cred}' fetch origin {branch}"),
            repo_path,
            tokio_util::sync::CancellationToken::new(),
            &[("GH_TOKEN", tok.as_str())],
        )
        .await;
    }

    let _ = tokio::fs::remove_dir_all(&worktree_path).await;
    let retry = takuto_core::process::run_shell_command_with_env(
        &add_cmd,
        repo_path,
        tokio_util::sync::CancellationToken::new(),
        &[],
    )
    .await
    .map_err(|e| e.to_string())?;
    if retry.success() {
        Ok(worktree_path)
    } else {
        Err(retry.stderr.trim().to_string())
    }
}

/// Look up the workflow's repository remote URL via its `repository_id`
/// (canonical) or `workspace_name` (fallback). Returns `None` when no DB is
/// attached or no registered repository carries a `repo_url`.
async fn lookup_repo_remote_url(engine: &EngineState, id: &str) -> Option<String> {
    let db = engine.engine.db()?;
    let (repository_id, workspace_name) = {
        let workflows = engine.engine.workflows_arc();
        let wf = workflows.read().await;
        let w = wf.get(id)?;
        (w.repository_id.clone(), w.workspace_name.clone())
    };

    if let Some(repo_id) = repository_id.as_deref()
        && let Ok(Some(row)) = takuto_core::db::repositories::get(db.adapter(), repo_id).await
        && let Some(url) = row.repo_url
    {
        return Some(url);
    }

    if !workspace_name.is_empty()
        && let Ok(Some(row)) =
            takuto_core::db::repositories::get_by_name(db.adapter(), &workspace_name).await
        && let Some(url) = row.repo_url
    {
        return Some(url);
    }

    None
}

/// Parse an `owner/repo` pair out of a GitHub HTTPS URL such as
/// `https://github.com/owner/repo` (with or without a trailing `.git`).
fn parse_github_owner_repo(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://github.com/")
        .or_else(|| url.strip_prefix("http://github.com/"))?;
    let rest = rest.trim_end_matches('/').trim_end_matches(".git");
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let repo = parts.next().filter(|s| !s.is_empty())?;
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::parse_github_owner_repo;

    #[test]
    fn parses_plain_https_url() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/acme/quantum-budget"),
            Some("acme/quantum-budget".to_string())
        );
    }

    #[test]
    fn parses_url_with_dot_git_and_trailing_slash() {
        assert_eq!(
            parse_github_owner_repo("https://github.com/acme/quantum-budget.git"),
            Some("acme/quantum-budget".to_string())
        );
        assert_eq!(
            parse_github_owner_repo("https://github.com/acme/quantum-budget/"),
            Some("acme/quantum-budget".to_string())
        );
    }

    #[test]
    fn parses_http_scheme() {
        assert_eq!(
            parse_github_owner_repo("http://github.com/acme/quantum-budget"),
            Some("acme/quantum-budget".to_string())
        );
    }

    #[test]
    fn rejects_non_github_host() {
        assert_eq!(
            parse_github_owner_repo("https://gitlab.com/acme/quantum-budget"),
            None
        );
        assert_eq!(
            parse_github_owner_repo("git@github.com:acme/repo.git"),
            None
        );
    }

    #[test]
    fn rejects_missing_repo_or_extra_segments() {
        assert_eq!(parse_github_owner_repo("https://github.com/acme"), None);
        assert_eq!(parse_github_owner_repo("https://github.com/"), None);
        assert_eq!(
            parse_github_owner_repo("https://github.com/acme/repo/extra"),
            None
        );
    }
}

/// Stop and remove the editor container for a workflow.
pub async fn close_editor(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> StatusCode {
    if require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .is_err()
    {
        return StatusCode::NOT_FOUND;
    }
    // Drop the path-token mapping BEFORE the port is torn down so any
    // in-flight `/s/<token>/...` request gets a clean 404 instead of a
    // hung connection or — worse — a successful upgrade right as the
    // backend dies. Both editor and terminal entries for this ticket
    // are removed because closing the editor implicitly tears down the
    // terminal.
    let _ = editor.path_token_registry.remove_for_ticket(&id).await;
    // Cancel port scanner first so it doesn't try to scan a dying container.
    if let Some(token) = editor.editor_scanners.write().await.remove(&id) {
        token.cancel();
    }
    // Clean up dynamic forward tracking and terminal state.
    editor.dynamic_forwards.write().await.remove(&id);
    editor.terminal_ports.write().await.remove(&id);
    // Drop the bundle Arc — last strong reference triggers the
    // TempDir RAII cleanup. Done AFTER stop_editor below so the secret
    // files stay on disk for the container's final teardown read.
    container::stop_editor(&id).await;
    editor.editor_bundles.write().await.remove(&id);
    // Shadow-clean every port mapping for this work_item. Done after
    // stop_editor (and after the in-memory path-token registry was
    // cleared above) so the DB mirrors the post-close state with no
    // stale forward rows.
    takuto_core::db::work_items::shadow_delete_port_mappings_for_work_item(engine.engine.db(), &id)
        .await;
    StatusCode::OK
}

#[derive(Serialize)]
pub struct OpenTerminalResponse {
    /// Browser URL — `/s/<path-token>/<ttyd-token>/` when the shared-port
    /// proxy is in use.
    pub url: String,
    /// The raw authentication token (same value embedded in the URL path).
    /// Provided separately so programmatic consumers can use it independently.
    pub credential: String,
    /// 32-char hex CSPRNG path token registered in the shared-port proxy
    /// registry so `/s/<path_token>/<ttyd-token>/` routes to this terminal's
    /// loopback listener.
    pub path_token: String,
}

/// Ensure the shared per-item workspace container (`takuto-ws-<ticket>`) is up.
///
/// This is the SAME container the editor ([`container::start_editor`]) and
/// custom commands ([`container::start_run_command`]) use — all three bring it
/// up through the core `ensure_workspace_container` primitive and `docker exec`
/// into it. Calling this from the terminal entry point lets opening the
/// terminal first start the container without opening the editor first. Builds
/// and stashes the per-user secrets bundle for the container's lifetime.
/// Callers run `require_workflow_access` before invoking this.
async fn ensure_workspace_container_up(
    engine: &EngineState,
    auth_state: &AuthState,
    cfg_state: &ConfigState,
    editor: &EditorState,
    id: &str,
    user_id: &str,
) -> Result<(), (StatusCode, String)> {
    let wf_arc = engine.engine.workflows_arc();
    let (existing_worktree, branch_name, worktree_lock, ticket_key, workspace_name) = {
        let workflows = wf_arc.read().await;
        let w = workflows
            .get(id)
            .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
        if !can_open_editor(w) {
            return Err((
                StatusCode::CONFLICT,
                "Cannot open workspace: workflow is active, has no branch, or Docker is unavailable"
                    .into(),
            ));
        }
        (
            w.worktree_path.clone(),
            w.branch_name.clone(),
            w.worktree_lock.clone(),
            w.ticket_key.clone(),
            w.workspace_name.clone(),
        )
    };

    let worktree = ensure_worktree(
        engine,
        auth_state,
        id,
        user_id,
        existing_worktree,
        &branch_name,
        worktree_lock,
    )
    .await?;

    let image = ContainerRunner::discover_worker_image()
        .await
        .unwrap_or_else(|| "takuto:latest".to_string());

    // Per-user secrets bundle so in-terminal `claude`/`cursor`/`gh` see the
    // caller's credentials, same as the editor / run-command bring-up. The Arc
    // is stashed for the container's lifetime so its TempDir mount isn't removed
    // out from under the still-running container.
    let secrets_bundle =
        build_editor_or_run_command_bundle(engine, auth_state, cfg_state, id, user_id).await;
    if let Some(ref b) = secrets_bundle {
        editor
            .editor_bundles
            .write()
            .await
            .insert(ticket_key.clone(), b.clone());
    }

    let init_commands = takuto_core::workflow::engine::resolve_worktree_init_commands(
        Some(user_id),
        &workspace_name,
        auth_state.db.as_ref(),
    )
    .await;

    // Allocate the container's editor port set (vscode + dynamic spares),
    // exactly as the editor entry point does. `start_editor` reuses
    // `spare_ports[0]` as the IDE port, so this MUST reserve a dedicated port
    // for the editor — never the terminal's ttyd port (the caller allocated
    // that separately, so it is excluded from this set). Seeding the container
    // with the ttyd port instead would make the editor collide with ttyd on the
    // same port and the editor proxy route would 404.
    let dynamic_ports = cfg_state.config.read().await.editor.dynamic_ports;
    let ws_ports = container::allocate_editor_ports(1 + dynamic_ports)
        .await
        .unwrap_or_default();

    container::workspace::ensure_workspace_container(
        &ticket_key,
        &worktree,
        &image,
        true, // isolate_workspace: restrict the container to this issue's worktree
        secrets_bundle.as_deref(),
        &ws_ports,
        &init_commands,
    )
    .await
    .map_err(|e| {
        // Bring-up failed → drop the stashed bundle so its TempDir RAII fires.
        let editor = editor.clone();
        let tk = ticket_key.clone();
        tokio::spawn(async move {
            editor.editor_bundles.write().await.remove(&tk);
        });
        (StatusCode::INTERNAL_SERVER_ERROR, e)
    })?;

    Ok(())
}

/// Start a web terminal (ttyd) inside the shared per-item workspace container,
/// bringing that container up on demand if it isn't already running.
pub async fn open_terminal(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    State(editor): State<EditorState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<OpenTerminalResponse>, (StatusCode, String)> {
    require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    // Reuse existing terminal if already recorded in the in-memory map.
    if let Some((port, token)) = editor.terminal_ports.read().await.get(&id).cloned() {
        // Re-use the existing path token if one is already registered for
        // this terminal; otherwise register one now (covers the case of a
        // terminal that was started before the proxy registry shipped).
        let path_token = match editor
            .path_token_registry
            .find_token_for(&id, SessionRouteKind::Terminal)
            .await
        {
            Some(t) => t,
            None => editor
                .path_token_registry
                .register(SessionRoute {
                    kind: SessionRouteKind::Terminal,
                    host_port: port,
                    ticket_key: id.clone(),
                    user_id: auth.user_id.clone(),
                })
                .await
                .ok_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Could not allocate a proxy token for the terminal.".to_string(),
                ))?,
        };
        let url = container::build_session_terminal_url(&path_token, &token);
        return Ok(Json(OpenTerminalResponse {
            url,
            credential: token,
            path_token,
        }));
    }

    // Recover from a server restart: if the workspace container is already up, a
    // ttyd from a previous session may still be running — reuse it (read the
    // actual port + token via pgrep rather than the now-empty in-memory map).
    if container::workspace::workspace_status(&id).await
        == container::workspace::WorkspaceStatus::Running
        && let Some((port, token)) = container::find_running_terminal(&id).await
    {
        editor
            .terminal_ports
            .write()
            .await
            .insert(id.clone(), (port, token.clone()));
        let path_token = editor
            .path_token_registry
            .register(SessionRoute {
                kind: SessionRouteKind::Terminal,
                host_port: port,
                ticket_key: id.clone(),
                user_id: auth.user_id.clone(),
            })
            .await
            .ok_or((
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not allocate a proxy token for the terminal.".to_string(),
            ))?;
        let url = container::build_session_terminal_url(&path_token, &token);
        return Ok(Json(OpenTerminalResponse {
            url,
            credential: token,
            path_token,
        }));
    }

    // Allocate a single port for ttyd from the shared editor port range, up
    // front so it can be published when the container is brought up on demand
    // (local mode; ignored under DinD --network=host).
    let port = container::allocate_single_port().await.ok_or((
        StatusCode::CONFLICT,
        "No free ports available for terminal.".into(),
    ))?;

    // Bring up the shared per-item workspace container (`takuto-ws-<ticket>` —
    // the SAME container the editor and custom commands use) on demand when it
    // isn't already running, so opening the terminal first works without first
    // opening the editor.
    if container::workspace::workspace_status(&id).await
        != container::workspace::WorkspaceStatus::Running
    {
        ensure_workspace_container_up(
            &engine,
            &auth_state,
            &cfg_state,
            &editor,
            &id,
            &auth.user_id,
        )
        .await?;
    }

    let (_legacy_url, token) = container::start_terminal(&id, port)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    editor
        .terminal_ports
        .write()
        .await
        .insert(id.clone(), (port, token.clone()));

    // Register a fresh CSPRNG path token so the terminal is reachable
    // only via `/s/<path-token>/<ttyd-token>/` on the dashboard origin.
    let path_token = editor
        .path_token_registry
        .register(SessionRoute {
            kind: SessionRouteKind::Terminal,
            host_port: port,
            ticket_key: id.clone(),
            user_id: auth.user_id.clone(),
        })
        .await
        .ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not allocate a proxy token for the terminal.".to_string(),
        ))?;
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
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(editor): State<EditorState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> StatusCode {
    if require_workflow_access(&engine, &auth_state, &auth, &id)
        .await
        .is_err()
    {
        return StatusCode::NOT_FOUND;
    }
    // Drop the terminal's path-token mapping BEFORE we tear down the
    // listener, so any `/s/<token>/...` request mid-flight gets a clean
    // 404 instead of a hung connection. Editor entries for the same
    // ticket are intentionally left alone so closing the terminal doesn't
    // also break the editor.
    let _ = editor
        .path_token_registry
        .remove_for_ticket_kind(&id, SessionRouteKind::Terminal)
        .await;
    editor.terminal_ports.write().await.remove(&id);
    container::stop_terminal(&id).await;
    StatusCode::OK
}
