use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use maestro_core::config::{Config, TicketingSystem};
use maestro_core::workflow::engine::WorkflowEngine;

/// Port forwarding map: ticket_key → list of `(detected_port, host_port)` pairs.
type DynamicForwardsMap = Arc<RwLock<HashMap<String, Vec<(u16, u16)>>>>;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<WorkflowEngine>,
    pub config: Arc<RwLock<Config>>,
    /// Shared with `JiraPoller`: when `true`, poller skips `poll_once` (dashboard pause/resume or
    /// `[general] pause_jira_polling_on_startup` in `config.toml` at startup).
    pub polling_paused: Arc<AtomicBool>,
    /// `true` when acli (Atlassian CLI) passed preflight authentication.
    /// When `false`: no Jira polling, no Jira operations in workflows, manual description entry only.
    pub jira_available: Arc<AtomicBool>,
    /// Ticketing system configured at startup (read-only, from `[general] ticketing_system`).
    pub ticketing_system: TicketingSystem,
    /// Cancellation tokens for background port scanners, keyed by ticket_key.
    pub editor_scanners: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Active dynamic port forwards per editor, keyed by ticket_key: `(detected_port, host_port)`.
    pub dynamic_forwards: DynamicForwardsMap,
    /// Spare port and auth token allocated for ttyd web terminal per editor, keyed by ticket_key.
    pub terminal_ports: Arc<RwLock<HashMap<String, (u16, String)>>>,
}
