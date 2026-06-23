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
    /// User ID of the resolved poller owner (see `crates/takuto-cli/src/main.rs::resolve_poller_owner`).
    /// When `None`, the poller logs a warning and skips `start_workflow` calls so no orphan
    /// workflows are created.
    pub resolved_owner_id: Option<String>,
}

impl GitHubPoller {
    pub fn new(
        config: Arc<RwLock<Config>>,
        engine: Arc<WorkflowEngine>,
        cancel_token: CancellationToken,
        polling_paused: Arc<AtomicBool>,
        resolved_owner_id: Option<String>,
    ) -> Self {
        Self {
            config,
            engine,
            cancel_token,
            polling_paused,
            resolved_owner_id,
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
            // One shared loop cadence (deployment-global). Per-repo enable
            // (`auto_polling`) selects which repos are polled each cycle.
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
        let owner_id = match &self.resolved_owner_id {
            Some(id) => id.clone(),
            None => {
                info!(
                    "No resolved poller owner; skipping GitHub poll (no orphan workflows created)"
                );
                return Ok(());
            }
        };

        let Some(db) = self.engine.db() else {
            info!("No database available; skipping GitHub poll (per-repo settings unavailable)");
            return Ok(());
        };
        let adapter = db.adapter();

        let (remote, max_active, dry_mode, max_parallel_per_user) = {
            let config = self.config.read().await;
            (
                config.git.remote.clone(),
                config.general.effective_max_active_workflows() as usize,
                config.general.dry_mode,
                config.general.max_parallel_per_user,
            )
        };

        let repos = crate::db::repositories::list_for_user(adapter, &owner_id).await?;
        if repos.is_empty() {
            info!(owner = %owner_id, "Poller owner has no repositories; skipping GitHub poll");
            return Ok(());
        }
        let settings_by_repo: std::collections::HashMap<
            String,
            crate::db::user_repo_polling_settings::RepoPollingSettings,
        > = crate::db::user_repo_polling_settings::list_for_user(adapter, &owner_id)
            .await?
            .into_iter()
            .map(|r| (r.workspace_name, r.settings))
            .collect();

        let visible_count = self.engine.dashboard_workflow_count().await;
        // Global legacy ceiling stays deployment-wide; per-repo
        // `max_parallel_items` lives in settings.
        let legacy_slots = max_active.saturating_sub(visible_count);
        if legacy_slots == 0 {
            info!(
                visible = visible_count,
                max = max_active,
                "No available item slots (global max_active_workflows), skipping GitHub poll"
            );
            return Ok(());
        }

        let mut seen: std::collections::HashSet<String> =
            self.engine.get_workflow_ids().await.into_iter().collect();
        let mut started = 0usize;

        for repo in &repos {
            if started >= legacy_slots {
                break;
            }
            let Some(settings) = settings_by_repo.get(&repo.name) else {
                continue;
            };
            if !settings.auto_polling {
                continue;
            }

            let repo_path = std::path::PathBuf::from(&repo.local_path);
            let remote_url = match crate::git::remote::resolve_remote_url(&repo_path, &remote).await
            {
                Ok(url) => url,
                Err(e) => {
                    warn!(repo = %repo.name, error = %e, "GitHub poller: cannot resolve git remote URL — skipping repository");
                    continue;
                }
            };
            let Some(owner_repo) = parse_github_repo(&remote_url) else {
                warn!(repo = %repo.name, url = %remote_url, "GitHub poller: cannot parse owner/repo from remote — skipping");
                continue;
            };

            // Per-repo parallel-item cap (0 = unlimited).
            let repo_item_slots = if settings.max_parallel_items > 0 {
                let scope = if max_parallel_per_user {
                    Some(owner_id.as_str())
                } else {
                    None
                };
                let in_use = self.engine.active_item_count(scope).await;
                (settings.max_parallel_items as usize).saturating_sub(in_use)
            } else {
                usize::MAX
            };
            if repo_item_slots == 0 {
                info!(repo = %repo.name, "Per-repo parallel-item cap reached; skipping repository");
                continue;
            }
            let repo_budget = (legacy_slots - started).min(repo_item_slots);

            info!(
                repo = %owner_repo,
                dry_mode = dry_mode,
                dashboard_workflows = visible_count,
                max_active_workflows = max_active,
                "Polling GitHub issues"
            );

            let app_token = self
                .engine
                .actions
                .get_gh_installation_token(&repo_path)
                .await;
            let gh_token = crate::github::github_token_app_then_pat(
                app_token,
                self.engine.git_auth_resolver().as_ref(),
                Some(owner_id.as_str()),
                crate::github::auth_resolver::GitAction::Clone,
            )
            .await;
            let mut issues = match fetch_open_issues(&owner_repo, &repo_path, gh_token.as_deref())
                .await
            {
                Ok(i) => i,
                Err(e) => {
                    warn!(repo = %repo.name, error = %e, "GitHub poll failed for repository, continuing");
                    continue;
                }
            };

            // Per-repo label + title-keyword filters.
            let pre_filter = issues.len();
            issues.retain(|i| {
                let label_ok = settings.github_labels.is_empty()
                    || i.labels.iter().any(|l| settings.github_labels.contains(l));
                let title_ok =
                    crate::config::matches_any_keyword(&i.summary, &settings.github_title_keywords);
                label_ok && title_ok
            });
            if !settings.github_labels.is_empty() || !settings.github_title_keywords.is_empty() {
                info!(
                    repo = %repo.name,
                    labels = ?settings.github_labels,
                    title_keywords = ?settings.github_title_keywords,
                    before = pre_filter,
                    after = issues.len(),
                    "Applied GitHub issue filters"
                );
            }

            let mut repo_started = 0usize;
            for issue in issues {
                if started >= legacy_slots || repo_started >= repo_budget {
                    break;
                }
                if seen.contains(&issue.key) {
                    continue;
                }

                info!(key = %issue.key, repo = %repo.name, summary = %issue.summary, "Starting workflow for GitHub issue");

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
                        Some(owner_id.clone()),
                        Some(repo.id.clone()),
                    )
                    .await
                {
                    Ok(id) => {
                        info!(key = %issue.key, workflow_id = %id, "Workflow started");
                        seen.insert(issue.key.clone());
                        started += 1;
                        repo_started += 1;
                    }
                    Err(e) => {
                        warn!(key = %issue.key, error = %e, "Failed to start workflow for GitHub issue");
                    }
                }
            }
        }

        Ok(())
    }
}

