use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::actions::traits::ExternalActions;
use crate::agent_prompt::headless_instructions_suffix;
use crate::claude::session::ClaudeSession;
use crate::config::{cursor_model_for_cli, interpolate_agent_prompt, AiAgentProvider, Config};
use crate::cursor::session::CursorSession;
use crate::error::{MaestroError, Result};
use crate::git;
use crate::jira::client::JiraClient;
use crate::process::OutputLine;

use super::log_writer::WorkflowLogWriter;
use super::outcome::resolve_pr_url;
use super::state::WorkflowState;
use super::step::{StepLog, StepStatus};
use super::stream_humanize::humanize_agent_stream_line;

/// Maximum number of terminal lines stored per workflow for persistence.
const TERMINAL_LINES_MAX: usize = 100;

/// A single line of terminal output stored on the workflow for persistence
/// across page reloads. Populated by spawn_output_relay after humanizing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TerminalLine {
    pub text: String,
    pub stream: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct WorkflowEvent {
    pub event_type: String,
    pub workflow_id: String,
    pub ticket_key: String,
    pub state: String,
    pub timestamp: chrono::DateTime<Utc>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_line: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
}

pub struct Workflow {
    pub id: String,
    pub ticket_key: String,
    pub ticket_summary: String,
    pub ticket_description: String,
    pub ticket_type: String,
    pub state: WorkflowState,
    pub started_at: chrono::DateTime<Utc>,
    pub updated_at: chrono::DateTime<Utc>,
    pub steps_log: Vec<StepLog>,
    pub branch_name: String,
    pub worktree_path: Option<PathBuf>,
    pub pr_url: Option<String>,
    pub cancel_token: CancellationToken,
    /// Recent terminal output lines for persistence across page reloads.
    pub terminal_lines: Vec<TerminalLine>,
    /// Human-readable agent step label for the dashboard (e.g. `Implement (cycle 2/3, run 1/1)`).
    pub current_step_label: Option<String>,
}

impl Workflow {
    pub fn new(ticket_key: String, ticket_summary: String) -> Self {
        let now = Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            ticket_key,
            ticket_summary,
            ticket_description: String::new(),
            ticket_type: "Task".to_string(),
            state: WorkflowState::Pending,
            started_at: now,
            updated_at: now,
            steps_log: Vec::new(),
            branch_name: String::new(),
            worktree_path: None,
            pr_url: None,
            cancel_token: CancellationToken::new(),
            terminal_lines: Vec::new(),
            current_step_label: None,
        }
    }

    /// String shown on the dashboard and in WebSocket `workflow_updated` events.
    pub fn status_display(&self) -> String {
        match &self.state {
            WorkflowState::Paused { .. }
            | WorkflowState::Error { .. }
            | WorkflowState::Done
            | WorkflowState::Stopped => self.state.display_name(),
            WorkflowState::AddressingTicket { .. } => self
                .current_step_label
                .clone()
                .unwrap_or_else(|| "Running agent steps".to_string()),
            _ => self.state.display_name(),
        }
    }
}

pub struct WorkflowEngine {
    pub config: Arc<RwLock<Config>>,
    pub workflows: Arc<RwLock<HashMap<String, Workflow>>>,
    pub actions: Arc<dyn ExternalActions>,
    pub event_tx: broadcast::Sender<WorkflowEvent>,
}

impl WorkflowEngine {
    pub fn new(config: Arc<RwLock<Config>>, actions: Arc<dyn ExternalActions>) -> Self {
        let (event_tx, _) = broadcast::channel(256);
        Self {
            config,
            workflows: Arc::new(RwLock::new(HashMap::new())),
            actions,
            event_tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.event_tx.subscribe()
    }

    pub async fn start_workflow(
        &self,
        ticket_key: String,
        ticket_summary: String,
    ) -> Result<String> {
        let workflow = Workflow::new(ticket_key.clone(), ticket_summary);
        let id = workflow.id.clone();
        let cancel_token = workflow.cancel_token.clone();

        self.workflows
            .write()
            .await
            .insert(ticket_key.clone(), workflow);

        // Spawn the workflow driver task
        let engine_config = self.config.clone();
        let engine_workflows = self.workflows.clone();
        let engine_actions = self.actions.clone();
        let engine_event_tx = self.event_tx.clone();
        let ticket = ticket_key.clone();

        tokio::spawn(async move {
            drive_workflow(
                ticket,
                engine_config,
                engine_workflows,
                engine_actions,
                engine_event_tx,
                cancel_token,
            )
            .await;
        });

        Ok(id)
    }

    pub async fn get_workflow_ids(&self) -> Vec<String> {
        self.workflows.read().await.keys().cloned().collect()
    }

    pub async fn active_workflow_count(&self) -> usize {
        self.workflows
            .read()
            .await
            .values()
            .filter(|w| w.state.is_active())
            .count()
    }

    pub async fn pause_workflow(&self, ticket_key: &str) -> Result<()> {
        let mut workflows = self.workflows.write().await;
        let workflow = workflows
            .get_mut(ticket_key)
            .ok_or_else(|| MaestroError::Config(format!("Workflow not found: {ticket_key}")))?;

        if !workflow.state.is_active() {
            return Err(MaestroError::Config(format!(
                "Cannot pause workflow in state: {}",
                workflow.state
            )));
        }

        let source = Box::new(workflow.state.clone());
        workflow.state = WorkflowState::Paused {
            source_state: source,
        };
        workflow.updated_at = Utc::now();

        self.broadcast_event(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id: workflow.id.clone(),
            ticket_key: ticket_key.to_string(),
            state: "Paused".to_string(),
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
        });

        Ok(())
    }

