// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use clap::{Parser, Subcommand, ValueEnum};
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

use maestro_core::actions::dry_run::DryRunActions;
use maestro_core::actions::real::RealActions;
use maestro_core::actions::traits::ExternalActions;
use maestro_core::config::{Config, TicketingSystem};
use maestro_core::config_watcher::ConfigWatcher;
use maestro_core::config_writer::ConfigWriter;
use maestro_core::docker_hooks;
use maestro_core::github::poller::GitHubPoller;
use maestro_core::github::pr_merge_poller::PrMergePoller;
use maestro_core::jira::poller::JiraPoller;
use maestro_core::workflow::engine::WorkflowEngine;
use maestro_web::server::build_router;
use maestro_web::state::AppState;

#[derive(Copy, Clone, Debug, ValueEnum)]
enum DockerHookPhase {
    Build,
    Startup,
}

impl DockerHookPhase {
    fn label(self) -> &'static str {
        match self {
            DockerHookPhase::Build => "build",
            DockerHookPhase::Startup => "startup",
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Run shell hooks from [docker] in config (used by Dockerfile and entrypoint).
    DockerHooks {
        #[arg(value_enum)]
        phase: DockerHookPhase,
    },
    /// Verify GitHub, Atlassian, and provider-specific auth before starting the server.
    Preflight,
    /// Generate a GitHub App installation token and print it to stdout.
    /// Used by the setup script to clone the repository when no personal gh auth is present.
    GithubAppToken,
}

#[derive(Parser)]
#[command(
    name = "maestro",
    about = "Automated Jira ticket handler using Claude Code or Cursor Agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the configuration file (also reads MAESTRO_CONFIG env var)
    #[arg(
        short,
        long,
        default_value = "config.toml",
        env = "MAESTRO_CONFIG",
        global = true
    )]
    config: PathBuf,

    /// Enable dry-run mode (overrides config file); only applies to the default server command
    #[arg(long, global = true)]
    dry_run: bool,
}

