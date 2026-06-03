// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use axum::extract::FromRef;
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
pub struct ActiveRunCommand {
    /// Index of the command in the `[[run_commands]]` config array.
    pub cmd_index: usize,
    /// Name from config.
    pub name: String,
    /// Cancellation token for the background port scanner.
    pub scanner_cancel: CancellationToken,
    /// Detected port forwarding with proxy URL.
    pub forwarded_port: Option<DynamicPortForward>,
}

/// Map of active run commands: `ticket_key → vec of ActiveRunCommand`.
pub type RunCommandsMap = Arc<RwLock<HashMap<String, Vec<ActiveRunCommand>>>>;

/// Type alias for [`RunCommandState::run_command_bundles`]. Aliased so the
/// nested generic stays under clippy's `type_complexity` cap.
pub type RunCommandBundlesMap =
    Arc<RwLock<HashMap<(String, usize), Arc<maestro_core::auth::WorkerSecretsBundle>>>>;

/// Live mutable handles for long-running background work: the workflow engine,
/// the two `AtomicBool` gates (poller pause, in-flight repo clone), and the
/// integration health snapshot that `PUT /api/config/agent` mutates at runtime.
#[derive(Clone)]
pub struct EngineState {
    /// The workflow engine driving every workflow task.
    pub engine: Arc<WorkflowEngine>,
    /// Shared with `JiraPoller`: when `true`, poller skips `poll_once` (dashboard pause/resume or
    /// `[general] auto_polling = false` in `config.toml` at startup).
    pub polling_paused: Arc<AtomicBool>,
    /// `true` while an async `POST /api/repos/clone` operation is in progress.
    pub clone_in_progress: Arc<AtomicBool>,
    /// Structured auth + integration snapshot. Served by
    /// `GET /api/onboarding/status` and mirrored as three fields into
    /// `GET /api/auth/status`.
    ///
    /// Wrapped in `Arc<RwLock<…>>` because a successful
    /// `PUT /api/config/agent` recomputes the snapshot from the patched
    /// config and replaces the value here, so `auth_status` and
    /// `onboarding_status` reflect the new provider / degraded state without
    /// requiring a process restart.
    pub system_status: Arc<RwLock<maestro_core::docker_hooks::SystemStatus>>,
}

/// The DB plus the two GitHub-auth shims. Every protected route's middleware
/// reads `db`; `gh_client` and `git_auth_resolver` are read by PAT/credentials
/// handlers.
#[derive(Clone)]
pub struct AuthState {
    /// SQLite database for multi-user authentication and access control.
    /// `None` when the database has not been initialized (e.g., during tests that don't need it).
    pub db: Option<maestro_core::db::Database>,
    /// Injectable shim around the `gh` CLI for per-user PAT validation.
    /// Production uses [`maestro_core::auth::RealGhClient`]; tests inject a
    /// `MockGhClient` so the suite never touches github.com.
    pub gh_client: maestro_core::auth::SharedGhClient,
    /// Picks App vs user-PAT per [`GitAction`]. Holds the DB (for user PAT
    /// rows) and the optional `GitHubAppTokenManager` (for App installation
    /// tokens), and is threaded into the worker container spawn path.
    ///
    /// `None` only when `db: None` — every production AppState carries
    /// `Some(resolver)`. Test fixtures with no DB use `None` and the
    /// per-route helpers fall back to the legacy App-only token path.
    pub git_auth_resolver: Option<Arc<maestro_core::github::auth_resolver::GitAuthResolver>>,
}

/// The live `Config` plus how to persist it plus boot-time integration flags
/// read out of `[general]` plus the Phase-0 deprecated preflight string.
#[derive(Clone)]
pub struct ConfigState {
    /// The live in-memory configuration (hot-swappable via `ConfigWatcher`).
    pub config: Arc<RwLock<Config>>,
    /// Path to the config file on disk (for reload and persistence operations).
    pub config_path: PathBuf,
    /// Writer for atomic config persistence. `None` when the config file is not
    /// writable (e.g., the path is not set or the filesystem is read-only).
    pub config_writer: Option<Arc<ConfigWriter>>,
    /// Ticketing system configured at startup (read-only, from `[general] ticketing_system`).
    pub ticketing_system: TicketingSystem,
    /// `true` when acli (Atlassian CLI) passed preflight authentication.
    /// When `false`: no Jira polling, no Jira operations in workflows, manual description entry only.
    pub jira_available: Arc<AtomicBool>,
    /// **Deprecated.** Non-empty when preflight failed at startup
    /// (e.g. `gh` not authenticated). Kept for one release as a fallback when
    /// the DB is unavailable; the UI should read `system_status` instead.
    pub preflight_error: Option<String>,
}

