// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Read-only workflow list / get endpoints + report retrieval.

use std::collections::HashMap;

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use maestro_core::container;
use maestro_core::jira::ticket_browse_url;
use maestro_core::workflow::dashboard_progress;
use maestro_core::workflow::engine::Workflow;
use maestro_core::workflow::state::WorkflowState;

use crate::auth::AuthenticatedUser;
use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::{AppState, DynamicPortForward};

use super::dto::{
    TerminalLineDto, WorkflowCountsResponse, WorkflowSummary, build_issue_url,
    build_run_commands_status, can_open_editor, can_resume_from_error, can_start_workflow,
    extract_error, has_report_file, manual_cap_fields, workflow_action_flags,
    workflow_def_runs_display,
};
use super::require_workflow_access;

pub async fn list_workflows(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<Vec<WorkflowSummary>> {
    let cfg = state.config.config.read().await;
    // Plan-10: workflow visibility is gated by the caller's `user_repositories`
    // associations. Build two HashSets in ONE batched query so the in-memory
    // filter below is O(1) per workflow.
    let (allowed_repo_ids, allowed_repo_names): (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) = if let Some(database) = state.auth.db.as_ref() {
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
    let no_db = state.auth.db.is_none();
    let wf_arc = state.engine.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let dyn_fwd = state.editor.dynamic_forwards.read().await;
    let run_cmds_state = state.run_command.run_commands.read().await;
    // Build terminal URLs via the path-token registry so the frontend uses
    // the `/s/<token>/...` proxy path instead of a direct `localhost:<port>` URL.
    // Single-pass bulk lookup: acquire the registry read lock once rather than
    // N times in a serial loop (avoids O(K²) linear scans at scale).
    let terminal_ports_snap: Vec<(String, String)> = state
        .editor
        .terminal_ports
        .read()
        .await
        .iter()
        .map(|(k, (_port, ttyd_token))| (k.clone(), ttyd_token.clone()))
        .collect();
    let terminal_urls: HashMap<String, String> = {
        let registry = state.editor.path_token_registry.inner_read().await;
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
    > = match (pairs.is_empty(), state.auth.db.as_ref()) {
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
    let wf_arc = state.engine.engine.workflows_arc();
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
    let cfg = state.config.config.read().await;
    // Plan-10: visibility is gated by `require_workflow_access`, which checks
    // both user_id ownership AND repo association. The legacy
    // workspace_name-equals-current-workspace check is dropped because there
    // is no longer a single "current workspace" — every workflow knows its
    // own repo.
    require_workflow_access(&state, &auth, &id).await?;
    let wf_arc = state.engine.engine.workflows_arc();
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
    let dyn_fwd = state.editor.dynamic_forwards.read().await;
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
            let path_token = state.editor.path_token_registry.register(SessionRoute {
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
            let mut fwd = state.editor.dynamic_forwards.write().await;
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
                .editor
                .terminal_ports
                .read()
                .await
                .get(&ticket_key)
                .map(|(_port, token)| token.clone());
            match ttyd_token {
                Some(t) => state
                    .editor
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
                match (w.user_id.as_deref(), state.auth.db.as_ref()) {
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
            let run_cmds_state = state.run_command.run_commands.read().await;
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
    let wf_arc = state.engine.engine.workflows_arc();
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
