// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedUser;
use tokio_util::sync::CancellationToken;

use maestro_core::container::{self, ContainerRunner};
use maestro_core::jira::ticket_browse_url;
use maestro_core::workflow::dashboard_progress;
use maestro_core::workflow::definitions::{DiscoveredWorkflow, discover_workflows};
use maestro_core::workflow::engine::Workflow;
use maestro_core::workflow::state::WorkflowState;

use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::{AppState, DynamicPortForward};

mod dto;
mod lifecycle;
mod port_tracking;

pub use dto::{
    RunCommandStatus, TerminalLineDto, WorkflowCountsResponse, WorkflowSummary,
};
use dto::{
    build_issue_url, build_run_commands_status, can_open_editor, can_resume_from_error,
    can_start_workflow, extract_error, has_report_file, manual_cap_fields, workflow_action_flags,
    workflow_def_runs_display,
};
pub use lifecycle::{
    delete_workflow, mark_work_done, pause_workflow, resume_from_error, resume_workflow,
    retry_workflow, stop_workflow,
};
pub use port_tracking::track_port_forwards;
use port_tracking::run_command_port_tracker;

/// Check whether the authenticated user may act on the workflow with the given ticket key.
/// Users can only act on workflows they created.
///
/// Exposed `pub(crate)` so the ticket-action endpoints (`routes/tickets.rs`)
/// can reuse the same NOT_FOUND-on-mismatch convention (AC-2).
pub(crate) async fn require_workflow_access(
    state: &AppState,
    auth: &AuthenticatedUser,
    ticket_key: &str,
) -> Result<(), StatusCode> {
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows.get(ticket_key).ok_or(StatusCode::NOT_FOUND)?;
    if w.user_id.as_deref() != Some(&auth.user_id) {
        return Err(StatusCode::NOT_FOUND);
    }
    // Plan-10: workflow's repository must be one the caller has added.
    // Defensive back-compat: when `repository_id` is `None`, fall back to
    // matching `workspace_name` against the user's repo names. Without a DB
    // attached (test paths), skip the gate entirely.
    let Some(database) = state.db.as_ref() else {
        return Ok(());
    };
    let workflow_repo_id = w.repository_id.clone();
    let workflow_workspace = w.workspace_name.clone();
    drop(workflows);

    let conn = database.conn().lock().await;
    let repos = match maestro_core::db::repositories::list_for_user(&conn, &auth.user_id) {
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


pub async fn list_workflows(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<Vec<WorkflowSummary>> {
    let cfg = state.config.read().await;
    // Plan-10: workflow visibility is gated by the caller's `user_repositories`
    // associations. Build two HashSets in ONE batched query so the in-memory
    // filter below is O(1) per workflow.
    let (allowed_repo_ids, allowed_repo_names): (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) = if let Some(database) = state.db.as_ref() {
        let conn = database.conn().lock().await;
        match maestro_core::db::repositories::list_for_user(&conn, &auth.user_id) {
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
    let no_db = state.db.is_none();
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let dyn_fwd = state.dynamic_forwards.read().await;
    let run_cmds_state = state.run_commands.read().await;
    // Build terminal URLs via the path-token registry so the frontend uses
    // the `/s/<token>/...` proxy path instead of a direct `localhost:<port>` URL.
    // Single-pass bulk lookup: acquire the registry read lock once rather than
    // N times in a serial loop (avoids O(K²) linear scans at scale).
    let terminal_ports_snap: Vec<(String, String)> = state
        .terminal_ports
        .read()
        .await
        .iter()
        .map(|(k, (_port, ttyd_token))| (k.clone(), ttyd_token.clone()))
        .collect();
    let terminal_urls: HashMap<String, String> = {
        let registry = state.path_token_registry.inner_read().await;
        terminal_ports_snap
            .iter()
            .filter_map(|(ticket_key, ttyd_token)| {
                registry
                    .iter()
                    .find(|(_, r)| r.ticket_key == *ticket_key && r.kind == SessionRouteKind::Terminal)
                    .map(|(path_token, _)| {
                        (
                            ticket_key.clone(),
                            container::build_session_terminal_url(path_token, ttyd_token),
                        )
                    })
            })
            .collect()
    };
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
    let run_commands_by_pair: HashMap<
        (String, String),
        Vec<maestro_core::db::user_worktree_commands::RunCommand>,
    > = match (pairs.is_empty(), state.db.as_ref()) {
        (false, Some(database)) => {
            let conn = database.conn().lock().await;
            let pair_refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(u, w)| (u.as_str(), w.as_str()))
                .collect();
            maestro_core::db::user_worktree_commands::get_run_commands_for_pairs(&conn, &pair_refs)
                .unwrap_or_default()
        }
        _ => HashMap::new(),
    };
    let empty_run_cmds: Vec<maestro_core::db::user_worktree_commands::RunCommand> = Vec::new();
    let mut summaries: Vec<WorkflowSummary> = visible_workflows
        .into_iter()
        .map(|w| {
            let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
            let (started_manually, counts_toward_manual_cap) = manual_cap_fields(w);
            // Use the server-side dynamic-forwards cache so that port buttons
            // appear immediately on page load (no per-workflow Docker call).
            let port_mappings: Vec<(u16, String)> = dyn_fwd
                .get(&w.ticket_key)
                .map(|forwards| forwards.iter().map(|f| (f.container_port, f.proxy_url.clone())).collect())
                .unwrap_or_default();
            let configured_run_cmds: &[maestro_core::db::user_worktree_commands::RunCommand] =
                match w.user_id.as_deref() {
                    Some(uid) => run_commands_by_pair
                        .get(&(uid.to_string(), w.workspace_name.clone()))
                        .map(|v| v.as_slice())
                        .unwrap_or(&empty_run_cmds),
                    None => &empty_run_cmds,
                };
            let run_commands =
                build_run_commands_status(configured_run_cmds, run_cmds_state.get(&w.ticket_key));
            WorkflowSummary {
                id: w.id.clone(),
                ticket_key: w.ticket_key.clone(),
                ticket_summary: w.ticket_summary.clone(),
                ticket_description: w.ticket_description.clone(),
                ticket_type: w.ticket_type.clone(),
                state: w.status_display(),
                started_at: w.started_at.to_rfc3339(),
                updated_at: w.updated_at.to_rfc3339(),
                branch_name: w.branch_name.clone(),
                pr_url: w.pr_url.clone(),
                pr_merged: w.pr_merged,
                steps_log: w.steps_log.clone(),
                error: extract_error(&w.state),
                terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
                can_address_pr_comments,
                can_merge_base,
                can_mark_done,
                can_delete: !w.state.is_active() || can_start_workflow(w),
                can_start: can_start_workflow(w),
                progress_percent: dashboard_progress::workflow_progress_percent(w, &cfg),
                progress_steps_total: dashboard_progress::estimated_step_total(w, &cfg),
                started_manually,
                counts_toward_manual_cap,
                jira_browse_url: ticket_browse_url(&cfg.jira.site, &w.ticket_key),
                issue_url: build_issue_url(w, &cfg.jira.site),
                can_open_editor: can_open_editor(w),
                editor_url: None,
                editor_port_mappings: port_mappings,
                jira_available: w.jira_available,
                ticketing_system: w.ticketing_system.to_string(),
                can_resume_from_error: can_resume_from_error(w),
                terminal_url: terminal_urls.get(&w.ticket_key).cloned(),
                run_commands,
                generate_report: cfg.general.generate_report,
                has_report: has_report_file(w),
                workflow_def_runs: workflow_def_runs_display(w),
                worktree_path: w
                    .worktree_path
                    .as_ref()
                    .filter(|p| p.exists())
                    .and_then(|p| p.to_str().map(str::to_string)),
                user_id: w.user_id.clone(),
                workspace_name: w.workspace_name.clone(),
                repository_id: w.repository_id.clone(),
            }
        })
        .collect();
    // Oldest first — matches dashboard stable card order (new workflows last).
    summaries.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Json(summaries)
}

/// Per-user workflow counts for the dashboard summary bar.
pub async fn workflow_counts(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<WorkflowCountsResponse> {
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let mut running = 0u32;
    let mut completed = 0u32;
    let mut errors = 0u32;
    let mut paused = 0u32;
    for w in workflows.values() {
        if w.user_id.as_deref() != Some(&auth.user_id) {
            continue;
        }
        match &w.state {
            WorkflowState::Done => completed += 1,
            WorkflowState::Error { .. } | WorkflowState::Stopped => errors += 1,
            WorkflowState::Paused { .. } => paused += 1,
            WorkflowState::Pending => {} // Not yet running — don't count
            _ => running += 1,
        }
    }
    Json(WorkflowCountsResponse { running, completed, errors, paused })
}

pub async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<WorkflowSummary>, StatusCode> {
    let cfg = state.config.read().await;
    // Plan-10: visibility is gated by `require_workflow_access`, which checks
    // both user_id ownership AND repo association. The legacy
    // workspace_name-equals-current-workspace check is dropped because there
    // is no longer a single "current workspace" — every workflow knows its
    // own repo.
    require_workflow_access(&state, &auth, &id).await?;
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
    let (started_manually, counts_toward_manual_cap) = manual_cap_fields(w);
    let ticket_key = w.ticket_key.clone();
    let editor_info = container::get_editor_info(&ticket_key).await;
    // Prefer the server-side dynamic-forwards cache (includes both static Docker
    // mappings seeded at open-editor time and dynamically-detected socat forwards).
    // Fall back to Docker-queried port mappings for editors opened before this
    // Maestro process started (server restart).
    let dyn_fwd = state.dynamic_forwards.read().await;
    let port_mappings: Vec<(u16, String)> = if let Some(forwards) = dyn_fwd.get(&ticket_key) {
        forwards.iter().map(|f| (f.container_port, f.proxy_url.clone())).collect()
    } else {
        // Fallback: editor opened before this Maestro process started (restart
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
            let path_token = state.path_token_registry.register(SessionRoute {
                kind: SessionRouteKind::DynamicPort,
                host_port: *hp,
                ticket_key: ticket_key.clone(),
                user_id: auth.user_id.clone(),
            }).await;
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
            let mut fwd = state.dynamic_forwards.write().await;
            fwd.entry(ticket_key.clone()).or_insert(entries);
        }
        result
    };
    Ok(Json(WorkflowSummary {
        id: w.id.clone(),
        ticket_key: w.ticket_key.clone(),
        ticket_summary: w.ticket_summary.clone(),
        ticket_description: w.ticket_description.clone(),
        ticket_type: w.ticket_type.clone(),
        state: w.status_display(),
        started_at: w.started_at.to_rfc3339(),
        updated_at: w.updated_at.to_rfc3339(),
        branch_name: w.branch_name.clone(),
        pr_url: w.pr_url.clone(),
        pr_merged: w.pr_merged,
        steps_log: w.steps_log.clone(),
        error: extract_error(&w.state),
        terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
        can_address_pr_comments,
        can_merge_base,
        can_mark_done,
        can_delete: !w.state.is_active() || can_start_workflow(w),
        can_start: can_start_workflow(w),
        progress_percent: dashboard_progress::workflow_progress_percent(w, &cfg),
        progress_steps_total: dashboard_progress::estimated_step_total(w, &cfg),
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
            let ttyd_token = state
                .terminal_ports
                .read()
                .await
                .get(&ticket_key)
                .map(|(_port, token)| token.clone());
            match ttyd_token {
                Some(t) => state
                    .path_token_registry
                    .find_token_for(&ticket_key, SessionRouteKind::Terminal)
                    .await
                    .map(|pt| container::build_session_terminal_url(&pt, &t)),
                None => None,
            }
        },
        run_commands: {
            // Per-user-per-workspace lookup of configured run commands (plan-09).
            // Owner-less workflows, or workflows whose owner has no row, get an
            // empty list (no buttons rendered on the card).
            let configured: Vec<maestro_core::db::user_worktree_commands::RunCommand> =
                match (w.user_id.as_deref(), state.db.as_ref()) {
                    (Some(uid), Some(database)) => {
                        let conn = database.conn().lock().await;
                        maestro_core::db::user_worktree_commands::get(
                            &conn,
                            uid,
                            &w.workspace_name,
                        )
                        .ok()
                        .flatten()
                        .map(|r| r.run_commands)
                        .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
            let run_cmds_state = state.run_commands.read().await;
            build_run_commands_status(&configured, run_cmds_state.get(&ticket_key))
        },
        generate_report: cfg.general.generate_report,
        has_report: has_report_file(w),
        workflow_def_runs: workflow_def_runs_display(w),
        worktree_path: w
            .worktree_path
            .as_ref()
            .filter(|p| p.exists())
            .and_then(|p| p.to_str().map(str::to_string)),
        user_id: w.user_id.clone(),
        workspace_name: w.workspace_name.clone(),
        repository_id: w.repository_id.clone(),
    }))
}


/// Return the generated report markdown for a workflow (from `lore/reports/<key>_report.md` in the worktree).
pub async fn get_workflow_report(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<WorkflowReportResponse>, StatusCode> {
    require_workflow_access(&state, &auth, &id).await?;
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    let worktree_path = w
        .worktree_path
        .as_ref()
        .ok_or(StatusCode::NOT_FOUND)?
        .clone();
    let ticket_key = w.ticket_key.clone();
    drop(workflows);

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

#[derive(Deserialize)]
pub struct StartManualWorkflowBody {
    pub ticket_key: String,
    pub ticket_summary: String,
    /// Optional ticket description (used when Jira is unavailable and the user pastes the description).
    #[serde(default)]
    pub ticket_description: Option<String>,
    /// Direct URL to the issue in the ticketing system (e.g. GitHub issue `html_url`).
    /// Used so clicking the issue key on the dashboard opens the correct URL for GitHub workflows.
    #[serde(default)]
    pub issue_url: Option<String>,
    /// Plan-10: id of a `repositories` row the caller has added. When omitted,
    /// the server picks the caller's most-recently-added repo (or rejects when
    /// the caller has none).
    #[serde(default)]
    pub repository_id: Option<String>,
}

#[derive(Serialize)]
pub struct StartManualWorkflowResponse {
    pub workflow_id: String,
    pub ticket_key: String,
}

/// Start a ticket workflow from the dashboard (same pipeline as the poller). Respects **`[general] max_concurrent_manual_workflows`**.
///
/// When Jira is unavailable (`jira_available = false`), `ticket_key` may be empty — a synthetic
/// `MANUAL-{timestamp}` key is generated. The `ticket_description` field is stored on the workflow
/// so the agent prompt can use it.
pub async fn start_manual_workflow(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<StartManualWorkflowBody>,
) -> Result<Json<StartManualWorkflowResponse>, (StatusCode, String)> {
    let jira_on = state
        .jira_available
        .load(std::sync::atomic::Ordering::Relaxed);

    let ticket_key = {
        let k = body.ticket_key.trim().to_string();
        if k.is_empty() {
            if jira_on {
                return Err((StatusCode::BAD_REQUEST, "ticket_key is required".into()));
            }
            // Auto-generate a synthetic key when Jira is unavailable.
            format!("MANUAL-{}", chrono::Utc::now().timestamp_millis())
        } else {
            k
        }
    };
    let ticket_summary = {
        let s = body.ticket_summary.trim();
        if s.is_empty() {
            if jira_on {
                ticket_key.clone()
            } else {
                "Manual item".to_string()
            }
        } else {
            s.to_string()
        }
    };

    let max_manual = {
        let cfg = state.config.read().await;
        if jira_on && cfg.jira.project_keys.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "No Jira project keys configured".into(),
            ));
        }
        cfg.general.max_concurrent_manual_workflows
    };

    {
        let wf_arc = state.engine.workflows_arc();
        let map = wf_arc.read().await;
        if let Some(existing) = map.get(&ticket_key) {
            // Terminal-state entries (Done / Stopped / Error) are safe to replace —
            // the user is starting fresh on the same ticket. Replacement also recovers
            // from "orphan" rows (user_id = None) carried over from pre-plan-01 snapshots:
            // those rows are invisible to the caller (per-user isolation), so without
            // this branch they would be undeletable zombies blocking the re-add.
            let terminal = matches!(
                existing.state,
                WorkflowState::Done | WorkflowState::Stopped | WorkflowState::Error { .. }
            );
            if !terminal {
                return Err((
                    StatusCode::CONFLICT,
                    format!("An item already exists for {ticket_key}"),
                ));
            }
            tracing::info!(
                ticket = %ticket_key,
                prev_state = %existing.state,
                prev_owner = ?existing.user_id,
                new_owner = %auth.user_id,
                "Replacing terminal-state workflow with a fresh add"
            );
        }
    }

    if max_manual > 0 {
        // Count per-user, not global.
        let wf_arc = state.engine.workflows_arc();
        let map = wf_arc.read().await;
        let n = map
            .values()
            .filter(|w| w.user_id.as_deref() == Some(&auth.user_id))
            .filter(|w| w.started_manually && w.state.occupies_concurrency_slot())
            .count();
        if n >= max_manual as usize {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Maximum concurrent manual items ({max_manual}) reached; complete, stop, or delete a manual item first"
                ),
            ));
        }
    }

    let description = body
        .ticket_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let issue_url = body
        .issue_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Plan-10: resolve the workflow's repository_id. When the body specifies
    // one, validate the caller has it associated; otherwise, default to the
    // most-recently-added repo. Reject when the caller has zero repos.
    let repository_id = if let Some(database) = state.db.as_ref() {
        let conn = database.conn().lock().await;
        let user_repos = maestro_core::db::repositories::list_for_user(&conn, &auth.user_id)
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if user_repos.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Add a repository before starting an item.".into(),
            ));
        }
        let chosen_id = match body
            .repository_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(requested) => {
                if !user_repos.iter().any(|r| r.id == requested) {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "You do not have access to that repository".into(),
                    ));
                }
                requested.to_string()
            }
            None => user_repos
                .iter()
                .max_by_key(|r| r.created_at)
                .map(|r| r.id.clone())
                .expect("user_repos non-empty"),
        };
        Some(chosen_id)
    } else {
        // No DB attached (legacy test paths). Fall through with None — the
        // engine will derive workspace_name from cfg.git.repo_path.
        None
    };

    let workflow_id = state
        .engine
        .add_to_dashboard(
            ticket_key.clone(),
            ticket_summary,
            true,
            description,
            issue_url,
            Some(auth.user_id),
            repository_id,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(StartManualWorkflowResponse {
        workflow_id,
        ticket_key,
    }))
}

