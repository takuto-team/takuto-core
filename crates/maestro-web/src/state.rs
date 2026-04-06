use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;

use maestro_core::config::Config;
use maestro_core::workflow::engine::WorkflowEngine;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<WorkflowEngine>,
    pub config: Arc<RwLock<Config>>,
    /// Shared with `JiraPoller`: when `true`, poller skips `poll_once` (dashboard pause/resume or
    /// `[general] pause_jira_polling_on_startup` in `config.toml` at startup).
    pub polling_paused: Arc<AtomicBool>,
}
