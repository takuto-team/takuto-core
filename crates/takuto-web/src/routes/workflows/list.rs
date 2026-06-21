// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Read-only workflow list / get endpoints + report retrieval.

use std::collections::HashMap;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use takuto_core::container;
use takuto_core::jira::ticket_browse_url;
use takuto_core::workflow::dashboard_progress;
use takuto_core::workflow::engine::Workflow;
use takuto_core::workflow::state::WorkflowState;

use crate::auth::AuthenticatedUser;
use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::{
    AuthState, ConfigState, DynamicPortForward, EditorState, EngineState, RunCommandState,
};

use super::dto::{
    TerminalLineDto, WorkflowCountsResponse, WorkflowSummary, build_issue_url,
    build_run_commands_status, can_open_editor, can_resume_from_error, can_start_workflow,
    extract_error, has_report_file, manual_cap_fields, prep_state, workflow_action_flags,
    workflow_def_runs_display,
};

/// Resolve whether a parked workflow's repository is present on disk, from the
/// already-loaded `repo_paths` map (id + name → local_path). Only meaningful for
/// parked items; callers gate on `can_start_workflow`.
fn repo_available(
    w: &takuto_core::workflow::engine::Workflow,
    repo_paths: &std::collections::HashMap<String, String>,
) -> bool {
    let key = w
        .repository_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(w.workspace_name.as_str());
    repo_paths
        .get(key)
        .map(|p| std::path::Path::new(p).exists())
        .unwrap_or(false)
}

/// Single-workflow variant of [`repo_available`]: resolves the repo's
/// `local_path` from the DB (by `repository_id`, else `workspace_name`) and
/// checks it exists on disk. Used by `get_workflow`, which doesn't load the
/// full repo list.
async fn repo_available_db(
    db: Option<&takuto_core::db::Database>,
    w: &takuto_core::workflow::engine::Workflow,
) -> bool {
    let Some(db) = db else { return false };
    let row = if let Some(id) = w.repository_id.as_deref().filter(|s| !s.is_empty()) {
        takuto_core::db::repositories::get(db.adapter(), id)
            .await
            .ok()
            .flatten()
    } else {
        takuto_core::db::repositories::get_by_name(db.adapter(), &w.workspace_name)
            .await
            .ok()
            .flatten()
    };
    row.map(|r| std::path::Path::new(&r.local_path).exists())
        .unwrap_or(false)
}
use super::require_workflow_access;

/// Convert a Unix-seconds timestamp to RFC3339 with millisecond
/// precision. Used to project `work_items` BIGINT timestamps into
/// the same wire format the in-memory `Workflow` produces via
/// `chrono::DateTime::to_rfc3339`. Out-of-range values fall back
/// to the epoch — a value that far out should never appear in
/// practice and the fallback keeps the JSON well-formed.
fn unix_seconds_to_rfc3339(secs: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0)
        .unwrap_or(chrono::DateTime::<chrono::Utc>::UNIX_EPOCH)
        .to_rfc3339()
}