    pub async fn resume_workflow(&self, ticket_key: &str) -> Result<()> {
        let mut workflows = self.workflows.write().await;
        let workflow = workflows
            .get_mut(ticket_key)
            .ok_or_else(|| MaestroError::Config(format!("Workflow not found: {ticket_key}")))?;

        if let WorkflowState::Paused { source_state } = &workflow.state {
            let restored = *source_state.clone();
            workflow.state = restored;
            workflow.updated_at = Utc::now();

            let state_line = workflow.status_display();
            self.broadcast_event(WorkflowEvent {
                event_type: "workflow_updated".to_string(),
                workflow_id: workflow.id.clone(),
                ticket_key: ticket_key.to_string(),
                state: state_line,
                timestamp: Utc::now(),
                error: None,
                step_name: None,
                output_line: None,
                stream: None,
            });

            Ok(())
        } else {
            Err(MaestroError::Config(format!(
                "Cannot resume workflow in state: {}",
                workflow.state
            )))
        }
    }

    pub async fn stop_workflow(&self, ticket_key: &str) -> Result<()> {
        let mut workflows = self.workflows.write().await;
        let workflow = workflows
            .get_mut(ticket_key)
            .ok_or_else(|| MaestroError::Config(format!("Workflow not found: {ticket_key}")))?;

        // Cancel all running processes
        workflow.cancel_token.cancel();
        workflow.current_step_label = None;
        workflow.state = WorkflowState::Stopped;
        workflow.updated_at = Utc::now();

        let ticket_key_owned = ticket_key.to_string();
        let actions = self.actions.clone();

        // Unassign ticket and move back to To Do (fire and forget)
        tokio::spawn(async move {
            if let Err(e) = actions.unassign_ticket(&ticket_key_owned).await {
                warn!(error = %e, ticket = %ticket_key_owned, "Failed to unassign ticket on stop");
            }
            if let Err(e) = actions
                .transition_ticket(&ticket_key_owned, "To Do")
                .await
            {
                warn!(error = %e, ticket = %ticket_key_owned, "Failed to transition ticket back to To Do on stop");
            }
        });

        self.broadcast_event(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id: workflow.id.clone(),
            ticket_key: ticket_key.to_string(),
            state: "Stopped".to_string(),
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
        });

        Ok(())
    }

    pub async fn retry_workflow(&self, ticket_key: &str) -> Result<String> {
        let (ticket_summary,) = {
            let workflows = self.workflows.read().await;
            let workflow = workflows
                .get(ticket_key)
                .ok_or_else(|| MaestroError::Config(format!("Workflow not found: {ticket_key}")))?;

            if !workflow.state.is_terminal() {
                return Err(MaestroError::Config(format!(
                    "Cannot retry workflow in state: {} (must be Error, Stopped, or Done)",
                    workflow.state
                )));
            }

            (workflow.ticket_summary.clone(),)
        };

        // Remove the old workflow
        self.workflows.write().await.remove(ticket_key);

        // Start a fresh one
        self.start_workflow(ticket_key.to_string(), ticket_summary).await
    }

    pub async fn stop_all_workflows(&self) {
        let keys: Vec<String> = {
            let workflows = self.workflows.read().await;
            workflows
                .iter()
                .filter(|(_, w)| !w.state.is_terminal())
                .map(|(k, _)| k.clone())
                .collect()
        };

        for key in keys {
            if let Err(e) = self.stop_workflow(&key).await {
                warn!(ticket = %key, error = %e, "Failed to stop workflow during shutdown");
            }
        }
    }

    pub fn broadcast_event(&self, event: WorkflowEvent) {
        let _ = self.event_tx.send(event);
    }
}

