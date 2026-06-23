// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::Config;
use crate::workflow::engine::WorkflowEngine;

use super::source::TicketListerFactory;

pub struct JiraPoller {
    pub config: Arc<RwLock<Config>>,
    pub engine: Arc<WorkflowEngine>,
    pub cancel_token: CancellationToken,
    /// When `true`, the poller sleeps on schedule but does not call Jira or start workflows.
    pub polling_paused: Arc<AtomicBool>,
    /// User ID of the resolved poller owner (see `crates/takuto-cli/src/main.rs::resolve_poller_owner`).
    /// When `None`, the poller logs a warning and skips `start_workflow` calls so no orphan
    /// workflows are created.
    pub resolved_owner_id: Option<String>,
    /// Builds the per-poll [`TicketLister`]. Production passes
    /// [`super::source::RealJiraSourceFactory`]; tests inject a fake so the
    /// poll->start path runs with no `acli` binary.
    pub ticket_source: Arc<dyn TicketListerFactory>,
}

impl JiraPoller {
    pub fn new(
        config: Arc<RwLock<Config>>,
        engine: Arc<WorkflowEngine>,
        cancel_token: CancellationToken,
        polling_paused: Arc<AtomicBool>,
        resolved_owner_id: Option<String>,
        ticket_source: Arc<dyn TicketListerFactory>,
    ) -> Self {
        Self {
            config,
            engine,
            cancel_token,
            polling_paused,
            resolved_owner_id,
            ticket_source,
        }
    }