pub async fn list_workflows(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<Vec<WorkflowSummary>> {
    let cfg = cfg_state.config.read().await;
    // Workflow visibility is gated by the caller's `user_repositories`
    // associations. Build two HashSets in ONE batched query so the in-memory
    // filter below is O(1) per workflow.
    // `repo_paths` (id + name → local_path) lets a parked item resolve its
    // repo's on-disk presence for the `prep_state` readiness signal.
    let mut repo_paths: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let (allowed_repo_ids, allowed_repo_names): (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) = if let Some(database) = auth_state.db.as_ref() {
        match takuto_core::db::repositories::list_for_user(database.adapter(), &auth.user_id).await
        {
            Ok(repos) => {
                let mut ids = std::collections::HashSet::new();
                let mut names = std::collections::HashSet::new();
                for r in repos {
                    repo_paths.insert(r.id.clone(), r.local_path.clone());
                    repo_paths.insert(r.name.clone(), r.local_path);
                    ids.insert(r.id);
                    names.insert(r.name);
                }
                (ids, names)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load user repositories for workflow filter; returning empty list");
                (
                    std::collections::HashSet::new(),
                    std::collections::HashSet::new(),
                )
            }
        }
    } else {
        // No DB available (test paths). Fall back to legacy "all workflows owned
        // by the caller" with no repo gate so the existing unit tests keep
        // working.
        (
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        )
    };
    let no_db = auth_state.db.is_none();
    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let dyn_fwd = editor.dynamic_forwards.read().await;
    let run_cmds_state = run_command.run_commands.read().await;
    // Build terminal URLs via the path-token registry so the frontend uses
    // the `/s/<token>/...` proxy path instead of a direct `localhost:<port>` URL.
    // Single-pass bulk lookup: acquire the registry read lock once rather than
    // N times in a serial loop (avoids O(K²) linear scans at scale).
    let terminal_ports_snap: Vec<(String, String)> = editor
        .terminal_ports
        .read()
        .await
        .iter()
        .map(|(k, (_port, ttyd_token))| (k.clone(), ttyd_token.clone()))
        .collect();
    let terminal_urls: HashMap<String, String> = {
        let registry = editor.path_token_registry.inner_read().await;
        terminal_ports_snap
            .iter()
            .filter_map(|(ticket_key, ttyd_token)| {
                registry
                    .iter()
                    .find(|(_, r)| {
                        r.ticket_key == *ticket_key && r.kind == SessionRouteKind::Terminal
                    })
                    .map(|(path_token, _)| {
                        (
                            ticket_key.clone(),
                            container::build_session_terminal_url(path_token, ttyd_token),
                        )
                    })
            })
            .collect()
    };

    // Editor URLs for the workflows whose IDE is currently open, so the
    // dashboard card lights its editor icon green (matching the terminal). We
    // probe ONLY tickets that have a registered Editor route — the handful of
    // open editors, never every workflow — so the per-editor `get_editor_info`
    // docker call stays bounded. `get_editor_info` returns `None` when
    // openvscode isn't actually running, so a stale registry entry correctly
    // leaves the icon grey.
    let editor_open_tickets: Vec<String> = {
        let registry = editor.path_token_registry.inner_read().await;
        let mut tickets: Vec<String> = registry
            .iter()
            .filter(|(_, r)| r.kind == SessionRouteKind::Editor)
            .map(|(_, r)| r.ticket_key.clone())
            .collect();
        tickets.sort();
        tickets.dedup();
        tickets
    };
    let mut editor_urls: HashMap<String, String> = HashMap::new();
    for tk in editor_open_tickets {
        if let Some(info) = container::get_editor_info(&tk).await {
            editor_urls.insert(
                tk,
                container::build_session_editor_url(
                    &info.path_token,
                    &info.connection_token,
                    &info.folder,
                ),
            );
        }
    }
    // Collect the unique (user_id, workspace_name) pairs for this user's
    // workflows in the current workspace, then do ONE batched DB read for
    // the configured run-commands. (At most one pair in practice, since the
    // list is already filtered to the authenticated user + current workspace.
    // Still go through the batched helper for forward-compat / consistency.)
    let visible_workflows: Vec<&Workflow> = workflows
        .values()
        .filter(|w| w.user_id.as_deref() == Some(&auth.user_id))
        .filter(|w| {
            if no_db {
                // No DB → fall through with just the user_id gate (test paths).
                return true;
            }
            // Canonical filter: workflow.repository_id ∈ caller's user_repositories.
            if let Some(ref repo_id) = w.repository_id
                && allowed_repo_ids.contains(repo_id)
            {
                return true;
            }
            // Defensive back-compat: legacy workflows have repository_id=None but
            // may still carry a workspace_name matching a repo the user has
            // added (the startup reconciliation back-fills repository_id but
            // is best-effort).
            !w.workspace_name.is_empty() && allowed_repo_names.contains(&w.workspace_name)
        })
        .collect();
    let pairs: Vec<(String, String)> = {
        let mut seen: std::collections::HashSet<(String, String)> =
            std::collections::HashSet::new();
        let mut out: Vec<(String, String)> = Vec::new();
        for w in &visible_workflows {
            if let Some(uid) = w.user_id.as_deref() {
                let key = (uid.to_string(), w.workspace_name.clone());
                if seen.insert(key.clone()) {
                    out.push(key);
                }
            }
        }
        out
    };
    // Pre-fetch every work_items row for this user so the per-summary
    // projection below can override DB-authoritative fields (ticket
    // metadata, PR/branch state, worktree path, timestamps). One
    // query for the whole list keeps this O(1) per workflow rather
    // than O(N) DB round-trips.
    let db_rows: HashMap<String, takuto_core::db::work_items::WorkItemRow> =
        if let Some(database) = auth_state.db.as_ref() {
            match takuto_core::db::work_items::list_work_items(
                database.adapter(),
                &takuto_core::db::work_items::WorkItemListQuery {
                    caller_user_id: Some(auth.user_id.clone()),
                    caller_is_admin: false,
                    workspace_name: None,
                    state_filter: None,
                    include_team_visible: false,
                    limit: 10_000,
                    offset: 0,
                },
            )
            .await
            {
                // Key by the work item's unique `id`, NOT `ticket_key`: a
                // re-run leaves multiple non-deleted rows sharing a ticket_key,
                // and a ticket_key-keyed collect would keep an arbitrary
                // (oldest) duplicate — making the DTO render durable fields
                // (pr_url, branch, summary) from the wrong run. Looked up below
                // by the rendered workflow's own `w.id`.
                Ok(rows) => rows.into_iter().map(|r| (r.id.clone(), r)).collect(),
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Failed to batch-load work_items rows; falling back to in-memory map values"
                    );
                    HashMap::new()
                }
            }
        } else {
            HashMap::new()
        };

    // Authoritative step count of each work item's latest completed flow,
    // keyed by work_items.id. Used as the progress denominator for completed
    // workflows, whose in-memory `steps_log` / `current_def_total_steps` do
    // not survive a restart. Keyed by the rendered workflow's own id (like
    // `db_rows` above — never `ticket_key`, which collides across re-runs).
    // One batched query for the whole list.
    let completed_step_counts: HashMap<String, u32> = match auth_state.db.as_ref() {
        Some(database) if !visible_workflows.is_empty() => {
            let ids: Vec<String> = visible_workflows.iter().map(|w| w.id.clone()).collect();
            takuto_core::db::work_items::count_steps_of_latest_completed_def(
                database.adapter(),
                &ids,
            )
            .await
            .unwrap_or_default()
        }
        _ => HashMap::new(),
    };

    let run_commands_by_pair: HashMap<
        (String, String),
        Vec<takuto_core::db::user_worktree_commands::RunCommand>,
    > = match (pairs.is_empty(), auth_state.db.as_ref()) {
        (false, Some(database)) => {
            let pair_refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(u, w)| (u.as_str(), w.as_str()))
                .collect();
            takuto_core::db::user_worktree_commands::get_run_commands_for_pairs(
                database.adapter(),
                &pair_refs,
            )
            .await
            .unwrap_or_default()
        }
        _ => HashMap::new(),
    };
    let empty_run_cmds: Vec<takuto_core::db::user_worktree_commands::RunCommand> = Vec::new();
    let mut summaries: Vec<WorkflowSummary> = visible_workflows
        .into_iter()
        .map(|w| {
            let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
            let (started_manually, counts_toward_manual_cap) = manual_cap_fields(w);
            // Use the server-side dynamic-forwards cache so that port buttons
            // appear immediately on page load (no per-workflow Docker call).
            let port_mappings: Vec<(u16, String)> = dyn_fwd
                .get(&w.ticket_key)
                .map(|forwards| {
                    forwards
                        .iter()
                        .map(|f| (f.container_port, f.proxy_url.clone()))
                        .collect()
                })
                .unwrap_or_default();
            let configured_run_cmds: &[takuto_core::db::user_worktree_commands::RunCommand] =
                match w.user_id.as_deref() {
                    Some(uid) => run_commands_by_pair
                        .get(&(uid.to_string(), w.workspace_name.clone()))
                        .map(|v| v.as_slice())
                        .unwrap_or(&empty_run_cmds),
                    None => &empty_run_cmds,
                };
            let run_commands =
                build_run_commands_status(configured_run_cmds, run_cmds_state.get(&w.ticket_key));
            // When a work_items row exists, prefer its values for
            // every shadow-written scalar field. Engine-derived
            // fields (state display, action flags, in-memory caches)
            // stay on the Workflow path because they have no DB
            // equivalent.
            // The work_items row is authoritative for every durable field
            // (ticket metadata, timestamps, branch, PR, worktree path); they
            // are written to it as the engine learns them. After the state
            // cutover every live row is DB-backed, so a present row is used
            // wholesale — a single row-level branch, not a per-field ladder.
            // A cached entry with no row is a transitional anomaly: log it and
            // render durable fields from the cache so the row does not vanish
            // mid-migration. Engine-derived fields (state display, action
            // flags, port cache) come from the Workflow below.
            let row = db_rows.get(&w.id);
            if row.is_none() {
                tracing::warn!(
                    ticket = %w.ticket_key,
                    "work item in cache has no work_items row; rendering durable fields from the cache (transitional)"
                );
            }
            let (
                ticket_summary,
                ticket_description,
                ticket_type,
                started_at_rfc,
                updated_at_rfc,
                branch_name,
                pr_url,
                pr_merged,
                worktree_path,
            ) = match row {
                Some(r) => (
                    r.ticket_summary.clone().unwrap_or_default(),
                    r.ticket_description.clone().unwrap_or_default(),
                    r.ticket_type.clone().unwrap_or_default(),
                    unix_seconds_to_rfc3339(r.started_at),
                    unix_seconds_to_rfc3339(r.updated_at),
                    r.branch_name.clone().unwrap_or_default(),
                    r.pr_url.clone(),
                    r.pr_merged,
                    r.worktree_path
                        .as_deref()
                        .map(std::path::Path::new)
                        .filter(|p| p.exists())
                        .and_then(|p| p.to_str().map(str::to_string)),
                ),
                None => (
                    w.ticket_summary.clone(),
                    w.ticket_description.clone(),
                    w.ticket_type.clone(),
                    w.started_at.to_rfc3339(),
                    w.updated_at.to_rfc3339(),
                    w.branch_name.clone(),
                    w.pr_url.clone(),
                    w.pr_merged,
                    w.worktree_path
                        .as_ref()
                        .filter(|p| p.exists())
                        .and_then(|p| p.to_str().map(str::to_string)),
                ),
            };
            let progress = dashboard_progress::progress_fields(
                w,
                &cfg,
                completed_step_counts.get(&w.id).copied(),
            );
            // FS-check the repo only for parked items (short-circuits otherwise).
            let repo_ready = if can_start_workflow(w) {
                repo_available(w, &repo_paths)
            } else {
                true
            };
            WorkflowSummary {
                id: w.id.clone(),
                ticket_key: w.ticket_key.clone(),
                ticket_summary,
                ticket_description,
                ticket_type,
                state: w.status_display(),
                started_at: started_at_rfc,
                updated_at: updated_at_rfc,
                branch_name,
                pr_url,
                pr_merged,
                steps_log: w.steps_log.clone(),
                error: extract_error(&w.state),
                terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
                can_address_pr_comments,
                can_merge_base,
                can_mark_done,
                can_delete: !w.state.is_active() || can_start_workflow(w),
                can_start: can_start_workflow(w),
                progress_percent: progress.0,
                progress_steps_total: progress.1,
                started_manually,
                counts_toward_manual_cap,
                jira_browse_url: ticket_browse_url(&cfg.jira.site, &w.ticket_key),
                issue_url: build_issue_url(w, &cfg.jira.site),
                can_open_editor: can_open_editor(w),
                editor_url: editor_urls.get(&w.ticket_key).cloned(),
                editor_port_mappings: port_mappings,
                jira_available: w.jira_available,
                ticketing_system: w.ticketing_system.to_string(),
                can_resume_from_error: can_resume_from_error(w),
                terminal_url: terminal_urls.get(&w.ticket_key).cloned(),
                run_commands,
                generate_report: cfg.general.generate_report,
                has_report: has_report_file(w),
                workflow_def_runs: workflow_def_runs_display(w),
                worktree_path,
                user_id: w.user_id.clone(),
                workspace_name: w.workspace_name.clone(),
                repository_id: w.repository_id.clone(),
                prep_state: prep_state(w, repo_ready).map(str::to_string),
            }
        })
        .collect();
    // Oldest first — matches dashboard stable card order (new workflows last).
    summaries.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Json(summaries)
}