async fn drive_workflow(
    ticket_key: String,
    config: Arc<RwLock<Config>>,
    workflows: Arc<RwLock<HashMap<String, Workflow>>>,
    actions: Arc<dyn ExternalActions>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel_token: CancellationToken,
) {
    info!(ticket = %ticket_key, "Workflow driver started");

    let log_dir = {
        let cfg = config.read().await;
        PathBuf::from(&cfg.git.repo_path).join("logs")
    };
    let log_writer = Arc::new(WorkflowLogWriter::new(&log_dir, &ticket_key).await);

    let result = run_workflow_steps(
        &ticket_key,
        &config,
        &workflows,
        &actions,
        &event_tx,
        &cancel_token,
        &log_writer,
    )
    .await;

    if let Err(e) = result {
        error!(ticket = %ticket_key, error = %e, "Workflow failed");
        log_writer.write(&format!("WORKFLOW FAILED: {e}")).await;
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(&ticket_key) {
            let source = Box::new(workflow.state.clone());
            workflow.current_step_label = None;
            workflow.state = WorkflowState::Error {
                source_state: source,
                message: e.to_string(),
            };
            workflow.updated_at = Utc::now();
        }

        let _ = event_tx.send(WorkflowEvent {
            event_type: "workflow_error".to_string(),
            workflow_id: String::new(),
            ticket_key: ticket_key.clone(),
            state: "Error".to_string(),
            timestamp: Utc::now(),
            error: Some(e.to_string()),
            step_name: None,
            output_line: None,
            stream: None,
        });
    }
}

