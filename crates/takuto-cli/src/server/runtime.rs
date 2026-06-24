// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 4: decide which poller to run, assemble `AppState`, spawn the
//! background tasks, then hand the HTTP server + poller futures to
//! [`super::serve`] (the irreducible bind/serve/signal shell).

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;

use takuto_core::config::{Config, TicketingSystem};
use takuto_core::config_watcher::ConfigWatcher;
use takuto_core::config_writer::ConfigWriter;
use takuto_core::db::Database;
use takuto_core::db::user_work_item_flows::UserFlow;
use takuto_core::docker_hooks;
use takuto_core::docker_hooks::SystemStatus;
use takuto_core::github::auth_resolver::GitAuthResolver;
use takuto_core::github::poller::GitHubPoller;
use takuto_core::github::pr_merge_poller::PrMergePoller;
use takuto_core::jira::poller::JiraPoller;
use takuto_core::workflow::engine::WorkflowEngine;
use takuto_web::server::build_router;
use takuto_web::state::{
    AppState, AuthState, ConfigState, EditorState, EngineState, RunCommandState,
};

use super::{Bootstrap, EngineSetup, serve};
use crate::cli::Cli;

/// Which poller (if any) the active ticketing configuration calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PollerChoice {
    Jira,
    GitHub,
    Idle,
}

/// Pure decision: Jira and GitHub each get their poller whenever that
/// ticketing system is configured; anything else stays idle.
///
/// `acli` authentication does **not** gate the Jira poller: `poll_once`
/// resolves each repo's per-user REST credential (with `acli` fallback) and
/// skips repos with no creds / empty filters, so starting the loop is safe
/// even when `acli` is not logged in. Gating on `acli_ok` here meant a
/// REST-only Jira user's background polling never started — the whole point of
/// the per-repo polling feature. The `_acli_ok` param is retained (callers
/// pass the boot probe) but no longer influences the choice.
pub(super) fn select_poller(ticketing: TicketingSystem, _acli_ok: bool) -> PollerChoice {
    match ticketing {
        TicketingSystem::Jira => PollerChoice::Jira,
        TicketingSystem::GitHub => PollerChoice::GitHub,
        TicketingSystem::None => PollerChoice::Idle,
    }
}

/// The pieces [`build_app_state`] needs. A plain carrier so the assembler keeps
/// a readable call site instead of a dozen positional arguments.
pub(super) struct AppStateParts {
    pub engine: Arc<WorkflowEngine>,
    pub polling_paused: Arc<AtomicBool>,
    pub system_status: SystemStatus,
    pub db: Option<Database>,
    pub git_auth_resolver: Option<Arc<GitAuthResolver>>,
    pub config: Arc<RwLock<Config>>,
    pub config_path: PathBuf,
    pub config_writer: Arc<ConfigWriter>,
    pub ticketing_system: TicketingSystem,
    pub jira_available: Arc<AtomicBool>,
    pub preflight_error: Option<String>,
    pub work_item_flow_defaults: Arc<Vec<UserFlow>>,
}