fn run_docker_hooks(config_path: &std::path::Path, phase: DockerHookPhase) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let cwd = std::path::PathBuf::from(&config.git.repo_path);
    let commands = match phase {
        DockerHookPhase::Build => config.docker.build_commands.as_slice(),
        DockerHookPhase::Startup => config.docker.compose_up_commands.as_slice(),
    };

    if let Err(e) = docker_hooks::run_hook_commands(commands, &cwd, phase.label()) {
        eprintln!("{e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

fn run_preflight(config_path: &std::path::Path) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let ticketing = config.general.ticketing_system;
    match docker_hooks::preflight(&config) {
        Err(e) => {
            eprintln!("Preflight failed: {e}");
            ExitCode::FAILURE
        }
        Ok(result) => {
            match ticketing {
                TicketingSystem::Jira => {
                    if !result.acli_ok {
                        eprintln!(
                            "Preflight OK (warning: acli not authenticated — Jira integration disabled, falling back to manual mode)."
                        );
                    } else {
                        eprintln!("Preflight OK (ticketing_system = jira, acli authenticated).");
                    }
                }
                TicketingSystem::GitHub => {
                    eprintln!(
                        "Preflight OK (ticketing_system = github — polling GitHub issues, no Atlassian auth required)."
                    );
                }
                TicketingSystem::None => {
                    eprintln!(
                        "Preflight OK (ticketing_system = none — manual description entry only)."
                    );
                }
            }
            ExitCode::SUCCESS
        }
    }
}

async fn run_github_app_token(config_path: &std::path::Path) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let mgr = match maestro_core::github_app::try_create_token_manager(&config.github) {
        Some(mgr) => mgr,
        None => {
            eprintln!(
                "GitHub App not configured — set [github] app_id, app_installation_id, and app_private_key/app_private_key_path in config.toml."
            );
            return ExitCode::FAILURE;
        }
    };

    // Use the config file's directory as cwd for the curl invocation; fall back to /tmp.
    let cwd = config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    match mgr.get_installation_token(&cwd).await {
        Ok(token) => {
            println!("{token}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Failed to get GitHub App installation token: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::DockerHooks { phase }) => run_docker_hooks(&cli.config, *phase),
        Some(Commands::Preflight) => run_preflight(&cli.config),
        Some(Commands::GithubAppToken) => {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(run_github_app_token(&cli.config)),
                Err(e) => {
                    eprintln!("Failed to start async runtime: {e}");
                    ExitCode::FAILURE
                }
            }
        }
        None => match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => match rt.block_on(run_server(&cli)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("Maestro error: {e}");
                    ExitCode::FAILURE
                }
            },
            Err(e) => {
                eprintln!("Failed to start async runtime: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

async fn run_server(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let mut config = if cli.config.exists() {
        Config::load(&cli.config)?
    } else {
        Config::default()
    };

    if cli.dry_run {
        config.general.dry_mode = true;
    }

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive(config.general.log_level.parse()?),
        )
        .with_target(true)
        .init();

    if !cli.config.exists() {
        info!(
            path = %cli.config.display(),
            "Config file not found, using defaults"
        );
    }

    maestro_core::license::init_license_tier();

    // Resolve active workspace from the persistent data dir (survives rebuilds).
    // Ignores git.repo_path from config.toml — workspace selection is stored separately.
    if let Some(active_path) = maestro_core::workflow::snapshot::resolve_active_repo_path() {
        config.git.repo_path = active_path;
    }

    info!(dry_mode = config.general.dry_mode, "Maestro starting");

    let config = Arc::new(RwLock::new(config));

    {
        let c = config.read().await;
        if c.web.dashboard_auth_enabled() {
            info!(
                user = %c.web.dashboard_username.trim(),
                "Dashboard auth ON — open /login.html to sign in; use the same hostname always (localhost vs 127.0.0.1 are different cookie sites)"
            );
        } else {
            info!(
                "Dashboard auth OFF — set non-empty [web] dashboard_username and dashboard_password (or use the Configuration page) to require login"
            );
        }
    }

    let (repo_path, git_remote, dry_mode, github_app_mgr) = {
        let c = config.read().await;
        let mgr = maestro_core::github_app::try_create_token_manager(&c.github);
        (
            PathBuf::from(&c.git.repo_path),
            c.git.remote.clone(),
            c.general.dry_mode,
            mgr,
        )
    };

    let actions: Arc<dyn ExternalActions> = if dry_mode {
        info!("Running in DRY MODE — no external writes");
        Arc::new(DryRunActions::new(repo_path, git_remote, github_app_mgr))
    } else {
        // Pass the live config Arc so RealActions always reads the current
        // repo_path — a post-clone update takes effect without restart.
        Arc::new(RealActions::new(config.clone(), git_remote, github_app_mgr))
    };

    let ticketing_system = config.read().await.general.ticketing_system;

    let acli_ok = if ticketing_system == TicketingSystem::Jira {
        docker_hooks::check_acli_auth()
    } else {
        false
    };
    let jira_available = Arc::new(AtomicBool::new(acli_ok));
    if ticketing_system == TicketingSystem::Jira && !acli_ok {
        info!(
            "Atlassian CLI (acli) is not authenticated — Jira integration disabled. \
               No auto-polling; workflows skip Jira operations; manual description entry only."
        );
    }

    let max_concurrent = config.read().await.general.max_concurrent_workflows as usize;
    let workflows_dir = {
        let c = config.read().await;
        let config_file_dir = cli
            .config
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        maestro_core::config::resolve_config_relative_path(
            config_file_dir,
            &c.general.workflow_definitions_dir,
        )
    };
    let engine = Arc::new(WorkflowEngine::new(
        config.clone(),
        actions.clone(),
        max_concurrent,
        jira_available.clone(),
        ticketing_system,
        workflows_dir,
    ));

    match engine.restore_persisted_workflows().await {
        Ok(n) if n > 0 => {
            info!(
                count = n,
                "Restored workflow snapshot from previous run (includes Done rows left idle for dashboard actions)"
            );
        }
        Ok(_) => {}
        Err(e) => {
            tracing::warn!(error = %e, "Failed to restore workflow snapshot (continuing without restore)");
        }
    }

    let cancel_token = CancellationToken::new();

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
    );

    let polling_paused_for_gh = polling_paused.clone();
    let cancel_token_for_gh = cancel_token.clone();
    let preflight_error = std::env::var("MAESTRO_PREFLIGHT_ERROR")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(ref err) = preflight_error {
        tracing::warn!(error = %err, "Server starting in degraded mode (preflight failed)");
    }

    // Config writer — only available when the config file path is known.
    let config_path = std::fs::canonicalize(&cli.config).unwrap_or_else(|_| cli.config.clone());
    let config_writer = Arc::new(ConfigWriter::new(config_path.clone()));

    let app_state = AppState {
        engine: engine.clone(),
        config: config.clone(),
        polling_paused,
        jira_available: jira_available.clone(),
        ticketing_system,
        editor_scanners: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        dynamic_forwards: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        terminal_ports: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        run_commands: std::sync::Arc::new(tokio::sync::RwLock::new(
            std::collections::HashMap::new(),
        )),
        preflight_error,
        config_path: config_path.clone(),
        config_writer: Some(config_writer.clone()),
        clone_in_progress: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        path_token_registry: maestro_web::session_registry::PathTokenRegistry::new(),
    };
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
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
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
                let mut sigterm = signal(SignalKind::terminate())
                    .expect("failed to install SIGTERM handler");
                tokio::select! {
                    _ = ctrl_c => {
                        info!("Received SIGINT, initiating graceful shutdown");
                    }
                    _ = sigterm.recv() => {
                        info!("Received SIGTERM, initiating graceful shutdown");
                    }
                }
            }
            #[cfg(not(unix))]
            {
                ctrl_c.await.expect("failed to install Ctrl+C handler");
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
