// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! The default (no-subcommand) path: boot and run the Takuto web server.
//!
//! `run_server` is intentionally thin — it sequences the four bootstrap phases
//! and hands the values produced by each one forward to the next. The phases
//! are ordered and order-sensitive (DB opens before the engine, which is built
//! before the pollers; the boot [`SystemStatus`] is collected early and
//! recomputed once the DB is in scope), so this split preserves that exact
//! sequence — it only moves the code into cohesive modules.

mod bootstrap;
mod database;
mod engine;
mod poller_owner;
mod runtime;
mod serve;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;

use takuto_core::actions::traits::ExternalActions;
use takuto_core::config::{Config, TicketingSystem};
use takuto_core::db::user_work_item_flows::UserFlow;
use takuto_core::docker_hooks::SystemStatus;
use takuto_core::github::auth_resolver::GitAuthResolver;
use takuto_core::github_app::GitHubAppTokenManager;
use takuto_core::workflow::engine::WorkflowEngine;

use crate::cli::Cli;

/// Everything the [`bootstrap::init`] phase produces and later phases consume.
/// Pure data — no logic. `Arc` fields are cloned by later phases that need a
/// shared handle (`config`, `github_app_mgr`, `work_item_flow_defaults`).
pub(crate) struct Bootstrap {
    pub config: Arc<RwLock<Config>>,
    pub actions: Arc<dyn ExternalActions>,
    /// GitHub App token manager (when the App is configured) — reused for both
    /// the auth resolver and the background token-file writer.
    pub github_app_mgr: Option<Arc<GitHubAppTokenManager>>,
    pub ticketing_system: TicketingSystem,
    /// Boot status snapshot; recomputed with the DB in [`runtime::run`].
    pub system_status: SystemStatus,
    pub jira_available: Arc<AtomicBool>,
    pub acli_ok: bool,
    pub max_concurrent: usize,
    pub workflows_dir: PathBuf,
    pub work_item_flow_defaults: Arc<Vec<UserFlow>>,
}

/// Output of the [`engine::build`] phase: the engine plus the auth resolver and
/// resolved poller-owner that [`runtime::run`] needs for `AppState` and pollers.
pub(crate) struct EngineSetup {
    pub engine: Arc<WorkflowEngine>,
    pub git_auth_resolver: Option<Arc<GitAuthResolver>>,
    pub resolved_poller_owner: Option<String>,
}

pub async fn run_server(cli: &Cli) -> Result<(), Box<dyn std::error::Error>> {
    let boot = bootstrap::init(cli).await?;
    // Install the agent + Atlassian CLIs at runtime (not baked into the image)
    // into the shared tools volume, in the background, so the server keeps
    // serving and the dashboard can show "Installing dependencies" progress.
    takuto_web::dependency_status::spawn_install(boot.config.clone());
    let data_dir = takuto_core::workflow::snapshot::resolve_data_dir();
    let db = database::open_and_reconcile(&boot, data_dir.as_deref()).await;
    let eng = engine::build(&boot, &db).await;
    runtime::run(cli, boot, db, eng).await
}
