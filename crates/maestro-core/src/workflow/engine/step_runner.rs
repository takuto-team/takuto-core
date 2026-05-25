// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
#![allow(deprecated)] // Transitional: ConfigStr sites rewritten to ConfigError variants in C2.

//! Workflow-definition step runner: builds the agent-step container, walks
//! the `[steps]` list, fans out command steps + agent sessions, and emits
//! the per-step `step_started` / `step_completed` events.
//!
//! Entry contract: callers go through [`run_workflow_def_steps`] (called by
//! `driver::drive_workflow_def`). That function in turn calls
//! [`run_agent_step_sequence`] which is the canonical loop that walks the
//! `outer × steps × repeat` matrix and dispatches per-provider sessions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, Semaphore, broadcast};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::agent_prompt::{headless_instructions_suffix, report_injection_suffix};
use crate::claude::session::ClaudeSession;
use crate::codex::CodexSession;
use crate::config::{
    AgentStepConfig, AiAgentProvider, Config, cursor_model_for_cli, interpolate_agent_prompt,
    interpolate_command_template,
};
use crate::container::ContainerRunner;
use crate::cursor::session::CursorSession;
use crate::db::Database;
use crate::actions::AgentError;
use crate::error::{MaestroError, Result};
use crate::jira::client::JiraTicket;
use crate::opencode::OpenCodeSession;
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

use super::auth_pin::try_attach_secrets_bundle;
use super::driver::{
    add_step_log, transition, transition_to_agent_step, user_id_for_ticket, wait_if_paused,
};
use super::types::{TerminalLine, Workflow, WorkflowEvent};

/// Maximum number of terminal lines stored per workflow for persistence.
const TERMINAL_LINES_MAX: usize = 100;

/// Run the full step sequence for a workflow definition: build a fresh
/// agent-step container, interpolate prompts with ticket context, and walk
/// the `[steps]` list once (no outer-cycle loop is exposed by definition
/// runs — that's an `[[agent_step]]` `repeat` concern). On success the
/// workflow transitions to `Done`.
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
    event_tx: &broadcast::Sender<WorkflowEvent>,
    cancel_token: &CancellationToken,
    log_writer: &Arc<WorkflowLogWriter>,
    agent_run_semaphore: &Arc<Semaphore>,
    // Phase 2b.3 — passed through so the agent-step runner can rebuild a
    // fresh `WorkerSecretsBundle`. `None` preserves the legacy
    // `PASSTHROUGH_ENV` path. The pin is already on the workflow at this
    // point (bootstrap wrote it); the rebuild is idempotent in the sense
    // that it re-unseals the same DB rows the pin references.
    db: Option<&Database>,
    git_auth_resolver: Option<&Arc<crate::github::auth_resolver::GitAuthResolver>>,
) -> Result<()> {
    let ticket = JiraTicket {
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
        let mut runner =
            ContainerRunner::new(ticket_key, worktree_path, &image).with_isolate_workspace();
        // Phase 2b.3 (regression fix): the bootstrap runner gets a bundle
        // attached but it goes out of scope when bootstrap returns. The
        // *agent-step* runner constructed here is what actually spawns
        // claude / cursor / codex / opencode, so it needs its own bundle
        // — without it, `wrap_command` doesn't splice `BUNDLE_SOURCING_SH`
        // and the agent CLI sees no `CLAUDE_CODE_OAUTH_TOKEN` /
        // merged `.claude.json`, surfacing as "Not logged in".
        if let Some(bundle) =
            try_attach_secrets_bundle(ticket_key, config, workflows, db, git_auth_resolver).await
        {
            runner = runner.with_secrets_bundle(bundle);
        }
        Some(runner)
    } else {
        return Err(MaestroError::ConfigStr(
            "Docker daemon is not available. DinD is required for workflow isolation. \
             Ensure DOCKER_HOST is set and the DinD sidecar is running."
                .into(),
        ));
    };

    let cfg = config.read().await;
    let timeout = cfg.agent.step_timeout_secs;
    // Task #44: resolve via the sub-table-aware helper so an empty
    // `[agent.providers.claude].model` in /admin/ai correctly omits
    // `--model` from the agent argv. The pre-#44 code read the legacy
    // flat `cfg.agent.model` directly — which still holds a migrated
    // value even after the user blanks the sub-table field — and forced
    // an outdated model (`claude-opus-4-6`) on every spawn, which custom
    // proxies (pantheon) don't accept.
    let claude_model = cfg.agent.effective_claude_model().map(str::to_string);
    let cursor_model_buf = cfg.agent.cursor_model.clone();
    let cursor_model_pass = cursor_model_for_cli(&cursor_model_buf);
    let ai_stream_provider = cfg.agent.provider;
    let cursor_cli = cfg.agent.cursor_cli.clone();
    // Phase 4: codex and opencode share the same shape as claude/cursor —
    // a single sub-table per provider with an optional `model` string.
    // Empty string means "use the CLI's default" (no `-m` flag emitted).
    let codex_model = {
        let m = cfg.agent.providers.codex.model.trim();
        if m.is_empty() { None } else { Some(m.to_string()) }
    };
    let opencode_model = {
        let m = cfg.agent.providers.opencode.model.trim();
        if m.is_empty() { None } else { Some(m.to_string()) }
    };
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
        codex_model.as_deref(),
        opencode_model.as_deref(),
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

/// Walk the agent-step matrix (`outer × steps × repeat`) for a workflow.
/// This is the canonical agent-step runner — workflow-def runs call it
/// once with `outer_loops = 1`; legacy AddressingTicket loops call it with
/// higher outer counts.
///
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
    codex_model: Option<&str>,
    opencode_model: Option<&str>,
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
                    broadcast_step_started(event_tx, ticket_key, &step_label, workflows).await;
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
                        return Err(AgentError::CommandStepFailed.into());
                    }

                    add_step_log(workflows, ticket_key, step_log).await;
                    broadcast_step_completed(event_tx, ticket_key, &step_label, workflows, config)
                        .await;
                    continue;
                }

                // ── Agent step execution ────────────────────────────────────
                let mut step_log = StepLog::new(step_label.clone());
                broadcast_step_started(event_tx, ticket_key, &step_label, workflows).await;
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
                    AiAgentProvider::Codex => CodexSession::run_prompt(
                        worktree_path,
                        &full_prompt,
                        cancel_token.child_token(),
                        timeout,
                        Some(line_tx),
                        codex_model,
                        resume_id,
                        container_runner,
                    )
                    .await
                    .map(|s| (s.session_id, s.output)),
                    AiAgentProvider::OpenCode => OpenCodeSession::run_prompt(
                        worktree_path,
                        &full_prompt,
                        cancel_token.child_token(),
                        timeout,
                        Some(line_tx),
                        opencode_model,
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
                            let hint: &'static str = match ai_stream_provider {
                                AiAgentProvider::Claude => "check that Claude Code is authenticated in the container",
                                AiAgentProvider::Cursor => "check Cursor Agent (`agent login` or CURSOR_API_KEY) and agent.providers.cursor.cli",
                                AiAgentProvider::Codex => "check Codex (`codex login --with-api-key` or OPENAI_API_KEY) and agent.providers.codex.model",
                                AiAgentProvider::OpenCode => "check OpenCode (`opencode auth login` or a project opencode.json) and agent.providers.opencode.model",
                            };
                            return Err(AgentError::AgentStepAborted { hint }.into());
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
            .map_err(|_| MaestroError::ConfigStr("Agent concurrency semaphore closed".to_string())),
    }
}

