// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 4: assemble `AppState`, spawn the background tasks (snapshot syncer,
//! log retention, config watcher, dev-mock reload), and run the
//! `tokio::select!` loop that drives the pollers, the HTTP server, and graceful
//! shutdown.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio_util::sync::CancellationToken;
use tracing::info;

use takuto_core::config::TicketingSystem;
use takuto_core::config_watcher::ConfigWatcher;
use takuto_core::config_writer::ConfigWriter;
use takuto_core::db::Database;
use takuto_core::docker_hooks;
use takuto_core::github::poller::GitHubPoller;
use takuto_core::github::pr_merge_poller::PrMergePoller;
use takuto_core::jira::poller::JiraPoller;
use takuto_web::server::build_router;
use takuto_web::state::{
    AppState, AuthState, ConfigState, EditorState, EngineState, RunCommandState,
};

use super::{Bootstrap, EngineSetup};
use crate::cli::Cli;

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
        actions: _,
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

    let start_polling_paused = !config.read().await.general.auto_polling;
    let polling_paused = Arc::new(AtomicBool::new(start_polling_paused));
    if start_polling_paused {
        info!(
            "Jira polling starts paused ([general] auto_polling = false); use the dashboard Resume polling control or POST /api/polling/resume to pick up new To Do tickets"
        );
    }
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

    let app_state = AppState::new(
        EngineState {
            engine: engine.clone(),
            polling_paused,
            clone_in_progress: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            system_status: std::sync::Arc::new(tokio::sync::RwLock::new(system_status)),
        },
        AuthState {
            // Clone so the retention task below gets its own handle
            // without depriving AuthState of one.
            db: db.clone(),
            gh_client: std::sync::Arc::new(takuto_core::auth::RealGhClient::new()),
            git_auth_resolver,
            jira_http: std::sync::Arc::new(takuto_core::jira::RealJiraHttp::new()),
        },
        ConfigState {
            config: config.clone(),
            config_path: config_path.clone(),
            config_writer: Some(config_writer.clone()),
            ticketing_system,
            jira_available: jira_available.clone(),
            preflight_error,
            work_item_flow_defaults: work_item_flow_defaults.clone(),
        },
        EditorState {
            editor_scanners: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            dynamic_forwards: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            terminal_ports: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            // Hold per-workflow bundles alive for the lifetime of the
            // detached editor containers. Cleared by the matching
            // close handlers and by workflow teardown.
            editor_bundles: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            path_token_registry: takuto_web::session_registry::PathTokenRegistry::new(),
        },
        RunCommandState {
            run_commands: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            // Hold per-workflow bundles alive for the lifetime of the
            // detached run-command containers. Cleared by the matching
            // stop handlers and by workflow teardown.
            run_command_bundles: std::sync::Arc::new(tokio::sync::RwLock::new(
                std::collections::HashMap::new(),
            )),
            spawner: std::sync::Arc::new(takuto_web::container_spawner::DockerSpawner),
        },
    );
    let app = build_router(app_state);

    let web_host = config.read().await.web.host.clone();
    let web_port = config.read().await.web.port;
    let bind_addr = format!("{web_host}:{web_port}");

    info!(bind = %bind_addr, "Starting web server");

    let shutdown_token = cancel_token.clone();
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

    tokio::select! {
        _ = async {
            match ticketing_system {
                TicketingSystem::Jira if acli_ok => {
                    poller.run().await;
                }
                TicketingSystem::GitHub => {
                    let gh_poller = GitHubPoller::new(
                        config.clone(),
                        engine.clone(),
                        cancel_token_for_gh,
                        polling_paused_for_gh,
                        resolved_poller_owner.clone(),
                    );
                    gh_poller.run().await;
                }
                _ => {
                    // No ticketing integration or Jira not authenticated — poller stays idle forever.
                    std::future::pending::<()>().await;
                }
            }
        } => {
            info!("Poller stopped");
        }
        _ = pr_merge_poller.run() => {
            info!("PR merge status poller stopped");
        }
        _ = snapshot_task => {
            info!("Workflow snapshot syncer stopped");
        }
        _ = config_watcher_task => {
            info!("Config file watcher stopped");
        }
        result = async {
            let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
            axum::serve(listener, app)
                .with_graceful_shutdown(cancel_token.cancelled_owned())
                .await
        } => {
            if let Err(e) = result {
                tracing::error!(error = %e, "Web server error");
            }
        }
        _ = async {
            let ctrl_c = tokio::signal::ctrl_c();
            #[cfg(unix)]
            {
                use tokio::signal::unix::{SignalKind, signal};
                match signal(SignalKind::terminate()) {
                    Ok(mut sigterm) => {
                        tokio::select! {
                            _ = ctrl_c => {
                                info!("Received SIGINT, initiating graceful shutdown");
                            }
                            _ = sigterm.recv() => {
                                info!("Received SIGTERM, initiating graceful shutdown");
                            }
                        }
                    }
                    Err(e) => {
                        // Degrade gracefully: keep running with only the Ctrl+C
                        // hook. SIGTERM-based shutdown is unavailable, but the
                        // process still responds to SIGINT and cancellation.
                        tracing::error!(
                            error = %e,
                            "Failed to install SIGTERM handler; continuing with Ctrl+C only"
                        );
                        if let Err(e) = ctrl_c.await {
                            tracing::error!(
                                error = %e,
                                "Ctrl+C handler unavailable; running without signal-based shutdown"
                            );
                            std::future::pending::<()>().await;
                        }
                        info!("Received Ctrl+C, initiating graceful shutdown");
                    }
                }
            }
            #[cfg(not(unix))]
            {
                if let Err(e) = ctrl_c.await {
                    // No Ctrl+C hook available: park forever rather than triggering
                    // an immediate shutdown. The process still stops via cancellation.
                    tracing::error!(
                        error = %e,
                        "Ctrl+C handler unavailable; running without signal-based shutdown"
                    );
                    std::future::pending::<()>().await;
                }
                info!("Received Ctrl+C, initiating graceful shutdown");
            }
        } => {
            info!("Shutting down gracefully...");

            shutdown_token.cancel();

            info!("Persisting workflows and stopping drivers for resume after restart...");
            if let Err(e) = shutdown_engine.persist_interrupt_for_restart().await {
                tracing::warn!(error = %e, "Failed to write workflow snapshot; workflows may not resume cleanly");
            }

            info!("Waiting for cleanup tasks to complete...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            info!("Graceful shutdown complete");
        }
    }

    Ok(())
}
