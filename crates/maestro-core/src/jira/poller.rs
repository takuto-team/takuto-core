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

use super::client::JiraClient;

pub struct JiraPoller {
    pub config: Arc<RwLock<Config>>,
    pub engine: Arc<WorkflowEngine>,
    pub cancel_token: CancellationToken,
    /// When `true`, the poller sleeps on schedule but does not call Jira or start workflows.
    pub polling_paused: Arc<AtomicBool>,
}

impl JiraPoller {
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

        info!(
            projects = ?config.jira.project_keys,
            item_types = ?config.jira.item_types,
            dry_mode = dry_mode,
            dashboard_workflows = visible_count,
            max_active_workflows = max_active,
            "Polling Jira for tickets"
        );

        if visible_count >= max_active {
            info!(
                visible = visible_count,
                max = max_active,
                "At max active workflows (dashboard rows), skipping poll"
            );
            return Ok(());
        }

        let slots_available = max_active - visible_count;
        let repo_path = PathBuf::from(&config.git.repo_path);
        let project_keys = config.jira.project_keys.clone();
        let item_types = config.jira.item_types.clone();
        let acli_extras = config.jira.acli_extra_argv_prefixes();
        drop(config);

        let client = JiraClient::new(repo_path, acli_extras);
        let tickets = client.list_todo_tickets(&project_keys, &item_types).await?;

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

            match self
                .engine
                .start_workflow(ticket.key.clone(), ticket.summary.clone(), false, None)
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