/// Editor session container state — all 5 are keyed by `ticket_key` and
/// registered/cleared by the same lifecycle (open editor / close editor).
/// `path_token_registry` belongs here because `sessions::proxy_session`
/// resolves the token → editor backend on every `/s/{token}/…` request.
#[derive(Clone)]
pub struct EditorState {
    /// Cancellation tokens for background port scanners, keyed by ticket_key.
    pub editor_scanners: Arc<RwLock<HashMap<String, CancellationToken>>>,
    /// Active dynamic port forwards per editor, keyed by ticket_key: `(detected_port, host_port)`.
    pub dynamic_forwards: DynamicForwardsMap,
    /// Spare port and auth token allocated for ttyd web terminal per editor, keyed by ticket_key.
    pub terminal_ports: Arc<RwLock<HashMap<String, (u16, String)>>>,
    /// Keep the editor's `WorkerSecretsBundle` alive for the container's
    /// lifetime so the bind-mounted `/run/maestro-secrets/` stays populated.
    /// Without this map the `Arc` returned by
    /// `build_editor_or_run_command_bundle` is the only strong reference;
    /// when `start_editor` returns, the route handler's stack drops it,
    /// the bundle's `TempDir` RAII fires, and the host tmpfs dir gets
    /// `rm -rf`'d — leaving the still-running detached editor container
    /// bind-mounted onto an empty directory. Keyed by ticket_key
    /// (workflow id) since the editor is 1-per-workflow. Cleared in
    /// `close_editor`, `delete_workflow`, and `mark_done`.
    pub editor_bundles: Arc<RwLock<HashMap<String, Arc<maestro_core::auth::WorkerSecretsBundle>>>>,
    /// Registry of unguessable session path tokens for the shared-port proxy.
    /// Maps `{path-token} → SessionRoute` so `/s/{token}/...` requests can be
    /// dispatched to the right loopback backend (editor or terminal). The
    /// `routes::sessions::proxy_session` handler reads from this registry on
    /// every incoming request, and the workflow `open_*` / `close_*` handlers
    /// register/deregister entries as session containers come and go.
    pub path_token_registry: PathTokenRegistry,
}

/// Run-command companion to [`EditorState`]. Two fields are intentional —
/// adding a third (e.g. a future `run_command_scanners` map) is the natural
/// extension point.
#[derive(Clone)]
pub struct RunCommandState {
    /// Active run command processes, keyed by ticket_key.
    pub run_commands: RunCommandsMap,
    /// Keep run-command bundles alive for the lifetime of each detached
    /// run-command container. Keyed by `(ticket_key, cmd_index)` since a
    /// workflow can have multiple concurrent run-commands. Cleared in
    /// `stop_run_command`, `delete_workflow`, and `mark_done`.
    pub run_command_bundles: RunCommandBundlesMap,
}

/// Top-level application state — composed of 5 focused sub-structs so route
/// handlers extract only the slice they read via Axum's `FromRef`.
///
/// Construct via [`AppState::new`]; the 5 composition fields are
/// `pub(crate)` so no cross-crate struct-literal construction is possible.
/// External code (integration tests, the CLI) reads sub-states via the
/// thin accessor methods below.
#[derive(Clone)]
pub struct AppState {
    pub(crate) engine: EngineState,
    pub(crate) auth: AuthState,
    pub(crate) config: ConfigState,
    pub(crate) editor: EditorState,
    pub(crate) run_command: RunCommandState,
}

impl AppState {
    /// Compose an [`AppState`] from its 5 sub-states.
    pub fn new(
        engine: EngineState,
        auth: AuthState,
        config: ConfigState,
        editor: EditorState,
        run_command: RunCommandState,
    ) -> Self {
        Self {
            engine,
            auth,
            config,
            editor,
            run_command,
        }
    }

    /// Live mutable handles for long-running background work. Cross-crate
    /// readers (integration tests) reach the engine slice through this
    /// accessor; in-crate handlers extract `State<EngineState>` directly.
    pub fn engine(&self) -> &EngineState {
        &self.engine
    }

    /// DB + GitHub-auth shims. Cross-crate accessor (see [`Self::engine`]).
    pub fn auth(&self) -> &AuthState {
        &self.auth
    }

    /// Live `Config` + persistence shims. Cross-crate accessor.
    pub fn config(&self) -> &ConfigState {
        &self.config
    }

    /// Editor / terminal session container state. Cross-crate accessor.
    pub fn editor(&self) -> &EditorState {
        &self.editor
    }

    /// Run-command session container state. Cross-crate accessor.
    pub fn run_command(&self) -> &RunCommandState {
        &self.run_command
    }

    /// Mutable engine slice (test fixtures only — production hot-swaps go
    /// through `engine.engine`'s interior mutability instead).
    pub fn engine_mut(&mut self) -> &mut EngineState {
        &mut self.engine
    }

    /// Mutable auth slice (test fixtures swap in mock `gh_client` / `db`).
    pub fn auth_mut(&mut self) -> &mut AuthState {
        &mut self.auth
    }

    /// Mutable config slice (test fixtures override `config_path` and
    /// `config_writer` to point at a temp dir).
    pub fn config_mut(&mut self) -> &mut ConfigState {
        &mut self.config
    }

