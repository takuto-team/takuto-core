use std::path::PathBuf;
use std::sync::Arc;

use clap::Parser;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use tracing_subscriber::EnvFilter;

use maestro_core::actions::dry_run::DryRunActions;
use maestro_core::actions::real::RealActions;
use maestro_core::actions::traits::ExternalActions;
use maestro_core::config::Config;
use maestro_core::jira::poller::JiraPoller;
use maestro_core::workflow::engine::WorkflowEngine;
use maestro_web::server::build_router;
use maestro_web::state::AppState;

#[derive(Parser)]
#[command(name = "maestro", about = "Automated Jira ticket handler using Claude Code")]
struct Cli {
    /// Path to the configuration file (also reads MAESTRO_CONFIG env var)
    #[arg(short, long, default_value = "config.toml", env = "MAESTRO_CONFIG")]
    config: PathBuf,

    /// Enable dry-run mode (overrides config file)
    #[arg(long)]
    dry_run: bool,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Load configuration
    let mut config = if cli.config.exists() {
        Config::load(&cli.config)?
    } else {
        Config::default()
    };

    if cli.dry_run {
        config.general.dry_mode = true;
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(config.general.log_level.parse()?),
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

    // Select actions implementation based on dry mode
    let repo_path = PathBuf::from(&config.read().await.git.repo_path);
    let actions: Arc<dyn ExternalActions> = if config.read().await.general.dry_mode {
        info!("Running in DRY MODE — no external writes");
        Arc::new(DryRunActions::new(repo_path))
    } else {
        Arc::new(RealActions::new(repo_path))
    };

    // Create workflow engine
    let engine = Arc::new(WorkflowEngine::new(config.clone(), actions.clone()));

    // Create global cancellation token for graceful shutdown
    let cancel_token = CancellationToken::new();
    let poller = JiraPoller::new(config.clone(), engine.clone(), cancel_token.clone());

    // Build web server
    let app_state = AppState {
        engine: engine.clone(),
        config: config.clone(),
    };
    let app = build_router(app_state);

    let web_host = config.read().await.web.host.clone();
    let web_port = config.read().await.web.port;
    let bind_addr = format!("{web_host}:{web_port}");

    info!(bind = %bind_addr, "Starting web server");

    // Run poller, web server, and shutdown handler concurrently
    let shutdown_token = cancel_token.clone();
    let shutdown_engine = engine.clone();

    tokio::select! {
        _ = poller.run() => {
            info!("Jira poller stopped");
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
            // Wait for SIGTERM or SIGINT
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

            // 1. Cancel the poller (stops accepting new tickets)
            shutdown_token.cancel();

            // 2. Stop all running workflows (cancel processes, unassign tickets, move to To Do)
            info!("Stopping all active workflows...");
            shutdown_engine.stop_all_workflows().await;

            // 3. Give cleanup tasks a grace period to complete Jira transitions
            info!("Waiting for cleanup tasks to complete...");
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;

            info!("Graceful shutdown complete");
        }
    }

    Ok(())
}
