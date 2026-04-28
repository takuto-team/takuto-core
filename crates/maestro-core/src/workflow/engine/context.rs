// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::{RwLock, Semaphore};

use crate::actions::traits::ExternalActions;
use crate::config::{Config, TicketingSystem};

use super::event_bus::WorkflowEventBus;
use super::repository::WorkflowRepository;

pub(crate) struct WorkflowContext {
    pub config: Arc<RwLock<Config>>,
    pub repository: Arc<WorkflowRepository>,
    pub event_bus: Arc<WorkflowEventBus>,
    pub actions: Arc<dyn ExternalActions>,
    pub agent_run_semaphore: Arc<Semaphore>,
    pub suppress_cancelled_as_error: Arc<AtomicBool>,
    pub jira_available: Arc<AtomicBool>,
    pub ticketing_system: TicketingSystem,
    pub workflows_dir: PathBuf,
}

impl WorkflowContext {
    pub fn new(
        config: Arc<RwLock<Config>>,
        repository: Arc<WorkflowRepository>,
        event_bus: Arc<WorkflowEventBus>,
        actions: Arc<dyn ExternalActions>,
        agent_run_semaphore: Arc<Semaphore>,
        suppress_cancelled_as_error: Arc<AtomicBool>,
        jira_available: Arc<AtomicBool>,
        ticketing_system: TicketingSystem,
        workflows_dir: PathBuf,
    ) -> Self {
        Self {
            config,
            repository,
            event_bus,
            actions,
            agent_run_semaphore,
            suppress_cancelled_as_error,
            jira_available,
            ticketing_system,
            workflows_dir,
        }
    }
}