pub(super) async fn broadcast_step_started(
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    step_name: &str,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
) {
    let receiver_count = event_tx.receiver_count();
    info!(
        ticket = ticket_key,
        step = step_name,
        receivers = receiver_count,
        "Broadcasting step_started"
    );
    let owner_user_id = user_id_for_ticket(workflows, ticket_key).await;
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
        user_id: owner_user_id,
        ..Default::default()
    });
}

pub(super) async fn broadcast_step_completed(
    event_tx: &broadcast::Sender<WorkflowEvent>,
    ticket_key: &str,
    step_name: &str,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    config: &Arc<RwLock<Config>>,
) {
    let dash =
        super::driver::progress_dashboard_fields_for_ticket(workflows, config, ticket_key).await;
    let owner_user_id = user_id_for_ticket(workflows, ticket_key).await;
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
        user_id: owner_user_id,
        ..Default::default()
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
        // Resolve the owning user_id once at the start of the relay. The mapping
        // ticket → user is stable for the workflow's lifetime, so caching avoids
        // a read lock per output line.
        let owner_user_id = user_id_for_ticket(&workflows, &ticket_key).await;
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
                    user_id: owner_user_id.clone(),
                    ..Default::default()
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
        MaestroError::ConfigStr(format!(
            "Cannot close GitHub issue: '{ticket_key}' is not a GH-{{number}} key"
        ))
    })?;
    let owner_repo = crate::github::parse_github_repo(repo_url).ok_or_else(|| {
        MaestroError::ConfigStr(format!(
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
    .map_err(|e| MaestroError::ConfigStr(format!("gh api PATCH issue failed: {e}")))?;

    if !output.success() {
        return Err(MaestroError::ConfigStr(crate::github::gh_api_error_message(
            output.stderr.trim(),
            "Issues: Write",
        )));
    }

    info!(ticket = %ticket_key, issue = %issue_number, owner_repo = %owner_repo, "GitHub issue closed");
    Ok(())
}