async fn run_workflow_steps(
    ticket_key: &str,
    config: &Arc<RwLock<Config>>,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    actions: &Arc<dyn ExternalActions>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    cancel_token: &CancellationToken,
    log_writer: &Arc<WorkflowLogWriter>,
) -> Result<()> {
    // Step 1: Assign ticket
    transition(workflows, event_tx, ticket_key, WorkflowState::Assigning).await;
    let mut step_log = StepLog::new("Assign Ticket".to_string());

    let cfg = config.read().await;
    let repo_path = PathBuf::from(&cfg.git.repo_path);
    let project_keys = cfg.jira.project_keys.clone();
    drop(cfg);

    check_cancelled(cancel_token)?;

    match actions.assign_ticket(ticket_key).await {
        Ok(()) => {
            step_log.output.push("Ticket assigned to current Jira user".to_string());
        }
        Err(e) => {
            step_log.output.push(format!("[DRY/SKIP] {e}"));
            warn!(ticket = ticket_key, error = %e, "Failed to assign ticket, continuing");
        }
    }

    match actions.transition_ticket(ticket_key, "In Progress").await {
        Ok(()) => {
            step_log.output.push("Ticket moved to In Progress".to_string());
        }
        Err(e) => {
            step_log.output.push(format!("[DRY/SKIP] {e}"));
            warn!(ticket = ticket_key, error = %e, "Failed to transition ticket, continuing");
        }
    }

    step_log.complete(StepStatus::Success);
    add_step_log(workflows, ticket_key, step_log).await;

    // Step 2: Retrieve ticket details
    transition(workflows, event_tx, ticket_key, WorkflowState::RetrievingDetails).await;
    let mut step_log = StepLog::new("Retrieve Details".to_string());

    check_cancelled(cancel_token)?;

    let jira_client = JiraClient::new(repo_path.clone());
    let ticket_detail = match jira_client.get_ticket_details(ticket_key, &project_keys).await {
        Ok(detail) => {
            step_log.output.push(format!("Retrieved: {}", detail.summary));
            let mut wf = workflows.write().await;
            if let Some(workflow) = wf.get_mut(ticket_key) {
                workflow.ticket_description = detail.description.clone();
                workflow.ticket_type = detail.item_type.clone();
                workflow.ticket_summary = detail.summary.clone();
            }
            step_log.complete(StepStatus::Success);
            detail
        }
        Err(e) => {
            warn!(ticket = ticket_key, error = %e, "Failed to retrieve ticket details, using minimal context");
            step_log.fail(e.to_string());
            crate::jira::client::JiraTicket {
                key: ticket_key.to_string(),
                summary: workflows.read().await.get(ticket_key).map(|w| w.ticket_summary.clone()).unwrap_or_default(),
                description: String::new(),
                item_type: "Task".to_string(),
                status: "In Progress".to_string(),
                linked_items: Vec::new(),
            }
        }
    };
    add_step_log(workflows, ticket_key, step_log).await;

    // Step 3: Create worktree
    transition(workflows, event_tx, ticket_key, WorkflowState::CreatingWorktree).await;
    let mut step_log = StepLog::new("Create Worktree".to_string());

    check_cancelled(cancel_token)?;

    let branch_name = git::worktree::branch_name_for_ticket(ticket_key, &ticket_detail.item_type);
    let cfg = config.read().await;
    let base_branch = cfg.git.base_branch.clone();
    drop(cfg);

    let worktree_path = actions
        .create_worktree(&branch_name, &base_branch)
        .await?;

    {
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(ticket_key) {
            workflow.branch_name = branch_name.clone();
            workflow.worktree_path = Some(worktree_path.clone());
        }
    }

    step_log.output.push(format!("Branch: {branch_name}"));
    step_log.output.push(format!("Worktree: {}", worktree_path.display()));
    step_log.complete(StepStatus::Success);
    add_step_log(workflows, ticket_key, step_log).await;

    let cfg = config.read().await;
    let pre_install_cmds = cfg.commands.pre_install.clone();
    let install_cmd = cfg.commands.install.clone();
    let shell_stream_provider = cfg.agent.provider;
    drop(cfg);

    // Step 3a: mise — install pinned tools before shell hooks (pre_install / install).
    if crate::process::worktree_has_mise_config(&worktree_path) {
        let mut step_log = StepLog::new("Mise install".to_string());
        info!("Running mise install (project declares mise tools)");
        log_writer
            .write_step("Mise install", "Running: mise install")
            .await;

        broadcast_step_started(event_tx, ticket_key, "Mise install");
        let line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            "Mise install",
            log_writer,
            workflows,
            shell_stream_provider,
        );
        match crate::process::run_command_streaming(
            "mise",
            &["install"],
            &worktree_path,
            cancel_token.child_token(),
            line_tx,
        )
        .await
        {
            Ok(output) if output.success() => {
                step_log.output.push("mise install completed".to_string());
                step_log.complete(StepStatus::Success);
                broadcast_step_completed(event_tx, ticket_key, "Mise install");
            }
            Ok(output) => {
                let stderr_tail = output.stderr.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                let msg = format!("mise install failed (exit code {}):\n{}", output.exit_code, stderr_tail);
                step_log.fail(msg.clone());
                add_step_log(workflows, ticket_key, step_log).await;
                return Err(MaestroError::Git(msg));
            }
            Err(e) => {
                let msg = format!("mise install error: {e}");
                step_log.fail(msg.clone());
                add_step_log(workflows, ticket_key, step_log).await;
                return Err(MaestroError::Git(msg));
            }
        }
        add_step_log(workflows, ticket_key, step_log).await;
    }

    // Step 3b: Pre-install (e.g., registry auth) — each entry is a separate shell command
    if !pre_install_cmds.is_empty() {
        let total = pre_install_cmds.len();
        for (i, pre_install_cmd) in pre_install_cmds.iter().enumerate() {
            let step_name = format!("Pre-install ({}/{})", i + 1, total);
            let mut step_log = StepLog::new(step_name.clone());
            info!(command = %pre_install_cmd, step = i + 1, total, "Running pre-install command");
            log_writer
                .write_step(&step_name, &format!("Running: {pre_install_cmd}"))
                .await;

            broadcast_step_started(event_tx, ticket_key, &step_name);
            let line_tx = spawn_output_relay(
                event_tx,
                ticket_key,
                &step_name,
                log_writer,
                workflows,
                shell_stream_provider,
            );
            match crate::process::run_shell_command_streaming(
                pre_install_cmd,
                &worktree_path,
                cancel_token.child_token(),
                line_tx,
            )
            .await
            {
                Ok(output) if output.success() => {
                    step_log.output.push(format!("{step_name} completed"));
                    step_log.complete(StepStatus::Success);
                    broadcast_step_completed(event_tx, ticket_key, &step_name);
                }
                Ok(output) => {
                    let stderr_tail = output.stderr.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                    let msg = format!("{step_name} failed (exit code {}):\n{}", output.exit_code, stderr_tail);
                    step_log.fail(msg.clone());
                    add_step_log(workflows, ticket_key, step_log).await;
                    return Err(MaestroError::Git(msg));
                }
                Err(e) => {
                    let msg = format!("{step_name} error: {e}");
                    step_log.fail(msg.clone());
                    add_step_log(workflows, ticket_key, step_log).await;
                    return Err(MaestroError::Git(msg));
                }
            }
            add_step_log(workflows, ticket_key, step_log).await;
        }
    }

    // Step 3c: Install dependencies

    if !install_cmd.is_empty() {
        let mut step_log = StepLog::new("Install Dependencies".to_string());
        info!(command = %install_cmd, "Installing dependencies in worktree");
        log_writer.write_step("Install Dependencies", &format!("Running: {install_cmd}")).await;

        broadcast_step_started(event_tx, ticket_key, "Install Dependencies");
        let line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            "Install Dependencies",
            log_writer,
            workflows,
            shell_stream_provider,
        );
        match crate::process::run_shell_command_streaming(
            &install_cmd,
            &worktree_path,
            cancel_token.child_token(),
            line_tx,
        )
        .await
        {
            Ok(output) if output.success() => {
                step_log.output.push("Dependencies installed".to_string());
                step_log.complete(StepStatus::Success);
                broadcast_step_completed(event_tx, ticket_key, "Install Dependencies");
            }
            Ok(output) => {
                let stderr_tail = output.stderr.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                let stdout_tail = output.stdout.lines().rev().take(10).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                let msg = format!("Install failed (exit code {}):\nSTDERR:\n{}\nSTDOUT:\n{}", output.exit_code, stderr_tail, stdout_tail);
                step_log.fail(msg.clone());
                add_step_log(workflows, ticket_key, step_log).await;
                return Err(MaestroError::Git(msg));
            }
            Err(e) => {
                let msg = format!("Install command error: {e}");
                step_log.fail(msg.clone());
                add_step_log(workflows, ticket_key, step_log).await;
                return Err(MaestroError::Git(msg));
            }
        }
        add_step_log(workflows, ticket_key, step_log).await;
    }

    // Ticket context and interpolation vars for [[agent_steps]] prompts
    let ticket_context = build_ticket_context(&ticket_detail);
    let acceptance_criteria = extract_acceptance_criteria(&ticket_detail.description);
    let acceptance_criteria_str = format_acceptance_criteria_block(&acceptance_criteria);

    let mut interp_vars: HashMap<String, String> = HashMap::new();
    interp_vars.insert("ticket_key".into(), ticket_key.to_string());
    interp_vars.insert("ticket_summary".into(), ticket_detail.summary.clone());
    interp_vars.insert("ticket_description".into(), ticket_detail.description.clone());
    interp_vars.insert("ticket_type".into(), ticket_detail.item_type.clone());
    interp_vars.insert("acceptance_criteria".into(), acceptance_criteria_str);
    interp_vars.insert("ticket_context".into(), ticket_context);

    // Steps 4–N: agent_steps (or defaults) × outer loops; each step may repeat (see `repeat` on step)
    let cfg = config.read().await;
    let outer_loops = cfg.agent_sequence_outer_loops();
    let timeout = cfg.claude.step_timeout_secs;
    let claude_model = if cfg.claude.model.is_empty() { None } else { Some(cfg.claude.model.clone()) };
    let cursor_model_buf = cfg.agent.cursor_model.clone();
    let cursor_model_pass = cursor_model_for_cli(&cursor_model_buf);
    let ai_stream_provider = cfg.agent.provider;
    let cursor_cli = cfg.agent.cursor_cli.clone();
    let steps = cfg.resolved_agent_steps();
    let num_steps = steps.len();
    drop(cfg);

    let mut claude_session_id: Option<String> = None;
    // Last agent session stdout (used to parse `MAESTRO_PR_URL:` after the final step).
    let mut last_agent_output: Option<String> = None;

    for outer in 1..=outer_loops {
        check_cancelled(cancel_token)?;
        wait_if_paused(workflows, ticket_key, cancel_token).await?;

        for (step_idx, step) in steps.iter().enumerate() {
            let step_repeat = step.repeat;
            for r in 1..=step_repeat {
                check_cancelled(cancel_token)?;
                wait_if_paused(workflows, ticket_key, cancel_token).await?;

                let step_label = if outer_loops > 1 {
                    format!(
                        "{} (cycle {}/{}, run {}/{})",
                        step.name, outer, outer_loops, r, step_repeat
                    )
                } else {
                    format!("{} (run {}/{})", step.name, r, step_repeat)
                };

                transition_to_agent_step(
                    workflows,
                    event_tx,
                    ticket_key,
                    outer,
                    &step_label,
                )
                .await;

                let mut step_log = StepLog::new(step_label.clone());
                broadcast_step_started(event_tx, ticket_key, &step_label);
                log_writer.write_step(&step_label, "Starting").await;

                let interpolated = interpolate_agent_prompt(&step.prompt, &interp_vars);
                let headless = headless_instructions_suffix(ai_stream_provider);
                let full_prompt = format!("{interpolated}\n\n{headless}");

                let resume_id = if outer == 1 && step_idx == 0 && r == 1 {
                    None
                } else {
                    claude_session_id.as_deref()
                };

                let relay_label = format!(
                    "{} · step {}/{} · run {}/{}",
                    step.name,
                    step_idx + 1,
                    num_steps,
                    r,
                    step_repeat
                );
                let line_tx = spawn_output_relay(
                    event_tx,
                    ticket_key,
                    &relay_label,
                    log_writer,
                    workflows,
                    ai_stream_provider,
                );

                let session_result: Result<(String, String)> = match ai_stream_provider {
                    AiAgentProvider::Claude => ClaudeSession::run_prompt(
                        &worktree_path,
                        &full_prompt,
                        cancel_token.child_token(),
                        timeout,
                        Some(line_tx),
                        claude_model.as_deref(),
                        resume_id,
                    )
                    .await
                    .map(|s| (s.session_id, s.output)),
                    AiAgentProvider::Cursor => CursorSession::run_prompt(
                        &cursor_cli,
                        &worktree_path,
                        &full_prompt,
                        cancel_token.child_token(),
                        timeout,
                        Some(line_tx),
                        Some(cursor_model_pass),
                        resume_id,
                    )
                    .await
                    .map(|s| (s.session_id, s.output)),
                };

                let is_last_run_of_outer_cycle =
                    step_idx + 1 == num_steps && r == step_repeat;

                match session_result {
                    Ok((session_id, output)) => {
                        info!(
                            session_id = %session_id,
                            outer = outer,
                            step = %step.name,
                            run = r,
                            "Agent step session completed"
                        );
                        claude_session_id = Some(session_id.clone());
                        last_agent_output = Some(output);
                        step_log
                            .output
                            .push(format!("Session {session_id} completed"));

                        step_log.complete(StepStatus::Success);
                    }
                    Err(e) => {
                        warn!(
                            outer = outer,
                            step = %step.name,
                            run = r,
                            error = %e,
                            "Agent step session failed"
                        );
                        step_log.fail(e.to_string());
                        if !is_last_run_of_outer_cycle {
                            add_step_log(workflows, ticket_key, step_log).await;
                            error!(ticket = %ticket_key, "Agent step AI session failed — aborting workflow");
                            let msg = match ai_stream_provider {
                                AiAgentProvider::Claude => "Agent step failed — check that Claude Code is authenticated in the container".to_string(),
                                AiAgentProvider::Cursor => "Agent step failed — check Cursor Agent (`agent login` or CURSOR_API_KEY) and agent.cursor_cli".to_string(),
                            };
                            return Err(MaestroError::AiAgent(msg));
                        }
                        // Last run of this outer cycle: non-fatal (legacy review behavior)
                    }
                }

                add_step_log(workflows, ticket_key, step_log).await;
            }
        }
    }

    // Agent sequence finished — no engine-driven PR (open a PR from an agent step if required).
    check_cancelled(cancel_token)?;
    wait_if_paused(workflows, ticket_key, cancel_token).await?;

    {
        let wf = workflows.read().await;
        if let Some(workflow) = wf.get(ticket_key) {
            let failed_steps: Vec<_> = workflow
                .steps_log
                .iter()
                .filter(|s| s.status == StepStatus::Failed)
                .map(|s| s.step_name.as_str())
                .collect();

            if !failed_steps.is_empty() {
                let msg = format!("Workflow incomplete — failed steps: {}", failed_steps.join(", "));
                warn!(ticket = %ticket_key, message = %msg);

                let mut step_log = StepLog::new("Workflow complete".to_string());
                step_log.fail(msg.clone());
                drop(wf);
                add_step_log(workflows, ticket_key, step_log).await;

                return Err(MaestroError::Config(msg));
            }
        }
    }

    let pr_url = resolve_pr_url(&worktree_path, last_agent_output.as_deref());
    let mut complete_log = StepLog::new("Workflow complete".to_string());
    if let Some(ref url) = pr_url {
        complete_log.output.push(format!("Recorded PR URL: {url}"));
        let mut wf = workflows.write().await;
        if let Some(w) = wf.get_mut(ticket_key) {
            w.pr_url = Some(url.clone());
        }
    } else {
        complete_log.output.push(
            "Agent steps finished. No PR URL supplied (optional: .maestro/outcome.toml or MAESTRO_PR_URL: line)."
                .to_string(),
        );
    }
    complete_log.complete(StepStatus::Success);
    add_step_log(workflows, ticket_key, complete_log).await;

    transition(workflows, event_tx, ticket_key, WorkflowState::Done).await;
    info!(ticket = ticket_key, "Workflow completed successfully");

    Ok(())
}

