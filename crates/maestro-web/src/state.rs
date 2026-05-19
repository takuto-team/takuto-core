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

use crate::session_registry::PathTokenRegistry;

/// A single dynamically forwarded port with its proxy metadata.
#[derive(Debug, Clone)]
pub struct DynamicPortForward {
    /// Port inside the container (what the user's app listens on, e.g. 3000).
    pub container_port: u16,
    /// Host port that socat forwards to (kept for `SessionRoute` and cleanup).
    pub host_port: u16,
    /// Proxy URL: `/s/{path_token}/`.
    pub proxy_url: String,
    /// Path token registered in the proxy registry (for deregistration on removal).
    pub path_token: String,
}

/// Port forwarding map: ticket_key → list of [`DynamicPortForward`].
/// Includes both static Docker `-p` mappings (seeded at editor open) and dynamic
/// socat forwards (tracked by the event subscriber).
pub type DynamicForwardsMap = Arc<RwLock<HashMap<String, Vec<DynamicPortForward>>>>;

/// State for a single active run command.
#[derive(Debug, Clone)]
pub struct RunCommandState {
    /// Index of the command in the `[[run_commands]]` config array.
    pub cmd_index: usize,
    /// Name from config.
    pub name: String,
    /// Cancellation token for the background port scanner.
    pub scanner_cancel: CancellationToken,
    /// Detected port forwarding with proxy URL.
    pub forwarded_port: Option<DynamicPortForward>,
}

/// Map of active run commands: `ticket_key → vec of RunCommandState`.
pub type RunCommandsMap = Arc<RwLock<HashMap<String, Vec<RunCommandState>>>>;

#[derive(Clone)]
pub struct AppState {
    pub engine: Arc<WorkflowEngine>,
    pub config: Arc<RwLock<Config>>,
    /// SQLite database for multi-user authentication and access control.
    /// `None` when the database has not been initialized (e.g., during tests that don't need it).
    pub db: Option<maestro_core::db::Database>,
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
    /// **Deprecated (Phase 0).** Non-empty when preflight failed at startup
    /// (e.g. `gh` not authenticated). Kept for one release as a fallback when
    /// the DB is unavailable; the UI should read [`system_status`] instead.
    pub preflight_error: Option<String>,
    /// Structured auth + integration snapshot (Phase 0). Served by
    /// `GET /api/onboarding/status` and mirrored as three fields into
    /// `GET /api/auth/status`.
    ///
    /// Wrapped in `Arc<RwLock<…>>` because Phase 1 mutates it: a successful
    /// `PUT /api/config/agent` recomputes the snapshot from the patched
    /// config and replaces the value here, so `auth_status` and
    /// `onboarding_status` reflect the new provider / degraded state without
    /// requiring a process restart (Phase 1 AC-4).
    pub system_status: Arc<RwLock<maestro_core::docker_hooks::SystemStatus>>,
    /// Path to the config file on disk (for reload and persistence operations).
    pub config_path: PathBuf,
    /// Writer for atomic config persistence. `None` when the config file is not
    /// writable (e.g., the path is not set or the filesystem is read-only).
    pub config_writer: Option<Arc<ConfigWriter>>,
    /// `true` while an async `POST /api/repos/clone` operation is in progress.
    pub clone_in_progress: Arc<AtomicBool>,
    /// Phase 2b.1: injectable shim around the `gh` CLI for per-user PAT
    /// validation. Production uses [`maestro_core::auth::RealGhClient`];
    /// tests inject a `MockGhClient` so the suite never touches github.com.
    pub gh_client: maestro_core::auth::SharedGhClient,
    /// Phase 2b.2: picks App vs user-PAT per [`GitAction`] per
    /// 04_architecture.md §4.2. Holds the DB (for user PAT rows) and the
    /// optional `GitHubAppTokenManager` (for App installation tokens). Phase
    /// 2b.3 will thread it into the worker container spawn path.
    ///
    /// `None` only when `db: None` — every production AppState carries
    /// `Some(resolver)`. Test fixtures with no DB use `None` and the
    /// per-route helpers fall back to the legacy App-only token path.
    pub git_auth_resolver: Option<Arc<maestro_core::github::auth_resolver::GitAuthResolver>>,
    /// Registry of unguessable session path tokens (GH-45 shared-port proxy).
    /// Maps `{path-token} → SessionRoute` so `/s/{token}/...` requests can be
    /// dispatched to the right loopback backend (editor or terminal). The
    /// `routes::sessions::proxy_session` handler reads from this registry on
    /// every incoming request, and the workflow `open_*` / `close_*` handlers
    /// register/deregister entries as session containers come and go.
    pub path_token_registry: PathTokenRegistry,
    /// Task #42: keep the editor's `WorkerSecretsBundle` alive for the
    /// container's lifetime so the bind-mounted `/run/maestro-secrets/`
    /// stays populated. Without this map the `Arc` returned by
    /// `build_editor_or_run_command_bundle` is the only strong reference;
    /// when `start_editor` returns, the route handler's stack drops it,
    /// the bundle's `TempDir` RAII fires, and the host tmpfs dir gets
    /// `rm -rf`'d — leaving the still-running detached editor container
    /// bind-mounted onto an empty directory. Keyed by ticket_key
    /// (workflow id) since the editor is 1-per-workflow. Cleared in
    /// `close_editor`, `delete_workflow`, and `mark_done`.
    pub editor_bundles:
        Arc<RwLock<HashMap<String, Arc<maestro_core::auth::WorkerSecretsBundle>>>>,
    /// Task #42: keep run-command bundles alive for the lifetime of each
    /// detached run-command container. Keyed by `(ticket_key, cmd_index)`
    /// since a workflow can have multiple concurrent run-commands. Cleared
    /// in `stop_run_command`, `delete_workflow`, and `mark_done`.
    pub run_command_bundles: RunCommandBundlesMap,
}

/// Task #42: type alias for [`AppState::run_command_bundles`]. Aliased so
/// the nested generic stays under clippy's `type_complexity` cap.
pub type RunCommandBundlesMap =
    Arc<RwLock<HashMap<(String, usize), Arc<maestro_core::auth::WorkerSecretsBundle>>>>;
