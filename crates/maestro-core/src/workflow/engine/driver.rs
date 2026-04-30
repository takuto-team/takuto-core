// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tokio::sync::{RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::actions::traits::ExternalActions;
use crate::agent_prompt::{headless_instructions_suffix, report_injection_suffix};
use crate::claude::session::ClaudeSession;
use crate::config::{
    AgentStepConfig, AiAgentProvider, Config, cursor_model_for_cli, interpolate_agent_prompt,
    interpolate_command_template,
};
use crate::container::ContainerRunner;
use crate::cursor::session::CursorSession;
use crate::error::{MaestroError, Result};
use crate::git;
use crate::jira::client::JiraClient;
use crate::process::OutputLine;

use crate::workflow::helpers::{
    build_skill_search_paths, build_ticket_context, check_cancelled, extract_acceptance_criteria,
    format_acceptance_criteria_block, parse_gh_issue_number, step_already_succeeded,
};
use crate::workflow::log_writer::WorkflowLogWriter;
use crate::workflow::outcome::resolve_pr_url;
use crate::workflow::state::WorkflowState;
use crate::workflow::step::{StepLog, StepStatus};
use crate::workflow::stream_humanize::humanize_agent_stream_line;

use super::types::{TerminalLine, Workflow, WorkflowEvent};

/// Maximum number of terminal lines stored per workflow for persistence.
const TERMINAL_LINES_MAX: usize = 100;

/// Scan the definitions directory and return a sorted list of `(filename, modified_time)` tuples
/// for change detection.
pub(super) fn scan_definitions_dir(dir: &Path) -> Vec<(String, std::time::SystemTime)> {
    let mut entries = Vec::new();
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return entries;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if (ext == Some("yml") || ext == Some("yaml"))
            && let Ok(meta) = path.metadata()
        {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string();
            if !name.is_empty() {
                entries.push((name, meta.modified().unwrap_or(std::time::UNIX_EPOCH)));
            }
        }
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn drive_workflow_def(
    ticket_key: String,
    def_name: String,
    steps: Vec<AgentStepConfig>,
    worktree_path: Option<PathBuf>,
    ticket_summary: String,
    ticket_description: String,
    ticket_type: String,
    config: Arc<RwLock<Config>>,
    workflows: Arc<RwLock<HashMap<String, Workflow>>>,
    actions: Arc<dyn ExternalActions>,
    event_tx: broadcast::Sender<WorkflowEvent>,
    cancel_token: CancellationToken,
    agent_run_semaphore: Arc<Semaphore>,
    suppress_cancelled_as_error: Arc<AtomicBool>,
) {
    use crate::workflow::definitions::WorkflowDefRunState;

    info!(ticket = %ticket_key, def = %def_name, "Workflow definition driver started");

    let log_dir = {
        let cfg = config.read().await;
        PathBuf::from(&cfg.git.repo_path).join("logs")
    };
    let log_writer = Arc::new(WorkflowLogWriter::new(&log_dir, &ticket_key).await);

    let result = async {
        // Bootstrap if no worktree exists yet (Pending workflow, first run).
        let (resolved_wt, ts, td, tt) = match worktree_path {
            Some(p) => (p, ticket_summary, ticket_description, ticket_type),
            None => {
                let (wt, ticket_detail) = bootstrap_new_workflow(
                    &ticket_key,
                    &config,
                    &workflows,
                    &actions,
                    &event_tx,
                    &cancel_token,
                    &log_writer,
                    &agent_run_semaphore,
                )
                .await?;
                (
                    wt,
                    ticket_detail.summary,
                    ticket_detail.description,
                    ticket_detail.item_type,
                )
            }
        };
        run_workflow_def_steps(
            &ticket_key,
            &def_name,
            &steps,
            &resolved_wt,
            &ts,
            &td,
            &tt,
            &config,
            &workflows,
            &actions,
            &event_tx,
            &cancel_token,
            &log_writer,
            &agent_run_semaphore,
        )
        .await
    }
    .await;

    // Always clean up worker containers regardless of success/failure
    ContainerRunner::cleanup_for_ticket(&ticket_key).await;

    let workflow_id = {
        let wf = workflows.read().await;
        wf.get(&ticket_key)
            .map(|w| w.id.clone())
            .unwrap_or_default()
    };

    match result {
        Ok(()) => {
            // Set state to Completed
            {
                let mut wf_map = workflows.write().await;
                if let Some(w) = wf_map.get_mut(&ticket_key) {
                    w.workflow_def_runs
                        .insert(def_name.clone(), WorkflowDefRunState::Completed);
                    w.updated_at = Utc::now();
                }
            }

            info!(ticket = %ticket_key, def = %def_name, "Workflow definition completed");

            let _ = event_tx.send(WorkflowEvent {
                event_type: "workflow_updated".to_string(),
                workflow_id,
                ticket_key: ticket_key.clone(),
                state: {
                    let wf = workflows.read().await;
                    wf.get(&ticket_key)
                        .map(|w| w.status_display())
                        .unwrap_or_default()
                },
                timestamp: Utc::now(),
                error: None,
                step_name: None,
                output_line: None,
                stream: None,
                progress_percent: None,
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
            });
        }
        Err(e) => {
            if matches!(e, MaestroError::Cancelled)
                && suppress_cancelled_as_error.load(Ordering::SeqCst)
            {
                info!(
                    ticket = %ticket_key,
                    def = %def_name,
                    "Workflow def driver cancelled during shutdown; state preserved for resume"
                );
                return;
            }

            // When the user explicitly stops a workflow, the cancel token fires and
            // the parent workflow state transitions to Stopped before the driver
            // processes the cancellation. Do not overwrite the def run state with
            // Error when the workflow was intentionally stopped or removed.
            if matches!(e, MaestroError::Cancelled) {
                let snapshot = {
                    let wf = workflows.read().await;
                    wf.get(&ticket_key).map(|w| w.state.clone())
                };
                match snapshot {
                    None => {
                        info!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Workflow def driver cancelled; row no longer in map"
                        );
                        return;
                    }
                    Some(WorkflowState::Stopped) => {
                        info!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Workflow def driver cancelled; left in Stopped (operator stop)"
                        );
                        return;
                    }
                    Some(WorkflowState::Paused { .. }) => {
                        info!(
                            ticket = %ticket_key,
                            def = %def_name,
                            "Workflow def driver cancelled; left in Paused (resume will spawn a new driver)"
                        );
                        return;
                    }
                    _ => {}
                }
            }

            error!(ticket = %ticket_key, def = %def_name, error = %e, "Workflow definition failed");
            log_writer
                .write(&format!("WORKFLOW DEF '{}' FAILED: {e}", def_name))
                .await;

            {
                let mut wf_map = workflows.write().await;
                if let Some(w) = wf_map.get_mut(&ticket_key) {
                    w.workflow_def_runs.insert(
                        def_name.clone(),
                        WorkflowDefRunState::Error {
                            message: e.to_string(),
                        },
                    );
                    w.updated_at = Utc::now();
                }
            }

            let _ = event_tx.send(WorkflowEvent {
                event_type: "workflow_updated".to_string(),
                workflow_id,
                ticket_key: ticket_key.clone(),
                state: {
                    let wf = workflows.read().await;
                    wf.get(&ticket_key)
                        .map(|w| w.status_display())
                        .unwrap_or_default()
                },
                timestamp: Utc::now(),
                error: Some(e.to_string()),
                step_name: None,
                output_line: None,
                stream: None,
                progress_percent: None,
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
            });
        }
    }
}