/// Assemble the composed [`AppState`] from its parts. Pure (no I/O); the fresh
/// in-memory maps (editor/run-command scanners + bundle registries) start
/// empty, exactly as a cold boot expects.
pub(super) fn build_app_state(p: AppStateParts) -> AppState {
    AppState::new(
        EngineState {
            engine: p.engine,
            polling_paused: p.polling_paused,
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            system_status: Arc::new(RwLock::new(p.system_status)),
        },
        AuthState {
            db: p.db,
            gh_client: Arc::new(takuto_core::auth::RealGhClient::new()),
            git_auth_resolver: p.git_auth_resolver,
            jira_http: Arc::new(takuto_core::jira::RealJiraHttp::new()),
        },
        ConfigState {
            config: p.config,
            config_path: p.config_path,
            config_writer: Some(p.config_writer),
            ticketing_system: p.ticketing_system,
            jira_available: p.jira_available,
            preflight_error: p.preflight_error,
            work_item_flow_defaults: p.work_item_flow_defaults,
        },
        EditorState {
            editor_scanners: Arc::new(RwLock::new(std::collections::HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(std::collections::HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(std::collections::HashMap::new())),
            // Hold per-workflow bundles alive for the lifetime of the detached
            // editor containers. Cleared by the matching close handlers and by
            // workflow teardown.
            editor_bundles: Arc::new(RwLock::new(std::collections::HashMap::new())),
            path_token_registry: takuto_web::session_registry::PathTokenRegistry::new(),
        },
        RunCommandState {
            run_commands: Arc::new(RwLock::new(std::collections::HashMap::new())),
            // Hold per-workflow bundles alive for the lifetime of the detached
            // run-command containers. Cleared by the matching stop handlers and
            // by workflow teardown.
            run_command_bundles: Arc::new(RwLock::new(std::collections::HashMap::new())),
            spawner: Arc::new(takuto_web::container_spawner::DockerSpawner),
        },
    )
}

pub(super) async fn run(
    cli: &Cli,
    boot: Bootstrap,
    db: Option<Database>,
    eng: EngineSetup,
) -> Result<(), Box<dyn std::error::Error>> {
    let Bootstrap {
        config,
        github_app_mgr,
        ticketing_system,
        mut system_status,
        jira_available,
        acli_ok,
        work_item_flow_defaults,
        // Consumed by earlier phases; not needed past bootstrap.
        max_concurrent: _,
        workflows_dir: _,
    } = boot;
    let EngineSetup {
        engine,
        git_auth_resolver,
        resolved_poller_owner,
    } = eng;

    let cancel_token = CancellationToken::new();

    // Start the centralized GitHub App token file writer so worker containers
    // always read a fresh token from the shared volume instead of relying on a
    // frozen GH_TOKEN env var injected at `docker run` time.
    if let Some(ref mgr) = github_app_mgr {
        let cwd = config.read().await.git.repo_path.clone();
        mgr.start_token_file_writer(PathBuf::from(&cwd), cancel_token.clone());
    }

    // Start the background workflow definitions directory watcher.
    engine.start_definitions_watcher(cancel_token.clone());

    // The global master switch starts ON (not paused); per-repository
    // `auto_polling` (default off) gates which repos are actually polled, so a
    // fresh deploy never auto-starts workflows. Runtime Pause/Resume
    // (`/api/polling/{pause,resume}`) flips this flag.
    let polling_paused = Arc::new(AtomicBool::new(false));
    let poller = JiraPoller::new(
        config.clone(),
        engine.clone(),
        cancel_token.clone(),
        polling_paused.clone(),
        resolved_poller_owner.clone(),
        // Prefer the resolved owner's per-user Jira credential (REST) when one
        // is configured; the factory falls back to the global `acli` client
        // otherwise.
        Arc::new(takuto_core::jira::DbBackedJiraSourceFactory::new(
            db.clone(),
            resolved_poller_owner.clone(),
        )),
    );

    let polling_paused_for_gh = polling_paused.clone();
    let cancel_token_for_gh = cancel_token.clone();
    // Back-compat (one release): legacy `TAKUTO_PREFLIGHT_ERROR` env var set
    // by `docker/entrypoint.sh`. The UI now reads
    // `GET /api/onboarding/status`; this fallback is retained so the
    // dashboard can still render a banner when the DB is unavailable.
    let preflight_error = std::env::var("TAKUTO_PREFLIGHT_ERROR")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(ref err) = preflight_error {
        tracing::warn!(
            error = %err,
            "TAKUTO_PREFLIGHT_ERROR is set (legacy env var, deprecated — \
             dashboard should read /api/onboarding/status instead)"
        );
    }

    // Now that the DB has been opened (or not), set `per_user_required`. This
    // is the only mutation we make to the system_status after collection.
    system_status.per_user_required = db.is_some();

    // Recompute SystemStatus with the DB in scope so master-key
    // warnings (master_key_unavailable, secret_key_world_readable) join
    // the existing config-derived warnings. We do this AFTER the
    // `per_user_required` patch so the boot snapshot is complete.
    if let Some(ref db) = db {
        let refreshed = {
            let cfg_snapshot = config.read().await;
            docker_hooks::collect_system_status_with_db(&cfg_snapshot, Some(db))
        };
        // Preserve `per_user_required` (already computed) and merge the
        // refreshed warnings + provider/github/ticketing struct.
        let prior_per_user_required = system_status.per_user_required;
        system_status = refreshed;
        system_status.per_user_required = prior_per_user_required;
        // Probe the config directory for write-ability so a silently-failed
        // `chown /etc/takuto` in entrypoint.sh surfaces as a dashboard banner
        // rather than a confused "saves don't persist" UX. The probe is
        // non-destructive (tempfile created + dropped). Emits at critical
        // severity so it survives `apply_user_warning_filter`.
        if let Some(w) = docker_hooks::check_config_dir_writable(&cli.config) {
            tracing::warn!(
                code = %w.code,
                severity = %w.severity,
                "Config dir boot warning: {}",
                w.message
            );
            system_status.warnings.push(w);
        }
        for w in &system_status.warnings {
            if w.severity == "critical" {
                tracing::warn!(
                    code = %w.code,
                    severity = %w.severity,
                    "Boot warning: {}",
                    w.message
                );
            }
        }
    }

    // Config writer — only available when the config file path is known.
    let config_path = std::fs::canonicalize(&cli.config).unwrap_or_else(|_| cli.config.clone());
    let config_writer = Arc::new(ConfigWriter::new(config_path.clone()));

    let app_state = build_app_state(AppStateParts {
        engine: engine.clone(),
        polling_paused,
        system_status,
        db: db.clone(),
        git_auth_resolver,
        config: config.clone(),
        config_path: config_path.clone(),
        config_writer: config_writer.clone(),
        ticketing_system,
        jira_available,
        preflight_error,
        work_item_flow_defaults,
    });
    let app = build_router(app_state);

    let web_host = config.read().await.web.host.clone();
    let web_port = config.read().await.web.port;
    let bind_addr = format!("{web_host}:{web_port}");

    info!(bind = %bind_addr, "Starting web server");

    let shutdown_engine = engine.clone();
    let snapshot_engine = engine.clone();
    let snapshot_cancel = cancel_token.clone();

    // Periodic workflow snapshot syncer (every minute)
    let snapshot_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_mins(1));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = snapshot_engine.sync_workflow_snapshot().await {
                        tracing::warn!(error = %e, "Failed to sync workflow snapshot (continuing)");
                    }
                }
                _ = snapshot_cancel.cancelled() => {
                    break;
                }
            }
        }
    });

    // Hourly log-line retention purge. Skipped entirely when no DB is
    // attached (legacy single-user mode).
    // Re-reads `work_item_log_retention_days` from config every
    // tick so operators can adjust at runtime via the config
    // watcher without a restart. `0` days disables the purge —
    // run_once is a clean no-op in that case.
    let retention_db = db.clone();
    let retention_config = config.clone();
    let retention_cancel = cancel_token.clone();
    let _retention_task = tokio::spawn(async move {
        let Some(database) = retention_db else { return };
        let mut interval = tokio::time::interval(std::time::Duration::from_hours(1));
        // Skip the immediate first tick — restarts shouldn't
        // unconditionally hammer the DB before steady state.
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let retention_days = {
                        retention_config.read().await.general.work_item_log_retention_days
                    };
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_millis() as i64)
                        .unwrap_or(0);
                    takuto_core::db::log_retention::run_once(
                        &database,
                        now_ms,
                        retention_days,
                    )
                    .await;
                }
                _ = retention_cancel.cancelled() => break,
            }
        }
    });

    let pr_merge_poller = PrMergePoller::new(config.clone(), engine.clone(), cancel_token.clone());

    // Config file watcher — polls for external edits to config.toml and
    // hot-swaps the in-memory config when a valid change is detected.
    let config_watcher = ConfigWatcher::new(
        config_path,
        config.clone(),
        config_writer.last_write_epoch_ms().clone(),
        cancel_token.clone(),
    );
    let config_watcher_task = tokio::spawn(async move { config_watcher.run().await });

    // Re-install the [dev] block into the dev_mock module after every config reload.
    // The ConfigWatcher swaps the in-memory `Config` through the shared `Arc<RwLock>`;
    // we poll it at the same cadence and re-snapshot the dev knobs so flipping
    // `[dev] mock_agent` in `config.toml` (or via `POST /api/config/reload`) takes
    // effect without a restart.
    let dev_mock_config = config.clone();
    let dev_mock_cancel = cancel_token.clone();
    let _dev_mock_reload_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            takuto_core::config_watcher::DEFAULT_POLL_INTERVAL_SECS,
        ));
        // Skip the immediate first tick — initial install already happened in main().
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = dev_mock_cancel.cancelled() => break,
            }
            let dev = { dev_mock_config.read().await.dev.clone() };
            takuto_core::dev_mock::install_dev_config(&dev);
        }
    });

    // Build the chosen poller future (each owns its poller). Jira and GitHub
    // each get their poller whenever that ticketing system is configured (acli
    // auth no longer gates Jira — per-user REST is resolved at poll time);
    // otherwise idle forever.
    let poller_fut: Pin<Box<dyn Future<Output = ()> + Send>> =
        match select_poller(ticketing_system, acli_ok) {
            PollerChoice::Jira => Box::pin(async move { poller.run().await }),
            PollerChoice::GitHub => {
                let gh_poller = GitHubPoller::new(
                    config.clone(),
                    engine.clone(),
                    cancel_token_for_gh,
                    polling_paused_for_gh,
                    resolved_poller_owner.clone(),
                );
                Box::pin(async move { gh_poller.run().await })
            }
            PollerChoice::Idle => Box::pin(std::future::pending::<()>()),
        };
    let pr_merge_fut: Pin<Box<dyn Future<Output = ()> + Send>> =
        Box::pin(async move { pr_merge_poller.run().await });

    serve::run_until_shutdown(
        app,
        bind_addr,
        cancel_token,
        shutdown_engine,
        poller_fut,
        pr_merge_fut,
        snapshot_task,
        config_watcher_task,
    )
    .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use takuto_core::actions::dry_run::DryRunActions;

    #[test]
    fn select_poller_rules() {
        assert_eq!(
            select_poller(TicketingSystem::Jira, true),
            PollerChoice::Jira
        );
        // Jira polls regardless of acli auth — a REST-only user (acli not
        // logged in) must still get the background poller (per-user REST is
        // resolved at poll time, with acli fallback).
        assert_eq!(
            select_poller(TicketingSystem::Jira, false),
            PollerChoice::Jira
        );
        assert_eq!(
            select_poller(TicketingSystem::GitHub, false),
            PollerChoice::GitHub
        );
        assert_eq!(
            select_poller(TicketingSystem::None, true),
            PollerChoice::Idle
        );
    }

    /// `build_app_state` assembles a router-ready state from a temp DB + engine,
    /// exercising every substate constructor without binding a socket.
    #[tokio::test]
    async fn build_app_state_assembles_a_routable_state() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path(), true).expect("open temp db");
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn takuto_core::actions::traits::ExternalActions> =
            Arc::new(DryRunActions::new("origin".to_string(), None));
        let jira_available = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(WorkflowEngine::new_with_db(
            config.clone(),
            actions,
            1,
            jira_available.clone(),
            TicketingSystem::None,
            dir.path().to_path_buf(),
            Some(db.clone()),
        ));
        let config_path = dir.path().join("config.toml");

        let state = build_app_state(AppStateParts {
            engine,
            polling_paused: Arc::new(AtomicBool::new(true)),
            system_status: SystemStatus::default(),
            db: Some(db),
            git_auth_resolver: None,
            config,
            config_path: config_path.clone(),
            config_writer: Arc::new(ConfigWriter::new(config_path)),
            ticketing_system: TicketingSystem::None,
            jira_available,
            preflight_error: None,
            work_item_flow_defaults: Arc::new(Vec::new()),
        });

        // The state composes into a working router (exercises the assembly).
        let _router = build_router(state);
    }
}
