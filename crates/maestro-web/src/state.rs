use std::sync::Arc;

use tokio::sync::RwLock;

use maestro_core::config::Config;
use maestro_core::workflow::engine::WorkflowEngine;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<WorkflowEngine>,
    pub config: Arc<RwLock<Config>>,
}