    pub async fn run(&self) {
        info!("Jira poller started");

        // Poll immediately on startup unless paused
        if self.polling_paused.load(Ordering::Relaxed) {
            info!("Jira polling is paused, skipping initial poll");
        } else {
            info!("Running initial poll...");
            if let Err(e) = self.poll_once().await {
                warn!(error = %e, "Initial Jira poll failed, will retry next interval");
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
                    info!("Jira poller shutting down");
                    return;
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(interval)) => {
                    if self.polling_paused.load(Ordering::Relaxed) {
                        info!("Jira polling is paused, skipping poll");
                        continue;
                    }
                    if let Err(e) = self.poll_once().await {
                        warn!(error = %e, "Jira poll failed, will retry next interval");
                    }
                }
            }
        }
    }

    async fn poll_once(&self) -> crate::error::Result<()> {
        // Resolve the poller owner up front: per-repo settings are owner-scoped,
        // and a workflow with no owner is invisible to every dashboard. Skipping
        // here (info, not error) preserves the orphan-workflow guard.
        let owner_id = match &self.resolved_owner_id {
            Some(id) => id.clone(),
            None => {
                info!("No resolved poller owner; skipping poll (no orphan workflows created)");
                return Ok(());
            }
        };

        // Per-repo settings live in the DB. Without a DB handle there is nothing
        // to resolve, so the poller is idle.
        let Some(db) = self.engine.db() else {
            info!("No database available; skipping Jira poll (per-repo settings unavailable)");
            return Ok(());
        };
        let adapter = db.adapter();

        // The owner's registered repositories (id + name + local_path).
        let repos = crate::db::repositories::list_for_user(adapter, &owner_id).await?;
        if repos.is_empty() {
            info!(owner = %owner_id, "Poller owner has no repositories; skipping poll");
            return Ok(());
        }

        // The owner's per-repo polling settings, keyed by workspace (repo) name.
        let settings_by_repo: HashMap<
            String,
            crate::db::user_repo_polling_settings::RepoPollingSettings,
        > = crate::db::user_repo_polling_settings::list_for_user(adapter, &owner_id)
            .await?
            .into_iter()
            .map(|r| (r.workspace_name, r.settings))
            .collect();

        let (max_active, dry_mode, max_parallel_per_user) = {
            let config = self.config.read().await;
            (
                config.general.effective_max_active_workflows() as usize,
                config.general.dry_mode,
                config.general.max_parallel_per_user,
            )
        };

        let visible_count = self.engine.dashboard_workflow_count().await;

        // Global legacy ceiling: every dashboard row (including terminal
        // Done/Stopped/Error) counts toward `max_active_workflows`. This stays
        // a deployment-global limit; only the per-repo `max_parallel_items`
        // moved into per-repo settings.
        let legacy_slots = max_active.saturating_sub(visible_count);
        if legacy_slots == 0 {
            info!(
                visible = visible_count,
                max = max_active,
                "No available item slots (global max_active_workflows), skipping poll"
            );
            return Ok(());
        }

        // Tickets already on the board (and ones started earlier in this poll)
        // must not be started twice. Jira keys are globally unique, so a key
        // started for one repo is skipped for any later repo this cycle too.
        let mut seen: HashSet<String> = self.engine.get_workflow_ids().await.into_iter().collect();
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
            if settings.project_keys.is_empty() {
                info!(
                    repo = %repo.name,
                    "auto_polling on but no Jira project keys configured; skipping"
                );
                continue;
            }

            // Per-repo `max_parallel_items` cap (0 = unlimited). Scope per the
            // repo's `max_parallel_per_user` (poller owner) or globally.
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
                info!(
                    repo = %repo.name,
                    max_parallel_items = settings.max_parallel_items,
                    "Per-repo parallel-item cap reached; skipping repository"
                );
                continue;
            }
            // The tighter of the global legacy budget remaining and this repo's
            // own parallel-item budget.
            let repo_budget = (legacy_slots - started).min(repo_item_slots);

            info!(
                repo = %repo.name,
                projects = ?settings.project_keys,
                item_types = ?settings.item_types,
                dry_mode = dry_mode,
                dashboard_workflows = visible_count,
                max_active_workflows = max_active,
                "Polling Jira for tickets"
            );

            let client = self.ticket_source.lister(PathBuf::from(&repo.local_path));
            let mut tickets = match client
                .list_todo_tickets(&settings.project_keys, &settings.item_types)
                .await
            {
                Ok(t) => t,
                Err(e) => {
                    warn!(
                        repo = %repo.name,
                        error = %e,
                        "Jira poll failed for repository, continuing with other repositories"
                    );
                    continue;
                }
            };

            // Item types are already applied by `list_todo_tickets`. Apply the
            // per-repo summary-keyword filter here (empty = no filter).
            let pre_filter = tickets.len();
            tickets.retain(|t| {
                crate::config::matches_any_keyword(&t.summary, &settings.jira_summary_keywords)
            });
            if !settings.jira_summary_keywords.is_empty() {
                info!(
                    repo = %repo.name,
                    keywords = ?settings.jira_summary_keywords,
                    before = pre_filter,
                    after = tickets.len(),
                    "Applied Jira summary keyword filter"
                );
            }

            let mut repo_started = 0usize;
            for ticket in tickets {
                if started >= legacy_slots || repo_started >= repo_budget {
                    break;
                }
                if seen.contains(&ticket.key) {
                    continue;
                }

                info!(
                    ticket = %ticket.key,
                    repo = %repo.name,
                    summary = %ticket.summary,
                    "Starting workflow for ticket"
                );

                match self
                    .engine
                    .start_workflow(
                        ticket.key.clone(),
                        ticket.summary.clone(),
                        false,
                        None,
                        None,
                        Some(owner_id.clone()),
                        Some(repo.id.clone()),
                    )
                    .await
                {
                    Ok(id) => {
                        info!(ticket = %ticket.key, workflow_id = %id, "Workflow started");
                        seen.insert(ticket.key.clone());
                        started += 1;
                        repo_started += 1;
                    }
                    Err(e) => {
                        warn!(ticket = %ticket.key, error = %e, "Failed to start workflow");
                    }
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::actions::dry_run::DryRunActions;
    use crate::actions::traits::ExternalActions;
    use crate::config::TicketingSystem;
    use crate::db::{Database, DbValue};
    use crate::jira::client::JiraTicket;
    use crate::jira::source::testing::FakeJiraSourceFactory;

    const REPO_NAME: &str = "takuto-core";

    fn ticket(key: &str) -> JiraTicket {
        JiraTicket {
            key: key.to_string(),
            summary: format!("summary {key}"),
            description: String::new(),
            item_type: "Task".to_string(),
            status: "To Do".to_string(),
            linked_items: Vec::new(),
        }
    }

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

    /// Build an engine backed by an in-memory DB, seed a poller-owner user with
    /// one repository and (optionally) per-repo polling settings with
    /// auto_polling + keys. Returns `(engine, owner_id)`. `with_keys = false`
    /// seeds the repo but no settings row, so the poller skips it.
    async fn engine_with_owner_repo(with_keys: bool) -> Arc<WorkflowEngine> {
        let (engine, _owner) = engine_with_owner_repo_id(with_keys).await;
        engine
    }

    async fn engine_with_owner_repo_id(with_keys: bool) -> (Arc<WorkflowEngine>, String) {
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
        if with_keys {
            let settings = crate::db::user_repo_polling_settings::RepoPollingSettings {
                auto_polling: true,
                project_keys: vec!["PROJ".to_string()],
                item_types: vec!["Task".to_string()],
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
        // Plenty of slots so the poll->start cap is not what limits the test.
        config.general.max_concurrent_workflows = 10;
        config.general.max_active_workflows = 0;
        let config = Arc::new(RwLock::new(config));

        let actions: Arc<dyn ExternalActions> = Arc::new(DryRunActions::new("origin".into(), None));
        let jira_available = Arc::new(AtomicBool::new(false));
        // An empty workflows dir means no dep-free definitions exist, so
        // `start_workflow` inserts the row but spawns no driver — the
        // poll->start path completes with no git/docker side effects.
        let workflows_dir =
            std::env::temp_dir().join(format!("takuto-poller-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&workflows_dir).expect("create workflows dir");

        let engine = Arc::new(WorkflowEngine::new_with_db(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::None,
            workflows_dir,
            Some(db),
        ));
        (engine, owner_id)
    }

    /// The poll->start path runs end to end with a fake ticket source and no
    /// external `acli`/`docker` binary: each To Do ticket for the owner's repo
    /// becomes a workflow.
    #[tokio::test]
    async fn poll_once_starts_a_workflow_for_each_todo_ticket_without_acli() {
        let (engine, owner_id) = engine_with_owner_repo_id(true).await;
        let factory = Arc::new(FakeJiraSourceFactory {
            tickets: vec![ticket("PROJ-1"), ticket("PROJ-2")],
        });

        let poller = JiraPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Some(owner_id),
            factory,
        );

        poller.poll_once().await.expect("poll_once should succeed");

        let mut ids = engine.get_workflow_ids().await;
        ids.sort();
        assert_eq!(ids, vec!["PROJ-1".to_string(), "PROJ-2".to_string()]);
    }

    /// With no resolved owner, the poller must not create orphan workflows.
    #[tokio::test]
    async fn poll_once_skips_start_when_no_owner_resolved() {
        let engine = engine_with_owner_repo(true).await;
        let factory = Arc::new(FakeJiraSourceFactory {
            tickets: vec![ticket("PROJ-9")],
        });

        let poller = JiraPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            None,
            factory,
        );

        poller.poll_once().await.expect("poll_once should succeed");
        assert!(engine.get_workflow_ids().await.is_empty());
    }

    /// A repository with no configured Jira keys is skipped — no workflows.
    #[tokio::test]
    async fn poll_once_skips_repo_without_keys() {
        let (engine, owner_id) = engine_with_owner_repo_id(false).await;
        let factory = Arc::new(FakeJiraSourceFactory {
            tickets: vec![ticket("PROJ-1")],
        });

        let poller = JiraPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Some(owner_id),
            factory,
        );

        poller.poll_once().await.expect("poll_once should succeed");
        assert!(engine.get_workflow_ids().await.is_empty());
    }

    /// Tickets that already have a workflow are not started a second time.
    #[tokio::test]
    async fn poll_once_skips_tickets_with_existing_workflow() {
        let (engine, owner_id) = engine_with_owner_repo_id(true).await;
        engine
            .start_workflow(
                "PROJ-1".into(),
                "existing".into(),
                false,
                None,
                None,
                Some(owner_id.clone()),
                None,
            )
            .await
            .expect("seed existing workflow");

        let factory = Arc::new(FakeJiraSourceFactory {
            tickets: vec![ticket("PROJ-1"), ticket("PROJ-2")],
        });
        let poller = JiraPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Some(owner_id),
            factory,
        );

        poller.poll_once().await.expect("poll_once should succeed");

        let mut ids = engine.get_workflow_ids().await;
        ids.sort();
        assert_eq!(ids, vec!["PROJ-1".to_string(), "PROJ-2".to_string()]);
    }
}