/// Bootstrap a new (Pending) workflow: assign Jira ticket, create git worktree, run
/// mise/install/pre-workflow setup commands.
///
/// Called by `drive_workflow_def` when the workflow has no existing worktree (first run).
/// Pre-create the git worktree immediately when a ticket is added to the dashboard.
///
/// This is a best-effort background operation.  Failures are logged as warnings; the full
/// bootstrap that runs when a workflow def starts will create the worktree if needed.
pub(super) async fn prepare_worktree_for_ticket(
    ticket_key: &str,
    config: &Arc<RwLock<Config>>,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    actions: &Arc<dyn ExternalActions>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
) {
    let (repo_path, base_branch) = {
        let cfg = config.read().await;
        (
            PathBuf::from(&cfg.git.repo_path),
            cfg.git.base_branch.clone(),
        )
    };

    // Configure git credentials before fetching.
    if let Err(e) = actions.configure_git_author_from_github(&repo_path).await {
        warn!(
            ticket = %ticket_key,
            error = %e,
            "Failed to configure git credentials for worktree pre-creation"
        );
    }

    // Use "Task" as the default item type at add time (Jira details not fetched yet).
    let item_type = {
        let wf = workflows.read().await;
        wf.get(ticket_key)
            .map(|w| w.ticket_type.clone())
            .unwrap_or_else(|| "Task".to_string())
    };
    let branch_name = git::worktree::branch_name_for_ticket(ticket_key, &item_type);

    match actions.create_worktree(&branch_name, &base_branch).await {
        Ok(worktree_path) => {
            // Configure git identity on the new worktree.
            if let Err(e) = actions
                .configure_git_author_from_github(&worktree_path)
                .await
            {
                warn!(
                    ticket = %ticket_key,
                    error = %e,
                    "Failed to configure git author on pre-created worktree"
                );
            }

            info!(
                ticket = %ticket_key,
                path = %worktree_path.display(),
                branch = %branch_name,
                "Worktree pre-created at ticket-add time"
            );

            let workflow_id = {
                let mut wf = workflows.write().await;
                if let Some(w) = wf.get_mut(ticket_key) {
                    w.worktree_path = Some(worktree_path.clone());
                    w.branch_name = branch_name.clone();
                    w.id.clone()
                } else {
                    return; // Workflow was removed before task finished.
                }
            };

            let _ = event_tx.send(WorkflowEvent {
                event_type: "workflow_updated".to_string(),
                workflow_id,
                ticket_key: ticket_key.to_string(),
                state: "Pending".to_string(),
                timestamp: Utc::now(),
                error: None,
                step_name: None,
                output_line: None,
                stream: None,
                progress_percent: Some(0),
                progress_steps_total: None,
                forwarded_port: None,
                pr_merged: None,
            });
        }
        Err(e) => {
            warn!(
                ticket = %ticket_key,
                error = %e,
                "Failed to pre-create worktree at ticket-add time; bootstrap will create it when workflow starts"
            );
        }
    }
}