fn build_ticket_context(ticket: &crate::jira::client::JiraTicket) -> String {
    let mut context = format!(
        "Ticket: {key}\nSummary: {summary}\n\nDescription:\n{description}",
        key = ticket.key,
        summary = ticket.summary,
        description = ticket.description,
    );

    // Extract acceptance criteria from description
    let ac = extract_acceptance_criteria(&ticket.description);
    if !ac.is_empty() {
        context.push_str("\n\n## Acceptance Criteria\n");
        for criterion in &ac {
            context.push_str(&format!("- {criterion}\n"));
        }
    }

    if !ticket.linked_items.is_empty() {
        context.push_str("\n\n## Linked Items\n");
        for item in &ticket.linked_items {
            context.push_str(&format!(
                "\n### {key} ({link_type})\nSummary: {summary}\nStatus: {status}\nDescription: {description}\n",
                key = item.key,
                link_type = item.link_type,
                summary = item.summary,
                status = item.status,
                description = item.description,
            ));
        }
    }

    context
}

fn broadcast_step_started(
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    step_name: &str,
) {
    let receiver_count = event_tx.receiver_count();
    info!(
        ticket = ticket_key,
        step = step_name,
        receivers = receiver_count,
        "Broadcasting step_started"
    );
    let _ = event_tx.send(WorkflowEvent {
        event_type: "step_started".to_string(),
        workflow_id: String::new(),
        ticket_key: ticket_key.to_string(),
        state: String::new(),
        timestamp: Utc::now(),
        error: None,
        step_name: Some(step_name.to_string()),
        output_line: None,
        stream: None,
    });
}

