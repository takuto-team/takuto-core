use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::actions::traits::ExternalActions;
use crate::claude::pm_agent::{PmAgent, PmVerdict};
use crate::claude::session::ClaudeSession;
use crate::config::{cursor_model_for_cli, AiAgentProvider, Config};
use crate::cursor::session::CursorSession;
use crate::error::{MaestroError, Result};
use crate::git;
use crate::jira::client::JiraClient;
use crate::process::OutputLine;

use super::log_writer::WorkflowLogWriter;
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

            self.broadcast_event(WorkflowEvent {
                event_type: "workflow_updated".to_string(),
                workflow_id: workflow.id.clone(),
                ticket_key: ticket_key.to_string(),
                state: workflow.state.display_name(),
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

    match actions.assign_ticket(ticket_key, "maestro").await {
        Ok(()) => {
            step_log.output.push("Ticket assigned to maestro".to_string());
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

    // Step 3a: mise — install pinned tools before shell hooks (pre_install / install / lint / test).
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

    // Build ticket context for Claude
    let ticket_context = build_ticket_context(&ticket_detail);

    // Steps 4-9: Address ticket passes (3 rounds of address + review)
    let cfg = config.read().await;
    let passes = cfg.claude.address_ticket_passes;
    let timeout = cfg.claude.step_timeout_secs;
    let claude_model = if cfg.claude.model.is_empty() { None } else { Some(cfg.claude.model.clone()) };
    let cursor_model_buf = cfg.agent.cursor_model.clone();
    let cursor_model_pass = cursor_model_for_cli(&cursor_model_buf);
    let ai_stream_provider = cfg.agent.provider;
    let cursor_cli = cfg.agent.cursor_cli.clone();
    let agent_cfg_for_pm = cfg.agent.clone();
    let model_for_pm = match cfg.agent.provider {
        AiAgentProvider::Claude => cfg.claude.model.clone(),
        AiAgentProvider::Cursor => cfg.agent.cursor_model.clone(),
    };
    drop(cfg);

    // Create PM agent for plan validation
    let acceptance_criteria = extract_acceptance_criteria(&ticket_detail.description);
    let pm_agent = PmAgent::new(ticket_detail.description.clone(), acceptance_criteria);

    let mut has_critical_failure = false;
    // Track Claude session ID across passes for --resume (keeps conversation context)
    let mut claude_session_id: Option<String> = None;

    for pass in 1..=passes {
        check_cancelled(cancel_token)?;

        // Wait if paused
        wait_if_paused(workflows, ticket_key, cancel_token).await?;

        // Address ticket
        transition(
            workflows,
            event_tx,
            ticket_key,
            WorkflowState::AddressingTicket { pass },
        )
        .await;
        let step_label = format!("Address Ticket (Pass {pass}/{passes})");
        let mut step_log = StepLog::new(step_label.clone());
        broadcast_step_started(event_tx, ticket_key, &step_label);
        log_writer.write_step(&step_label, "Starting").await;

        let address_line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            &format!("Address Ticket (Pass {pass}/{passes})"),
            log_writer,
            workflows,
            ai_stream_provider,
        );
        let address_result = match ai_stream_provider {
            AiAgentProvider::Claude => {
                ClaudeSession::start_address_ticket(
                    &worktree_path,
                    &ticket_context,
                    cancel_token.child_token(),
                    timeout,
                    Some(address_line_tx),
                    claude_model.as_deref(),
                    claude_session_id.as_deref(),
                )
                .await
                .map(|s| (s.session_id, s.output))
            }
            AiAgentProvider::Cursor => {
                CursorSession::start_address_ticket(
                    &cursor_cli,
                    &worktree_path,
                    &ticket_context,
                    cancel_token.child_token(),
                    timeout,
                    Some(address_line_tx),
                    Some(cursor_model_pass),
                    claude_session_id.as_deref(),
                )
                .await
                .map(|s| (s.session_id, s.output))
            }
        };

        match address_result
        {
            Ok((session_id, output)) => {
                info!(session_id = %session_id, pass = pass, "Address ticket session completed");
                claude_session_id = Some(session_id.clone());
                step_log.output.push(format!("Session {session_id} completed"));

                // PM agent validates the session output against ticket requirements
                match pm_agent
                    .validate_plan(
                        &output,
                        &worktree_path,
                        cancel_token.child_token(),
                        &agent_cfg_for_pm,
                        &model_for_pm,
                    )
                    .await
                {
                    Ok(PmVerdict::Approved) => {
                        step_log.output.push("PM agent: APPROVED".to_string());
                    }
                    Ok(PmVerdict::Rejected { reasons }) => {
                        let reasons_str = reasons.join("; ");
                        step_log.output.push(format!("PM agent: REJECTED — {reasons_str}"));
                        warn!(pass = pass, reasons = %reasons_str, "PM agent rejected plan");
                    }
                    Err(e) => {
                        step_log.output.push(format!("PM agent validation failed: {e}"));
                        warn!(pass = pass, error = %e, "PM agent validation error, continuing");
                    }
                }

                step_log.complete(StepStatus::Success);
            }
            Err(e) => {
                warn!(pass = pass, error = %e, "Address ticket session failed");
                step_log.fail(e.to_string());
                has_critical_failure = true;
            }
        }
        add_step_log(workflows, ticket_key, step_log).await;

        if has_critical_failure {
            error!(ticket = %ticket_key, "Address ticket AI session failed — aborting workflow");
            let msg = match ai_stream_provider {
                AiAgentProvider::Claude => "Address ticket session failed — check that Claude Code is authenticated in the container".to_string(),
                AiAgentProvider::Cursor => "Address ticket session failed — check Cursor Agent (`agent login` or CURSOR_API_KEY) and agent.cursor_cli".to_string(),
            };
            return Err(MaestroError::AiAgent(msg));
        }

        check_cancelled(cancel_token)?;
        wait_if_paused(workflows, ticket_key, cancel_token).await?;

        // Review changes — resume same session for context continuity
        transition(workflows, event_tx, ticket_key, WorkflowState::Reviewing).await;
        let review_label = format!("Review Changes (Pass {pass}/{passes})");
        let mut step_log = StepLog::new(review_label.clone());
        broadcast_step_started(event_tx, ticket_key, &review_label);
        log_writer.write_step(&review_label, "Starting").await;

        let review_line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            &format!("Review Changes (Pass {pass}/{passes})"),
            log_writer,
            workflows,
            ai_stream_provider,
        );
        let review_result = match ai_stream_provider {
            AiAgentProvider::Claude => {
                ClaudeSession::start_review_changes(
                    &worktree_path,
                    cancel_token.child_token(),
                    timeout,
                    Some(review_line_tx),
                    claude_model.as_deref(),
                    claude_session_id.as_deref(),
                )
                .await
                .map(|s| s.session_id)
            }
            AiAgentProvider::Cursor => {
                CursorSession::start_review_changes(
                    &cursor_cli,
                    &worktree_path,
                    cancel_token.child_token(),
                    timeout,
                    Some(review_line_tx),
                    Some(cursor_model_pass),
                    claude_session_id.as_deref(),
                )
                .await
                .map(|s| s.session_id)
            }
        };

        match review_result
        {
            Ok(session_id) => {
                claude_session_id = Some(session_id.clone());
                step_log.output.push(format!("Review session {session_id} completed"));
                step_log.complete(StepStatus::Success);
            }
            Err(e) => {
                warn!(pass = pass, error = %e, "Review changes session failed");
                step_log.fail(e.to_string());
                // Review failure is non-fatal, continue to next pass
            }
        }
        add_step_log(workflows, ticket_key, step_log).await;
    }

    // Step 10: Linting
    let cfg = config.read().await;
    let lint_cmd = cfg.commands.lint.clone();
    let max_fix = cfg.general.max_fix_attempts;
    let unit_test_cmd = cfg.commands.unit_test.clone();
    let e2e_test_cmd = cfg.commands.e2e_test.clone();
    let fix_ai_stream_provider = cfg.agent.provider;
    let fix_cursor_cli = cfg.agent.cursor_cli.clone();
    drop(cfg);

    check_cancelled(cancel_token)?;
    wait_if_paused(workflows, ticket_key, cancel_token).await?;

    if !lint_cmd.is_empty() {
        transition(workflows, event_tx, ticket_key, WorkflowState::Linting).await;
        run_fix_loop(
            "Lint",
            &lint_cmd,
            "Fix the following lint errors",
            &worktree_path,
            actions,
            cancel_token,
            max_fix,
            timeout,
            workflows,
            ticket_key,
            event_tx,
            log_writer,
            claude_model.as_deref(),
            claude_session_id.as_deref(),
            Some(cursor_model_pass),
            fix_ai_stream_provider,
            &fix_cursor_cli,
        )
        .await;

        // Commit after linting
        if let Err(e) = actions.commit_changes(&worktree_path, "fix: lint fixes").await {
            warn!(error = %e, "Failed to commit lint fixes");
        }
    }

    // Step 11: Unit tests
    check_cancelled(cancel_token)?;
    wait_if_paused(workflows, ticket_key, cancel_token).await?;

    if !unit_test_cmd.is_empty() {
        transition(workflows, event_tx, ticket_key, WorkflowState::UnitTesting).await;
        run_fix_loop(
            "Unit Tests",
            &unit_test_cmd,
            "Fix the following unit test failures",
            &worktree_path,
            actions,
            cancel_token,
            max_fix,
            timeout,
            workflows,
            ticket_key,
            event_tx,
            log_writer,
            claude_model.as_deref(),
            claude_session_id.as_deref(),
            Some(cursor_model_pass),
            fix_ai_stream_provider,
            &fix_cursor_cli,
        )
        .await;

        if let Err(e) = actions
            .commit_changes(&worktree_path, "fix: unit test fixes")
            .await
        {
            warn!(error = %e, "Failed to commit unit test fixes");
        }
    }

    // Step 12: E2E tests
    check_cancelled(cancel_token)?;
    wait_if_paused(workflows, ticket_key, cancel_token).await?;

    if !e2e_test_cmd.is_empty() {
        transition(workflows, event_tx, ticket_key, WorkflowState::E2ETesting).await;
        run_fix_loop(
            "E2E Tests",
            &e2e_test_cmd,
            "Fix the following e2e test failures",
            &worktree_path,
            actions,
            cancel_token,
            max_fix,
            timeout,
            workflows,
            ticket_key,
            event_tx,
            log_writer,
            claude_model.as_deref(),
            claude_session_id.as_deref(),
            Some(cursor_model_pass),
            fix_ai_stream_provider,
            &fix_cursor_cli,
        )
        .await;

        if let Err(e) = actions
            .commit_changes(&worktree_path, "fix: e2e test fixes")
            .await
        {
            warn!(error = %e, "Failed to commit e2e test fixes");
        }
    }

    // Step 13: Create PR — only if there were no critical failures
    check_cancelled(cancel_token)?;
    wait_if_paused(workflows, ticket_key, cancel_token).await?;

    // Check if any steps failed — if so, don't create PR
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
                let msg = format!("Skipping PR creation — failed steps: {}", failed_steps.join(", "));
                warn!(ticket = %ticket_key, message = %msg);

                let mut step_log = StepLog::new("Create PR".to_string());
                step_log.fail(msg.clone());
                drop(wf);
                add_step_log(workflows, ticket_key, step_log).await;

                return Err(MaestroError::Config(msg));
            }
        }
    }

    transition(workflows, event_tx, ticket_key, WorkflowState::CreatingPR).await;
    let mut step_log = StepLog::new("Create PR".to_string());

    let pr_title = git::pr::pr_title(ticket_key, &ticket_detail.summary, &ticket_detail.item_type);
    let pr_body = build_pr_body(ticket_key, &ticket_detail, workflows).await;

    match actions
        .create_pr(&pr_title, &pr_body, &branch_name, &base_branch)
        .await
    {
        Ok(pr_url) => {
            info!(ticket = ticket_key, pr_url = %pr_url, "PR created");
            step_log.output.push(format!("PR created: {pr_url}"));
            step_log.complete(StepStatus::Success);

            let mut wf = workflows.write().await;
            if let Some(workflow) = wf.get_mut(ticket_key) {
                workflow.pr_url = Some(pr_url);
            }
        }
        Err(e) => {
            warn!(ticket = ticket_key, error = %e, "Failed to create PR");
            step_log.fail(e.to_string());
        }
    }
    add_step_log(workflows, ticket_key, step_log).await;

    // Done
    transition(workflows, event_tx, ticket_key, WorkflowState::Done).await;
    info!(ticket = ticket_key, "Workflow completed successfully");

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_fix_loop(
    step_name: &str,
    command: &str,
    fix_instructions: &str,
    worktree_path: &PathBuf,
    _actions: &Arc<dyn ExternalActions>,
    cancel_token: &CancellationToken,
    max_attempts: u32,
    timeout: u64,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    log_writer: &Arc<WorkflowLogWriter>,
    claude_model: Option<&str>,
    claude_session_id: Option<&str>,
    cursor_model: Option<&str>,
    ai_stream_provider: AiAgentProvider,
    cursor_cli: &str,
) {
    let mut step_log = StepLog::new(step_name.to_string());
    broadcast_step_started(event_tx, ticket_key, step_name);
    log_writer.write_step(step_name, &format!("Running: {command}")).await;

    for attempt in 1..=max_attempts {
        check_cancelled_silent(cancel_token);

        info!(step = step_name, attempt = attempt, "Running command");
        step_log.output.push(format!("Attempt {attempt}/{max_attempts}: {command}"));

        let line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            step_name,
            log_writer,
            workflows,
            ai_stream_provider,
        );
        match crate::process::run_shell_command_streaming(
            command,
            worktree_path,
            cancel_token.child_token(),
            line_tx,
        )
        .await
        {
            Ok(output) if output.success() => {
                info!(step = step_name, "Command passed");
                step_log.output.push("PASSED".to_string());
                step_log.complete(StepStatus::Success);
                broadcast_step_completed(event_tx, ticket_key, step_name);
                add_step_log(workflows, ticket_key, step_log).await;
                return;
            }
            Ok(output) => {
                warn!(step = step_name, attempt = attempt, "Command failed");
                step_log.output.push(format!("FAILED (exit code {})", output.exit_code));

                if attempt < max_attempts {
                    let error_output = if output.stderr.is_empty() {
                        &output.stdout
                    } else {
                        &output.stderr
                    };

                    info!(step = step_name, "Spawning AI agent to fix errors");
                    let fix_line_tx = spawn_output_relay(
                        event_tx,
                        ticket_key,
                        &format!("{step_name} (fix)"),
                        log_writer,
                        workflows,
                        ai_stream_provider,
                    );
                    match ai_stream_provider {
                        AiAgentProvider::Claude => {
                            match ClaudeSession::start_fix_session(
                                worktree_path,
                                error_output,
                                fix_instructions,
                                cancel_token.child_token(),
                                timeout,
                                Some(fix_line_tx),
                                claude_model,
                                claude_session_id,
                            )
                            .await
                            {
                                Ok(_) => {
                                    step_log.output.push("Fix session completed".to_string());
                                }
                                Err(e) => {
                                    step_log.output.push(format!("Fix session failed: {e}"));
                                }
                            }
                        }
                        AiAgentProvider::Cursor => {
                            match CursorSession::start_fix_session(
                                cursor_cli,
                                worktree_path,
                                error_output,
                                fix_instructions,
                                cancel_token.child_token(),
                                timeout,
                                Some(fix_line_tx),
                                cursor_model,
                                claude_session_id,
                            )
                            .await
                            {
                                Ok(_) => {
                                    step_log.output.push("Fix session completed".to_string());
                                }
                                Err(e) => {
                                    step_log.output.push(format!("Fix session failed: {e}"));
                                }
                            }
                        }
                    }
                }
            }
            Err(e) => {
                warn!(step = step_name, error = %e, "Command execution error");
                step_log.output.push(format!("Execution error: {e}"));
                break;
            }
        }
    }

    step_log.fail(format!("{step_name} failed after {max_attempts} attempts"));
    add_step_log(workflows, ticket_key, step_log).await;
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