/// Returns `(worktree_path, ticket_detail)`.
#[allow(clippy::too_many_arguments)]
pub(super) async fn bootstrap_new_workflow(
    ticket_key: &str,
    config: &Arc<RwLock<Config>>,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    actions: &Arc<dyn ExternalActions>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    cancel_token: &CancellationToken,
    log_writer: &Arc<WorkflowLogWriter>,
    agent_run_semaphore: &Arc<Semaphore>,
) -> Result<(PathBuf, crate::jira::client::JiraTicket)> {
    wait_if_paused(workflows, ticket_key, cancel_token).await?;
    check_cancelled(cancel_token)?;

    let jira_available = {
        let wf = workflows.read().await;
        wf.get(ticket_key).map(|w| w.jira_available).unwrap_or(true)
    };

    let cfg = config.read().await;
    let repo_path = PathBuf::from(&cfg.git.repo_path);
    let project_keys = cfg.jira.project_keys.clone();
    drop(cfg);

    // Step 1: Assign + Retrieve ticket (or use in-memory data when Jira is unavailable).
    let ticket_detail = if jira_available {
        transition(
            workflows,
            event_tx,
            ticket_key,
            WorkflowState::Assigning,
            config,
        )
        .await;
        let mut step_log = StepLog::new("Assign Ticket".to_string());
        check_cancelled(cancel_token)?;

        match actions.assign_ticket(ticket_key).await {
            Ok(()) => {
                step_log
                    .output
                    .push("Ticket assigned to current Jira user".to_string());
            }
            Err(e) => {
                step_log.output.push(format!("[DRY/SKIP] {e}"));
                warn!(ticket = ticket_key, error = %e, "Failed to assign ticket, continuing");
            }
        }
        match actions.transition_ticket(ticket_key, "In Progress").await {
            Ok(()) => {
                step_log
                    .output
                    .push("Ticket moved to In Progress".to_string());
            }
            Err(e) => {
                step_log.output.push(format!("[DRY/SKIP] {e}"));
                warn!(ticket = ticket_key, error = %e, "Failed to transition ticket, continuing");
            }
        }
        step_log.complete(StepStatus::Success);
        add_step_log(workflows, ticket_key, step_log).await;

        transition(
            workflows,
            event_tx,
            ticket_key,
            WorkflowState::RetrievingDetails,
            config,
        )
        .await;
        let mut step_log = StepLog::new("Retrieve Details".to_string());
        check_cancelled(cancel_token)?;

        let jira_client = JiraClient::new(repo_path.clone());
        let detail = match jira_client
            .get_ticket_details(ticket_key, &project_keys)
            .await
        {
            Ok(detail) => {
                step_log
                    .output
                    .push(format!("Retrieved: {}", detail.summary));
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
                warn!(
                    ticket = ticket_key,
                    error = %e,
                    "Failed to retrieve ticket details, using minimal context"
                );
                step_log.fail(e.to_string());
                crate::jira::client::JiraTicket {
                    key: ticket_key.to_string(),
                    summary: workflows
                        .read()
                        .await
                        .get(ticket_key)
                        .map(|w| w.ticket_summary.clone())
                        .unwrap_or_default(),
                    description: String::new(),
                    item_type: "Task".to_string(),
                    status: "In Progress".to_string(),
                    linked_items: Vec::new(),
                }
            }
        };
        add_step_log(workflows, ticket_key, step_log).await;
        detail
    } else {
        info!(
            ticket = %ticket_key,
            "Jira unavailable — skipping Assign and Retrieve steps"
        );
        let wf = workflows.read().await;
        let (summary, description, item_type) = wf
            .get(ticket_key)
            .map(|w| {
                (
                    w.ticket_summary.clone(),
                    w.ticket_description.clone(),
                    w.ticket_type.clone(),
                )
            })
            .unwrap_or_default();
        drop(wf);
        crate::jira::client::JiraTicket {
            key: ticket_key.to_string(),
            summary,
            description,
            item_type: if item_type.is_empty() {
                "Task".to_string()
            } else {
                item_type
            },
            status: "In Progress".to_string(),
            linked_items: Vec::new(),
        }
    };

    // Step 2: Create git worktree (skip if pre-created at ticket-add time).
    transition(
        workflows,
        event_tx,
        ticket_key,
        WorkflowState::CreatingWorktree,
        config,
    )
    .await;
    check_cancelled(cancel_token)?;

    // Check whether a worktree was already pre-created when the ticket was added.
    let pre_created = {
        let wf = workflows.read().await;
        wf.get(ticket_key).and_then(|w| {
            w.worktree_path
                .as_ref()
                .filter(|p| p.exists())
                .map(|p| (p.clone(), w.branch_name.clone()))
        })
    };

    let (worktree_path, branch_name) = if let Some((existing_path, existing_branch)) = pre_created {
        // Re-use the pre-created worktree; skip git fetch + worktree add.
        info!(
            ticket = %ticket_key,
            path = %existing_path.display(),
            branch = %existing_branch,
            "Using pre-created worktree (created at ticket-add time)"
        );
        let mut step_log = StepLog::new("Create Worktree".to_string());
        step_log.output.push(format!("Branch: {existing_branch}"));
        step_log
            .output
            .push(format!("Worktree: {}", existing_path.display()));
        step_log
            .output
            .push("(pre-created at ticket-add time)".to_string());
        step_log.complete(StepStatus::Success);
        add_step_log(workflows, ticket_key, step_log).await;
        (existing_path, existing_branch)
    } else {
        // Full worktree creation path.
        let mut step_log = StepLog::new("Create Worktree".to_string());

        // Configure the git credential helper (gh auth setup-git) on the repo root BEFORE
        // fetching, so `git fetch` can authenticate via the GitHub App token.
        if let Err(e) = actions.configure_git_author_from_github(&repo_path).await {
            warn!(
                ticket = %ticket_key,
                error = %e,
                "Could not configure git credential helper before fetch; git fetch may fail"
            );
        }

        let branch_name =
            git::worktree::branch_name_for_ticket(ticket_key, &ticket_detail.item_type);
        let cfg = config.read().await;
        let base_branch = cfg.git.base_branch.clone();
        drop(cfg);

        let worktree_path = actions.create_worktree(&branch_name, &base_branch).await?;

        {
            let mut wf = workflows.write().await;
            if let Some(workflow) = wf.get_mut(ticket_key) {
                workflow.branch_name = branch_name.clone();
                workflow.worktree_path = Some(worktree_path.clone());
            }
        }

        step_log.output.push(format!("Branch: {branch_name}"));
        step_log
            .output
            .push(format!("Worktree: {}", worktree_path.display()));
        step_log.complete(StepStatus::Success);
        add_step_log(workflows, ticket_key, step_log).await;

        (worktree_path, branch_name)
    };

    // Align git author with the authenticated GitHub CLI user.
    match actions
        .configure_git_author_from_github(&worktree_path)
        .await
    {
        Ok(()) => {
            info!(
                ticket = %ticket_key,
                path = %worktree_path.display(),
                "Git author aligned with authenticated GitHub CLI user"
            );
        }
        Err(e) => {
            warn!(
                ticket = %ticket_key,
                error = %e,
                "Could not set worktree git author from `gh`; agent commits may use the wrong identity"
            );
        }
    }
    let _ = branch_name; // used in step_log, suppress unused warning

    // Build container runner for setup commands (mise, pre-install, install, pre-workflow).
    let container_runner = if ContainerRunner::is_available() {
        let cfg = config.read().await;
        let image = if cfg.general.worker_image.is_empty() {
            drop(cfg);
            ContainerRunner::discover_worker_image()
                .await
                .unwrap_or_else(|| "maestro:latest".to_string())
        } else {
            let img = cfg.general.worker_image.clone();
            drop(cfg);
            img
        };
        let maestro_shared = PathBuf::from("/workspace/.maestro");
        if !maestro_shared.exists() {
            let _ = std::fs::create_dir_all(&maestro_shared);
        }
        info!(
            ticket = %ticket_key,
            image = %image,
            "Container isolation enabled for workflow"
        );
        let gh_token = actions.get_gh_installation_token(&worktree_path).await;
        let runner =
            ContainerRunner::new(ticket_key, &worktree_path, &image).with_isolate_workspace();
        Some(if let Some(token) = gh_token {
            runner.with_gh_token(token)
        } else {
            runner
        })
    } else {
        return Err(MaestroError::Config(
            "Docker daemon is not available. DinD is required for workflow isolation. \
             Ensure DOCKER_HOST is set and the DinD sidecar is running."
                .into(),
        ));
    };

    let cfg = config.read().await;
    let pre_install_cmds = cfg.commands.pre_install.clone();
    let install_cmd = cfg.commands.install.clone();
    let pre_workflow_cmds = cfg.commands.pre_workflow.clone();
    let shell_stream_provider = cfg.agent.provider;
    drop(cfg);

    // Mise install (if project declares mise tools).
    if crate::process::worktree_has_mise_config(&worktree_path) {
        let _shell_slot = acquire_agent_slot(agent_run_semaphore, cancel_token).await?;
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
        let mise_result = if let Some(ref runner) = container_runner {
            let (prog, docker_args) = runner.wrap_command("mise", &["install"]);
            let refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
            crate::process::run_command_streaming(
                &prog,
                &refs,
                &worktree_path,
                cancel_token.child_token(),
                line_tx,
            )
            .await
        } else {
            crate::process::run_command_streaming(
                "mise",
                &["install"],
                &worktree_path,
                cancel_token.child_token(),
                line_tx,
            )
            .await
        };
        match mise_result {
            Ok(output) if output.success() => {
                step_log.output.push("mise install completed".to_string());
                step_log.complete(StepStatus::Success);
                add_step_log(workflows, ticket_key, step_log).await;
                broadcast_step_completed(event_tx, ticket_key, "Mise install", workflows, config)
                    .await;
            }
            Ok(output) => {
                let stderr_tail = output
                    .stderr
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                let msg = format!(
                    "mise install failed (exit code {}):\n{}",
                    output.exit_code, stderr_tail
                );
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
    }

    // Pre-install commands.
    if !pre_install_cmds.is_empty() {
        let total = pre_install_cmds.len();
        for (i, pre_install_cmd) in pre_install_cmds.iter().enumerate() {
            let step_name = format!("Pre-install ({}/{})", i + 1, total);
            let _shell_slot = acquire_agent_slot(agent_run_semaphore, cancel_token).await?;
            let mut step_log = StepLog::new(step_name.clone());
            info!(
                command = %pre_install_cmd,
                step = i + 1,
                total,
                "Running pre-install command"
            );
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
            let pre_result = if let Some(ref runner) = container_runner {
                let (prog, docker_args) = runner.wrap_shell_command(pre_install_cmd);
                let refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
                crate::process::run_command_streaming(
                    &prog,
                    &refs,
                    &worktree_path,
                    cancel_token.child_token(),
                    line_tx,
                )
                .await
            } else {
                crate::process::run_shell_command_streaming(
                    pre_install_cmd,
                    &worktree_path,
                    cancel_token.child_token(),
                    line_tx,
                )
                .await
            };
            match pre_result {
                Ok(output) if output.success() => {
                    step_log.output.push(format!("{step_name} completed"));
                    step_log.complete(StepStatus::Success);
                    add_step_log(workflows, ticket_key, step_log).await;
                    broadcast_step_completed(event_tx, ticket_key, &step_name, workflows, config)
                        .await;
                }
                Ok(output) => {
                    let stderr_tail = output
                        .stderr
                        .lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n");
                    let msg = format!(
                        "{step_name} failed (exit code {}):\n{}",
                        output.exit_code, stderr_tail
                    );
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
        }
    }

    // Install dependencies.
    if !install_cmd.is_empty() {
        let _shell_slot = acquire_agent_slot(agent_run_semaphore, cancel_token).await?;
        let mut step_log = StepLog::new("Install Dependencies".to_string());
        info!(command = %install_cmd, "Installing dependencies in worktree");
        log_writer
            .write_step("Install Dependencies", &format!("Running: {install_cmd}"))
            .await;

        broadcast_step_started(event_tx, ticket_key, "Install Dependencies");
        let line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            "Install Dependencies",
            log_writer,
            workflows,
            shell_stream_provider,
        );
        let install_result = if let Some(ref runner) = container_runner {
            let (prog, docker_args) = runner.wrap_shell_command(&install_cmd);
            let refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
            crate::process::run_command_streaming(
                &prog,
                &refs,
                &worktree_path,
                cancel_token.child_token(),
                line_tx,
            )
            .await
        } else {
            crate::process::run_shell_command_streaming(
                &install_cmd,
                &worktree_path,
                cancel_token.child_token(),
                line_tx,
            )
            .await
        };
        match install_result {
            Ok(output) if output.success() => {
                step_log.output.push("Dependencies installed".to_string());
                step_log.complete(StepStatus::Success);
                add_step_log(workflows, ticket_key, step_log).await;
                broadcast_step_completed(
                    event_tx,
                    ticket_key,
                    "Install Dependencies",
                    workflows,
                    config,
                )
                .await;
            }
            Ok(output) => {
                let stderr_tail = output
                    .stderr
                    .lines()
                    .rev()
                    .take(20)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                let stdout_tail = output
                    .stdout
                    .lines()
                    .rev()
                    .take(10)
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect::<Vec<_>>()
                    .join("\n");
                let msg = format!(
                    "Install failed (exit code {}):\nSTDERR:\n{}\nSTDOUT:\n{}",
                    output.exit_code, stderr_tail, stdout_tail
                );
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
    }

    // Pre-workflow commands.
    if !pre_workflow_cmds.is_empty() {
        let total = pre_workflow_cmds.len();
        for (i, pre_workflow_cmd) in pre_workflow_cmds.iter().enumerate() {
            let step_name = format!("Pre-workflow ({}/{})", i + 1, total);
            let _shell_slot = acquire_agent_slot(agent_run_semaphore, cancel_token).await?;
            let mut step_log = StepLog::new(step_name.clone());
            info!(
                command = %pre_workflow_cmd,
                step = i + 1,
                total,
                "Running pre-workflow command"
            );
            log_writer
                .write_step(&step_name, &format!("Running: {pre_workflow_cmd}"))
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
            let pre_result = if let Some(ref runner) = container_runner {
                let (prog, docker_args) = runner.wrap_shell_command(pre_workflow_cmd);
                let refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
                crate::process::run_command_streaming(
                    &prog,
                    &refs,
                    &worktree_path,
                    cancel_token.child_token(),
                    line_tx,
                )
                .await
            } else {
                crate::process::run_shell_command_streaming(
                    pre_workflow_cmd,
                    &worktree_path,
                    cancel_token.child_token(),
                    line_tx,
                )
                .await
            };
            match pre_result {
                Ok(output) if output.success() => {
                    step_log.output.push(format!("{step_name} completed"));
                    step_log.complete(StepStatus::Success);
                    add_step_log(workflows, ticket_key, step_log).await;
                    broadcast_step_completed(event_tx, ticket_key, &step_name, workflows, config)
                        .await;
                }
                Ok(output) => {
                    let stderr_tail = output
                        .stderr
                        .lines()
                        .rev()
                        .take(20)
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                        .join("\n");
                    let msg = format!(
                        "{step_name} failed (exit code {}):\n{}",
                        output.exit_code, stderr_tail
                    );
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
        }
    }

    // Mark bootstrap as fully complete so future def runs skip it entirely (resume path).
    {
        let mut wf = workflows.write().await;
        if let Some(w) = wf.get_mut(ticket_key) {
            w.worktree_bootstrapped = true;
        }
    }

    Ok((worktree_path, ticket_detail))
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_workflow_def_steps(
    ticket_key: &str,
    def_name: &str,
    steps: &[AgentStepConfig],
    worktree_path: &Path,
    ticket_summary: &str,
    ticket_description: &str,
    ticket_type: &str,
    config: &Arc<RwLock<Config>>,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    actions: &Arc<dyn ExternalActions>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    cancel_token: &CancellationToken,
    log_writer: &Arc<WorkflowLogWriter>,
    agent_run_semaphore: &Arc<Semaphore>,
) -> Result<()> {
    let ticket = crate::jira::client::JiraTicket {
        key: ticket_key.to_string(),
        summary: ticket_summary.to_string(),
        description: ticket_description.to_string(),
        item_type: ticket_type.to_string(),
        status: String::new(),
        linked_items: Vec::new(),
    };
    let jira_cfg = {
        let c = config.read().await;
        c.jira.clone()
    };
    let ticket_context = build_ticket_context(&ticket, &jira_cfg);
    let acceptance_criteria = extract_acceptance_criteria(&ticket.description);
    let acceptance_criteria_str = format_acceptance_criteria_block(&acceptance_criteria);

    let mut interp_vars: HashMap<String, String> = HashMap::new();
    interp_vars.insert("ticket_key".into(), ticket_key.to_string());
    interp_vars.insert("ticket_summary".into(), ticket_summary.to_string());
    interp_vars.insert("ticket_description".into(), ticket_description.to_string());
    interp_vars.insert("description".into(), ticket_description.to_string());
    interp_vars.insert("ticket_type".into(), ticket_type.to_string());
    interp_vars.insert("acceptance_criteria".into(), acceptance_criteria_str);
    interp_vars.insert("ticket_context".into(), ticket_context);
    interp_vars.insert("pr_url".into(), {
        let wf = workflows.read().await;
        wf.get(ticket_key)
            .and_then(|w| w.pr_url.clone())
            .unwrap_or_default()
    });
    {
        let cfg = config.read().await;
        interp_vars.insert("base_branch".into(), cfg.git.base_branch.clone());
    }

    // Construct container runner for isolation
    let container_runner = if ContainerRunner::is_available() {
        let cfg = config.read().await;
        let image = if cfg.general.worker_image.is_empty() {
            drop(cfg);
            ContainerRunner::discover_worker_image()
                .await
                .unwrap_or_else(|| "maestro:latest".to_string())
        } else {
            let img = cfg.general.worker_image.clone();
            drop(cfg);
            img
        };
        let gh_token = actions.get_gh_installation_token(worktree_path).await;
        let runner =
            ContainerRunner::new(ticket_key, worktree_path, &image).with_isolate_workspace();
        Some(if let Some(token) = gh_token {
            runner.with_gh_token(token)
        } else {
            runner
        })
    } else {
        return Err(MaestroError::Config(
            "Docker daemon is not available. DinD is required for workflow isolation. \
             Ensure DOCKER_HOST is set and the DinD sidecar is running."
                .into(),
        ));
    };

    let cfg = config.read().await;
    let timeout = cfg.agent.step_timeout_secs;
    let claude_model = if cfg.agent.model.is_empty() {
        None
    } else {
        Some(cfg.agent.model.clone())
    };
    let cursor_model_buf = cfg.agent.cursor_model.clone();
    let cursor_model_pass = cursor_model_for_cli(&cursor_model_buf);
    let ai_stream_provider = cfg.agent.provider;
    let cursor_cli = cfg.agent.cursor_cli.clone();
    let ticketing_avail = {
        let wf = workflows.read().await;
        wf.get(ticket_key)
            .map(|w| w.ticketing_available)
            .unwrap_or(false)
    };
    let filtered_steps: Vec<_> = steps
        .iter()
        .filter(|s| s.available_for(ticketing_avail))
        .cloned()
        .collect();
    drop(cfg);

    let skill_paths = build_skill_search_paths(worktree_path, ai_stream_provider);

    wait_if_paused(workflows, ticket_key, cancel_token).await?;
    check_cancelled(cancel_token)?;

    let prior_steps: Vec<StepLog> = {
        let wf = workflows.read().await;
        wf.get(ticket_key)
            .map(|w| w.steps_log.clone())
            .unwrap_or_default()
    };

    let last_agent_output = run_agent_step_sequence(
        ticket_key,
        worktree_path,
        &interp_vars,
        &filtered_steps,
        1, // single pass
        ai_stream_provider,
        &cursor_cli,
        cursor_model_pass,
        claude_model.as_deref(),
        timeout,
        workflows,
        event_tx,
        cancel_token,
        log_writer,
        agent_run_semaphore.clone(),
        container_runner.as_ref(),
        &prior_steps,
        false, // do not skip prior successes — always run all steps
        config,
        &skill_paths,
        None,  // no initial session id
        false, // not a snapshot resume
        false, // no report injection
    )
    .await?;

    info!(ticket = %ticket_key, def = %def_name, "Workflow definition steps completed");

    // Extract PR URL from outcome.toml or MAESTRO_PR_URL marker in agent output,
    // then transition the workflow to Done.
    let resolved = resolve_pr_url(worktree_path, last_agent_output.as_deref());
    if let Some(ref url) = resolved {
        let mut wf = workflows.write().await;
        if let Some(w) = wf.get_mut(ticket_key) {
            w.pr_url = Some(url.clone());
        }
    }
    transition(workflows, event_tx, ticket_key, WorkflowState::Done, config).await;

    Ok(())
}

/// `apply_prior_success_skip`: enabled for the main **AddressingTicket** flow (resume after restart).
///
/// `initial_session_id`: when restoring from a snapshot, the last Claude/Cursor session ID so the
/// first re-executed step can use `--resume` instead of starting a fresh conversation.
///
/// `is_snapshot_resume`: when `true`, the first step that actually runs (not skipped) will use
/// `--resume` with a "continue where you left off" prompt instead of the step's original prompt.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_agent_step_sequence(
    ticket_key: &str,
    worktree_path: &Path,
    interp_vars: &HashMap<String, String>,
    steps: &[AgentStepConfig],
    outer_loops: u8,
    ai_stream_provider: AiAgentProvider,
    cursor_cli: &str,
    cursor_model_pass: &str,
    claude_model: Option<&str>,
    timeout: u64,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    cancel_token: &CancellationToken,
    log_writer: &Arc<WorkflowLogWriter>,
    agent_run_semaphore: Arc<Semaphore>,
    container_runner: Option<&ContainerRunner>,
    prior_steps_log: &[StepLog],
    // If true: skip agent steps already Success in prior_steps_log (restart resume).
    apply_prior_success_skip: bool,
    config: &Arc<RwLock<Config>>,
    skill_search_paths: &[PathBuf],
    initial_session_id: Option<String>,
    is_snapshot_resume: bool,
    // When true, each agent step prompt gets the report-generation injection suffix.
    // Pass `false` for the consolidation step (which already has its own dedicated prompt).
    inject_report: bool,
) -> Result<Option<String>> {
    let num_steps = steps.len();
    let mut claude_session_id: Option<String> = initial_session_id;
    let mut last_agent_output: Option<String> = None;
    let mut snapshot_resume_pending = is_snapshot_resume && claude_session_id.is_some();

    for outer in 1..=outer_loops {
        check_cancelled(cancel_token)?;
        wait_if_paused(workflows, ticket_key, cancel_token).await?;

        for (step_idx, step) in steps.iter().enumerate() {
            let step_repeat = step.repeat;
            for r in 1..=step_repeat {
                check_cancelled(cancel_token)?;
                wait_if_paused(workflows, ticket_key, cancel_token).await?;

                let step_label_core = if outer_loops > 1 {
                    format!(
                        "{} (cycle {}/{}, run {}/{})",
                        step.name, outer, outer_loops, r, step_repeat
                    )
                } else {
                    format!("{} (run {}/{})", step.name, r, step_repeat)
                };
                let step_label = step_label_core.clone();

                // Skip steps that already succeeded in a prior run (main flow only — resume after restart).
                // The original Success entry is already in steps_log from the snapshot — don't add a duplicate.
                if apply_prior_success_skip && step_already_succeeded(prior_steps_log, &step_label)
                {
                    info!(ticket = %ticket_key, step = %step_label, "Skipping agent step — succeeded in prior run");
                    continue;
                }

                let _agent_slot = acquire_agent_slot(&agent_run_semaphore, cancel_token).await?;

                transition_to_agent_step(
                    workflows,
                    event_tx,
                    ticket_key,
                    outer,
                    &step_label,
                    config,
                )
                .await;

                let is_last_run_of_outer_cycle = step_idx + 1 == num_steps && r == step_repeat;

                // ── Command step execution ──────────────────────────────────
                if step.is_command_step() {
                    // Command steps don't use AI session resumption. Clear the flag
                    // so the next agent step (if any) starts with its own prompt rather
                    // than the snapshot-resume "Continue what you were doing…" message.
                    snapshot_resume_pending = false;

                    let mut step_log = StepLog::new(step_label.clone());
                    broadcast_step_started(event_tx, ticket_key, &step_label);
                    log_writer
                        .write_step(&step_label, "Starting command step")
                        .await;

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

                    let total_cmds = step.commands.len();
                    let mut cmd_failed = false;

                    for (cmd_idx, cmd) in step.commands.iter().enumerate() {
                        check_cancelled(cancel_token)?;
                        wait_if_paused(workflows, ticket_key, cancel_token).await?;

                        let interpolated_cmd = interpolate_command_template(cmd, interp_vars);
                        info!(
                            ticket = %ticket_key,
                            step = %step_label,
                            command = %interpolated_cmd,
                            index = cmd_idx + 1,
                            total = total_cmds,
                            "Running command"
                        );
                        log_writer
                            .write_step(
                                &step_label,
                                &format!(
                                    "Running command {}/{}: {}",
                                    cmd_idx + 1,
                                    total_cmds,
                                    interpolated_cmd
                                ),
                            )
                            .await;

                        let cmd_result = if let Some(runner) = container_runner {
                            let (prog, docker_args) = runner.wrap_shell_command(&interpolated_cmd);
                            let refs: Vec<&str> = docker_args.iter().map(|s| s.as_str()).collect();
                            crate::process::run_command_streaming_with_timeout(
                                &prog,
                                &refs,
                                worktree_path,
                                cancel_token.child_token(),
                                line_tx.clone(),
                                timeout,
                            )
                            .await
                        } else {
                            crate::process::run_shell_command_streaming_with_timeout(
                                &interpolated_cmd,
                                worktree_path,
                                cancel_token.child_token(),
                                line_tx.clone(),
                                timeout,
                            )
                            .await
                        };

                        match cmd_result {
                            Ok(output) if output.success() => {
                                step_log.output.push(format!(
                                    "Command {}/{} completed",
                                    cmd_idx + 1,
                                    total_cmds
                                ));
                            }
                            Ok(output) => {
                                let stderr_tail = output
                                    .stderr
                                    .lines()
                                    .rev()
                                    .take(20)
                                    .collect::<Vec<_>>()
                                    .into_iter()
                                    .rev()
                                    .collect::<Vec<_>>()
                                    .join("\n");
                                let msg = format!(
                                    "Command {}/{} failed (exit code {}):\n{}",
                                    cmd_idx + 1,
                                    total_cmds,
                                    output.exit_code,
                                    stderr_tail
                                );
                                warn!(
                                    ticket = %ticket_key,
                                    step = %step_label,
                                    command = %interpolated_cmd,
                                    exit_code = output.exit_code,
                                    "Command step command failed"
                                );
                                step_log.fail(msg);
                                cmd_failed = true;
                                break;
                            }
                            Err(e) => {
                                let msg =
                                    format!("Command {}/{} error: {}", cmd_idx + 1, total_cmds, e);
                                warn!(
                                    ticket = %ticket_key,
                                    step = %step_label,
                                    command = %interpolated_cmd,
                                    error = %e,
                                    "Command step command error"
                                );
                                step_log.fail(msg);
                                cmd_failed = true;
                                break;
                            }
                        }
                    }

                    if !cmd_failed {
                        step_log
                            .output
                            .push(format!("All {total_cmds} command(s) completed"));
                        step_log.complete(StepStatus::Success);
                    }

                    if cmd_failed && !is_last_run_of_outer_cycle {
                        add_step_log(workflows, ticket_key, step_log).await;
                        error!(
                            ticket = %ticket_key,
                            step = %step_label,
                            "Command step failed — aborting workflow"
                        );
                        return Err(MaestroError::AiAgent("Command step failed".to_string()));
                    }

                    add_step_log(workflows, ticket_key, step_log).await;
                    broadcast_step_completed(event_tx, ticket_key, &step_label, workflows, config)
                        .await;
                    continue;
                }

                // ── Agent step execution ────────────────────────────────────
                let mut step_log = StepLog::new(step_label.clone());
                broadcast_step_started(event_tx, ticket_key, &step_label);
                log_writer.write_step(&step_label, "Starting").await;

                // Build system prompt from step skills (Claude --bare only).
                let system_prompt =
                    if ai_stream_provider == AiAgentProvider::Claude && !step.skills.is_empty() {
                        crate::skill_resolve::build_system_prompt(&step.skills, skill_search_paths)
                            .await
                    } else {
                        None
                    };

                // For Cursor: prepend /skill invocations to the prompt (native support).
                let step_prompt =
                    if ai_stream_provider == AiAgentProvider::Cursor && !step.skills.is_empty() {
                        let invocations =
                            crate::skill_resolve::build_cursor_skill_invocations(&step.skills);
                        if step.prompt.is_empty() {
                            invocations
                        } else {
                            format!("{invocations}\n\n{}", step.prompt)
                        }
                    } else {
                        step.prompt.clone()
                    };

                // When Claude has skills as system prompt but no task prompt,
                // direct it to follow the system prompt instructions.
                let effective_prompt = if ai_stream_provider == AiAgentProvider::Claude
                    && step_prompt.trim().is_empty()
                    && system_prompt.is_some()
                {
                    "Follow the instructions in the system prompt.".to_string()
                } else {
                    step_prompt.clone()
                };

                // Determine whether to resume a prior session and what prompt to use.
                let (final_effective_prompt, resume_id) = if r > 1 {
                    // Repeat runs within the same step always resume
                    (effective_prompt, claude_session_id.as_deref())
                } else if step.resume_previous {
                    // Explicitly opted in to resuming the prior step's session
                    (effective_prompt, claude_session_id.as_deref())
                } else if snapshot_resume_pending {
                    // Resuming after container restart — the prior session has the full
                    // conversation context; just ask the agent to continue.
                    snapshot_resume_pending = false;
                    (
                        "Resume what you were doing before the session was closed.".to_string(),
                        claude_session_id.as_deref(),
                    )
                } else {
                    (effective_prompt, None)
                };

                let interpolated = interpolate_agent_prompt(&final_effective_prompt, interp_vars);
                let headless = headless_instructions_suffix(ai_stream_provider);
                let full_prompt = if inject_report {
                    let report_suffix = report_injection_suffix(ticket_key);
                    format!("{interpolated}\n\n{report_suffix}\n\n{headless}")
                } else {
                    format!("{interpolated}\n\n{headless}")
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
                        worktree_path,
                        &full_prompt,
                        cancel_token.child_token(),
                        timeout,
                        Some(line_tx),
                        claude_model,
                        resume_id,
                        container_runner,
                        system_prompt.as_deref(),
                    )
                    .await
                    .map(|s| (s.session_id, s.output)),
                    AiAgentProvider::Cursor => CursorSession::run_prompt(
                        cursor_cli,
                        worktree_path,
                        &full_prompt,
                        cancel_token.child_token(),
                        timeout,
                        Some(line_tx),
                        Some(cursor_model_pass),
                        resume_id,
                        container_runner,
                    )
                    .await
                    .map(|s| (s.session_id, s.output)),
                };

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
                        // Persist session ID on the workflow for snapshot resume.
                        {
                            let mut wf = workflows.write().await;
                            if let Some(w) = wf.get_mut(ticket_key) {
                                w.last_session_id = Some(session_id.clone());
                            }
                        }
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
                    }
                }

                add_step_log(workflows, ticket_key, step_log).await;
                broadcast_step_completed(event_tx, ticket_key, &step_label, workflows, config)
                    .await;
            }
        }
    }

    Ok(last_agent_output)
}

pub(super) async fn acquire_agent_slot(
    sem: &Arc<Semaphore>,
    cancel_token: &CancellationToken,
) -> Result<tokio::sync::OwnedSemaphorePermit> {
    tokio::select! {
        _ = cancel_token.cancelled() => Err(MaestroError::Cancelled),
        permit = sem.clone().acquire_owned() => permit
            .map_err(|_| MaestroError::Config("Agent concurrency semaphore closed".to_string())),
    }
}

pub(super) fn broadcast_step_started(
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
        progress_percent: None,
        progress_steps_total: None,
        forwarded_port: None,
        pr_merged: None,
    });
}

pub(super) async fn broadcast_step_completed(
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    step_name: &str,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    config: &Arc<RwLock<Config>>,
) {
    let dash = progress_dashboard_fields_for_ticket(workflows, config, ticket_key).await;
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
        progress_percent: dash.map(|(p, _)| p),
        progress_steps_total: dash.map(|(_, t)| t),
        forwarded_port: None,
        pr_merged: None,
    });
}