fn broadcast_step_completed(
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    step_name: &str,
) {
    let _ = event_tx.send(WorkflowEvent {
        event_type: "step_completed".to_string(),
        workflow_id: String::new(),
        ticket_key: ticket_key.to_string(),
        state: String::new(),
        timestamp: Utc::now(),
        error: None,
        step_name: Some(step_name.to_string()),
        output_line: None,
        stream: None,
    });
}

fn spawn_output_relay(
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    step_name: &str,
    log_writer: &Arc<WorkflowLogWriter>,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    stream_provider: AiAgentProvider,
) -> tokio::sync::mpsc::UnboundedSender<OutputLine> {
    let (line_tx, mut line_rx) = tokio::sync::mpsc::unbounded_channel::<OutputLine>();
    let event_tx = event_tx.clone();
    let ticket_key = ticket_key.to_string();
    let step_name = step_name.to_string();
    let log_writer = log_writer.clone();
    let workflows = workflows.clone();

    tokio::spawn(async move {
        while let Some(line) = line_rx.recv().await {
            // Always write raw output to log file
            log_writer
                .write_output(&step_name, &line.stream, &line.content)
                .await;

            // Parse and humanize the output for display
            let humanized = humanize_agent_stream_line(stream_provider, &line.content);
            if let Some(display_text) = humanized {
                // Store in workflow's terminal_lines for persistence
                {
                    let mut wf = workflows.write().await;
                    if let Some(workflow) = wf.get_mut(&ticket_key) {
                        workflow.terminal_lines.push(TerminalLine {
                            text: display_text.clone(),
                            stream: line.stream.clone(),
                        });
                        // Cap at TERMINAL_LINES_MAX
                        if workflow.terminal_lines.len() > TERMINAL_LINES_MAX {
                            let drain_count = workflow.terminal_lines.len() - TERMINAL_LINES_MAX;
                            workflow.terminal_lines.drain(..drain_count);
                        }
                    }
                }

                // Broadcast humanized text to WebSocket
                let result = event_tx.send(WorkflowEvent {
                    event_type: "step_output".to_string(),
                    workflow_id: String::new(),
                    ticket_key: ticket_key.clone(),
                    state: String::new(),
                    timestamp: Utc::now(),
                    error: None,
                    step_name: Some(step_name.clone()),
                    output_line: Some(display_text),
                    stream: Some(line.stream),
                });
                match result {
                    Ok(count) => {
                        debug!(receivers = count, step = %step_name, "step_output broadcast sent");
                    }
                    Err(_) => {
                        warn!(step = %step_name, "step_output broadcast: no receivers");
                    }
                }
            }
        }
    });

    line_tx
}

