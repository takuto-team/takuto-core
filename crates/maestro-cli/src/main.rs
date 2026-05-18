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
use maestro_core::db::Database;
use maestro_core::docker_hooks;
use maestro_core::github::poller::GitHubPoller;
use maestro_core::github::pr_merge_poller::PrMergePoller;
use maestro_core::jira::poller::JiraPoller;
use maestro_core::workflow::engine::WorkflowEngine;
use maestro_core::repo_reconcile;
use maestro_web::server::build_router;
use maestro_web::state::AppState;

/// Resolve the owner of poller-created workflows at startup.
///
/// Resolution order:
///   1. If `cfg_username` is provided AND the user exists AND is not suspended, return their id.
///   2. Otherwise, return the id of the lexicographically-first non-suspended admin.
///   3. Otherwise, return `None` (poller will skip `start_workflow` and log).
///
/// Warnings are logged when (1) is provided but the user is missing or suspended,
/// and when neither path resolves (the caller may log an additional summary).
async fn resolve_poller_owner(db: &Database, cfg_username: Option<&str>) -> Option<String> {
    let conn = db.conn().lock().await;
    if let Some(username) = cfg_username {
        match maestro_core::db::users::get_user_by_username(&conn, username) {
            Ok(Some(user)) if !user.suspended => {
                info!(
                    username = %user.username,
                    user_id = %user.id,
                    "Poller owner resolved from [general] poller_owner_username"
                );
                return Some(user.id);
            }
            Ok(Some(user)) => {
                tracing::warn!(
                    username = %username,
                    user_id = %user.id,
                    "Configured poller_owner_username is suspended; falling back to admin"
                );
            }
            Ok(None) => {
                tracing::warn!(
                    username = %username,
                    "Configured poller_owner_username not found; falling back to admin"
                );
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    username = %username,
                    "Lookup for poller_owner_username failed; falling back to admin"
                );
            }
        }
    }

    match maestro_core::db::users::list_admins(&conn) {
        Ok(admins) => admins.into_iter().next().map(|u| {
            info!(
                username = %u.username,
                user_id = %u.id,
                "Poller owner resolved to first non-suspended admin"
            );
            u.id
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to list admins for poller-owner resolution");
            None
        }
    }
}

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
    ///
    /// Phase 0 (04_architecture.md §1.2, §8): the subcommand normally exits `0`
    /// even when checks fail — the dashboard renders the degraded-mode banner
    /// from `GET /api/onboarding/status`. The `--strict` flag flips back to a
    /// hard-fail exit code for CI pipelines that want to gate on a clean boot.
    Preflight {
        /// Exit non-zero when any critical warning is present.
        #[arg(long)]
        strict: bool,
    },
    /// Generate a GitHub App installation token and print it to stdout.
    /// Used by the setup script to clone the repository when no personal gh auth is present.
    GithubAppToken,
    /// Master-key management. v1 ships reset-only (04_architecture.md §3.2 / A5).
    Keys {
        #[command(subcommand)]
        action: KeysAction,
    },
}

#[derive(Subcommand)]
enum KeysAction {
    /// Reset the deployment master key and every credential row.
    ///
    /// Lossy by design: every user must re-paste their credentials after a
    /// reset. Use only when the master key has been compromised or the
    /// keyfile is otherwise unrecoverable.
    ///
    /// Refuses to run while any workflow is `Running` per the persisted
    /// snapshot — graceful-shutdown the server first.
    ///
    /// Requires `--yes-i-am-sure` so no shell mishap can wipe production
    /// credentials.
    Reset {
        /// Explicit acknowledgement that every credential row will be deleted.
        #[arg(long = "yes-i-am-sure")]
        yes_i_am_sure: bool,
    },
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

fn run_preflight(config_path: &std::path::Path, strict: bool) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    // Phase 0 (04_architecture.md §1): collect_system_status never returns Err
    // — every former hard-error becomes a structured warning. The CLI prints
    // them to stderr (so docker logs surface them) and exits 0 unless the
    // operator passed --strict.
    let status = docker_hooks::collect_system_status(&config);

    for w in &status.warnings {
        eprintln!(
            "[maestro preflight] {sev}: {code} — {msg}",
            sev = w.severity,
            code = w.code,
            msg = w.message
        );
    }

