// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! Workflow bootstrap: ticket-add-time worktree pre-creation and the
//! full `bootstrap_new_workflow` flow (assign + retrieve + create
//! worktree + mise install + worktree init commands) called at the
//! start of every workflow definition run that has no existing worktree.
//!
//! Entry contract: callers go through [`bootstrap_new_workflow`] when
//! `Workflow.worktree_path` is `None`. The function returns the resolved
//! `(worktree_path, ticket_detail)` once setup is complete, or an error
//! that the workflow def driver propagates into the per-def `Error` state.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::actions::traits::ExternalActions;
use crate::config::{Config, ConfigError};
use crate::container::ContainerRunner;
use crate::db::Database;
use crate::error::Result;
use crate::git::{self, GitError};
use crate::jira::client::{JiraClient, JiraTicket};
use crate::workflow::helpers::check_cancelled;
use crate::workflow::log_writer::WorkflowLogWriter;
use crate::workflow::state::WorkflowState;
use crate::workflow::step::{StepLog, StepStatus};

use super::auth_pin::try_attach_secrets_bundle;
use super::driver::{add_step_log, transition, wait_if_paused};
use super::resolve::{resolve_repo_for_ticket, resolve_worktree_init_commands};
use super::step_runner::{
    acquire_agent_slot, broadcast_step_completed, broadcast_step_started, spawn_output_relay,
};
use super::types::{Workflow, WorkflowEvent};

/// Resolve a per-user GitHub token (bearer) and author identity for a workflow.
///
/// Returns `(token, identity)` where `token` is the bearer to inject into the
/// base-branch fetch and `identity` is `(author_name, author_email)` to set as
/// the git author. Both are `None` when there is no resolver, no `user_id`, or
/// the resolver could not produce a token (e.g. App-only mode, where the fetch
/// keeps its ambient App-token behaviour and the author identity comes from the
/// App / `gh` path instead).
async fn resolve_fetch_token_and_identity(
    git_auth_resolver: Option<&Arc<crate::github::auth_resolver::GitAuthResolver>>,
    user_id: Option<&str>,
) -> (Option<String>, Option<(String, String)>) {
    let (resolver, uid) = match (git_auth_resolver, user_id) {
        (Some(r), Some(u)) => (r, u),
        _ => return (None, None),
    };
    match resolver
        .token_for(crate::github::auth_resolver::GitAction::Fetch, uid)
        .await
    {
        Ok(tok) => {
            let identity = match (tok.author_name.clone(), tok.author_email.clone()) {
                (Some(name), Some(email)) => Some((name, email)),
                _ => None,
            };
            (Some(tok.bearer.expose().to_string()), identity)
        }
        Err(e) => {
            warn!(error = %e, "Could not resolve per-user GitHub token for worktree fetch; falling back to ambient auth");
            (None, None)
        }
    }
}

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
    db: Option<&Database>,
    git_auth_resolver: Option<&Arc<crate::github::auth_resolver::GitAuthResolver>>,
) {
    // The repository path is per-workflow (the registered repo the
    // caller picked when starting the workflow). Fall back to the global
    // `cfg.git.repo_path` only when no DB / `repository_id` is available.
    let (repo_path, base_branch) = resolve_repo_for_ticket(ticket_key, workflows, config, db).await;

    // Resolve the workflow owner's per-user GitHub token + identity (PAT path).
    let owner_user_id = {
        let wf = workflows.read().await;
        wf.get(ticket_key).and_then(|w| w.user_id.clone())
    };
    let (gh_token, identity) =
        resolve_fetch_token_and_identity(git_auth_resolver, owner_user_id.as_deref()).await;
    let identity_refs = identity.as_ref().map(|(n, e)| (n.as_str(), e.as_str()));

    // Configure git credentials before fetching.
    if let Err(e) = actions
        .configure_git_author_from_github(&repo_path, identity_refs)
        .await
    {
        warn!(
            ticket = %ticket_key,
            error = %e,
            "Failed to configure git credentials for worktree pre-creation"
        );
    }

    // Use "Task" as the default item type at add time (Jira details not fetched yet).
    let (item_type, worktree_lock, existing_branch) = {
        let wf = workflows.read().await;
        match wf.get(ticket_key) {
            Some(w) => (
                w.ticket_type.clone(),
                w.worktree_lock.clone(),
                w.branch_name.clone(),
            ),
            None => return, // Workflow was removed before the task started.
        }
    };
    // Prefer a branch already chosen at add time (carries the re-add `-N`
    // suffix that makes a fresh run open its own PR); else derive the canonical
    // name for back-compat with items added before that logic existed.
    let branch_name = if existing_branch.is_empty() {
        git::worktree::branch_name_for_ticket(ticket_key, &item_type)
    } else {
        existing_branch
    };

    // Hold the per-workflow lock for the duration of create_worktree. If
    // `bootstrap_new_workflow` fires concurrently, it acquires the same
    // lock and waits, then sees our completed worktree_path.
    let _guard = worktree_lock.lock().await;

    match actions
        .create_worktree(&repo_path, &branch_name, &base_branch, gh_token.as_deref())
        .await
    {
        Ok(worktree_path) => {
            // Configure git identity on the new worktree.
            if let Err(e) = actions
                .configure_git_author_from_github(&worktree_path, identity_refs)
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

            let (workflow_id, owner_user_id, updated_at) = {
                let mut wf = workflows.write().await;
                if let Some(w) = wf.get_mut(ticket_key) {
                    w.worktree_path = Some(worktree_path.clone());
                    w.branch_name = branch_name.clone();
                    w.worktree_preparing = false;
                    w.updated_at = Utc::now();
                    (w.id.clone(), w.user_id.clone(), w.updated_at.timestamp())
                } else {
                    return; // Workflow was removed before task finished.
                }
            };

            // Persist branch + worktree path atomically — the initial INSERT
            // recorded both empty (the worktree had not been created yet).
            let wt_str = worktree_path.display().to_string();
            super::driver::shadow_persist_branch_and_worktree(
                db,
                &workflow_id,
                &branch_name,
                Some(wt_str.as_str()),
                updated_at,
            )
            .await;

            let _ = event_tx.send(WorkflowEvent {
                event_type: "work_item_updated".to_string(),
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
                user_id: owner_user_id,
                ..Default::default()
            });
        }
        Err(e) => {
            // Clear the in-flight marker so the dashboard stops showing
            // "preparing" — the item falls back to "ready" (bootstrap will
            // create the worktree on first run).
            if let Some(w) = workflows.write().await.get_mut(ticket_key) {
                w.worktree_preparing = false;
            }
            warn!(
                ticket = %ticket_key,
                error = %e,
                "Failed to pre-create worktree at ticket-add time; bootstrap will create it when workflow starts"
            );
        }
    }
}

