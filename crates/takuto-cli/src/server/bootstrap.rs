// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 1: load config, initialise logging, build the external-actions
//! backend, collect the boot [`SystemStatus`], and derive the config-driven
//! values later phases need.

use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::AtomicBool;

use tokio::sync::RwLock;
use tracing::info;
use tracing_subscriber::EnvFilter;

use takuto_core::actions::dry_run::DryRunActions;
use takuto_core::actions::real::RealActions;
use takuto_core::actions::traits::ExternalActions;
use takuto_core::config::{Config, TicketingSystem};
use takuto_core::docker_hooks;

use super::Bootstrap;
use crate::cli::Cli;

/// Install the global JSON tracing subscriber exactly once per process.
///
/// `tracing_subscriber::*::init()` panics if a global subscriber is already
/// set, which makes any code path that reaches it impossible to exercise more
/// than once (e.g. parallel tests calling [`init`]). Guarding with [`Once`] +
/// `try_init` keeps the first install authoritative and turns later calls into
/// a no-op. The directive is parsed *before* the `Once` so a bad `log_level`
/// still surfaces as an error rather than being swallowed.
fn init_logging(log_level: &str) -> Result<(), Box<dyn std::error::Error>> {
    static LOGGING_INIT: Once = Once::new();
    let directive = log_level.parse()?;
    LOGGING_INIT.call_once(move || {
        let _ = tracing_subscriber::fmt()
            .json()
            .with_env_filter(EnvFilter::from_default_env().add_directive(directive))
            .with_target(true)
            .try_init();
    });
    Ok(())
}

pub(super) async fn init(cli: &Cli) -> Result<Bootstrap, Box<dyn std::error::Error>> {
    // Detect stale `[commands]` / `[[run_commands]]` keys BEFORE `tracing_subscriber::init`
    // so we can replay the warnings via tracing after the subscriber is up. Inline
    // tracing calls inside Config::load on the first invocation go to the no-op default
    // subscriber and are silently dropped — this two-step path is the workaround.
    let legacy_warnings = if cli.config.exists() {
        match std::fs::read_to_string(&cli.config) {
            Ok(content) => takuto_core::config::detect_legacy_command_keys(&content),
            Err(_) => Vec::new(),
        }
    } else {
        Vec::new()
    };

    let mut config = if cli.config.exists() {
        Config::load(&cli.config)?
    } else {
        Config::default()
    };

    if cli.dry_run {
        config.general.dry_mode = true;
    }

    init_logging(&config.general.log_level)?;

    // Replay legacy-key warnings now that the subscriber is initialised.
    for msg in &legacy_warnings {
        tracing::warn!("{msg}");
    }

    // Item polling enable/disable is driven by `[general] auto_polling` (and
    // the live Pause/Resume + Configuration → Item Polling toggle). It only
    // actually polls when a ticketing system is configured (Jira with acli, or
    // GitHub); with `ticketing_system = none` the poller stays idle regardless.
    if config.general.auto_polling {
        info!(
            "Item polling is enabled ([general] auto_polling = true) — active when a \
             ticketing system is configured. Disable it from Configuration → Item Polling."
        );
    } else {
        info!(
            "Item polling starts disabled ([general] auto_polling = false). Enable it \
             from Configuration → Item Polling or POST /api/polling/resume."
        );
    }

    if !cli.config.exists() {
        info!(
            path = %cli.config.display(),
            "Config file not found, using defaults"
        );
    }

    takuto_core::license::init_license_tier();

    // Resolve active workspace from the persistent data dir (survives rebuilds).
    // Ignores git.repo_path from config.toml — workspace selection is stored separately.
    if let Some(active_path) = takuto_core::workflow::snapshot::resolve_active_repo_path() {
        config.git.repo_path = active_path;
    }

    info!(dry_mode = config.general.dry_mode, "Takuto starting");

    // Install dev-mode flags so dev_mock::is_enabled_from_runtime() works before
    // any agent call. Off by default in production.
    takuto_core::dev_mock::install_dev_config(&config.dev);
    if takuto_core::dev_mock::is_enabled_from_runtime() {
        tracing::info!("[mock-agent] enabled");
    }

    let config = Arc::new(RwLock::new(config));

    let (git_remote, dry_mode, github_app_mgr) = {
        let c = config.read().await;
        let mgr = takuto_core::github_app::try_create_token_manager(&c.github);
        (c.git.remote.clone(), c.general.dry_mode, mgr)
    };

    let actions: Arc<dyn ExternalActions> = if dry_mode {
        info!("Running in DRY MODE — no external writes");
        Arc::new(DryRunActions::new(git_remote, github_app_mgr.clone()))
    } else {
        // Pass the live config Arc so RealActions always reads the current
        // repo_path — a post-clone update takes effect without restart.
        Arc::new(RealActions::new(
            config.clone(),
            git_remote,
            github_app_mgr.clone(),
        ))
    };

    let ticketing_system = config.read().await.general.ticketing_system;

    // Phase 0 (04_architecture.md §1): collect the structured SystemStatus
    // snapshot once at startup. This is the single source of truth the
    // dashboard reads from `GET /api/onboarding/status`. The standalone
    // `acli_ok` probe is replaced by reading `system_status.ticketing.acli_ok`
    // so we don't shell out twice.
    let system_status = {
        let cfg_snapshot = config.read().await;
        docker_hooks::collect_system_status(&cfg_snapshot)
    };
    for w in &system_status.warnings {
        match w.severity.as_str() {
            "critical" => tracing::warn!(
                code = %w.code,
                severity = %w.severity,
                "Boot warning (degraded mode): {}",
                w.message
            ),
            _ => tracing::info!(
                code = %w.code,
                severity = %w.severity,
                "Boot advisory: {}",
                w.message
            ),
        }
    }

    let acli_ok = system_status.ticketing.acli_ok;
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
        takuto_core::config::resolve_config_relative_path(
            config_file_dir,
            &c.general.workflow_definitions_dir,
        )
    };
    // Parse the default per-user work-item flows once at startup. Shared by
    // the startup seeding backfill and the web layer (seeding new users, the
    // "Re-seed from defaults" action).
    let work_item_flow_defaults = Arc::new(
        takuto_core::workflow::definitions::default_flows_from_dir(&workflows_dir),
    );

    Ok(Bootstrap {
        config,
        actions,
        github_app_mgr,
        ticketing_system,
        system_status,
        jira_available,
        acli_ok,
        max_concurrent,
        workflows_dir,
        work_item_flow_defaults,
    })
}