pub(super) fn spawn_output_relay(
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
                    progress_percent: None,
                    progress_steps_total: None,
                    forwarded_port: None,
                    pr_merged: None,
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

pub(super) async fn progress_dashboard_fields_for_ticket(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    config: &Arc<RwLock<Config>>,
    ticket_key: &str,
) -> Option<(u8, u32)> {
    let cfg = config.read().await;
    let wf = workflows.read().await;
    wf.get(ticket_key).map(|w| {
        (
            crate::workflow::dashboard_progress::workflow_progress_percent(w, &cfg),
            crate::workflow::dashboard_progress::estimated_step_total(w, &cfg),
        )
    })
}

pub(super) async fn transition_to_agent_step(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    pass: u8,
    step_label: &str,
    config: &Arc<RwLock<Config>>,
) {
    info!(
        ticket = %ticket_key,
        pass,
        step = %step_label,
        "Agent step (state + dashboard label)"
    );

    let updated = {
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(ticket_key) {
            workflow.state = WorkflowState::AddressingTicket { pass };
            workflow.current_step_label = Some(step_label.to_string());
            workflow.updated_at = Utc::now();
            Some((workflow.id.clone(), workflow.status_display()))
        } else {
            None
        }
    };
    if let Some((id, display)) = updated {
        let dash = progress_dashboard_fields_for_ticket(workflows, config, ticket_key).await;
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
            progress_percent: dash.map(|(p, _)| p),
            progress_steps_total: dash.map(|(_, t)| t),
            forwarded_port: None,
            pr_merged: None,
        });
    }
}