/// Bootstrap a new (Pending) workflow: assign Jira ticket, create git
/// worktree, run mise/install/worktree-init setup commands.
///
/// Called by `drive_workflow_def` when the workflow has no existing
/// worktree (first run). Returns `(worktree_path, ticket_detail)`.
///
/// On error the workflow stays in whatever pre-bootstrap state it had —
/// the caller (`drive_workflow_def`) is responsible for transitioning to
/// `Error` and emitting the dashboard event.
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
    db: Option<&Database>,
    // When `Some` plus the workflow has a `user_id`, the bootstrap pins
    // credentials at the start of the workflow and builds a per-step
    // `WorkerSecretsBundle` that the ContainerRunner mounts as a tmpfs
    // directory.
    git_auth_resolver: Option<&Arc<crate::github::auth_resolver::GitAuthResolver>>,
) -> Result<(PathBuf, JiraTicket)> {
    wait_if_paused(workflows, ticket_key, cancel_token).await?;
    check_cancelled(cancel_token)?;

    let jira_available = {
        let wf = workflows.read().await;
        wf.get(ticket_key).map(|w| w.jira_available).unwrap_or(true)
    };

    // Resolve the workflow owner's per-user GitHub token + author identity once
    // (PAT path). The token authenticates the base-branch fetch; the identity
    // sets the git author without a fragile `gh api user` shell-out. Both are
    // `None` for App-only / no-resolver paths, which keep the prior behaviour.
    let owner_user_id_for_auth = {
        let wf = workflows.read().await;
        wf.get(ticket_key).and_then(|w| w.user_id.clone())
    };
    let (gh_token, identity) =
        resolve_fetch_token_and_identity(git_auth_resolver, owner_user_id_for_auth.as_deref())
            .await;
    let identity_refs = identity.as_ref().map(|(n, e)| (n.as_str(), e.as_str()));

    // Repo path comes from the workflow's `repository_id` lookup, not
    // from a global `cfg.git.repo_path`. `project_keys` stays in config —
    // it is workflow-independent.
    let (repo_path, base_branch) = resolve_repo_for_ticket(ticket_key, workflows, config, db).await;
    let project_keys = {
        let cfg = config.read().await;
        cfg.jira.project_keys.clone()
    };

    // Step 1: Assign + Retrieve ticket (or use in-memory data when Jira is unavailable).
    let ticket_detail = if jira_available {
        transition(
            workflows,
            event_tx,
            ticket_key,
            WorkflowState::Assigning,
            config,
            db,
        )
        .await;
        let mut step_log = StepLog::bootstrap("Assign Ticket".to_string());
        check_cancelled(cancel_token)?;

        match actions.assign_ticket(&repo_path, ticket_key).await {
            Ok(()) => {
                step_log
                    .output
                    .push("Ticket assigned to current Jira user".to_string());
            }
            Err(e) => {
                step_log.output.push(format!("[DRY/SKIP] {e}"));
                warn!(ticket = ticket_key, error = ?e, "Failed to assign ticket, continuing");
            }
        }
        match actions
            .transition_ticket(&repo_path, ticket_key, "In Progress")
            .await
        {
            Ok(()) => {
                step_log
                    .output
                    .push("Ticket moved to In Progress".to_string());
            }
            Err(e) => {
                step_log.output.push(format!("[DRY/SKIP] {e}"));
                warn!(ticket = ticket_key, error = ?e, "Failed to transition ticket, continuing");
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
            db,
        )
        .await;
        let mut step_log = StepLog::bootstrap("Retrieve Details".to_string());
        check_cancelled(cancel_token)?;

        // Prefer the workflow owner's per-user Jira credential (REST) when one
        // is configured; fall back to the global `acli` client otherwise.
        let owner_user_id = {
            let wf = workflows.read().await;
            wf.get(ticket_key).and_then(|w| w.user_id.clone())
        };
        let reader: Box<dyn crate::jira::TicketReader> = match (db, owner_user_id.as_deref()) {
            (Some(database), Some(uid)) => {
                match crate::jira::resolve_rest_credential(database, uid).await {
                    Some(cred) => Box::new(crate::jira::JiraRestClient::real(cred)),
                    None => Box::new(JiraClient::new(repo_path.clone())),
                }
            }
            _ => Box::new(JiraClient::new(repo_path.clone())),
        };
        let detail = match reader.get_ticket_details(ticket_key, &project_keys).await {
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
                JiraTicket {
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
        JiraTicket {
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
        db,
    )
    .await;
    check_cancelled(cancel_token)?;

    // Acquire the per-workflow worktree lock before checking / creating, so
    // we serialise with any in-flight `prepare_worktree_for_ticket` task.
    // Holding the lock for the whole Step 2 prevents a concurrent pre-create
    // from clobbering the path we (or the pre-create) is producing.
    let worktree_lock = {
        let wf = workflows.read().await;
        wf.get(ticket_key).map(|w| w.worktree_lock.clone())
    };
    let _worktree_guard = if let Some(lock) = worktree_lock {
        Some(lock.lock_owned().await)
    } else {
        None
    };

    // Check whether a worktree was already pre-created when the ticket was added.
    // Re-read AFTER taking the lock so we observe any pre-create that just finished.
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
        let mut step_log = StepLog::bootstrap("Create Worktree".to_string());
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
        let mut step_log = StepLog::bootstrap("Create Worktree".to_string());

        // Configure the git credential helper / author identity on the repo root
        // BEFORE fetching, so `git fetch` can authenticate.
        if let Err(e) = actions
            .configure_git_author_from_github(&repo_path, identity_refs)
            .await
        {
            warn!(
                ticket = %ticket_key,
                error = %e,
                "Could not configure git credential helper before fetch; git fetch may fail"
            );
        }

        // Prefer a branch already chosen at add time (carries the re-add `-N`
        // suffix); else derive the canonical name for back-compat.
        let branch_name = {
            let existing = {
                let wf = workflows.read().await;
                wf.get(ticket_key)
                    .map(|w| w.branch_name.clone())
                    .unwrap_or_default()
            };
            if existing.is_empty() {
                git::worktree::branch_name_for_ticket(ticket_key, &ticket_detail.item_type)
            } else {
                existing
            }
        };

        let worktree_path = actions
            .create_worktree(&repo_path, &branch_name, &base_branch, gh_token.as_deref())
            .await?;

        let branch_persist = {
            let mut wf = workflows.write().await;
            if let Some(workflow) = wf.get_mut(ticket_key) {
                workflow.branch_name = branch_name.clone();
                workflow.worktree_path = Some(worktree_path.clone());
                workflow.updated_at = Utc::now();
                Some((workflow.id.clone(), workflow.updated_at.timestamp()))
            } else {
                None
            }
        };
        // Persist branch + worktree path atomically — the initial INSERT
        // recorded both empty (the worktree had not been created yet).
        if let Some((work_item_id, updated_at)) = branch_persist {
            let wt_str = worktree_path.display().to_string();
            super::driver::shadow_persist_branch_and_worktree(
                db,
                &work_item_id,
                &branch_name,
                Some(wt_str.as_str()),
                updated_at,
            )
            .await;
        }

        step_log.output.push(format!("Branch: {branch_name}"));
        step_log
            .output
            .push(format!("Worktree: {}", worktree_path.display()));
        step_log.complete(StepStatus::Success);
        add_step_log(workflows, ticket_key, step_log).await;

        (worktree_path, branch_name)
    };

    // Align git author with the resolved identity (PAT) or the GitHub App / `gh` user.
    match actions
        .configure_git_author_from_github(&worktree_path, identity_refs)
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

    // Build container runner for setup commands (mise + worktree init).
    let container_runner = if ContainerRunner::is_available() {
        let cfg = config.read().await;
        let image = if cfg.general.worker_image.is_empty() {
            drop(cfg);
            ContainerRunner::discover_worker_image()
                .await
                .unwrap_or_else(|| "takuto:latest".to_string())
        } else {
            let img = cfg.general.worker_image.clone();
            drop(cfg);
            img
        };
        let takuto_shared = PathBuf::from("/workspace/.takuto");
        if !takuto_shared.exists() {
            let _ = std::fs::create_dir_all(&takuto_shared);
        }
        info!(
            ticket = %ticket_key,
            image = %image,
            "Container isolation enabled for workflow"
        );
        let mut runner =
            ContainerRunner::new(ticket_key, &worktree_path, &image).with_isolate_workspace();

        // Pin credentials + attach the per-workflow secrets bundle so the
        // worker entrypoint reads tokens from tmpfs files instead of
        // `docker run -e` strings. Skip / fallback handling lives in
        // `try_attach_secrets_bundle`.
        if let Some(bundle) =
            try_attach_secrets_bundle(ticket_key, config, workflows, db, git_auth_resolver).await
        {
            runner = runner.with_secrets_bundle(bundle);
        }

        Some(runner)
    } else {
        return Err(ConfigError::DockerUnavailable.into());
    };

    let shell_stream_provider = {
        let cfg = config.read().await;
        cfg.agent.provider
    };
    // Resolve worktree init commands from the workflow owner's per-user
    // per-workspace DB row. No row, or no owner, or no db → run zero
    // init commands. There is no global default.
    let (workflow_user_id, workspace_name) = {
        let wf = workflows.read().await;
        wf.get(ticket_key)
            .map(|w| (w.user_id.clone(), w.workspace_name.clone()))
            .unwrap_or_default()
    };
    let init_commands =
        resolve_worktree_init_commands(workflow_user_id.as_deref(), &workspace_name, db).await;

    // Mise install (if project declares mise tools).
    if crate::process::worktree_has_mise_config(&worktree_path) {
        let _shell_slot = acquire_agent_slot(agent_run_semaphore, cancel_token).await?;
        let mut step_log = StepLog::bootstrap("Mise install".to_string());
        info!("Running mise install (project declares mise tools)");
        log_writer
            .write_step("Mise install", "Running: mise install")
            .await;

        broadcast_step_started(event_tx, ticket_key, "Mise install", workflows).await;
        let line_tx = spawn_output_relay(
            event_tx,
            ticket_key,
            "Mise install",
            log_writer,
            workflows,
            shell_stream_provider,
            // Bootstrap shell output is not an agent session — guardrail off.
            0,
            Arc::new(std::sync::Mutex::new(None)),
            CancellationToken::new(),
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
                let err = GitError::MiseInstallFailed {
                    exit_code: output.exit_code,
                    stderr_tail,
                };
                step_log.fail(err.to_string());
                add_step_log(workflows, ticket_key, step_log).await;
                return Err(err.into());
            }
            Err(e) => {
                step_log.fail(format!("mise install error: {e}"));
                add_step_log(workflows, ticket_key, step_log).await;
                return Err(e);
            }
        }
    }

    // Worktree init commands: replaces the legacy pre_install / install /
    // pre_workflow loops. An empty list (no override + empty default, or
    // an explicit `[]` override) skips this entire section and proceeds
    // straight to the agent steps.
    if !init_commands.is_empty() {
        let total = init_commands.len();
        for (i, cmd) in init_commands.iter().enumerate() {
            // Build a friendly step label: first 40 chars of the command, with
            // an ellipsis when truncated. Keeps the dashboard's per-step signal
            // ("which command is slow") that the old labels used to provide.
            let snippet: String = cmd.chars().take(40).collect();
            let snippet_display = if cmd.chars().count() > 40 {
                format!("{snippet}…")
            } else {
                snippet
            };
            let step_name = format!("Worktree init ({}/{}): {}", i + 1, total, snippet_display);

            let _shell_slot = acquire_agent_slot(agent_run_semaphore, cancel_token).await?;
            let mut step_log = StepLog::bootstrap(step_name.clone());
            info!(
                command = %cmd,
                step = i + 1,
                total,
                "Running worktree init command"
            );
            log_writer
                .write_step(&step_name, &format!("Running: {cmd}"))
                .await;

            broadcast_step_started(event_tx, ticket_key, &step_name, workflows).await;
            let line_tx = spawn_output_relay(
                event_tx,
                ticket_key,
                &step_name,
                log_writer,
                workflows,
                shell_stream_provider,
                // Pre-workflow shell output is not an agent session — guardrail off.
                0,
                Arc::new(std::sync::Mutex::new(None)),
                CancellationToken::new(),
            );
            let run_result = if let Some(ref runner) = container_runner {
                let (prog, docker_args) = runner.wrap_shell_command(cmd);
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
                    cmd,
                    &worktree_path,
                    cancel_token.child_token(),
                    line_tx,
                )
                .await
            };
            match run_result {
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
                    let err = GitError::WorktreeInitCommandFailed {
                        step_name: step_name.clone(),
                        exit_code: output.exit_code,
                        stderr_tail,
                        stdout_tail,
                    };
                    step_log.fail(err.to_string());
                    add_step_log(workflows, ticket_key, step_log).await;
                    return Err(err.into());
                }
                Err(e) => {
                    step_log.fail(format!("{step_name} error: {e}"));
                    add_step_log(workflows, ticket_key, step_log).await;
                    return Err(e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{MasterKey, seal};
    use crate::db::{Database, DbValue, github_credentials};
    use crate::github::auth_resolver::GitAuthResolver;

    fn in_mem_db() -> Database {
        Database::open_in_memory()
            .expect("in-mem DB")
            .with_test_master_key(MasterKey::from_bytes([0x5A; 32]))
    }

    async fn seed_user(db: &Database, user_id: &str) {
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
                vec![
                    DbValue::Text(user_id.to_string()),
                    DbValue::Text(user_id.to_string()),
                ],
            )
            .await
            .unwrap();
    }

    async fn seed_pat(db: &Database, user_id: &str, login: &str) {
        let mk = db.master_key().expect("test mk").key.clone();
        let sealed = seal(&mk, b"ghp_bootstrap_pat").unwrap();
        let adapter = db.adapter();
        let mut tx = adapter.begin().await.unwrap();
        github_credentials::upsert(&mut tx, user_id, &sealed, login, "[\"repo\"]", true)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn resolve_fetch_token_returns_pat_and_identity_for_pat_user() {
        let db = in_mem_db();
        seed_user(&db, "u-alice").await;
        seed_pat(&db, "u-alice", "alice").await;
        let resolver = Arc::new(GitAuthResolver::new(db, None));

        let (token, identity) =
            resolve_fetch_token_and_identity(Some(&resolver), Some("u-alice")).await;
        assert_eq!(token.as_deref(), Some("ghp_bootstrap_pat"));
        assert_eq!(
            identity,
            Some((
                "alice".to_string(),
                "alice@users.noreply.github.com".to_string()
            ))
        );
    }

    #[tokio::test]
    async fn resolve_fetch_token_returns_none_without_resolver() {
        let (token, identity) = resolve_fetch_token_and_identity(None, Some("u-alice")).await;
        assert!(token.is_none());
        assert!(identity.is_none());
    }

    #[tokio::test]
    async fn resolve_fetch_token_returns_none_without_user_id() {
        let db = in_mem_db();
        let resolver = Arc::new(GitAuthResolver::new(db, None));
        let (token, identity) = resolve_fetch_token_and_identity(Some(&resolver), None).await;
        assert!(token.is_none());
        assert!(identity.is_none());
    }
}
