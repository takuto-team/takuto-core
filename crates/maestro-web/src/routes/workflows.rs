use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use maestro_core::container::{self, ContainerRunner};
use maestro_core::jira::ticket_browse_url;
use maestro_core::workflow::dashboard_progress;
use maestro_core::workflow::engine::{MarkDoneOutcome, TerminalLine, Workflow};
use maestro_core::workflow::state::WorkflowState;
use maestro_core::workflow::step::StepLog;

use crate::state::AppState;

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
    pub steps_log: Vec<StepLog>,
    pub error: Option<String>,
    pub terminal_lines: Vec<TerminalLineDto>,
    /// **Address PR Comments** is allowed (main flow **Done** and `pr_url` set).
    pub can_address_pr_comments: bool,
    /// **Merge base branch** is allowed (main flow **Done**, `pr_url` set, worktree exists).
    pub can_merge_base: bool,
    /// **Mark as Done** is allowed (workflow state is **Done**).
    pub can_mark_done: bool,
    /// **Delete** is allowed when the workflow is not **running** (`WorkflowState::is_active` is false).
    pub can_delete: bool,
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
    /// **Resume from error** is allowed (Error or Stopped, worktree exists on disk).
    pub can_resume_from_error: bool,
    /// Set when a web terminal (ttyd) is running for this workflow's editor container.
    pub terminal_url: Option<String>,
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

fn can_resume_from_error(w: &Workflow) -> bool {
    matches!(w.state, WorkflowState::Error { .. } | WorkflowState::Stopped)
        && w.worktree_path.as_ref().is_some_and(|p| p.exists())
}

fn extract_error(state: &WorkflowState) -> Option<String> {
    match state {
        WorkflowState::Error { message, .. } => Some(message.clone()),
        _ => None,
    }
}

pub async fn list_workflows(State(state): State<AppState>) -> Json<Vec<WorkflowSummary>> {
    let cfg = state.config.read().await;
    let workflows = state.engine.workflows.read().await;
    let mut summaries: Vec<WorkflowSummary> = workflows
        .values()
        .map(|w| {
            let (can_address_pr_comments, can_merge_base, can_mark_done) = workflow_action_flags(w);
            let (started_manually, counts_toward_manual_cap) = manual_cap_fields(w);
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
                steps_log: w.steps_log.clone(),
                error: extract_error(&w.state),
                terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
                can_address_pr_comments,
                can_merge_base,
                can_mark_done,
                can_delete: !w.state.is_active(),
                progress_percent: dashboard_progress::workflow_progress_percent(w, &cfg),
                progress_steps_total: dashboard_progress::estimated_step_total(w, &cfg),
                started_manually,
                counts_toward_manual_cap,
                jira_browse_url: ticket_browse_url(&cfg.jira.site, &w.ticket_key),
                can_open_editor: can_open_editor(w),
                editor_url: None,
                editor_port_mappings: Vec::new(),
                jira_available: w.jira_available,
                can_resume_from_error: can_resume_from_error(w),
                terminal_url: None,
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
    let editor_info = container::get_editor_info(&w.ticket_key).await;
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
        steps_log: w.steps_log.clone(),
        error: extract_error(&w.state),
        terminal_lines: w.terminal_lines.iter().map(TerminalLineDto::from).collect(),
        can_address_pr_comments,
        can_merge_base,
        can_mark_done,
        can_delete: !w.state.is_active(),
        progress_percent: dashboard_progress::workflow_progress_percent(w, &cfg),
        progress_steps_total: dashboard_progress::estimated_step_total(w, &cfg),
        started_manually,
        counts_toward_manual_cap,
        jira_browse_url: ticket_browse_url(&cfg.jira.site, &w.ticket_key),
        can_open_editor: can_open_editor(w),
        editor_url: editor_info.as_ref().map(|e| e.url.clone()),
        editor_port_mappings: editor_info.map(|e| e.port_mappings).unwrap_or_default(),
        jira_available: w.jira_available,
        can_resume_from_error: can_resume_from_error(w),
        terminal_url: state
            .terminal_ports
            .read()
            .await
            .get(&w.ticket_key)
            .map(|port| format!("http://localhost:{port}")),
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
    state
        .engine
        .delete_workflow(&id)
        .await
        .map(|()| StatusCode::OK)
        .map_err(|e| (StatusCode::CONFLICT, e.to_string()))
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

/// Start a ticket workflow from the dashboard (same pipeline as the poller). Respects **`[general] max_concurrent_manual_workflows`**.
///
/// When Jira is unavailable (`jira_available = false`), `ticket_key` may be empty — a synthetic
/// `MANUAL-{timestamp}` key is generated. The `ticket_description` field is stored on the workflow
/// so the agent prompt can use it.
pub async fn start_manual_workflow(
    State(state): State<AppState>,
    Json(body): Json<StartManualWorkflowBody>,
) -> Result<Json<StartManualWorkflowResponse>, (StatusCode, String)> {
    let jira_on = state.jira_available.load(std::sync::atomic::Ordering::Relaxed);

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
            if jira_on { ticket_key.clone() } else { "Manual workflow".to_string() }
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
        .start_workflow(ticket_key.clone(), ticket_summary, true, description)
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
            "Cannot open editor: workflow is active, worktree missing, or Docker unavailable".into(),
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
    drop(workflows);
    drop(cfg);

    let image = ContainerRunner::discover_worker_image()
        .await
        .unwrap_or_else(|| "maestro:latest".to_string());

    let info = container::start_editor(
        &ticket_key, &worktree, &image, &app_ports, dynamic_ports,
        &theme, &extensions, &settings, &setup_commands,
    )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

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
    }

    Ok(Json(OpenEditorResponse {
        url: info.url,
        vscode_port: info.vscode_port,
        port_mappings: info.port_mappings,
    }))
}

/// Stop and remove the editor container for a workflow.
pub async fn close_editor(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> StatusCode {
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
}

/// Start a web terminal (ttyd) inside the running editor container.
/// The editor container must already be running (use open-editor first).
pub async fn open_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<OpenTerminalResponse>, (StatusCode, String)> {
    // Reuse existing terminal if already started.
    if let Some(&port) = state.terminal_ports.read().await.get(&id) {
        return Ok(Json(OpenTerminalResponse {
            url: format!("http://localhost:{port}"),
        }));
    }

    // Editor container must be running.
    let info = container::get_editor_info(&id)
        .await
        .ok_or((StatusCode::CONFLICT, "Editor container is not running — open the editor first.".into()))?;

    // Pick a spare port not already used by the port scanner's socat or another terminal.
    let used_by_terminals: Vec<u16> = state.terminal_ports.read().await.values().copied().collect();
    let port = info
        .spare_ports
        .iter()
        .copied()
        .find(|p| !used_by_terminals.contains(p))
        .ok_or((
            StatusCode::CONFLICT,
            "No spare ports available for terminal.".into(),
        ))?;

    let url = container::start_terminal(&id, port)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))?;

    state.terminal_ports.write().await.insert(id, port);

    Ok(Json(OpenTerminalResponse { url }))
}

/// Stop the web terminal for a workflow's editor container.
pub async fn close_terminal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> StatusCode {
    state.terminal_ports.write().await.remove(&id);
    container::stop_terminal(&id).await;
    StatusCode::OK
}
