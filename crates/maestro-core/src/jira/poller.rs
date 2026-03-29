use std::path::PathBuf;
use std::sync::Arc;

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
}

impl JiraPoller {
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
        info!("Jira poller started");
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

        let max_concurrent = config.general.max_concurrent_workflows as usize;
        let active_count = self.engine.active_workflow_count().await;
        if active_count >= max_concurrent {
            info!(
                active = active_count,
                max = max_concurrent,
                "At max concurrent workflows, skipping poll"
            );
            return Ok(());
        }

        let slots_available = max_concurrent - active_count;
        let repo_path = PathBuf::from(&config.git.repo_path);
        let project_keys = config.jira.project_keys.clone();
        let item_types = config.jira.item_types.clone();
        drop(config);

        let client = JiraClient::new(repo_path);
        let tickets = client.list_todo_tickets(&project_keys, &item_types).await?;

        info!(count = tickets.len(), "Found tickets in To Do status");

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
                .start_workflow(ticket.key.clone(), ticket.summary.clone())
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
