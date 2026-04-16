use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::Config;
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
        Self { config, engine, cancel_token, polling_paused }
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
        let dry_mode = config.general.dry_mode;
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

        // Fetch open issues via `gh api`
        let issues = fetch_open_issues(&owner_repo).await?;

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

            match self.engine
                .start_workflow(
                    issue.key.clone(),
                    issue.summary.clone(),
                    false,
                    Some(issue.description),
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

struct GitHubIssue {
    key: String,
    summary: String,
    description: String,
}

/// Parse `owner/repo` from a GitHub URL or bare `owner/repo` string.
fn parse_github_repo(repo_url: &str) -> Option<String> {
    let url = repo_url.trim().trim_end_matches('/').trim_end_matches(".git");
    // "https://github.com/owner/repo" or "git@github.com:owner/repo"
    if let Some(rest) = url.strip_prefix("https://github.com/") {
        if rest.contains('/') {
            return Some(rest.to_string());
        }
    }
    if let Some(rest) = url.strip_prefix("git@github.com:") {
        if rest.contains('/') {
            return Some(rest.to_string());
        }
    }
    // bare "owner/repo"
    if url.contains('/') && !url.contains("://") {
        return Some(url.to_string());
    }
    None
}

/// Fetch open GitHub issues using `gh api`. Returns issues as key/summary/description.
async fn fetch_open_issues(owner_repo: &str) -> crate::error::Result<Vec<GitHubIssue>> {
    let output = tokio::process::Command::new("gh")
        .args([
            "api",
            "--method", "GET",
            &format!("repos/{owner_repo}/issues"),
            "--field", "state=open",
            "--field", "per_page=50",
        ])
        .output()
        .await
        .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(crate::error::MaestroError::Config(format!(
            "gh api repos/{owner_repo}/issues failed: {stderr}"
        )));
    }

    let json: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| crate::error::MaestroError::Config(e.to_string()))?;

    let issues = json.as_array().map(|arr| {
        arr.iter().filter_map(|v| {
            // Skip pull requests (GitHub API returns PRs in issues endpoint)
            if v.get("pull_request").is_some() {
                return None;
            }
            let number = v.get("number")?.as_u64()?;
            let title = v.get("title")?.as_str().unwrap_or("").to_string();
            let body = v.get("body")
                .and_then(|b| b.as_str())
                .unwrap_or("")
                .to_string();
            Some(GitHubIssue {
                key: format!("GH-{number}"),
                summary: title,
                description: body,
            })
        }).collect::<Vec<_>>()
    }).unwrap_or_default();

    Ok(issues)
}
