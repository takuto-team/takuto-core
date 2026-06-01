// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Background poller that checks the merge status of PRs associated with workflows.

use std::sync::Arc;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::config::Config;
use crate::workflow::engine::{WorkflowEngine, WorkflowEvent};

use super::parse_pr_url;
use crate::process;

pub struct PrMergePoller {
    pub config: Arc<RwLock<Config>>,
    pub engine: Arc<WorkflowEngine>,
    pub cancel_token: CancellationToken,
}

impl PrMergePoller {
    pub fn new(
        config: Arc<RwLock<Config>>,
        engine: Arc<WorkflowEngine>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            config,
            engine,
            cancel_token,
        }
    }

    pub async fn run(&self) {
        info!("PR merge status poller started");

        loop {
            let interval = {
                let config = self.config.read().await;
                config.general.pr_merge_poll_interval_secs
            };

            // Disabled when interval is 0.
            if interval == 0 {
                info!("PR merge poll interval is 0, poller disabled — waiting for cancellation");
                self.cancel_token.cancelled().await;
                info!("PR merge status poller shutting down (was disabled)");
                return;
            }

            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("PR merge status poller shutting down");
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {
                    self.poll_once().await;
                }
            }
        }
    }

    async fn poll_once(&self) {
        // Collect eligible workflows: have a GitHub PR URL, not already merged.
        let eligible: Vec<(String, String, u64)> = {
            let wf_arc = self.engine.workflows_arc();
            let workflows = wf_arc.read().await;
            workflows
                .values()
                .filter_map(|w| {
                    if w.pr_merged {
                        return None;
                    }
                    let pr_url = w.pr_url.as_deref()?.trim();
                    if pr_url.is_empty() {
                        return None;
                    }
                    let (owner_repo, pr_number) = parse_pr_url(pr_url)?;
                    Some((w.ticket_key.clone(), owner_repo, pr_number))
                })
                .collect()
        };

        if eligible.is_empty() {
            debug!("No eligible workflows to check for PR merge status");
            return;
        }

        debug!(
            count = eligible.len(),
            "Checking PR merge status for eligible workflows"
        );

        let repo_path = {
            let config = self.config.read().await;
            std::path::PathBuf::from(&config.git.repo_path)
        };

        let gh_token = self
            .engine
            .actions
            .get_gh_installation_token(&repo_path)
            .await;

        for (ticket_key, owner_repo, pr_number) in eligible {
            // Check cancellation between API calls to exit promptly.
            if self.cancel_token.is_cancelled() {
                return;
            }

            match check_pr_merged(&owner_repo, pr_number, &repo_path, gh_token.as_deref()).await {
                Ok(true) => {
                    info!(
                        ticket = %ticket_key,
                        pr = format!("{owner_repo}#{pr_number}"),
                        "PR merged — updating workflow"
                    );
                    // Update the workflow's pr_merged flag.
                    {
                        let wf_arc = self.engine.workflows_arc();
                        let mut workflows = wf_arc.write().await;
                        if let Some(wf) = workflows.get_mut(&ticket_key) {
                            wf.pr_merged = true;
                            wf.updated_at = chrono::Utc::now();
                        }
                    }
                    // Broadcast the state change so the dashboard picks it up.
                    let (state_line, owner_user_id) = {
                        let wf_arc = self.engine.workflows_arc();
                        let workflows = wf_arc.read().await;
                        workflows
                            .get(&ticket_key)
                            .map(|w| (w.status_display(), w.user_id.clone()))
                            .unwrap_or_default()
                    };
                    self.engine.broadcast_event(WorkflowEvent {
                        event_type: "work_item_updated".to_string(),
                        workflow_id: String::new(),
                        ticket_key: ticket_key.clone(),
                        state: state_line,
                        timestamp: chrono::Utc::now(),
                        error: None,
                        step_name: None,
                        output_line: None,
                        stream: None,
                        progress_percent: None,
                        progress_steps_total: None,
                        forwarded_port: None,
                        pr_merged: Some(true),
                        user_id: owner_user_id,
                        ..Default::default()
                    });
                }
                Ok(false) => {
                    debug!(
                        ticket = %ticket_key,
                        pr = format!("{owner_repo}#{pr_number}"),
                        "PR not yet merged"
                    );
                }
                Err(e) => {
                    warn!(
                        ticket = %ticket_key,
                        pr = format!("{owner_repo}#{pr_number}"),
                        error = %e,
                        "Failed to check PR merge status"
                    );
                }
            }
        }
    }
}

/// Check whether a GitHub PR has been merged using `gh api`.
async fn check_pr_merged(
    owner_repo: &str,
    pr_number: u64,
    cwd: &std::path::Path,
    gh_token: Option<&str>,
) -> Result<bool, String> {
    let endpoint = format!("repos/{owner_repo}/pulls/{pr_number}");
    let env: Vec<(&str, &str)> = gh_token.map(|t| vec![("GH_TOKEN", t)]).unwrap_or_default();
    let output = process::run_command_with_env(
        "gh",
        &["api", &endpoint, "--jq", ".merged"],
        cwd,
        tokio_util::sync::CancellationToken::new(),
        &env,
    )
    .await
    .map_err(|e| format!("gh api failed: {e}"))?;

    if !output.success() {
        return Err(format!(
            "gh api failed: {}",
            crate::github::gh_api_error_message(output.stderr.trim(), "Pull requests: Read")
        ));
    }

    let trimmed = output.stdout.trim();
    match trimmed {
        "true" => Ok(true),
        "false" => Ok(false),
        other => Err(format!("unexpected gh output: {other:?}")),
    }
}