    /// Mutable editor slice (test fixtures swap session maps).
    pub fn editor_mut(&mut self) -> &mut EditorState {
        &mut self.editor
    }

    /// Mutable run-command slice (test fixtures swap session maps).
    pub fn run_command_mut(&mut self) -> &mut RunCommandState {
        &mut self.run_command
    }
}

impl FromRef<AppState> for EngineState {
    fn from_ref(state: &AppState) -> Self {
        state.engine.clone()
    }
}

impl FromRef<AppState> for AuthState {
    fn from_ref(state: &AppState) -> Self {
        state.auth.clone()
    }
}

impl FromRef<AppState> for ConfigState {
    fn from_ref(state: &AppState) -> Self {
        state.config.clone()
    }
}

impl FromRef<AppState> for EditorState {
    fn from_ref(state: &AppState) -> Self {
        state.editor.clone()
    }
}

impl FromRef<AppState> for RunCommandState {
    fn from_ref(state: &AppState) -> Self {
        state.run_command.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use axum::extract::FromRef;
    use maestro_core::config::TicketingSystem;

    use super::{AppState, AuthState, ConfigState, EditorState, EngineState, RunCommandState};

    /// Lock-in: the 5 sub-state slices carved out of [`AppState`] must each
    /// be extractable via Axum's `FromRef` and their documented fields must
    /// be reachable through the resulting slice. If anyone collapses a
    /// sub-state back into `AppState` (regressing the carve), the
    /// corresponding `FromRef` impl disappears and this test stops compiling.
    /// If anyone changes a field type / drops a field, the per-field
    /// assertions below force a deliberate update.
    #[tokio::test]
    async fn appstate_substruct_boundary_via_fromref() {
        let state = crate::test_helpers::test_state_with_db();

        // 1. EngineState — 4 fields (engine, polling_paused, clone_in_progress,
        //    system_status). The test fixture wires both AtomicBools to
        //    `false` and a default `SystemStatus` snapshot.
        let engine_state = <EngineState as FromRef<AppState>>::from_ref(&state);
        assert!(!engine_state.polling_paused.load(Ordering::Relaxed));
        assert!(!engine_state.clone_in_progress.load(Ordering::Relaxed));
        assert_eq!(
            engine_state.engine.ticketing_system(),
            TicketingSystem::None,
        );
        assert!(engine_state.engine.workflows_dir().is_absolute());
        // `system_status` is `Arc<RwLock<…>>` — exercise the read guard.
        let _snapshot = engine_state.system_status.read().await;

        // 2. AuthState — 3 fields (db, gh_client, git_auth_resolver). The
        //    fixture seeds Some(db) + Some(resolver) + a real `gh_client`.
        let auth_state = <AuthState as FromRef<AppState>>::from_ref(&state);
        assert!(auth_state.db.is_some());
        assert!(auth_state.git_auth_resolver.is_some());
        let cloned_gh = std::sync::Arc::clone(&auth_state.gh_client);
        assert!(std::sync::Arc::strong_count(&cloned_gh) >= 2);

        // 3. ConfigState — 6 fields (config, config_path, config_writer,
        //    ticketing_system, jira_available, preflight_error). The fixture
        //    points `config_path` at `temp_dir()/config.toml`, ticketing at
        //    `None`, and leaves writer / preflight empty.
        let config_state = <ConfigState as FromRef<AppState>>::from_ref(&state);
        assert_eq!(
            config_state
                .config_path
                .file_name()
                .and_then(|s| s.to_str()),
            Some("config.toml"),
        );
        assert!(matches!(
            config_state.ticketing_system,
            TicketingSystem::None,
        ));
        assert!(!config_state.jira_available.load(Ordering::Relaxed));
        assert!(config_state.config_writer.is_none());
        assert!(config_state.preflight_error.is_none());
        let _live_cfg = config_state.config.read().await;

        // 4. EditorState — 5 fields (editor_scanners, dynamic_forwards,
        //    terminal_ports, editor_bundles, path_token_registry). The
        //    fixture starts every map empty and the registry fresh.
        let editor_state = <EditorState as FromRef<AppState>>::from_ref(&state);
        assert_eq!(editor_state.editor_scanners.read().await.len(), 0);
        assert_eq!(editor_state.dynamic_forwards.read().await.len(), 0);
        assert_eq!(editor_state.terminal_ports.read().await.len(), 0);
        assert_eq!(editor_state.editor_bundles.read().await.len(), 0);
        // PathTokenRegistry has no length getter — assert the public
        // surface answers `None` for an unknown token in a fresh registry.
        assert!(
            editor_state
                .path_token_registry
                .lookup("__lock_in_unknown__")
                .await
                .is_none(),
        );

        // 5. RunCommandState — 2 fields (run_commands, run_command_bundles).
        //    Both maps empty in the fixture.
        let run_command_state = <RunCommandState as FromRef<AppState>>::from_ref(&state);
        assert_eq!(run_command_state.run_commands.read().await.len(), 0);
        assert_eq!(run_command_state.run_command_bundles.read().await.len(), 0);
    }
}