/// Per-user workflow counts for the dashboard summary bar.
///
/// Counts EXACTLY what the grid renders: the caller's in-memory workflows in
/// repositories they've added — same source (the workflow map) and same filter
/// as `list_workflows`, tallying the live in-memory state. This is deliberate:
/// sourcing counts from the DB (across all workspaces, with shadow-write lag)
/// let the summary bar diverge from the cards (e.g. a running item showing as 0
/// while terminal rows from other workspaces inflated "errors").
pub async fn workflow_counts(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<WorkflowCountsResponse> {
    // Repo gate identical to `list_workflows`: a workflow is visible when its
    // repository_id (or, for legacy rows, workspace_name) belongs to a repo the
    // caller has added.
    let (allowed_repo_ids, allowed_repo_names): (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) = if let Some(database) = auth_state.db.as_ref() {
        match takuto_core::db::repositories::list_for_user(database.adapter(), &auth.user_id).await
        {
            Ok(repos) => {
                let mut ids = std::collections::HashSet::new();
                let mut names = std::collections::HashSet::new();
                for r in repos {
                    ids.insert(r.id);
                    names.insert(r.name);
                }
                (ids, names)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to load user repositories for count filter");
                (
                    std::collections::HashSet::new(),
                    std::collections::HashSet::new(),
                )
            }
        }
    } else {
        // No DB (test paths): fall back to the user_id gate only.
        (
            std::collections::HashSet::new(),
            std::collections::HashSet::new(),
        )
    };
    let no_db = auth_state.db.is_none();

    let mut running = 0u32;
    let mut completed = 0u32;
    let mut errors = 0u32;
    let mut paused = 0u32;
    let mut pending = 0u32;

    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    for w in workflows.values() {
        if w.user_id.as_deref() != Some(&auth.user_id) {
            continue;
        }
        let visible = no_db
            || w.repository_id
                .as_ref()
                .is_some_and(|id| allowed_repo_ids.contains(id))
            || (!w.workspace_name.is_empty() && allowed_repo_names.contains(&w.workspace_name));
        if !visible {
            continue;
        }
        match &w.state {
            WorkflowState::Done => completed += 1,
            WorkflowState::Error { .. } | WorkflowState::Stopped => errors += 1,
            WorkflowState::Paused { .. } => paused += 1,
            // Added to the dashboard but not yet started.
            WorkflowState::Pending => pending += 1,
            // Every active driver state counts as "running".
            _ => running += 1,
        }
    }

    Json(WorkflowCountsResponse {
        running,
        completed,
        errors,
        paused,
        pending,
    })
}

pub async fn get_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    State(editor): State<EditorState>,
    State(run_command): State<RunCommandState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<WorkflowSummary>, StatusCode> {
    let cfg = cfg_state.config.read().await;
    // Visibility is gated by `require_workflow_access`, which checks
    // both user_id ownership AND repo association.
    require_workflow_access(&engine, &auth_state, &auth, &id).await?;
    // Read the full work_items row when one exists. Every shadow-
    // written scalar field gets projected from the row instead of
    // the in-memory Workflow. Engine-derived fields (state display,
    // action flags, in-memory caches) stay on the Workflow path —
    // they have no DB equivalent.
    let db_row: Option<takuto_core::db::work_items::WorkItemRow> =
        if let Some(database) = auth_state.db.as_ref() {
            match takuto_core::db::work_items::get_work_item_by_ticket_key(database.adapter(), &id)
                .await
            {
                Ok(row) => row,
                Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
            }
        } else {
            None
        };
    let wf_arc = engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
    let (started_manually, counts_toward_manual_cap) = manual_cap_fields(w);
    let ticket_key = w.ticket_key.clone();
    let editor_info = container::get_editor_info(&ticket_key).await;
    // Prefer the server-side dynamic-forwards cache (includes both static Docker
    // mappings seeded at open-editor time and dynamically-detected socat forwards).
    // Fall back to Docker-queried port mappings for editors opened before this
    // Takuto process started (server restart).
    let dyn_fwd = editor.dynamic_forwards.read().await;
    let port_mappings: Vec<(u16, String)> = if let Some(forwards) = dyn_fwd.get(&ticket_key) {
        forwards
            .iter()
            .map(|f| (f.container_port, f.proxy_url.clone()))
            .collect()
    } else {
        // Fallback: editor opened before this Takuto process started (restart
        // recovery). Register proxy tokens on the fly and seed the cache so
        // subsequent calls don't re-register.
        let raw = editor_info
            .as_ref()
            .map(|e| e.port_mappings.clone())
            .unwrap_or_default();
        drop(dyn_fwd);
        let mut entries = Vec::new();
        let mut result = Vec::new();
        for (cp, hp) in &raw {
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
            result.push((*cp, proxy_url.clone()));
            entries.push(DynamicPortForward {
                container_port: *cp,
                host_port: *hp,
                proxy_url,
                path_token,
            });
        }
        if !entries.is_empty() {
            let mut fwd = editor.dynamic_forwards.write().await;
            fwd.entry(ticket_key.clone()).or_insert(entries);
        }
        result
    };
    // See the list endpoint: the work_items row is authoritative for every
    // durable field; a present row is used wholesale (one row-level branch,
    // not a per-field ladder). A missing row is a transitional anomaly logged
    // here, with the cache rendered so the item still resolves.
    if db_row.is_none() {
        tracing::warn!(
            ticket = %w.ticket_key,
            "work item in cache has no work_items row; rendering durable fields from the cache (transitional)"
        );
    }
    let (
        ticket_summary,
        ticket_description,
        ticket_type,
        started_at_rfc,
        updated_at_rfc,
        branch_name,
        pr_url,
        pr_merged,
    ) = match &db_row {
        Some(r) => (
            r.ticket_summary.clone().unwrap_or_default(),
            r.ticket_description.clone().unwrap_or_default(),
            r.ticket_type.clone().unwrap_or_default(),
            unix_seconds_to_rfc3339(r.started_at),
            unix_seconds_to_rfc3339(r.updated_at),
            r.branch_name.clone().unwrap_or_default(),
            r.pr_url.clone(),
            r.pr_merged,
        ),
        None => (
            w.ticket_summary.clone(),
            w.ticket_description.clone(),
            w.ticket_type.clone(),
            w.started_at.to_rfc3339(),
            w.updated_at.to_rfc3339(),
            w.branch_name.clone(),
            w.pr_url.clone(),
            w.pr_merged,
        ),
    };
    // Authoritative step count of the latest completed flow (terminal workflows
    // only; in-progress bars come from the in-memory estimate). Keyed by the
    // rendered workflow's own id, which equals its work_items.id.
    let completed_steps: Option<u32> = match auth_state.db.as_ref() {
        Some(database) if w.state.is_terminal() => {
            takuto_core::db::work_items::count_steps_of_latest_completed_def(
                database.adapter(),
                std::slice::from_ref(&w.id),
            )
            .await
            .unwrap_or_default()
            .get(&w.id)
            .copied()
        }
        _ => None,
    };
    let progress = dashboard_progress::progress_fields(w, &cfg, completed_steps);
    // Resolve repo presence only for parked items (drives `prep_state`).
    let repo_ready = if can_start_workflow(w) {
        repo_available_db(auth_state.db.as_ref(), w).await
    } else {
        true
    };
    Ok(Json(WorkflowSummary {
        id: w.id.clone(),
        ticket_key: w.ticket_key.clone(),
        ticket_summary,
        ticket_description,
        ticket_type,
        state: w.status_display(),
        started_at: started_at_rfc,
        updated_at: updated_at_rfc,
        branch_name,
        pr_url,
        pr_merged,
        steps_log: w.steps_log.clone(),
        error: extract_error(&w.state),
        terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
        can_address_pr_comments,
        can_merge_base,
        can_mark_done,
        can_delete: !w.state.is_active() || can_start_workflow(w),
        can_start: can_start_workflow(w),
        progress_percent: progress.0,
        progress_steps_total: progress.1,
        started_manually,
        counts_toward_manual_cap,
        jira_browse_url: ticket_browse_url(&cfg.jira.site, &w.ticket_key),
        issue_url: build_issue_url(w, &cfg.jira.site),
        can_open_editor: can_open_editor(w),
        editor_url: editor_info.as_ref().map(|e| e.url.clone()),
        editor_port_mappings: port_mappings,
        jira_available: w.jira_available,
        ticketing_system: w.ticketing_system.to_string(),
        can_resume_from_error: can_resume_from_error(w),
        terminal_url: {
            let ttyd_token = editor
                .terminal_ports
                .read()
                .await
                .get(&ticket_key)
                .map(|(_port, token)| token.clone());
            match ttyd_token {
                Some(t) => editor
                    .path_token_registry
                    .find_token_for(&ticket_key, SessionRouteKind::Terminal)
                    .await
                    .map(|pt| container::build_session_terminal_url(&pt, &t)),
                None => None,
            }
        },
        run_commands: {
            // Per-user-per-workspace lookup of configured run commands.
            // Owner-less workflows, or workflows whose owner has no row, get
            // an empty list (no buttons rendered on the card).
            let configured: Vec<takuto_core::db::user_worktree_commands::RunCommand> =
                match (w.user_id.as_deref(), auth_state.db.as_ref()) {
                    (Some(uid), Some(database)) => takuto_core::db::user_worktree_commands::get(
                        database.adapter(),
                        uid,
                        &w.workspace_name,
                    )
                    .await
                    .ok()
                    .flatten()
                    .map(|r| r.run_commands)
                    .unwrap_or_default(),
                    _ => Vec::new(),
                };
            let run_cmds_state = run_command.run_commands.read().await;
            build_run_commands_status(&configured, run_cmds_state.get(&ticket_key))
        },
        generate_report: cfg.general.generate_report,
        has_report: has_report_file(w),
        workflow_def_runs: workflow_def_runs_display(w),
        worktree_path: match &db_row {
            Some(r) => r
                .worktree_path
                .as_deref()
                .map(std::path::Path::new)
                .filter(|p| p.exists())
                .and_then(|p| p.to_str().map(str::to_string)),
            None => w
                .worktree_path
                .as_ref()
                .filter(|p| p.exists())
                .and_then(|p| p.to_str().map(str::to_string)),
        },
        user_id: w.user_id.clone(),
        workspace_name: w.workspace_name.clone(),
        repository_id: w.repository_id.clone(),
        prep_state: prep_state(w, repo_ready).map(str::to_string),
    }))
}

/// Return the generated report markdown for a workflow (from `lore/reports/<key>_report.md` in the worktree).
///
/// **DB is the primary source for `worktree_path` and `ticket_key`.**
/// The route already gated access via `require_workflow_access`, so
/// we read the full row without an additional visibility filter. The
/// HashMap fallback covers workflows that pre-date the shadow-write.
pub async fn get_workflow_report(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<WorkflowReportResponse>, StatusCode> {
    require_workflow_access(&engine, &auth_state, &auth, &id).await?;

    // ── DB-first read of (worktree_path, ticket_key) ────────────
    let mut resolved: Option<(std::path::PathBuf, String)> = None;
    if let Some(database) = auth_state.db.as_ref() {
        match takuto_core::db::work_items::get_work_item_by_ticket_key(database.adapter(), &id)
            .await
        {
            Ok(Some(row)) => {
                let Some(wt) = row.worktree_path else {
                    return Err(StatusCode::NOT_FOUND);
                };
                resolved = Some((std::path::PathBuf::from(wt), row.ticket_key));
            }
            Ok(None) => { /* fall through to HashMap */ }
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        }
    }

    // ── Legacy HashMap fallback ────────────────────────────────
    let (worktree_path, ticket_key) = match resolved {
        Some(v) => v,
        None => {
            let wf_arc = engine.engine.workflows_arc();
            let workflows = wf_arc.read().await;
            let w = workflows.get(&id).ok_or(StatusCode::NOT_FOUND)?;
            let wt = w
                .worktree_path
                .as_ref()
                .ok_or(StatusCode::NOT_FOUND)?
                .clone();
            (wt, w.ticket_key.clone())
        }
    };

    let report_path = worktree_path.join(format!("lore/reports/{ticket_key}_report.md"));
    if !report_path.exists() {
        return Err(StatusCode::NOT_FOUND);
    }

    let content =
        std::fs::read_to_string(&report_path).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(WorkflowReportResponse { content }))
}

#[derive(Serialize)]
pub struct WorkflowReportResponse {
    pub content: String,
}
