// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::Config;
use crate::github::{fetch_open_issues, parse_github_repo};
use crate::workflow::engine::WorkflowEngine;

pub struct GitHubPoller {
    pub config: Arc<RwLock<Config>>,
    pub engine: Arc<WorkflowEngine>,
    pub cancel_token: CancellationToken,
    pub polling_paused: Arc<AtomicBool>,
}

impl GitHubPoller {
    pub fn new(
        config: Arc<RwLock<Config>>,
        engine: Arc<WorkflowEngine>,
        cancel_token: CancellationToken,
        polling_paused: Arc<AtomicBool>,
    ) -> Self {
        Self {
            config,
            engine,
            cancel_token,
            polling_paused,
        }
    }

    pub async fn run(&self) {
        info!("GitHub issue poller started");

        if self.polling_paused.load(Ordering::Relaxed) {
            info!("Polling is paused, skipping initial poll");
        } else {
            info!("Running initial GitHub poll...");
            if let Err(e) = self.poll_once().await {
                warn!(error = %e, "Initial GitHub poll failed, will retry next interval");
            }
        }

        loop {
            let interval = {
                let config = self.config.read().await;
                config.general.poll_interval_secs
            };

            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("GitHub issue poller shutting down");
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {
                    if self.polling_paused.load(Ordering::Relaxed) {
                        info!("Polling is paused, skipping GitHub poll");
                        continue;
                    }
                    if let Err(e) = self.poll_once().await {
                        warn!(error = %e, "GitHub poll failed, will retry next interval");
                    }
                }
            }
        }
    }

    async fn poll_once(&self) -> crate::error::Result<()> {
        let config = self.config.read().await;

        // Derive owner/repo from git.repo_url (e.g. "https://github.com/owner/repo" or "owner/repo")
        let repo_url = config.git.repo_url.clone();
        let max_active = config.general.effective_max_active_workflows() as usize;
        // In dry mode, external GitHub API writes (issue comments, etc.) are skipped by
        // DryRunActions, but local workflow state (worktrees, steps_log) is still created.
        // This matches the Jira poller's behaviour: dry_mode affects side-effects, not polling.
        let dry_mode = config.general.dry_mode;
        let repo_path = std::path::PathBuf::from(&config.git.repo_path);
        drop(config);

        let owner_repo = parse_github_repo(&repo_url).ok_or_else(|| {
            crate::error::MaestroError::Config(format!(
                "Cannot parse GitHub owner/repo from git.repo_url: {repo_url:?}. \
                 Expected format: https://github.com/owner/repo or owner/repo"
            ))
        })?;

        let visible_count = self.engine.dashboard_workflow_count().await;
        info!(
            repo = %owner_repo,
            dry_mode = dry_mode,
            dashboard_workflows = visible_count,
            max_active_workflows = max_active,
            "Polling GitHub issues"
        );

        if visible_count >= max_active {
            info!(
                visible = visible_count,
                max = max_active,
                "At max active workflows (dashboard rows), skipping GitHub poll"
            );
            return Ok(());
        }

        let slots_available = max_active - visible_count;

        // Fetch open issues via `gh api`, injecting the GitHub App token when configured.
        // Note: unlike the Jira poller (which supports `jql_filter` and `item_types`),
        // the GitHub poller currently fetches all open issues without label/milestone
        // filtering. A future `[github] label_filter` config option could narrow this.
        let gh_token = self
            .engine
            .actions
            .get_gh_installation_token(&repo_path)
            .await;
        let issues = fetch_open_issues(&owner_repo, &repo_path, gh_token.as_deref()).await?;

        if issues.is_empty() {
            info!("No open GitHub issues found");
        }

        let active_keys = self.engine.get_workflow_ids().await;

        let mut started = 0;
        for issue in issues {
            if started >= slots_available {
                break;
            }
            if active_keys.contains(&issue.key) {
                continue;
            }

            info!(
                key = %issue.key,
                summary = %issue.summary,
                "Starting workflow for GitHub issue"
            );

            let html_url = if issue.html_url.is_empty() {
                None
            } else {
                Some(issue.html_url.clone())
            };
            match self
                .engine
                .start_workflow(
                    issue.key.clone(),
                    issue.summary.clone(),
                    false,
                    Some(issue.body),
                    html_url,
                )
                .await
            {
                Ok(id) => {
                    info!(key = %issue.key, workflow_id = %id, "Workflow started");
                    started += 1;
                }
                Err(e) => {
                    warn!(key = %issue.key, error = %e, "Failed to start workflow for GitHub issue");
                }
            }
        }

        Ok(())
    }
}

// `GitHubIssue` and `fetch_open_issues` are defined in the parent module
// (`crate::github`) and shared with the web route handler.