    // Seed the GitHub App token file so it is available before the main server
    // starts. The server's background task handles subsequent refreshes. This
    // runs even on degraded boots — it's a no-op when the App is not
    // configured.
    if let Some(mgr) = maestro_core::github_app::try_create_token_manager(&config.github) {
        let cwd = config_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .filter(|p| p.exists())
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            match rt.block_on(mgr.get_installation_token(&cwd)) {
                Ok(token) => {
                    let token_path =
                        std::path::Path::new(maestro_core::github_app::TOKEN_FILE_PATH);
                    match maestro_core::github_app::write_token_file(token_path, &token) {
                        Ok(()) => {
                            eprintln!(
                                "[maestro preflight] GitHub App token written to {}",
                                token_path.display()
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "[maestro preflight] WARNING: failed to write token file: {e}"
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!(
                        "[maestro preflight] WARNING: failed to fetch GitHub App token: {e}"
                    );
                }
            }
        }
    }

    match config.general.ticketing_system {
        TicketingSystem::Jira => {
            if status.ticketing.acli_ok {
                eprintln!("[maestro preflight] ticketing_system = jira, acli authenticated.");
            } else {
                eprintln!(
                    "[maestro preflight] ticketing_system = jira but acli is not authenticated — Jira integration disabled, manual entry only."
                );
            }
        }
        TicketingSystem::GitHub => {
            eprintln!(
                "[maestro preflight] ticketing_system = github — polling GitHub issues, no Atlassian auth required."
            );
        }
        TicketingSystem::None => {
            eprintln!(
                "[maestro preflight] ticketing_system = none — manual description entry only."
            );
        }
    }

    if status.has_critical() {
        eprintln!(
            "[maestro preflight] {n} critical warning(s) present — the dashboard will boot in degraded mode (see GET /api/onboarding/status).",
            n = status
                .warnings
                .iter()
                .filter(|w| w.severity == "critical")
                .count()
        );
        if strict {
            eprintln!("[maestro preflight] --strict was passed — exiting FAILURE for CI.");
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!("[maestro preflight] OK.");
    }

    ExitCode::SUCCESS
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

/// `maestro keys reset` — Phase 2a (04_architecture.md §3.2, A5).
///
/// Clears every credential / audit / onboarding row and rewrites the master
/// keyfile under `${data_dir}/secret.key`. Lossy by design: every user must
/// re-paste their credentials afterwards.
///
/// Refusal cases (non-zero exit):
/// - `--yes-i-am-sure` not passed
/// - any workflow in a non-terminal, non-paused state per the snapshot file
///   (graceful-shutdown the server first; see QA T-KEYS-002)
/// - config load failure
/// - data_dir cannot be resolved
fn run_keys_reset(config_path: &std::path::Path, yes_i_am_sure: bool) -> ExitCode {
    if !yes_i_am_sure {
        eprintln!(
            "maestro keys reset: refuses to run without --yes-i-am-sure (this command is destructive)."
        );
        return ExitCode::FAILURE;
    }

    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let Some(data_dir) = maestro_core::workflow::snapshot::resolve_data_dir() else {
        eprintln!(
            "maestro keys reset: cannot resolve data directory (set MAESTRO_DATA_DIR, MAESTRO_HOME, or HOME)."
        );
        return ExitCode::FAILURE;
    };

    // Refuse when an in-flight workflow is present in any per-workspace
    // snapshot. The helper lives in maestro-core::workflow::snapshot so it
    // can be unit-tested without spawning the CLI.
    match maestro_core::workflow::snapshot::scan_in_flight_workflow_keys(&data_dir) {
        Ok(active) if !active.is_empty() => {
            eprintln!(
                "maestro keys reset: refuses to run — {} active workflow(s) present in the snapshot: {}.",
                active.len(),
                active.join(", ")
            );
            eprintln!("Stop or finish them, then retry.");
            return ExitCode::FAILURE;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("maestro keys reset: failed to read workflow snapshots: {e}");
            return ExitCode::FAILURE;
        }
    }

    // Open the DB so we can clear the credential / audit / onboarding tables.
    // Pass `allow_auto_generate_secret_key = false` here — we're about to
    // rewrite the keyfile ourselves; we don't want Database::open creating
    // a new one mid-reset.
    let db = match maestro_core::db::Database::open(&data_dir, false) {
        Ok(db) => db,
        Err(e) => {
            eprintln!(
                "maestro keys reset: failed to open database at {}: {e}",
                data_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = clear_credential_tables(&db) {
        eprintln!("maestro keys reset: failed to clear credential tables: {e}");
        return ExitCode::FAILURE;
    }

    // Regenerate (or wipe) the master keyfile.
    let keyfile = maestro_core::auth::master_key::keyfile_path(&data_dir);
    let env_set = std::env::var("MAESTRO_SECRET_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if env_set {
        // Operator manages the master key via env var; we only nuked the rows.
        // Don't touch the keyfile (if any) — it may not even exist. Warn so
        // the operator knows to rotate the env var manually.
        eprintln!(
            "maestro keys reset: credential rows cleared. MAESTRO_SECRET_KEY is set — rotate it manually before bringing the server back up."
        );
    } else {
        // Best-effort delete + regenerate.
        let _ = std::fs::remove_file(&keyfile);
        match maestro_core::auth::master_key::load_or_init_master_key(
            &data_dir,
            config.general.allow_auto_generate_secret_key,
        ) {
            Ok(_) => {
                eprintln!(
                    "maestro keys reset: credential rows cleared and {} regenerated.",
                    keyfile.display()
                );
            }
            Err(e) => {
                eprintln!(
                    "maestro keys reset: credential rows cleared but failed to regenerate {}: {e}",
                    keyfile.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

/// Wipe every Phase 2a credential / audit / onboarding row. Caller has already
/// opened the database and verified no workflows are in flight.
fn clear_credential_tables(db: &maestro_core::db::Database) -> Result<(), Box<dyn std::error::Error>> {
    // The CLI is sync; use `blocking_lock` on the tokio mutex (not inside an
    // async runtime — fine per tokio's docs).
    let conn = db.conn().blocking_lock();
    let tx = conn.unchecked_transaction()?;
    for table in [
        "user_provider_credentials",
        "user_github_credentials",
        "credential_audit",
        "onboarding_state",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), [])?;
    }
    tx.commit()?;
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::DockerHooks { phase }) => run_docker_hooks(&cli.config, *phase),
        Some(Commands::Preflight { strict }) => run_preflight(&cli.config, *strict),
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
        Some(Commands::Keys {
            action: KeysAction::Reset { yes_i_am_sure },
        }) => run_keys_reset(&cli.config, *yes_i_am_sure),
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
    // Detect stale `[commands]` / `[[run_commands]]` keys BEFORE `tracing_subscriber::init`
    // so we can replay the warnings via tracing after the subscriber is up. Inline
    // tracing calls inside Config::load on the first invocation go to the no-op default
    // subscriber and are silently dropped — this two-step path is the workaround.
    let legacy_warnings = if cli.config.exists() {
        match std::fs::read_to_string(&cli.config) {
            Ok(content) => maestro_core::config::detect_legacy_command_keys(&content),
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

    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive(config.general.log_level.parse()?),
        )
        .with_target(true)
        .init();

    // Replay legacy-key warnings now that the subscriber is initialised.
    for msg in &legacy_warnings {
        tracing::warn!("{msg}");
    }

    // Plan-10: auto-polling is disabled in this build. Operators need a visible
    // signal at startup; if they had `auto_polling = true` configured, the
    // override is louder still (warn vs info).
    if config.general.auto_polling {
        tracing::warn!(
            "[general] auto_polling = true is ignored: auto-polling is disabled \
             in this build (plan-11 TODO: per-repo polling). Items must be added \
             manually via the dashboard."
        );
    } else {
        info!(
            "Auto-polling is disabled in this build (plan-11 TODO: per-repo \
             polling). Items must be added manually via the dashboard."
        );
    }

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

    // Install dev-mode flags so dev_mock::is_enabled_from_runtime() works before
    // any agent call. Off by default in production.
    maestro_core::dev_mock::install_dev_config(&config.dev);
    if maestro_core::dev_mock::is_enabled_from_runtime() {
        tracing::info!("[mock-agent] enabled");
    }

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

    let (git_remote, dry_mode, github_app_mgr) = {
        let c = config.read().await;
        let mgr = maestro_core::github_app::try_create_token_manager(&c.github);
        (c.git.remote.clone(), c.general.dry_mode, mgr)
    };

    // Keep a reference to the token manager so we can start the background writer.
    let github_app_for_token_writer = github_app_mgr.clone();

    let actions: Arc<dyn ExternalActions> = if dry_mode {
        info!("Running in DRY MODE — no external writes");
        Arc::new(DryRunActions::new(git_remote, github_app_mgr))
    } else {
        // Pass the live config Arc so RealActions always reads the current
        // repo_path — a post-clone update takes effect without restart.
        Arc::new(RealActions::new(config.clone(), git_remote, github_app_mgr))
    };

    let ticketing_system = config.read().await.general.ticketing_system;

    // Phase 0 (04_architecture.md §1): collect the structured SystemStatus
    // snapshot once at startup. This is the single source of truth the
    // dashboard reads from `GET /api/onboarding/status`. The standalone
    // `acli_ok` probe is replaced by reading `system_status.ticketing.acli_ok`
    // so we don't shell out twice.
    let mut system_status = {
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
        maestro_core::config::resolve_config_relative_path(
            config_file_dir,
            &c.general.workflow_definitions_dir,
        )
    };
    // Initialize the SQLite database for multi-user auth. This happens BEFORE
    // engine construction so the engine can thread the DB handle into the
    // bootstrap driver for per-workspace `worktree_init_commands` overrides
    // (plan-08), and BEFORE poller construction so we can resolve the
    // poller-owner user_id and pass it into both pollers.
    let resolved_data_dir = maestro_core::workflow::snapshot::resolve_data_dir();
    let allow_auto_generate_secret_key = config.read().await.general.allow_auto_generate_secret_key;
    let db = match resolved_data_dir.as_deref() {
        Some(data_dir) => match maestro_core::db::Database::open(data_dir, allow_auto_generate_secret_key) {
            Ok(db) => {
                info!(path = %data_dir.join("maestro.db").display(), "Multi-user database initialized");
                Some(db)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to open multi-user database (multi-user auth unavailable; legacy auth still works)");
                None
            }
        },
        None => {
            tracing::warn!(
                "No data directory resolved — multi-user database unavailable (set MAESTRO_DATA_DIR, MAESTRO_HOME, or HOME)"
            );
            None
        }
    };

    // Plan-10: filesystem ↔ DB reconciliation must run AFTER DB open and
    // BEFORE engine.restore_persisted_workflows(). Reviewer G2: otherwise
    // restored workflows have no `repositories` row to look up by
    // workspace_name and the workflow filter (Step 6) hides every legacy
    // workflow from its owner's dashboard until an admin manually re-adds.
    if let (Some(db), Some(data_dir)) = (db.as_ref(), resolved_data_dir.as_deref()) {
        let migrate_associations = config
            .read()
            .await
            .general
            .migrate_orphan_repo_associations;

        let conn_arc = db.conn().clone();
        let conn_guard = conn_arc.lock().await;
        let conn = &*conn_guard;

        // 3.1 Filesystem → `repositories` reconciliation.
        match repo_reconcile::reconcile_repositories(
            conn,
            maestro_core::workflow::snapshot::WORKSPACES_DIR,
        ) {
            Ok(n) if n > 0 => info!(
                count = n,
                workspaces_dir = maestro_core::workflow::snapshot::WORKSPACES_DIR,
                "Plan-10 reconciliation: registered repositories from on-disk clones"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Plan-10 reconciliation failed (continuing)"),
        }

        // 3.2 Backfill `user_repositories` from restored snapshot workflows
        // (gated; default on).
        if migrate_associations {
            match repo_reconcile::backfill_user_repositories_from_snapshots(conn, data_dir) {
                Ok(n) if n > 0 => info!(
                    count = n,
                    "Backfilled user_repositories from restored workflow snapshots (plan-10)"
                ),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "Plan-10 association backfill failed (continuing)")
                }
            }
        } else {
            info!(
                "[general] migrate_orphan_repo_associations = false — skipping snapshot-driven \
                 user_repositories backfill; existing workflows will be invisible until each user \
                 re-adds the repository from the dashboard"
            );
        }

        // 3.3 active_workspace file cleanup. The active-workspace concept is
        // dead after plan-10; each workflow carries its own repo association.
        let aw_path = data_dir.join("active_workspace");
        if aw_path.exists() {
            if let Ok(value) = std::fs::read_to_string(&aw_path) {
                tracing::info!(
                    value = %value.trim(),
                    "Removing dead `active_workspace` file (plan-10)"
                );
            }
            let _ = std::fs::remove_file(&aw_path);
        }

        // 3.4 Deprecation warning for `[git] repo_path` when set and not
        // matching any registered repository.
        let cfg_repo_path = config.read().await.git.repo_path.clone();
        if !cfg_repo_path.is_empty() && cfg_repo_path != "/workspace" {
            let matches_any = maestro_core::db::repositories::get_by_path(conn, &cfg_repo_path)
                .ok()
                .flatten()
                .is_some();
            if !matches_any {
                tracing::warn!(
                    repo_path = %cfg_repo_path,
                    "[git] repo_path is deprecated and ignored; configure repositories via the dashboard's My Repositories tab."
                );
            }
        }
    } else {
        tracing::warn!(
            "Skipping plan-10 reconciliation: no multi-user database available — \
             repositories cannot be registered until the data dir is configured"
        );
    }

    let engine = Arc::new(WorkflowEngine::new_with_db(
        config.clone(),
        actions.clone(),
        max_concurrent,
        jira_available.clone(),
        ticketing_system,
        workflows_dir,
        db.clone(),
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

    // Resolve the poller owner now that the DB is open. When `None`, the pollers
    // will log a warning and skip `start_workflow` calls so no orphan workflows
    // are created — the web server still serves login/setup so an admin can be
    // created to enable polling later.
    let (resolved_poller_owner, migrate_orphans) = {
        let cfg = config.read().await;
        let username = cfg.general.poller_owner_username.clone();
        let migrate = cfg.general.migrate_orphan_workflows;
        let owner = match &db {
            Some(db) => resolve_poller_owner(db, username.as_deref()).await,
            None => None,
        };
        if owner.is_none() {
            tracing::warn!(
                "No poller owner could be resolved (no admin exists and no override set); \
                 poller will run but skip workflow creation until an admin is registered"
            );
        }
        (owner, migrate)
    };

    // One-shot orphan migration (gated by `[general] migrate_orphan_workflows`).
    // Reassigns any restored workflow with `user_id == None` to the resolved
    // poller owner so it becomes visible on that user's dashboard.
    if migrate_orphans
        && let Some(ref owner_id) = resolved_poller_owner
    {
        let migrated = engine.migrate_orphan_workflows_to_owner(owner_id).await;
        if migrated > 0 {
            // Persist immediately so the migration survives a crash.
            if let Err(e) = engine.sync_workflow_snapshot().await {
                tracing::warn!(error = %e, "Failed to persist workflow snapshot after orphan migration");
            } else {
                info!(count = migrated, "Orphan workflow migration complete and persisted");
            }
        }
    }

    let cancel_token = CancellationToken::new();

    // Start the centralized GitHub App token file writer so worker containers
    // always read a fresh token from the shared volume instead of relying on a
    // frozen GH_TOKEN env var injected at `docker run` time.
    if let Some(ref mgr) = github_app_for_token_writer {
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
    );

    let polling_paused_for_gh = polling_paused.clone();
    let cancel_token_for_gh = cancel_token.clone();
    // Back-compat (one release): legacy `MAESTRO_PREFLIGHT_ERROR` env var set
    // by `docker/entrypoint.sh`. The UI now reads
    // `GET /api/onboarding/status`; this fallback is retained so the
    // dashboard can still render a banner when the DB is unavailable.
    let preflight_error = std::env::var("MAESTRO_PREFLIGHT_ERROR")
        .ok()
        .filter(|s| !s.is_empty());
    if let Some(ref err) = preflight_error {
        tracing::warn!(
            error = %err,
            "MAESTRO_PREFLIGHT_ERROR is set (legacy env var, deprecated — \
             dashboard should read /api/onboarding/status instead)"
        );
    }

    // Now that the DB has been opened (or not), set `per_user_required`. This
    // is the only mutation we make to the system_status after collection.
    system_status.per_user_required = db.is_some();

    // Phase 2a: recompute SystemStatus with the DB in scope so master-key
    // warnings (master_key_unavailable, secret_key_world_readable) join the
    // existing config-derived warnings. We do this AFTER the
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
        for w in &system_status.warnings {
            if w.severity == "critical" {
                tracing::warn!(
                    code = %w.code,
                    severity = %w.severity,
                    "Phase 2a boot warning: {}",
                    w.message
                );
            }
        }
    }

    // Config writer — only available when the config file path is known.
    let config_path = std::fs::canonicalize(&cli.config).unwrap_or_else(|_| cli.config.clone());
    let config_writer = Arc::new(ConfigWriter::new(config_path.clone()));

    // `db` was initialised above (before poller construction) so we could resolve
    // the poller owner. Move it into the AppState here.

    let app_state = AppState {
        engine: engine.clone(),
        config: config.clone(),
        db,
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
        system_status: std::sync::Arc::new(tokio::sync::RwLock::new(system_status)),
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

    // Re-install the [dev] block into the dev_mock module after every config reload.
    // The ConfigWatcher swaps the in-memory `Config` through the shared `Arc<RwLock>`;
    // we poll it at the same cadence and re-snapshot the dev knobs so flipping
    // `[dev] mock_agent` in `config.toml` (or via `POST /api/config/reload`) takes
    // effect without a restart.
    let dev_mock_config = config.clone();
    let dev_mock_cancel = cancel_token.clone();
    let _dev_mock_reload_task = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(
            maestro_core::config_watcher::DEFAULT_POLL_INTERVAL_SECS,
        ));
        // Skip the immediate first tick — initial install already happened in main().
        interval.tick().await;
        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = dev_mock_cancel.cancelled() => break,
            }
            let dev = { dev_mock_config.read().await.dev.clone() };
            maestro_core::dev_mock::install_dev_config(&dev);
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
