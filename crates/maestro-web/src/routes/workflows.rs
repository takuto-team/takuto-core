// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Extension;
use serde::{Deserialize, Serialize};

use crate::auth::AuthenticatedUser;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use maestro_core::container::{self, ContainerRunner};
use maestro_core::jira::ticket_browse_url;
use maestro_core::workflow::dashboard_progress;
use maestro_core::workflow::definitions::{DiscoveredWorkflow, discover_workflows};
use maestro_core::workflow::engine::{MarkDoneOutcome, TerminalLine, Workflow, WorkflowEvent};
use maestro_core::workflow::state::WorkflowState;
use maestro_core::workflow::step::StepLog;

use crate::session_registry::{PathTokenRegistry, SessionRoute, SessionRouteKind};
use crate::state::{AppState, DynamicForwardsMap, DynamicPortForward};

/// Listen on the workflow event broadcast channel and keep the dynamic-forwards
/// map in sync for the given ticket.  Runs until `cancel` fires or the channel
/// closes.
pub async fn track_port_forwards(
    ticket_key: String,
    user_id: String,
    dyn_fwd: DynamicForwardsMap,
    registry: PathTokenRegistry,
    mut rx: broadcast::Receiver<WorkflowEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            msg = rx.recv() => {
                match msg {
                    Ok(evt) if evt.ticket_key == ticket_key => {
                        if evt.event_type == "port_forwarded"
                            && let Some((cp, hp)) = evt.forwarded_port
                        {
                            let mut map = dyn_fwd.write().await;
                            let list = map.entry(ticket_key.clone()).or_default();
                            if !list.iter().any(|f| f.container_port == cp) {
                                let path_token = registry.register(SessionRoute {
                                    kind: SessionRouteKind::DynamicPort,
                                    host_port: hp,
                                    ticket_key: ticket_key.clone(),
                                    user_id: user_id.clone(),
                                }).await;
                                let proxy_url = container::build_session_dynamic_port_url(&path_token);
                                list.push(DynamicPortForward {
                                    container_port: cp,
                                    host_port: hp,
                                    proxy_url,
                                    path_token,
                                });
                            }
                        } else if evt.event_type == "port_unforwarded"
                            && let Some((cp, _)) = evt.forwarded_port
                        {
                            let mut map = dyn_fwd.write().await;
                            if let Some(list) = map.get_mut(&ticket_key)
                                && let Some(pos) = list.iter().position(|f| f.container_port == cp)
                            {
                                let removed = list.remove(pos);
                                registry.remove(&removed.path_token).await;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }
}

/// Track port events for a single run command. Registers the reserved proxy
/// token when a port is detected and cleans up on stop/unforward events.
#[allow(clippy::too_many_arguments)]
async fn run_command_port_tracker(
    ticket_key: String,
    cmd_index: usize,
    user_id: String,
    reserved_token: String,
    proxy_base: String,
    run_cmds_map: crate::state::RunCommandsMap,
    registry: PathTokenRegistry,
    mut rx: broadcast::Receiver<WorkflowEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        if event.ticket_key != ticket_key {
                            continue;
                        }
                        let evt_cmd_index: usize = match event.step_name.as_deref().unwrap_or("").parse() {
                            Ok(i) => i,
                            Err(_) => continue,
                        };
                        if evt_cmd_index != cmd_index {
                            continue;
                        }
                        match event.event_type.as_str() {
                            "run_command_port_forwarded" => {
                                if let Some((cp, hp)) = event.forwarded_port {
                                    registry.register_with_token(
                                        reserved_token.clone(),
                                        SessionRoute {
                                            kind: SessionRouteKind::DynamicPort,
                                            host_port: hp,
                                            ticket_key: ticket_key.clone(),
                                            user_id: user_id.clone(),
                                        },
                                    ).await;
                                    let mut map = run_cmds_map.write().await;
                                    if let Some(cmd) = map.get_mut(&ticket_key)
                                        .and_then(|cmds| cmds.iter_mut().find(|c| c.cmd_index == cmd_index))
                                    {
                                        cmd.forwarded_port = Some(DynamicPortForward {
                                            container_port: cp,
                                            host_port: hp,
                                            proxy_url: proxy_base.clone(),
                                            path_token: reserved_token.clone(),
                                        });
                                    }
                                }
                            }
                            "run_command_port_unforwarded" => {
                                let mut map = run_cmds_map.write().await;
                                if let Some(cmd) = map.get_mut(&ticket_key)
                                    .and_then(|cmds| cmds.iter_mut().find(|c| c.cmd_index == cmd_index))
                                    && let Some((gone_cp, _)) = event.forwarded_port
                                    && cmd.forwarded_port.as_ref().map(|f| f.container_port) == Some(gone_cp)
                                {
                                    if let Some(ref fwd) = cmd.forwarded_port {
                                        registry.remove(&fwd.path_token).await;
                                    }
                                    cmd.forwarded_port = None;
                                }
                            }
                            "run_command_stopped" => {
                                let mut map = run_cmds_map.write().await;
                                if let Some(cmds) = map.get_mut(&ticket_key) {
                                    if let Some(cmd) = cmds.iter().find(|c| c.cmd_index == cmd_index)
                                        && let Some(ref fwd) = cmd.forwarded_port
                                    {
                                        registry.remove(&fwd.path_token).await;
                                    }
                                    cmds.retain(|c| c.cmd_index != cmd_index);
                                    if cmds.is_empty() {
                                        map.remove(&ticket_key);
                                    }
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[derive(Serialize)]
pub struct TerminalLineDto {
    pub text: String,
    pub stream: String,
}

impl From<&TerminalLine> for TerminalLineDto {
    fn from(tl: &TerminalLine) -> Self {
        Self {
            text: tl.text.clone(),
            stream: tl.stream.clone(),
        }
    }
}

#[derive(Serialize)]
pub struct WorkflowSummary {
    pub id: String,
    pub ticket_key: String,
    pub ticket_summary: String,
    pub ticket_description: String,
    pub ticket_type: String,
    pub state: String,
    pub started_at: String,
    pub updated_at: String,
    pub branch_name: String,
    pub pr_url: Option<String>,
    pub pr_merged: bool,
    pub steps_log: Vec<StepLog>,
    pub error: Option<String>,
    pub terminal_lines: Vec<TerminalLineDto>,
    /// **Address PR Comments** is allowed (main flow **Done** and `pr_url` set).
    pub can_address_pr_comments: bool,
    /// **Merge base branch** is allowed (main flow **Done**, `pr_url` set, worktree exists).
    pub can_merge_base: bool,
    /// **Mark as Done** is allowed (workflow state is **Done**).
    pub can_mark_done: bool,
    /// **Delete** is allowed when the workflow is not **running** (`WorkflowState::is_active` is false),
    /// or when the workflow is on the dashboard but the driver has not been started yet.
    pub can_delete: bool,
    /// **Start** is allowed (workflow on dashboard but driver not yet spawned).
    pub can_start: bool,
    /// Step-based progress 0–100 (see `dashboard_progress` in maestro-core).
    pub progress_percent: u8,
    /// Estimated step count for the current phase (discrete progress segments / `N` in `k/N`).
    pub progress_steps_total: u32,
    /// Started via dashboard **+** manual picker.
    pub started_manually: bool,
    /// Counts against **`[general] max_concurrent_manual_workflows`** (manual start and not Done/Stopped/Error).
    pub counts_toward_manual_cap: bool,
    /// Jira **browse** URL from **`[jira] site`** + **`ticket_key`** (dashboard **Go to ticket**).
    pub jira_browse_url: String,
    /// Direct URL to the issue in the ticketing system. For GitHub workflows, this is the
    /// `html_url` stored when the workflow was created. For Jira workflows, it is the
    /// computed browse URL. `None` when no URL is available (e.g. `ticketing_system = none`).
    pub issue_url: Option<String>,
    /// **Open editor** is allowed (workflow not active, worktree exists, Docker available).
    pub can_open_editor: bool,
    /// Set when an editor container is already running for this workflow.
    pub editor_url: Option<String>,
    /// `(container_port, proxy_url)` pairs for user-configured application ports.
    pub editor_port_mappings: Vec<(u16, String)>,
    /// `true` when Jira (acli) was available when this workflow was created.
    pub jira_available: bool,
    /// Which ticketing system was active when this workflow was created: `"jira"`, `"github"`, or `"none"`.
    pub ticketing_system: String,
    /// **Resume from error** is allowed (Error or Stopped, worktree exists on disk).
    pub can_resume_from_error: bool,
    /// Set when a web terminal (ttyd) is running for this workflow's editor container.
    pub terminal_url: Option<String>,
    /// Configured run commands (from `[[run_commands]]` in config), with current running status.
    pub run_commands: Vec<RunCommandStatus>,
    /// Whether report generation is enabled in config (`[general] generate_report`).
    pub generate_report: bool,
    /// Whether a generated report file exists at `lore/reports/<key>_report.md` in the worktree.
    pub has_report: bool,
    /// Status of each dynamic workflow definition run for this ticket: def_name -> state display name.
    pub workflow_def_runs: HashMap<String, String>,
    /// Absolute path of the git worktree on disk, if it exists.
    /// `None` while the worktree is still being pre-created in the background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<String>,
    /// ID of the user who created this workflow. `None` for legacy/poller workflows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
}

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
    if w.user_id.as_deref() == Some(&auth.user_id) {
        Ok(())
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

fn workflow_action_flags(w: &Workflow) -> (bool, bool, bool) {
    let done = matches!(w.state, WorkflowState::Done);
    let has_pr = w
        .pr_url
        .as_deref()
        .map(str::trim)
        .is_some_and(|s| !s.is_empty());
    let has_worktree = w.worktree_path.as_ref().is_some_and(|p| p.exists());
    let can_address = done && has_pr;
    let can_merge_base = done && has_pr && has_worktree;
    let can_mark = done;
    (can_address, can_merge_base, can_mark)
}

fn manual_cap_fields(w: &Workflow) -> (bool, bool) {
    let toward = w.started_manually && w.state.occupies_concurrency_slot();
    (w.started_manually, toward)
}

fn can_open_editor(w: &Workflow) -> bool {
    w.worktree_path.as_ref().is_some_and(|p| p.exists()) && ContainerRunner::is_available()
}

fn has_report_file(w: &Workflow) -> bool {
    w.worktree_path.as_ref().is_some_and(|p| {
        p.join(format!("lore/reports/{}_report.md", w.ticket_key))
            .exists()
    })
}

fn can_start_workflow(w: &Workflow) -> bool {
    matches!(w.state, WorkflowState::Pending) && !w.driver_started
}

fn can_resume_from_error(w: &Workflow) -> bool {
    matches!(
        w.state,
        WorkflowState::Error { .. } | WorkflowState::Stopped
    ) && w.worktree_path.as_ref().is_some_and(|p| p.exists())
}

/// Compute the canonical issue URL for a workflow.
///
/// - If `ticket_url` is `Some`, use it directly (GitHub issues and future providers).
/// - If `ticket_url` is `None` and the ticketing system is Jira with a configured site,
///   compute the Jira browse URL.
/// - Otherwise `None`.
fn build_issue_url(w: &Workflow, jira_site: &str) -> Option<String> {
    if let Some(ref url) = w.ticket_url {
        return Some(url.clone());
    }
    if w.ticketing_system == maestro_core::config::TicketingSystem::Jira && !jira_site.is_empty() {
        let url = ticket_browse_url(jira_site, &w.ticket_key);
        if !url.is_empty() {
            return Some(url);
        }
    }
    None
}

// `TicketingSystem` implements `Display`, so use `.to_string()` directly.

fn workflow_def_runs_display(w: &Workflow) -> HashMap<String, String> {
    w.workflow_def_runs
        .iter()
        .map(|(k, v)| (k.clone(), v.display_name().to_string()))
        .collect()
}

fn extract_error(state: &WorkflowState) -> Option<String> {
    match state {
        WorkflowState::Error { message, .. } => Some(message.clone()),
        _ => None,
    }
}

/// Build the run command status list for a given workflow's ticket key.
fn build_run_commands_status(
    configured: &[maestro_core::db::user_worktree_commands::RunCommand],
    active_cmds: Option<&Vec<crate::state::RunCommandState>>,
) -> Vec<RunCommandStatus> {
    configured
        .iter()
        .enumerate()
        .map(|(i, rc)| {
            let (running, forwarded_port) = if let Some(active) = active_cmds {
                if let Some(cmd_state) = active.iter().find(|c| c.cmd_index == i) {
                    (true, cmd_state.forwarded_port.as_ref().map(|f| (f.container_port, f.proxy_url.clone())))
                } else {
                    (false, None)
                }
            } else {
                (false, None)
            };
            RunCommandStatus {
                index: i,
                name: rc.name.clone(),
                running,
                forwarded_port,
            }
        })
        .collect()
}

pub async fn list_workflows(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Json<Vec<WorkflowSummary>> {
    let cfg = state.config.read().await;
    let current_ws = maestro_core::workflow::snapshot::workspace_name_from_repo_path(
        std::path::Path::new(&cfg.git.repo_path),
    );
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
        .filter(|w| w.workspace_name == current_ws)
        .filter(|w| w.user_id.as_deref() == Some(&auth.user_id))
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
            }
        })
        .collect();
    // Oldest first — matches dashboard stable card order (new workflows last).
    summaries.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Json(summaries)
}

/// Typed response for cross-workspace workflow counts.
#[derive(Serialize)]
pub struct WorkflowCountsResponse {
    pub running: u32,
    pub completed: u32,
    pub errors: u32,
    pub paused: u32,
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
    let current_ws = maestro_core::workflow::snapshot::workspace_name_from_repo_path(
        std::path::Path::new(&cfg.git.repo_path),
    );
    let wf_arc = state.engine.workflows_arc();
    let workflows = wf_arc.read().await;
    let w = workflows.get(&id).ok_or(StatusCode::NOT_FOUND)?;
    if w.workspace_name != current_ws {
        return Err(StatusCode::NOT_FOUND);
    }
    // Users can only access their own workflows.
    if w.user_id.as_deref() != Some(&auth.user_id) {
        return Err(StatusCode::NOT_FOUND);
    }
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
    }))
}

/// Pause a running workflow. Delegates to WorkflowEngine::pause_workflow
/// which sets Paused state and broadcasts a WebSocket event.
pub async fn pause_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
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
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
        .engine
        .resume_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Resume a failed/stopped workflow from the last failed step, reusing the existing worktree and
/// skipping already-succeeded steps. The worktree must still exist on disk.
pub async fn resume_from_error(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&state, &id).await;

    state
        .engine
        .resume_from_error(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Retry a failed/stopped/completed workflow. Removes the old workflow and starts fresh.
pub async fn retry_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&state, &id).await;

    state
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
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    state
        .engine
        .stop_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Jira transition to configured **Done** status and remove worktree; removes the workflow on full success.
pub async fn mark_work_done(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<MarkDoneOutcome>, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&state, &id).await;

    state
        .engine
        .mark_work_done(&id)
        .await
        .map(Json)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Remove workflow from the map (not **running**), best-effort worktree cleanup, no Jira changes.
pub async fn delete_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_workflow_access(&state, &auth, &id)
        .await
        .map_err(|s| (s, "Workflow not found".into()))?;
    cleanup_run_commands(&state, &id).await;

    state
        .engine
        .delete_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Stop all run commands and clean up state for a workflow.
async fn cleanup_run_commands(state: &AppState, ticket_key: &str) {
    let mut run_cmds = state.run_commands.write().await;
    if let Some(cmds) = run_cmds.remove(ticket_key) {
        for cmd in &cmds {
            cmd.scanner_cancel.cancel();
        }
        drop(run_cmds);
        container::stop_all_run_commands(ticket_key).await;
    }
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

    let workflow_id = state
        .engine
        .add_to_dashboard(
            ticket_key.clone(),
            ticket_summary,
            true,
            description,
            issue_url,
            Some(auth.user_id),
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
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
    container::stop_editor(&id).await;
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

/// Status of a single run command.
#[derive(Serialize)]
pub struct RunCommandStatus {
    /// Index of the command in the `[[run_commands]]` config array.
    pub index: usize,
    /// Display name from config.
    pub name: String,
    /// Whether the command is currently running.
    pub running: bool,
    /// Forwarded port `(container_port, proxy_url)`, if detected.
    pub forwarded_port: Option<(u16, String)>,
}

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

    let spare_ports = container::start_run_command(
        &ticket_key,
        &worktree,
        &image,
        &rc_command,
        index,
        dynamic_ports,
        true, // isolate_workspace: restrict container to this issue's worktree
        &[("MAESTRO_PROXY_BASE", &proxy_base)],
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::session_registry::PathTokenRegistry;

    /// Create a minimal `WorkflowEvent` for port-forwarding tests.
    fn port_event(
        event_type: &str,
        ticket_key: &str,
        container_port: u16,
        host_port: u16,
    ) -> WorkflowEvent {
        WorkflowEvent {
            event_type: event_type.to_string(),
            workflow_id: String::new(),
            ticket_key: ticket_key.to_string(),
            state: String::new(),
            timestamp: chrono::Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: None,
            progress_steps_total: None,
            forwarded_port: Some((container_port, host_port)),
            pr_merged: None,
            user_id: None,
        }
    }

    /// Helper: extract `(container_port, host_port)` pairs from the map.
    fn port_pairs(fwd: &[DynamicPortForward]) -> Vec<(u16, u16)> {
        fwd.iter().map(|f| (f.container_port, f.host_port)).collect()
    }

    /// `track_port_forwards` adds ports on `port_forwarded` events and
    /// registers proxy tokens.
    #[tokio::test]
    async fn track_port_forwards_adds_on_forwarded() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(port_pairs(ports), vec![(3000, 9100)]);
            assert!(ports[0].proxy_url.starts_with("/s/"));
            assert!(!ports[0].path_token.is_empty());
            // Token should be registered in the registry.
            assert!(registry.lookup(&ports[0].path_token).await.is_some());
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` removes ports on `port_unforwarded` events and
    /// deregisters proxy tokens.
    #[tokio::test]
    async fn track_port_forwards_removes_on_unforwarded() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c));

        // Forward two ports.
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tx.send(port_event("port_forwarded", "T-1", 5000, 9101)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Capture the token for port 3000 before removal.
        let token_3000 = {
            let fwd = map.read().await;
            fwd.get("T-1").unwrap().iter().find(|f| f.container_port == 3000).unwrap().path_token.clone()
        };

        // Unforward port 3000.
        tx.send(port_event("port_unforwarded", "T-1", 3000, 9100)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(port_pairs(ports), vec![(5000, 9101)]);
            // Token for 3000 should be deregistered.
            assert!(registry.lookup(&token_3000).await.is_none());
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` ignores events for other tickets.
    #[tokio::test]
    async fn track_port_forwards_ignores_other_tickets() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c));

        tx.send(port_event("port_forwarded", "T-2", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            assert!(fwd.get("T-1").is_none(), "should not add ports for T-1");
            assert!(fwd.get("T-2").is_none(), "should not add ports for T-2");
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` deduplicates by container port.
    #[tokio::test]
    async fn track_port_forwards_deduplicates_by_container_port() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports.len(), 1, "duplicate container port should not be added twice");
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` handles multiple ports for the same ticket.
    #[tokio::test]
    async fn track_port_forwards_multiple_ports() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tx.send(port_event("port_forwarded", "T-1", 5173, 9101)).unwrap();
        tx.send(port_event("port_forwarded", "T-1", 8080, 9102)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports.len(), 3);
            let pairs = port_pairs(ports);
            assert!(pairs.contains(&(3000, 9100)));
            assert!(pairs.contains(&(5173, 9101)));
            assert!(pairs.contains(&(8080, 9102)));
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` exits when the cancellation token is cancelled.
    #[tokio::test]
    async fn track_port_forwards_exits_on_cancel() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), map, registry, rx, cancel.clone()));

        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("task should exit within 1 second")
            .expect("task should not panic");
    }

    /// `track_port_forwards` exits when the broadcast channel is closed.
    #[tokio::test]
    async fn track_port_forwards_exits_on_channel_close() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), map, registry, rx, cancel));

        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("task should exit within 1 second")
            .expect("task should not panic");
    }

    /// Build a minimal `Workflow` in `Pending` state with the given `driver_started` value.
    fn wf_pending(driver_started: bool) -> Workflow {
        let mut w = Workflow::new(
            "T-1".into(),
            "summary".into(),
            true,
            false,
            maestro_core::config::TicketingSystem::None,
            None,
            "test-workspace".into(),
        );
        w.driver_started = driver_started;
        w
    }

    #[test]
    fn can_start_pending_not_started() {
        assert!(can_start_workflow(&wf_pending(false)));
    }

    #[test]
    fn can_start_false_when_started() {
        assert!(!can_start_workflow(&wf_pending(true)));
    }

    #[test]
    fn can_start_false_when_not_pending() {
        let mut w = wf_pending(false);
        w.state = WorkflowState::Done;
        assert!(!can_start_workflow(&w));
    }
}