async fn build_pr_body(
    ticket_key: &str,
    ticket: &crate::jira::client::JiraTicket,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
) -> String {
    let wf = workflows.read().await;
    let (step_summary, test_results) = if let Some(workflow) = wf.get(ticket_key) {
        let summary = workflow
            .steps_log
            .iter()
            .map(|s| {
                let status = match s.status {
                    StepStatus::Success => "pass",
                    StepStatus::Failed => "FAIL",
                    StepStatus::Skipped => "skip",
                    StepStatus::Running => "...",
                };
                format!("- [{}] {}", status, s.step_name)
            })
            .collect::<Vec<_>>()
            .join("\n");

        // Extract test/lint result counts from step logs
        let mut results = Vec::new();
        for step in &workflow.steps_log {
            let name = step.step_name.as_str();
            if name == "Lint" || name == "Unit Tests" || name == "E2E Tests" {
                let passed = step.status == StepStatus::Success;
                let attempts = step
                    .output
                    .iter()
                    .filter(|l| l.starts_with("Attempt "))
                    .count();
                let status_str = if passed { "passed" } else { "failed" };
                results.push(format!("| {name} | {status_str} | {attempts} |"));
            }
        }

        let test_section = if results.is_empty() {
            String::new()
        } else {
            format!(
                "\n## Test Results\n\n| Check | Result | Attempts |\n|-------|--------|----------|\n{}\n",
                results.join("\n")
            )
        };

        (summary, test_section)
    } else {
        (String::new(), String::new())
    };

    format!(
        r#"## {summary}

Jira: [{key}](https://jira.atlassian.net/browse/{key})

## Steps

{step_summary}
{test_results}
---
_Auto-generated by Maestro_"#,
        summary = ticket.summary,
        key = ticket_key,
        step_summary = step_summary,
        test_results = test_results,
    )
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
        workflow.state = new_state;
        workflow.updated_at = Utc::now();

        let _ = event_tx.send(WorkflowEvent {
            event_type: "workflow_updated".to_string(),
            workflow_id: workflow.id.clone(),
            ticket_key: ticket_key.to_string(),
            state: state_name,
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

fn check_cancelled_silent(cancel_token: &CancellationToken) {
    if cancel_token.is_cancelled() {
        info!("Workflow cancellation detected");
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
