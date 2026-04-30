// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use maestro_core::config::{Config, TicketingSystem};
use maestro_core::config_writer::ConfigWriter;
use maestro_core::workflow::engine::WorkflowEngine;

/// Port forwarding map: ticket_key → list of `(container_port, host_port)` pairs.
/// Includes both static Docker `-p` mappings (seeded at editor open) and dynamic
/// socat forwards (tracked by the event subscriber).
///
/// Unit-test note: `DynamicForwardsMap` is a type alias with no custom methods;
/// merge/dedup logic lives in `routes/workflows.rs` (`track_port_forwards` and
/// the list/detail handlers). Port dedup and "dynamic wins on conflict" are tested
/// at the route level, not here.
pub type DynamicForwardsMap = Arc<RwLock<HashMap<String, Vec<(u16, u16)>>>>;

/// State for a single active run command.
#[derive(Debug, Clone)]
pub struct RunCommandState {
    /// Index of the command in the `[[run_commands]]` config array.
    pub cmd_index: usize,
    /// Name from config.
    pub name: String,
    /// Cancellation token for the background port scanner.
    pub scanner_cancel: CancellationToken,
    /// Detected port forwarding: `(container_port, host_port)`.
    pub forwarded_port: Option<(u16, u16)>,
}

/// Map of active run commands: `ticket_key → vec of RunCommandState`.
pub type RunCommandsMap = Arc<RwLock<HashMap<String, Vec<RunCommandState>>>>;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<WorkflowEngine>,
    pub config: Arc<RwLock<Config>>,
    /// Shared with `JiraPoller`: when `true`, poller skips `poll_once` (dashboard pause/resume or
    /// `[general] auto_polling = false` in `config.toml` at startup).
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
    /// Active run command processes, keyed by ticket_key.
    pub run_commands: RunCommandsMap,
    /// Non-empty when preflight failed at startup (e.g. `gh` not authenticated).
    /// Exposed via `GET /api/config` so the UI can show a blocking error banner.
    pub preflight_error: Option<String>,
    /// Path to the config file on disk (for reload and persistence operations).
    pub config_path: PathBuf,
    /// Writer for atomic config persistence. `None` when the config file is not
    /// writable (e.g., the path is not set or the filesystem is read-only).
    pub config_writer: Option<Arc<ConfigWriter>>,
    /// `true` while an async `POST /api/repos/clone` operation is in progress.
    pub clone_in_progress: Arc<AtomicBool>,
}
