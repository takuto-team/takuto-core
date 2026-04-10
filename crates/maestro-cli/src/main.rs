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
use maestro_core::config::Config;
use maestro_core::docker_hooks;
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

    match docker_hooks::preflight(&config) {
        Err(e) => {
            eprintln!("Preflight failed: {e}");
            ExitCode::FAILURE
        }
        Ok(result) => {
            if !result.acli_ok {
                eprintln!(
                    "Preflight OK (warning: acli not authenticated — Jira integration disabled)."
                );
            } else {
                eprintln!("Preflight OK.");
            }
            ExitCode::SUCCESS
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::DockerHooks { phase }) => run_docker_hooks(&cli.config, *phase),
        Some(Commands::Preflight) => run_preflight(&cli.config),
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

    let (repo_path, git_remote, dry_mode, acli_extras) = {
        let c = config.read().await;
        (
            PathBuf::from(&c.git.repo_path),
            c.git.remote.clone(),
            c.general.dry_mode,
            c.jira.acli_extra_argv_prefixes(),
        )
    };
    let actions: Arc<dyn ExternalActions> = if dry_mode {
        info!("Running in DRY MODE — no external writes");
        Arc::new(DryRunActions::new(repo_path, git_remote, acli_extras))
    } else {
        Arc::new(RealActions::new(repo_path, git_remote, acli_extras))
    };

    let acli_ok = docker_hooks::check_acli_auth();
    let jira_available = Arc::new(AtomicBool::new(acli_ok));
    if !acli_ok {
        info!("Atlassian CLI (acli) is not authenticated — Jira integration disabled. \
               No auto-polling; workflows skip Jira operations; manual description entry only.");
    }

    let max_concurrent = config.read().await.general.max_concurrent_workflows as usize;
    let engine = Arc::new(WorkflowEngine::new(
        config.clone(),
        actions.clone(),
        max_concurrent,
        jira_available.clone(),
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
    let start_polling_paused = config.read().await.general.pause_jira_polling_on_startup;
    let polling_paused = Arc::new(AtomicBool::new(start_polling_paused));
    if start_polling_paused {
        info!(
            "Jira polling starts paused ([general] pause_jira_polling_on_startup = true); use the dashboard Resume polling control or POST /api/polling/resume to pick up new To Do tickets"
        );
    }
    let poller = JiraPoller::new(
        config.clone(),
        engine.clone(),
        cancel_token.clone(),
        polling_paused.clone(),
    );

    let app_state = AppState {
        engine: engine.clone(),
        config: config.clone(),
        polling_paused,
        jira_available: jira_available.clone(),
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

    tokio::select! {
        _ = async {
            if acli_ok {
                poller.run().await;
            } else {
                // No Jira integration — poller stays idle forever.
                std::future::pending::<()>().await;
            }
        } => {
            info!("Jira poller stopped");
        }
        _ = snapshot_task => {
            info!("Workflow snapshot syncer stopped");
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
