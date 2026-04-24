// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use maestro_core::container::{self, ContainerRunner};
use maestro_core::jira::ticket_browse_url;
use maestro_core::workflow::dashboard_progress;
use maestro_core::workflow::engine::{MarkDoneOutcome, TerminalLine, Workflow, WorkflowEvent};
use maestro_core::workflow::state::WorkflowState;
use maestro_core::workflow::step::StepLog;

use crate::state::{AppState, DynamicForwardsMap};

/// Listen on the workflow event broadcast channel and keep the dynamic-forwards
/// map in sync for the given ticket.  Runs until `cancel` fires or the channel
/// closes.
pub async fn track_port_forwards(
    ticket_key: String,
    dyn_fwd: DynamicForwardsMap,
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
                            if !list.iter().any(|&(c, _)| c == cp) {
                                list.push((cp, hp));
                            }
                        } else if evt.event_type == "port_unforwarded"
                            && let Some((cp, _)) = evt.forwarded_port
                        {
                            let mut map = dyn_fwd.write().await;
                            if let Some(list) = map.get_mut(&ticket_key) {
                                list.retain(|&(c, _)| c != cp);
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
    /// **Open editor** is allowed (workflow not active, worktree exists, Docker available).
    pub can_open_editor: bool,
    /// Set when an editor container is already running for this workflow.
    pub editor_url: Option<String>,
    /// `(container_port, host_port)` pairs for user-configured application ports.
    pub editor_port_mappings: Vec<(u16, u16)>,
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
    !w.state.is_active()
        && w.worktree_path.as_ref().is_some_and(|p| p.exists())
        && ContainerRunner::is_available()
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

// `TicketingSystem` implements `Display`, so use `.to_string()` directly.

fn extract_error(state: &WorkflowState) -> Option<String> {
    match state {
        WorkflowState::Error { message, .. } => Some(message.clone()),
        _ => None,
    }
}

/// Build the run command status list for a given workflow's ticket key.
fn build_run_commands_status(
    cfg_commands: &[maestro_core::config::RunCommandConfig],
    active_cmds: Option<&Vec<crate::state::RunCommandState>>,
) -> Vec<RunCommandStatus> {
    cfg_commands
        .iter()
        .enumerate()
        .map(|(i, rc)| {
            let (running, forwarded_port) = if let Some(active) = active_cmds {
                if let Some(cmd_state) = active.iter().find(|c| c.cmd_index == i) {
                    (true, cmd_state.forwarded_port)
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

pub async fn list_workflows(State(state): State<AppState>) -> Json<Vec<WorkflowSummary>> {
    let cfg = state.config.read().await;
    let workflows = state.engine.workflows.read().await;
    let dyn_fwd = state.dynamic_forwards.read().await;
    let run_cmds_state = state.run_commands.read().await;
    let mut summaries: Vec<WorkflowSummary> = workflows
        .values()
        .map(|w| {
            let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
            let (started_manually, counts_toward_manual_cap) = manual_cap_fields(w);
            // Use the server-side dynamic-forwards cache so that port buttons
            // appear immediately on page load (no per-workflow Docker call).
            let port_mappings = dyn_fwd.get(&w.ticket_key).cloned().unwrap_or_default();
            let run_commands =
                build_run_commands_status(&cfg.run_commands, run_cmds_state.get(&w.ticket_key));
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
                can_open_editor: can_open_editor(w),
                editor_url: None,
                editor_port_mappings: port_mappings,
                jira_available: w.jira_available,
                ticketing_system: w.ticketing_system.to_string(),
                can_resume_from_error: can_resume_from_error(w),
                terminal_url: None,
                run_commands,
                generate_report: cfg.general.generate_report,
                has_report: has_report_file(w),
            }
        })
        .collect();
    // Oldest first — matches dashboard stable card order (new workflows last).
    summaries.sort_by(|a, b| a.started_at.cmp(&b.started_at));
    Json(summaries)
}

pub async fn get_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<WorkflowSummary>, StatusCode> {
    let cfg = state.config.read().await;
    let workflows = state.engine.workflows.read().await;
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
    let port_mappings = if let Some(forwards) = dyn_fwd.get(&ticket_key) {
        forwards.clone()
    } else {
        editor_info
            .as_ref()
            .map(|e| e.port_mappings.clone())
            .unwrap_or_default()
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
        can_open_editor: can_open_editor(w),
        editor_url: editor_info.as_ref().map(|e| e.url.clone()),
        editor_port_mappings: port_mappings,
        jira_available: w.jira_available,
        ticketing_system: w.ticketing_system.to_string(),
        can_resume_from_error: can_resume_from_error(w),
        terminal_url: state
            .terminal_ports
            .read()
            .await
            .get(&ticket_key)
            .map(|(port, token)| {
                container::build_terminal_url(container::editor_host_port(*port), token)
            }),
        run_commands: {
            let run_cmds_state = state.run_commands.read().await;
            build_run_commands_status(&cfg.run_commands, run_cmds_state.get(&ticket_key))
        },
        generate_report: cfg.general.generate_report,
        has_report: has_report_file(w),
    }))
}

/// Pause a running workflow. Delegates to WorkflowEngine::pause_workflow
/// which sets Paused state and broadcasts a WebSocket event.
pub async fn pause_workflow(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
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
) -> Result<StatusCode, (StatusCode, String)> {
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
) -> Result<StatusCode, (StatusCode, String)> {
    // Stop any running run commands — the workflow is transitioning back to active
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
) -> Result<StatusCode, (StatusCode, String)> {
    // Stop any running run commands — the old workflow is being removed
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
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .stop_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Run the configured **`[[review_agent_steps]]`** sequence in the existing worktree (requires **Done** + PR URL).
pub async fn address_pr_comments(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Stop any running run commands — the workflow is transitioning back to active
    cleanup_run_commands(&state, &id).await;

    state
        .engine
        .start_pr_review_workflow(&id)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Run the configured **`[[merge_base_agent_steps]]`** sequence in the existing worktree (requires **Done** + PR URL).
pub async fn merge_base_branch(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Stop any running run commands — the workflow is transitioning back to active
    cleanup_run_commands(&state, &id).await;

    state
        .engine
        .start_merge_base_workflow(&id)
        .await
        .map(|()| StatusCode::ACCEPTED)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
}

/// Jira transition to configured **Done** status and remove worktree; removes the workflow on full success.
pub async fn mark_work_done(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<MarkDoneOutcome>, (StatusCode, String)> {
    // Stop any running run commands for this workflow
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
) -> Result<StatusCode, (StatusCode, String)> {
    // Stop any running run commands for this workflow
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
) -> Result<Json<WorkflowReportResponse>, StatusCode> {
    let workflows = state.engine.workflows.read().await;
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
}

#[derive(Serialize)]
pub struct StartManualWorkflowResponse {
    pub workflow_id: String,
    pub ticket_key: String,
}

/// Start the agent pipeline for a workflow that was added to the dashboard.
pub async fn start_workflow_from_dashboard(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    state
        .engine
        .start_pending_workflow(&id)
        .await
        .map(|_| StatusCode::OK)
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                (StatusCode::NOT_FOUND, msg)
            } else {
                (StatusCode::CONFLICT, msg)
            }
        })
}

/// Start a ticket workflow from the dashboard (same pipeline as the poller). Respects **`[general] max_concurrent_manual_workflows`**.
///
/// When Jira is unavailable (`jira_available = false`), `ticket_key` may be empty — a synthetic
/// `MANUAL-{timestamp}` key is generated. The `ticket_description` field is stored on the workflow
/// so the agent prompt can use it.
pub async fn start_manual_workflow(
    State(state): State<AppState>,
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
                "Manual workflow".to_string()
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
        let map = state.engine.workflows.read().await;
        if map.contains_key(&ticket_key) {
            return Err((
                StatusCode::CONFLICT,
                format!("A workflow already exists for {ticket_key}"),
            ));
        }
    }

    if max_manual > 0 {
        let n = state.engine.manual_workflows_toward_cap_count().await;
        if n >= max_manual as usize {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Maximum concurrent manual workflows ({max_manual}) reached; complete, stop, or delete a manual workflow first"
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

    let workflow_id = state
        .engine
        .add_to_dashboard(ticket_key.clone(), ticket_summary, true, description)
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
    pub url: String,
    /// Connection token for openvscode-server authentication.
    pub connection_token: String,
    pub vscode_port: u16,
    pub port_mappings: Vec<(u16, u16)>,
}

/// Start a browser VS Code editor container for a workflow.
pub async fn open_editor(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<OpenEditorResponse>, (StatusCode, String)> {
    let cfg = state.config.read().await;
    let workflows = state.engine.workflows.read().await;
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
    )
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    // Seed the server-side dynamic-forwards map with the static (Docker -p) port
    // mappings so that `GET /api/workflows` returns them immediately (no need to
    // wait for the port scanner or call get_editor_info per-workflow).
    {
        let mut fwd = state.dynamic_forwards.write().await;
        fwd.insert(ticket_key.clone(), info.port_mappings.clone());
    }

    // Spawn background port scanner if dynamic ports are available.
    if !info.spare_ports.is_empty() {
        let scanner_ticket = ticket_key.clone();
        let scanner_spare = info.spare_ports.clone();
        let scanner_vscode = info.vscode_port;
        let scanner_event_tx = state.engine.event_tx.clone();
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

        tokio::spawn(async move {
            container::run_port_scanner(
                &scanner_ticket,
                scanner_vscode,
                scanner_spare,
                scanner_event_tx,
                scanner_cancel_clone,
            )
            .await;
        });

        // Spawn a companion task that subscribes to broadcast events and keeps
        // `dynamic_forwards` in sync with the port scanner's forwarded/unforwarded
        // events.  This allows the list endpoint to return current port data without
        // per-workflow Docker calls.
        let dyn_fwd = state.dynamic_forwards.clone();
        let rx = state.engine.event_tx.subscribe();
        let tracker_ticket = ticket_key.clone();
        let tracker_cancel = {
            let scanners = state.editor_scanners.read().await;
            scanners.get(&ticket_key).cloned()
        };
        if let Some(cancel_tok) = tracker_cancel {
            tokio::spawn(track_port_forwards(tracker_ticket, dyn_fwd, rx, cancel_tok));
        }
    }

    Ok(Json(OpenEditorResponse {
        url: info.url,
        connection_token: info.connection_token,
        vscode_port: info.vscode_port,
        port_mappings: info.port_mappings,
    }))
}

/// Stop and remove the editor container for a workflow.
pub async fn close_editor(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
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
    pub url: String,
    /// The raw authentication token (same value embedded in the URL path).
    /// Provided separately so programmatic consumers can use it independently.
    pub credential: String,
}

/// Start a web terminal (ttyd) inside the running editor container.
/// The editor container must already be running (use open-editor first).
pub async fn open_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<OpenTerminalResponse>, (StatusCode, String)> {
    // Reuse existing terminal if already recorded in the in-memory map.
    if let Some((port, token)) = state.terminal_ports.read().await.get(&id) {
        return Ok(Json(OpenTerminalResponse {
            url: container::build_terminal_url(container::editor_host_port(*port), token),
            credential: token.clone(),
        }));
    }

    // Editor container must be running.
    let info = container::get_editor_info(&id).await.ok_or((
        StatusCode::CONFLICT,
        "Editor container is not running — open the editor first.".into(),
    ))?;

    // Recover from a server restart: ttyd may already be running from a previous session.
    // Ask the container for the actual port and token (via pgrep) rather than trusting the now-empty map.
    if let Some((port, token)) = container::find_running_terminal(&id).await {
        let url = container::build_terminal_url(container::editor_host_port(port), &token);
        state
            .terminal_ports
            .write()
            .await
            .insert(id.clone(), (port, token.clone()));
        return Ok(Json(OpenTerminalResponse {
            url,
            credential: token,
        }));
    }

    // Pick a spare port that is:
    //  1. Not already allocated to another workflow's terminal.
    //  2. Not currently listening inside this container (socat dynamic forwards also bind
    //     spare ports, so ttyd would fail to bind if we picked one socat already holds).
    let used_by_terminals: Vec<u16> = state
        .terminal_ports
        .read()
        .await
        .values()
        .map(|(port, _)| *port)
        .collect();
    let in_use = container::listening_ports_in_editor(&id).await;
    let port = info
        .spare_ports
        .iter()
        .copied()
        .find(|p| !used_by_terminals.contains(p) && !in_use.contains(p))
        .ok_or((
            StatusCode::CONFLICT,
            "No spare ports available for terminal.".into(),
        ))?;

    let (url, token) = container::start_terminal(&id, port)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    state
        .terminal_ports
        .write()
        .await
        .insert(id.clone(), (port, token.clone()));
    tracing::info!(workflow = %id, port, "Terminal started on port");

    Ok(Json(OpenTerminalResponse {
        url,
        credential: token,
    }))
}

/// Stop the web terminal for a workflow's editor container.
pub async fn close_terminal(State(state): State<AppState>, Path(id): Path<String>) -> StatusCode {
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
    /// Forwarded port `(container_port, host_port)`, if detected.
    pub forwarded_port: Option<(u16, u16)>,
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
    let cfg = state.config.read().await;
    let workflows = state.engine.workflows.read().await;
    let _w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;

    let run_cmds_state = state.run_commands.read().await;
    let active_cmds = run_cmds_state.get(&id);

    let commands: Vec<RunCommandStatus> = cfg
        .run_commands
        .iter()
        .enumerate()
        .map(|(i, rc)| {
            let (running, forwarded_port) = if let Some(active) = active_cmds {
                if let Some(cmd_state) = active.iter().find(|c| c.cmd_index == i) {
                    (true, cmd_state.forwarded_port)
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
        .collect();

    Ok(Json(RunCommandsStatusResponse { commands }))
}

/// Start a run command for a workflow.
pub async fn start_run_command(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, usize)>,
) -> Result<Json<StartRunCommandResponse>, (StatusCode, String)> {
    let cfg = state.config.read().await;
    let rc = cfg.run_commands.get(index).ok_or((
        StatusCode::BAD_REQUEST,
        format!(
            "Run command index {index} out of range (max {})",
            cfg.run_commands.len()
        ),
    ))?;
    let rc_name = rc.name.clone();
    let rc_command = rc.command.clone();
    let dynamic_ports = cfg.editor.dynamic_ports;
    drop(cfg);

    let workflows = state.engine.workflows.read().await;
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

    let spare_ports = container::start_run_command(
        &ticket_key,
        &worktree,
        &image,
        &rc_command,
        index,
        dynamic_ports,
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
    let event_tx = state.engine.event_tx.clone();
    let ticket_for_scanner = ticket_key.clone();

    let run_cmds_map = state.run_commands.clone();
    let ticket_for_tracker = ticket_key.clone();

    // Spawn port scanner
    tokio::spawn({
        let spare = spare_ports.clone();
        async move {
            container::run_run_command_port_scanner(
                &ticket_for_scanner,
                index,
                spare,
                event_tx,
                scanner_cancel,
            )
            .await;
        }
    });

    // Spawn tracker that updates run_commands state on port events
    let mut rx = state.engine.subscribe();
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tracker_cancel.cancelled() => break,
                result = rx.recv() => {
                    match result {
                        Ok(event) => {
                            if event.ticket_key != ticket_for_tracker {
                                continue;
                            }
                            let cmd_idx_str = event.step_name.as_deref().unwrap_or("");
                            let evt_cmd_index: usize = match cmd_idx_str.parse() {
                                Ok(i) => i,
                                Err(_) => continue,
                            };
                            if evt_cmd_index != index {
                                continue;
                            }
                            match event.event_type.as_str() {
                                "run_command_port_forwarded" => {
                                    if let Some(fwd) = event.forwarded_port {
                                        let mut map = run_cmds_map.write().await;
                                        if let Some(cmd) = map.get_mut(&ticket_for_tracker).and_then(|cmds| cmds.iter_mut().find(|c| c.cmd_index == index)) {
                                            cmd.forwarded_port = Some(fwd);
                                        }
                                    }
                                }
                                "run_command_port_unforwarded" => {
                                    let mut map = run_cmds_map.write().await;
                                    if let Some(cmd) = map.get_mut(&ticket_for_tracker).and_then(|cmds| cmds.iter_mut().find(|c| c.cmd_index == index))
                                        && let Some(gone) = event.forwarded_port && cmd.forwarded_port.map(|f| f.0) == Some(gone.0) {
                                        cmd.forwarded_port = None;
                                    }
                                }
                                "run_command_stopped" => {
                                    // Container exited on its own — clean up state
                                    let mut map = run_cmds_map.write().await;
                                    if let Some(cmds) = map.get_mut(&ticket_for_tracker) {
                                        cmds.retain(|c| c.cmd_index != index);
                                        if cmds.is_empty() {
                                            map.remove(&ticket_for_tracker);
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
    });

    Ok(Json(StartRunCommandResponse {
        index,
        name: rc_name,
    }))
}

/// Stop a running run command.
pub async fn stop_run_command(
    State(state): State<AppState>,
    Path((id, index)): Path<(String, usize)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let workflows = state.engine.workflows.read().await;
    let w = workflows
        .get(&id)
        .ok_or((StatusCode::NOT_FOUND, "Workflow not found".into()))?;
    let ticket_key = w.ticket_key.clone();
    drop(workflows);

    // Cancel scanner and remove from state
    {
        let mut run_cmds = state.run_commands.write().await;
        if let Some(cmds) = run_cmds.get_mut(&ticket_key) {
            if let Some(pos) = cmds.iter().position(|c| c.cmd_index == index) {
                cmds[pos].scanner_cancel.cancel();
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

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
        }
    }

    /// `track_port_forwards` adds ports on `port_forwarded` events.
    #[tokio::test]
    async fn track_port_forwards_adds_on_forwarded() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), m, rx, c));

        // Send a port_forwarded event.
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        // Give the task time to process.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports, &[(3000, 9100)]);
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` removes ports on `port_unforwarded` events.
    #[tokio::test]
    async fn track_port_forwards_removes_on_unforwarded() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        // Seed with an existing port.
        map.write()
            .await
            .insert("T-1".into(), vec![(3000, 9100), (5000, 9101)]);
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), m, rx, c));

        // Unforward port 3000.
        tx.send(port_event("port_unforwarded", "T-1", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports, &[(5000, 9101)]);
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` ignores events for other tickets.
    #[tokio::test]
    async fn track_port_forwards_ignores_other_tickets() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), m, rx, c));

        // Send event for a different ticket.
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
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), m, rx, c));

        // Send the same port twice.
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(
                ports.len(),
                1,
                "duplicate container port should not be added twice"
            );
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` handles multiple ports for the same ticket.
    #[tokio::test]
    async fn track_port_forwards_multiple_ports() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), m, rx, c));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        tx.send(port_event("port_forwarded", "T-1", 5173, 9101))
            .unwrap();
        tx.send(port_event("port_forwarded", "T-1", 8080, 9102))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports.len(), 3);
            assert!(ports.contains(&(3000, 9100)));
            assert!(ports.contains(&(5173, 9101)));
            assert!(ports.contains(&(8080, 9102)));
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` exits when the cancellation token is cancelled.
    #[tokio::test]
    async fn track_port_forwards_exits_on_cancel() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards("T-1".into(), map, rx, cancel.clone()));

        cancel.cancel();
        // Should exit promptly.
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("task should exit within 1 second")
            .expect("task should not panic");
    }

    /// `track_port_forwards` exits when the broadcast channel is closed.
    #[tokio::test]
    async fn track_port_forwards_exits_on_channel_close() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards("T-1".into(), map, rx, cancel));

        // Drop the sender to close the channel.
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