async fn transition_to_agent_step(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    pass: u8,
    step_label: &str,
) {
    info!(
        ticket = %ticket_key,
        pass,
        step = %step_label,
        "Agent step (state + dashboard label)"
    );

    let mut wf = workflows.write().await;
    if let Some(workflow) = wf.get_mut(ticket_key) {
        workflow.state = WorkflowState::AddressingTicket { pass };
        workflow.current_step_label = Some(step_label.to_string());
        workflow.updated_at = Utc::now();
        let display = workflow.status_display();
        let id = workflow.id.clone();
        let _ = event_tx.send(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id: id,
            ticket_key: ticket_key.to_string(),
            state: display,
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
        });
    }
}

async fn transition(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    new_state: WorkflowState,
) {
    let state_name = new_state.display_name();
    info!(ticket = ticket_key, state = %state_name, "Transitioning workflow");

    let mut wf = workflows.write().await;
    if let Some(workflow) = wf.get_mut(ticket_key) {
        workflow.current_step_label = None;
        workflow.state = new_state;
        workflow.updated_at = Utc::now();
        let display = workflow.status_display();

        let _ = event_tx.send(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id: workflow.id.clone(),
            ticket_key: ticket_key.to_string(),
            state: display,
            timestamp: Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
        });
    }
}

async fn add_step_log(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
    step_log: StepLog,
) {
    let mut wf = workflows.write().await;
    if let Some(workflow) = wf.get_mut(ticket_key) {
        workflow.steps_log.push(step_log);
    }
}