// ---------------------------------------------------------------------------
// Editor (openvscode-server) endpoints
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Run commands — start/stop user-defined shell commands in dedicated containers
// ---------------------------------------------------------------------------

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
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<RunCommandsStatusResponse>, (StatusCode, String)> {
    let wf_arc = state.engine.workflows_arc();
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
        match (owner_user_id.as_deref(), state.db.as_ref()) {
            (Some(uid), Some(database)) => {
                let conn = database.conn().lock().await;
                maestro_core::db::user_worktree_commands::get(&conn, uid, &workspace_name)
                    .ok()
                    .flatten()
                    .map(|r| r.run_commands)
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };

    let run_cmds_state = state.run_commands.read().await;
    let commands = build_run_commands_status(&configured, run_cmds_state.get(&id));

    Ok(Json(RunCommandsStatusResponse { commands }))
}

/// Start a run command for a workflow.
pub async fn start_run_command(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, usize)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<StartRunCommandResponse>, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;

    // Resolve owner + workspace before opening the DB to keep the workflow
    // read-lock short.
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
    let owner_user_id = w.user_id.clone();
    let workspace_name = w.workspace_name.clone();
    drop(workflows);

    let configured: Vec<maestro_core::db::user_worktree_commands::RunCommand> =
        match (owner_user_id.as_deref(), state.db.as_ref()) {
            (Some(uid), Some(database)) => {
                let conn = database.conn().lock().await;
                maestro_core::db::user_worktree_commands::get(&conn, uid, &workspace_name)
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
        let cfg = state.config.read().await;
        cfg.editor.dynamic_ports
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
        let run_cmds = state.run_commands.read().await;
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
        build_editor_or_run_command_bundle(&state, &id, &auth.user_id).await;

    // Task #42: stash the bundle Arc keyed by (ticket, cmd_index). Same
    // rationale as the editor branch: the run-command container is
    // detached, so the route handler's stack scope can't be the sole
    // owner of the bundle's `TempDir` lifetime.
    if let Some(ref b) = secrets_bundle {
        let mut map = state.run_command_bundles.write().await;
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
        let st = state.clone();
        let key = (ticket_key.clone(), index);
        tokio::spawn(async move {
            st.run_command_bundles.write().await.remove(&key);
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
        let mut run_cmds = state.run_commands.write().await;
        let entry = run_cmds.entry(ticket_key.clone()).or_default();
        entry.push(crate::state::RunCommandState {
            cmd_index: index,
            name: rc_name.clone(),
            scanner_cancel: cancel,
            forwarded_port: None,
        });
    }

    // Start background port scanner for this run command
    let event_tx = state.engine.event_sender();
    let ticket_for_scanner = ticket_key.clone();

    let run_cmds_map = state.run_commands.clone();
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
        state.path_token_registry.clone(),
        state.engine.subscribe(),
        tracker_cancel,
    ));

    Ok(Json(StartRunCommandResponse {
        index,
        name: rc_name,
    }))
}

/// Stop a running run command.
pub async fn stop_run_command(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, usize)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
    let ticket_key = w.ticket_key.clone();
    drop(workflows);

    // Cancel scanner, deregister proxy token, and remove from state
    {
        let mut run_cmds = state.run_commands.write().await;
        if let Some(cmds) = run_cmds.get_mut(&ticket_key) {
            if let Some(pos) = cmds.iter().position(|c| c.cmd_index == index) {
                cmds[pos].scanner_cancel.cancel();
                if let Some(ref fwd) = cmds[pos].forwarded_port {
                    state.path_token_registry.remove(&fwd.path_token).await;
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
    state
        .run_command_bundles
        .write()
        .await
        .remove(&(ticket_key.clone(), index));

    Ok(StatusCode::OK)
}

/// List all discovered workflow definitions from the workflows directory.
pub async fn list_workflow_definitions(
    State(state): State<AppState>,
) -> Json<Vec<DiscoveredWorkflow>> {
    let dir = state.engine.workflows_dir.clone();
    let result = discover_workflows(&dir);
    Json(result.workflows)
}

/// Start a specific workflow definition for a ticket.
pub async fn run_workflow_def(
    State(state): State<AppState>,
    Path((id, def_name)): Path<(String, String)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
        .engine
        .start_workflow_def(&id, &def_name)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Retry a failed workflow definition for a ticket (resets Error -> Idle, then starts).
pub async fn retry_workflow_def(
    State(state): State<AppState>,
    Path((id, def_name)): Path<(String, String)>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
        .engine
        .retry_workflow_def(&id, &def_name)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
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
async fn build_editor_or_run_command_bundle(
    state: &AppState,
    workflow_id_or_ticket_key: &str,
    user_id: &str,
) -> Option<std::sync::Arc<maestro_core::auth::WorkerSecretsBundle>> {
    let resolver = state.git_auth_resolver.as_ref()?;
    let db = state.db.as_ref()?;
    db.master_key()?;
    let cfg_snapshot = state.config.read().await.clone();

    // If the workflow already pinned its credentials, prefer that pin so
    // the editor sees the same row the agent path used.
    let pin = {
        let wf_arc = state.engine.workflows_arc();
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

