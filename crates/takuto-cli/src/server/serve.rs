// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! The irreducible serve shell: bind the TCP listener, serve the router, and
//! race it against the pollers, background tasks, and OS shutdown signals.
//!
//! This is deliberately the *only* thing left in its own file — it performs
//! real syscalls (`TcpListener::bind`, `axum::serve`, signal handlers) that
//! can't be exercised by a unit test, so it is excluded from coverage while
//! everything that *decides* what to serve (poller selection, AppState
//! assembly) lives in testable functions in [`super::runtime`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::info;

use takuto_core::workflow::engine::WorkflowEngine;

#[allow(clippy::too_many_arguments)]
pub(super) async fn run_until_shutdown(
    app: axum::Router,
    bind_addr: String,
    cancel_token: CancellationToken,
    shutdown_engine: Arc<WorkflowEngine>,
    poller_fut: Pin<Box<dyn Future<Output = ()> + Send>>,
    pr_merge_fut: Pin<Box<dyn Future<Output = ()> + Send>>,
    snapshot_task: JoinHandle<()>,
    config_watcher_task: JoinHandle<()>,
) {
    let shutdown_token = cancel_token.clone();

    tokio::select! {
        _ = poller_fut => {
            info!("Poller stopped");
        }
        _ = pr_merge_fut => {
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
}