// `GitHubIssue` and `fetch_open_issues` are defined in the parent module
// (`crate::github`) and shared with the web route handler.

#[cfg(test)]
mod tests {
    use super::*;

    use crate::actions::dry_run::DryRunActions;
    use crate::actions::traits::ExternalActions;
    use crate::config::TicketingSystem;
    use crate::db::{Database, DbValue};

    const REPO_NAME: &str = "takuto-core";

    async fn seed_user(db: &Database, username: &str) -> String {
        let id = format!("u-{username}");
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'admin')",
                vec![
                    DbValue::Text(id.clone()),
                    DbValue::Text(username.to_string()),
                ],
            )
            .await
            .expect("seed user");
        id
    }

    /// Build an engine backed by an in-memory DB. The owner gets one repository;
    /// `auto_polling` controls whether a per-repo settings row enables polling.
    /// The repo's `local_path` does not exist on disk, so any repo that *is*
    /// enabled is skipped gracefully at the git-remote resolution step (no `gh`
    /// binary is ever invoked) — exactly the early-exit branches under test.
    async fn engine_with_owner_repo(auto_polling: bool) -> (Arc<WorkflowEngine>, String) {
        let db = Database::open_in_memory().expect("in-memory db");
        let owner_id = seed_user(&db, "owner").await;
        let repo_id = crate::db::repositories::upsert(
            db.adapter(),
            REPO_NAME,
            None,
            &format!("/workspaces/{REPO_NAME}"),
            "main",
            Some(&owner_id),
        )
        .await
        .expect("seed repo");
        crate::db::repositories::add_for_user(db.adapter(), &owner_id, &repo_id)
            .await
            .expect("add repo for user");
        if auto_polling {
            let settings = crate::db::user_repo_polling_settings::RepoPollingSettings {
                auto_polling: true,
                ..crate::db::user_repo_polling_settings::RepoPollingSettings::default()
            };
            crate::db::user_repo_polling_settings::set(
                db.adapter(),
                &owner_id,
                REPO_NAME,
                &settings,
            )
            .await
            .expect("seed settings");
        }

        let mut config = Config::default();
        config.general.max_concurrent_workflows = 10;
        config.general.max_active_workflows = 0;
        let config = Arc::new(RwLock::new(config));

        let actions: Arc<dyn ExternalActions> = Arc::new(DryRunActions::new("origin".into(), None));
        let jira_available = Arc::new(AtomicBool::new(false));
        let workflows_dir =
            std::env::temp_dir().join(format!("takuto-gh-poller-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");

        let engine = Arc::new(WorkflowEngine::new_with_db(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::GitHub,
            workflows_dir,
            Some(db),
        ));
        (engine, owner_id)
    }

    /// With no resolved owner, the poller must not create orphan workflows.
    #[tokio::test]
    async fn poll_once_skips_start_when_no_owner_resolved() {
        let (engine, _owner) = engine_with_owner_repo(true).await;
        let poller = GitHubPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            None,
        );
        poller.poll_once().await.expect("poll_once should succeed");
        assert!(engine.get_workflow_ids().await.is_empty());
    }

    /// A repository with no per-repo settings row (auto_polling defaults off) is
    /// skipped entirely — no workflows started.
    #[tokio::test]
    async fn poll_once_skips_repo_without_settings() {
        let (engine, owner_id) = engine_with_owner_repo(false).await;
        let poller = GitHubPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Some(owner_id),
        );
        poller.poll_once().await.expect("poll_once should succeed");
        assert!(engine.get_workflow_ids().await.is_empty());
    }

    /// An enabled repo whose local path / remote cannot be resolved is skipped
    /// gracefully (no panic, no workflow) — the poll completes Ok.
    #[tokio::test]
    async fn poll_once_skips_enabled_repo_with_unresolvable_remote() {
        let (engine, owner_id) = engine_with_owner_repo(true).await;
        let poller = GitHubPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Some(owner_id),
        );
        poller.poll_once().await.expect("poll_once should succeed");
        assert!(engine.get_workflow_ids().await.is_empty());
    }
}
