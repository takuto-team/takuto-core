// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

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
        let config = self.config.read().await;

        if config.jira.project_keys.is_empty() {
            info!("No Jira project keys configured, skipping poll");
            return Ok(());
        }

        let max_active = config.general.effective_max_active_workflows() as usize;
        let visible_count = self.engine.dashboard_workflow_count().await;
        let dry_mode = config.general.dry_mode;
        let max_parallel_items = config.polling.max_parallel_items;
        let max_parallel_per_user = config.polling.max_parallel_per_user;

        info!(
            projects = ?config.jira.project_keys,
            item_types = ?config.jira.item_types,
            dry_mode = dry_mode,
            dashboard_workflows = visible_count,
            max_active_workflows = max_active,
            "Polling Jira for tickets"
        );

        // Legacy ceiling: every dashboard row (including terminal Done/Stopped/
        // Error) counts toward `max_active_workflows`.
        let legacy_slots = max_active.saturating_sub(visible_count);

        // New `[polling] max_parallel_items` ceiling: only items occupying a
        // concurrency slot count (a different, narrower set than the legacy
        // gate — terminal rows are excluded). When `0`, the cap is unlimited
        // and only the legacy ceiling applies. `min()` of the two picks the
        // tighter limit.
        let item_slots = if max_parallel_items > 0 {
            let scope = if max_parallel_per_user {
                self.resolved_owner_id.as_deref()
            } else {
                None
            };
            let in_use = self.engine.active_item_count(scope).await;
            (max_parallel_items as usize).saturating_sub(in_use)
        } else {
            usize::MAX
        };

        let slots_available = legacy_slots.min(item_slots);
        if slots_available == 0 {
            info!(
                visible = visible_count,
                max = max_active,
                max_parallel_items = max_parallel_items,
                "No available item slots (legacy or parallel-item cap), skipping poll"
            );
            return Ok(());
        }

        let repo_path = PathBuf::from(&config.git.repo_path);
        let project_keys = config.jira.project_keys.clone();
        let item_types = config.jira.item_types.clone();
        let summary_keywords = config.polling.jira.summary_keywords.clone();
        drop(config);

        let client = self.ticket_source.lister(repo_path);
        let mut tickets = client.list_todo_tickets(&project_keys, &item_types).await?;

        // Item types are already applied by `list_todo_tickets`. Apply the
        // admin-configured summary-keyword filter here (empty list = no filter).
        let pre_filter = tickets.len();
        tickets.retain(|t| crate::config::matches_any_keyword(&t.summary, &summary_keywords));
        if !summary_keywords.is_empty() {
            info!(
                keywords = ?summary_keywords,
                before = pre_filter,
                after = tickets.len(),
                "Applied Jira summary keyword filter"
            );
        }

        if tickets.is_empty() {
            info!("No tickets found in To Do status");
        } else {
            for t in &tickets {
                info!(
                    ticket = %t.key,
                    summary = %t.summary,
                    item_type = %t.item_type,
                    status = %t.status,
                    "Found ticket"
                );
            }
            info!(count = tickets.len(), "Total tickets found in To Do status");
        }

        let active_keys = self.engine.get_workflow_ids().await;

        let mut started = 0;
        for ticket in tickets {
            if started >= slots_available {
                break;
            }

            // Skip tickets that already have an active workflow
            if active_keys.contains(&ticket.key) {
                continue;
            }

            info!(
                ticket = %ticket.key,
                summary = %ticket.summary,
                "Starting workflow for ticket"
            );

            // Skip when no owner could be resolved at startup — creating an orphan
            // workflow would hide it from every user's dashboard.
            let owner_id = match &self.resolved_owner_id {
                Some(id) => id.clone(),
                None => {
                    warn!(
                        ticket = %ticket.key,
                        "No resolved poller owner; skipping start_workflow to avoid orphan creation"
                    );
                    continue;
                }
            };

            match self
                .engine
                .start_workflow(
                    ticket.key.clone(),
                    ticket.summary.clone(),
                    false,
                    None,
                    None,
                    Some(owner_id),
                    // Auto-polling disabled; left unset until per-repo polling
                    // is wired in.
                    None,
                )
                .await
            {
                Ok(id) => {
                    info!(
                        ticket = %ticket.key,
                        workflow_id = %id,
                        "Workflow started"
                    );
                    started += 1;
                }
                Err(e) => {
                    warn!(
                        ticket = %ticket.key,
                        error = %e,
                        "Failed to start workflow"
                    );
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
    use crate::jira::client::JiraTicket;
    use crate::jira::source::testing::FakeJiraSourceFactory;

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

    fn engine_with_empty_definitions() -> Arc<WorkflowEngine> {
        let mut config = Config::default();
        config.jira.project_keys = vec!["PROJ".to_string()];
        config.jira.item_types = vec!["Task".to_string()];
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

        Arc::new(WorkflowEngine::new(
            config,
            actions,
            1,
            jira_available,
            TicketingSystem::None,
            workflows_dir,
        ))
    }

    /// The poll->start path runs end to end with a fake ticket source and no
    /// external `acli`/`docker` binary: each To Do ticket becomes a workflow.
    #[tokio::test]
    async fn poll_once_starts_a_workflow_for_each_todo_ticket_without_acli() {
        let engine = engine_with_empty_definitions();
        let factory = Arc::new(FakeJiraSourceFactory {
            tickets: vec![ticket("PROJ-1"), ticket("PROJ-2")],
        });

        let poller = JiraPoller::new(
            engine.config(),
            engine.clone(),
            CancellationToken::new(),
            Arc::new(AtomicBool::new(false)),
            Some("owner-1".to_string()),
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
        let engine = engine_with_empty_definitions();
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

    /// Tickets that already have a workflow are not started a second time.
    #[tokio::test]
    async fn poll_once_skips_tickets_with_existing_workflow() {
        let engine = engine_with_empty_definitions();
        engine
            .start_workflow(
                "PROJ-1".into(),
                "existing".into(),
                false,
                None,
                None,
                Some("owner-1".into()),
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
            Some("owner-1".to_string()),
            factory,
        );

        poller.poll_once().await.expect("poll_once should succeed");

        let mut ids = engine.get_workflow_ids().await;
        ids.sort();
        assert_eq!(ids, vec!["PROJ-1".to_string(), "PROJ-2".to_string()]);
    }
}