async fn wait_if_paused(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
    cancel_token: &CancellationToken,
) -> Result<()> {
    loop {
        let is_paused = {
            let wf = workflows.read().await;
            wf.get(ticket_key)
                .is_some_and(|w| matches!(w.state, WorkflowState::Paused { .. }))
        };

        if !is_paused {
            return Ok(());
        }

        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Err(MaestroError::Cancelled);
            }
            _ = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                // Check again
            }
        }
    }
}

fn check_cancelled(cancel_token: &CancellationToken) -> Result<()> {
    if cancel_token.is_cancelled() {
        Err(MaestroError::Cancelled)
    } else {
        Ok(())
    }
}

fn format_acceptance_criteria_block(criteria: &[String]) -> String {
    if criteria.is_empty() {
        "(none extracted from ticket)".to_string()
    } else {
        criteria
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{}. {}", i + 1, s))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn extract_acceptance_criteria(description: &str) -> Vec<String> {
    let mut criteria = Vec::new();
    let mut in_ac_section = false;

    for line in description.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Detect start of acceptance criteria section
        if lower.contains("acceptance criteria")
            || lower.contains("acceptance criterion")
            || lower.starts_with("ac:")
        {
            in_ac_section = true;
            continue;
        }

        // Detect end of section (next heading)
        if in_ac_section && (trimmed.starts_with('#') || trimmed.starts_with("##")) {
            in_ac_section = false;
            continue;
        }

        // Collect bullet points / numbered items in AC section
        if in_ac_section {
            let cleaned = trimmed
                .trim_start_matches('-')
                .trim_start_matches('*')
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.')
                .trim();
            if !cleaned.is_empty() {
                criteria.push(cleaned.to_string());
            }
        }
    }

    criteria
}

#[cfg(test)]
mod jira_browse_url_tests {
    /// Builds `https://…/browse/TICKET` from `[jira] site` (kept for URL normalization tests).
    fn jira_ticket_browse_url(site: &str, ticket_key: &str) -> String {
        let mut s = site.trim();
        if s.is_empty() {
            return format!("https://jira.atlassian.net/browse/{ticket_key}");
        }
        if let Some(rest) = s.strip_prefix("https://") {
            s = rest;
        } else if let Some(rest) = s.strip_prefix("http://") {
            s = rest;
        }
        let s = s.trim().trim_end_matches('/');
        if s.is_empty() {
            return format!("https://jira.atlassian.net/browse/{ticket_key}");
        }
        format!("https://{s}/browse/{ticket_key}")
    }

    #[test]
    fn empty_site_uses_legacy_atlassian_host() {
        assert_eq!(
            jira_ticket_browse_url("", "PROJ-1"),
            "https://jira.atlassian.net/browse/PROJ-1"
        );
        assert_eq!(
            jira_ticket_browse_url("   ", "PROJ-1"),
            "https://jira.atlassian.net/browse/PROJ-1"
        );
    }

    #[test]
    fn host_only_site() {
        assert_eq!(
            jira_ticket_browse_url("acme.atlassian.net", "CORE-42"),
            "https://acme.atlassian.net/browse/CORE-42"
        );
    }

    #[test]
    fn site_with_https_prefix_and_trailing_slash() {
        assert_eq!(
            jira_ticket_browse_url("https://acme.atlassian.net/", "X-9"),
            "https://acme.atlassian.net/browse/X-9"
        );
    }

    #[test]
    fn site_with_context_path() {
        assert_eq!(
            jira_ticket_browse_url("https://jira.corp.example.com/jira", "BUG-1"),
            "https://jira.corp.example.com/jira/browse/BUG-1"
        );
    }
}