pub(super) async fn transition(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    new_state: WorkflowState,
    config: &Arc<RwLock<Config>>,
) {
    let state_name = new_state.display_name();
    info!(ticket = ticket_key, state = %state_name, "Transitioning workflow");

    let updated = {
        let mut wf = workflows.write().await;
        if let Some(workflow) = wf.get_mut(ticket_key) {
            workflow.current_step_label = None;
            workflow.state = new_state;
            workflow.updated_at = Utc::now();
            Some((workflow.id.clone(), workflow.status_display()))
        } else {
            None
        }
    };
    if let Some((id, display)) = updated {
        let dash = progress_dashboard_fields_for_ticket(workflows, config, ticket_key).await;
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
            progress_percent: dash.map(|(p, _)| p),
            progress_steps_total: dash.map(|(_, t)| t),
            forwarded_port: None,
            pr_merged: None,
        });
    }
}

pub(super) async fn add_step_log(
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    ticket_key: &str,
    step_log: StepLog,
) {
    let mut wf = workflows.write().await;
    if let Some(workflow) = wf.get_mut(ticket_key) {
        workflow.steps_log.push(step_log);
    }
}

pub(super) async fn wait_if_paused(
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

/// Close a GitHub issue via `gh api PATCH repos/{owner_repo}/issues/{number}`.
///
/// Uses the GitHub App installation token when one is available (GitHub App configured);
/// falls back to the ambient `gh` auth otherwise.
pub(super) async fn close_github_issue(
    ticket_key: &str,
    repo_url: &str,
    cwd: &Path,
    actions: &dyn crate::actions::traits::ExternalActions,
) -> Result<()> {
    let issue_number = parse_gh_issue_number(ticket_key).ok_or_else(|| {
        MaestroError::Config(format!(
            "Cannot close GitHub issue: '{ticket_key}' is not a GH-{{number}} key"
        ))
    })?;
    let owner_repo = crate::github::parse_github_repo(repo_url).ok_or_else(|| {
        MaestroError::Config(format!(
            "Cannot close GitHub issue: failed to parse owner/repo from '{repo_url}'"
        ))
    })?;

    let gh_token = actions.get_gh_installation_token(cwd).await;
    let env: Vec<(&str, &str)> = gh_token
        .as_deref()
        .map(|t| vec![("GH_TOKEN", t)])
        .unwrap_or_default();
    let endpoint = format!("repos/{owner_repo}/issues/{issue_number}");
    let output = crate::process::run_command_with_env(
        "gh",
        &[
            "api",
            "--method",
            "PATCH",
            &endpoint,
            "--field",
            "state=closed",
        ],
        cwd,
        CancellationToken::new(),
        &env,
    )
    .await
    .map_err(|e| MaestroError::Config(format!("gh api PATCH issue failed: {e}")))?;

    if !output.success() {
        return Err(MaestroError::Config(crate::github::gh_api_error_message(
            output.stderr.trim(),
            "Issues: Write",
        )));
    }

    info!(ticket = %ticket_key, issue = %issue_number, owner_repo = %owner_repo, "GitHub issue closed");
    Ok(())
}
