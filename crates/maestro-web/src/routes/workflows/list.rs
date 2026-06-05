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
use crate::state::{
    AuthState, ConfigState, DynamicPortForward, EditorState, EngineState, RunCommandState,
};

use super::dto::{
    TerminalLineDto, WorkflowCountsResponse, WorkflowSummary, build_issue_url,
    build_run_commands_status, can_open_editor, can_resume_from_error, can_start_workflow,
    extract_error, has_report_file, manual_cap_fields, workflow_action_flags,
    workflow_def_runs_display,
};
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
    let (allowed_repo_ids, allowed_repo_names): (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ) = if let Some(database) = auth_state.db.as_ref() {
        match maestro_core::db::repositories::list_for_user(database.adapter(), &auth.user_id).await
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
    let db_rows: HashMap<String, maestro_core::db::work_items::WorkItemRow> =
        if let Some(database) = auth_state.db.as_ref() {
            match maestro_core::db::work_items::list_work_items(
                database.adapter(),
                &maestro_core::db::work_items::WorkItemListQuery {
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
                Ok(rows) => rows
                    .into_iter()
                    .map(|r| (r.ticket_key.clone(), r))
                    .collect(),
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

    let run_commands_by_pair: HashMap<
        (String, String),
        Vec<maestro_core::db::user_worktree_commands::RunCommand>,
    > = match (pairs.is_empty(), auth_state.db.as_ref()) {
        (false, Some(database)) => {
            let pair_refs: Vec<(&str, &str)> = pairs
                .iter()
                .map(|(u, w)| (u.as_str(), w.as_str()))
                .collect();
            maestro_core::db::user_worktree_commands::get_run_commands_for_pairs(
                database.adapter(),
                &pair_refs,
            )
            .await
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
                .map(|forwards| {
                    forwards
                        .iter()
                        .map(|f| (f.container_port, f.proxy_url.clone()))
                        .collect()
                })
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
            // When a work_items row exists, prefer its values for
            // every shadow-written scalar field. Engine-derived
            // fields (state display, action flags, in-memory caches)
            // stay on the Workflow path because they have no DB
            // equivalent.
            let row = db_rows.get(&w.ticket_key);
            let ticket_summary = match row {
                Some(r) => r.ticket_summary.clone().unwrap_or_default(),
                None => w.ticket_summary.clone(),
            };
            let ticket_description = match row {
                Some(r) => r.ticket_description.clone().unwrap_or_default(),
                None => w.ticket_description.clone(),
            };
            let ticket_type = match row {
                Some(r) => r.ticket_type.clone().unwrap_or_default(),
                None => w.ticket_type.clone(),
            };
            let started_at_rfc = match row {
                Some(r) => unix_seconds_to_rfc3339(r.started_at),
                None => w.started_at.to_rfc3339(),
            };
            let updated_at_rfc = match row {
                Some(r) => unix_seconds_to_rfc3339(r.updated_at),
                None => w.updated_at.to_rfc3339(),
            };
            // Prefer the DB shadow-row, but fall back to the in-memory
            // value when the DB column is empty/null. The shadow-write to
            // `work_items` can lag behind the engine (e.g. FK violations on
            // a sibling write block the row update entirely), and serving
            // the stale empty value hides PR links and branch names on the
            // dashboard for runs that actually completed cleanly in memory.
            let branch_name = row
                .and_then(|r| {
                    r.branch_name
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .unwrap_or_else(|| w.branch_name.clone());
            let pr_url = row
                .and_then(|r| {
                    r.pr_url
                        .as_deref()
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                })
                .or_else(|| w.pr_url.clone());
            // pr_merged is a flag, not a value, so true OR'd from either source.
            let pr_merged = row.map(|r| r.pr_merged).unwrap_or(false) || w.pr_merged;
            // Worktree path filtered by on-disk existence — a path
            // recorded in the row whose directory has been removed
            // is no longer useful to render.
            let worktree_path = match row {
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
                worktree_path,
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
/// Counts come from a DB GROUP-BY when a row exists for a given
/// ticket_key, falling back to the HashMap for legacy workflows that
/// have not yet been backfilled. The two sources are merged by
/// ticket_key (DB wins) so the same workflow is never double-counted
/// during the transition.
pub async fn workflow_counts(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<WorkflowCountsResponse> {
    use maestro_core::db::work_items::WorkItemStateKind;
    use std::collections::HashMap;

    let mut counted: HashMap<String, WorkItemStateKind> = HashMap::new();

    // ── DB primary ─────────────────────────────────────────────
    if let Some(database) = auth_state.db.as_ref()
        && let Ok(rows) =
            maestro_core::db::work_items::list_user_state_kinds(database.adapter(), &auth.user_id)
                .await
    {
        for (ticket_key, kind) in rows {
            counted.insert(ticket_key, kind);
        }
    }

    // ── HashMap fallback: only entries the DB didn't already
    //    cover. Legacy workflows still live only in memory.
    {
        let wf_arc = engine.engine.workflows_arc();
        let workflows = wf_arc.read().await;
        for w in workflows.values() {
            if w.user_id.as_deref() != Some(&auth.user_id) {
                continue;
            }
            if counted.contains_key(&w.ticket_key) {
                continue;
            }
            let kind = match &w.state {
                WorkflowState::Pending => WorkItemStateKind::Pending,
                WorkflowState::Assigning => WorkItemStateKind::Assigning,
                WorkflowState::RetrievingDetails => WorkItemStateKind::RetrievingDetails,
                WorkflowState::CreatingWorktree => WorkItemStateKind::CreatingWorktree,
                WorkflowState::AddressingTicket { .. } => WorkItemStateKind::AddressingTicket,
                WorkflowState::AddressingPrComments { .. } => {
                    WorkItemStateKind::AddressingPrComments
                }
                WorkflowState::MergingBaseBranch { .. } => WorkItemStateKind::MergingBaseBranch,
                WorkflowState::Reviewing => WorkItemStateKind::Reviewing,
                WorkflowState::CreatingPR => WorkItemStateKind::CreatingPr,
                WorkflowState::Done => WorkItemStateKind::Done,
                WorkflowState::Stopped => WorkItemStateKind::Stopped,
                WorkflowState::Error { .. } => WorkItemStateKind::Error,
                WorkflowState::Paused { .. } => WorkItemStateKind::Paused,
            };
            counted.insert(w.ticket_key.clone(), kind);
        }
    }

    let mut running = 0u32;
    let mut completed = 0u32;
    let mut errors = 0u32;
    let mut paused = 0u32;
    for kind in counted.values() {
        match kind {
            WorkItemStateKind::Done => completed += 1,
            WorkItemStateKind::Error | WorkItemStateKind::Stopped => errors += 1,
            WorkItemStateKind::Paused => paused += 1,
            // Pending hasn't started yet — don't count, matching
            // the legacy HashMap-only behaviour.
            WorkItemStateKind::Pending => {}
            // Every active driver state counts as "running" from
            // the dashboard's perspective.
            WorkItemStateKind::Assigning
            | WorkItemStateKind::RetrievingDetails
            | WorkItemStateKind::CreatingWorktree
            | WorkItemStateKind::AddressingTicket
            | WorkItemStateKind::AddressingPrComments
            | WorkItemStateKind::MergingBaseBranch
            | WorkItemStateKind::Reviewing
            | WorkItemStateKind::CreatingPr => running += 1,
        }
    }
    Json(WorkflowCountsResponse {
        running,
        completed,
        errors,
        paused,
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
    let db_row: Option<maestro_core::db::work_items::WorkItemRow> =
        if let Some(database) = auth_state.db.as_ref() {
            match maestro_core::db::work_items::get_work_item_by_ticket_key(database.adapter(), &id)
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
    // Maestro process started (server restart).
    let dyn_fwd = editor.dynamic_forwards.read().await;
    let port_mappings: Vec<(u16, String)> = if let Some(forwards) = dyn_fwd.get(&ticket_key) {
        forwards
            .iter()
            .map(|f| (f.container_port, f.proxy_url.clone()))
            .collect()
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
    let ticket_summary = match &db_row {
        Some(r) => r.ticket_summary.clone().unwrap_or_default(),
        None => w.ticket_summary.clone(),
    };
    let ticket_description = match &db_row {
        Some(r) => r.ticket_description.clone().unwrap_or_default(),
        None => w.ticket_description.clone(),
    };
    let ticket_type = match &db_row {
        Some(r) => r.ticket_type.clone().unwrap_or_default(),
        None => w.ticket_type.clone(),
    };
    let started_at_rfc = match &db_row {
        Some(r) => unix_seconds_to_rfc3339(r.started_at),
        None => w.started_at.to_rfc3339(),
    };
    let updated_at_rfc = match &db_row {
        Some(r) => unix_seconds_to_rfc3339(r.updated_at),
        None => w.updated_at.to_rfc3339(),
    };
    // See the list endpoint above: prefer a non-empty DB value, but fall
    // back to the in-memory workflow when the shadow-row lags behind.
    let branch_name = db_row
        .as_ref()
        .and_then(|r| {
            r.branch_name
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_else(|| w.branch_name.clone());
    let pr_url = db_row
        .as_ref()
        .and_then(|r| {
            r.pr_url
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(str::to_string)
        })
        .or_else(|| w.pr_url.clone());
    let pr_merged = db_row.as_ref().map(|r| r.pr_merged).unwrap_or(false) || w.pr_merged;
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
            let configured: Vec<maestro_core::db::user_worktree_commands::RunCommand> =
                match (w.user_id.as_deref(), auth_state.db.as_ref()) {
                    (Some(uid), Some(database)) => maestro_core::db::user_worktree_commands::get(
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
        match maestro_core::db::work_items::get_work_item_by_ticket_key(database.adapter(), &id)
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
