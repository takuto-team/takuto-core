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

use takuto_core::actions::dry_run::DryRunActions;
use takuto_core::actions::real::RealActions;
use takuto_core::actions::traits::ExternalActions;
use takuto_core::config::{Config, TicketingSystem};
use takuto_core::config_watcher::ConfigWatcher;
use takuto_core::config_writer::ConfigWriter;
use takuto_core::db::Database;
use takuto_core::docker_hooks;
use takuto_core::github::poller::GitHubPoller;
use takuto_core::github::pr_merge_poller::PrMergePoller;
use takuto_core::jira::poller::JiraPoller;
use takuto_core::repo_reconcile;
use takuto_core::workflow::engine::WorkflowEngine;
use takuto_web::server::build_router;
use takuto_web::state::{
    AppState, AuthState, ConfigState, EditorState, EngineState, RunCommandState,
};

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
    let adapter = db.adapter();
    if let Some(username) = cfg_username {
        match takuto_core::db::users::get_user_by_username(adapter, username).await {
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

    match takuto_core::db::users::list_admins(adapter).await {
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
    /// Read-only accessors over `[provisioning].install_commands`
    /// for the docker entrypoint shell. The entrypoint can't parse TOML
    /// itself, so this subcommand exposes the canonical SHA and the
    /// command list in shell-safe form.
    Provisioning {
        #[command(subcommand)]
        action: ProvisioningAction,
    },
}

#[derive(Subcommand)]
enum ProvisioningAction {
    /// Print the canonical sha256 of `[provisioning].install_commands` to
    /// stdout (followed by a single newline). The entrypoint compares
    /// this against `/opt/takuto-tools/.provisioning-sha` to decide
    /// whether to skip the install pass.
    Sha,
    /// Print each install command to stdout, NUL-separated, terminated by
    /// a final NUL. NUL is the only byte that can't appear inside a
    /// shell command string, so the entrypoint can safely iterate via
    /// `read -d ''`.
    Commands,
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
    name = "takuto",
    about = "Automated Jira ticket handler using Claude Code or Cursor Agent"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path to the configuration file (also reads TAKUTO_CONFIG env var)
    #[arg(
        short,
        long,
        default_value = "config.toml",
        env = "TAKUTO_CONFIG",
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
            "[takuto preflight] {sev}: {code} — {msg}",
            sev = w.severity,
            code = w.code,
            msg = w.message
        );
    }

    // Seed the GitHub App token file so it is available before the main server
    // starts. The server's background task handles subsequent refreshes. This
    // runs even on degraded boots — it's a no-op when the App is not
    // configured.
    if let Some(mgr) = takuto_core::github_app::try_create_token_manager(&config.github) {
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
                    let token_path = std::path::Path::new(takuto_core::github_app::TOKEN_FILE_PATH);
                    match takuto_core::github_app::write_token_file(token_path, &token) {
                        Ok(()) => {
                            eprintln!(
                                "[takuto preflight] GitHub App token written to {}",
                                token_path.display()
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "[takuto preflight] WARNING: failed to write token file: {e}"
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[takuto preflight] WARNING: failed to fetch GitHub App token: {e}");
                }
            }
        }
    }

    match config.general.ticketing_system {
        TicketingSystem::Jira => {
            if status.ticketing.acli_ok {
                eprintln!("[takuto preflight] ticketing_system = jira, acli authenticated.");
            } else {
                eprintln!(
                    "[takuto preflight] ticketing_system = jira but acli is not authenticated — Jira integration disabled, manual entry only."
                );
            }
        }
        TicketingSystem::GitHub => {
            eprintln!(
                "[takuto preflight] ticketing_system = github — polling GitHub issues, no Atlassian auth required."
            );
        }
        TicketingSystem::None => {
            eprintln!(
                "[takuto preflight] ticketing_system = none — manual description entry only."
            );
        }
    }

    if status.has_critical() {
        eprintln!(
            "[takuto preflight] {n} critical warning(s) present — the dashboard will boot in degraded mode (see GET /api/onboarding/status).",
            n = status
                .warnings
                .iter()
                .filter(|w| w.severity == "critical")
                .count()
        );
        if strict {
            eprintln!("[takuto preflight] --strict was passed — exiting FAILURE for CI.");
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!("[takuto preflight] OK.");
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

    let mgr = match takuto_core::github_app::try_create_token_manager(&config.github) {
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

/// `takuto keys reset` (04_architecture.md §3.2, A5).
///
/// Clears every credential / audit / onboarding row and rewrites the master
/// keyfile under `${data_dir}/secret.key`. Lossy by design: every user must
/// re-paste their credentials afterwards.
///
/// Read-only accessors over `[provisioning].install_commands`
/// for the docker entrypoint shell. The entrypoint can't parse TOML
/// itself, so this subcommand exposes the canonical SHA and the command
/// list. Both write to stdout so the entrypoint can capture via
/// `$(takuto provisioning sha)` / a `while read` loop.
///
/// Exit code is 0 on success even when the install_commands list is
/// empty (the entrypoint takes the no-op fast path).
fn run_provisioning(config_path: &std::path::Path, action: &ProvisioningAction) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };
    match action {
        ProvisioningAction::Sha => {
            println!("{}", config.provisioning_sha());
        }
        ProvisioningAction::Commands => {
            // NUL-terminated stream — `0x00` is the only byte that can't
            // appear inside a POSIX shell command, so the entrypoint can
            // iterate via `while IFS= read -r -d '' cmd; do ... done`.
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            for cmd in &config.provisioning.install_commands {
                if out.write_all(cmd.as_bytes()).is_err() || out.write_all(&[0u8]).is_err() {
                    return ExitCode::FAILURE;
                }
            }
            let _ = out.flush();
        }
    }
    ExitCode::SUCCESS
}

/// - `--yes-i-am-sure` not passed
/// - any workflow in a non-terminal, non-paused state per the snapshot file
///   (graceful-shutdown the server first; see QA T-KEYS-002)
/// - config load failure
/// - data_dir cannot be resolved
fn run_keys_reset(config_path: &std::path::Path, yes_i_am_sure: bool) -> ExitCode {
    if !yes_i_am_sure {
        eprintln!(
            "takuto keys reset: refuses to run without --yes-i-am-sure (this command is destructive)."
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

    let Some(data_dir) = takuto_core::workflow::snapshot::resolve_data_dir() else {
        eprintln!(
            "takuto keys reset: cannot resolve data directory (set TAKUTO_DATA_DIR, TAKUTO_HOME, or HOME)."
        );
        return ExitCode::FAILURE;
    };

    // Refuse when an in-flight workflow is present in any per-workspace
    // snapshot. The helper lives in takuto-core::workflow::snapshot so it
    // can be unit-tested without spawning the CLI.
    match takuto_core::workflow::snapshot::scan_in_flight_workflow_keys(&data_dir) {
        Ok(active) if !active.is_empty() => {
            eprintln!(
                "takuto keys reset: refuses to run — {} active workflow(s) present in the snapshot: {}.",
                active.len(),
                active.join(", ")
            );
            eprintln!("Stop or finish them, then retry.");
            return ExitCode::FAILURE;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("takuto keys reset: failed to read workflow snapshots: {e}");
            return ExitCode::FAILURE;
        }
    }

    // Open the DB so we can clear the credential / audit / onboarding tables.
    // Pass `allow_auto_generate_secret_key = false` here — we're about to
    // rewrite the keyfile ourselves; we don't want Database::open creating
    // a new one mid-reset.
    let db = match takuto_core::db::Database::open(&data_dir, false) {
        Ok(db) => db,
        Err(e) => {
            eprintln!(
                "takuto keys reset: failed to open database at {}: {e}",
                data_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };

    // Spin up a one-shot runtime to drive the async adapter call. The CLI
    // dispatcher is sync, so we need a local runtime for this single
    // transaction — keep it scoped tight so the runtime drops with the
    // transaction.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("takuto keys reset: failed to start async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = rt.block_on(clear_credential_tables(&db)) {
        eprintln!("takuto keys reset: failed to clear credential tables: {e}");
        return ExitCode::FAILURE;
    }

    // Regenerate (or wipe) the master keyfile.
    let keyfile = takuto_core::auth::master_key::keyfile_path(&data_dir);
    let env_set = std::env::var("TAKUTO_SECRET_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if env_set {
        // Operator manages the master key via env var; we only nuked the rows.
        // Don't touch the keyfile (if any) — it may not even exist. Warn so
        // the operator knows to rotate the env var manually.
        eprintln!(
            "takuto keys reset: credential rows cleared. TAKUTO_SECRET_KEY is set — rotate it manually before bringing the server back up."
        );
    } else {
        // Best-effort delete + regenerate.
        let _ = std::fs::remove_file(&keyfile);
        match takuto_core::auth::master_key::load_or_init_master_key(
            &data_dir,
            config.general.allow_auto_generate_secret_key,
        ) {
            Ok(_) => {
                eprintln!(
                    "takuto keys reset: credential rows cleared and {} regenerated.",
                    keyfile.display()
                );
            }
            Err(e) => {
                eprintln!(
                    "takuto keys reset: credential rows cleared but failed to regenerate {}: {e}",
                    keyfile.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

/// Wipe every credential / audit / onboarding row. Caller has already
/// opened the database and verified no workflows are in flight.
///
/// Goes through the agnostic database adapter. The CLI is sync at the
/// dispatch layer, so the call site spins up a one-shot current-thread
/// runtime to drive the async transaction.
async fn clear_credential_tables(
    db: &takuto_core::db::Database,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = db.adapter().begin().await?;
    for table in [
        "user_provider_credentials",
        "user_github_credentials",
        "credential_audit",
        "onboarding_state",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), vec![]).await?;
    }
    tx.commit().await?;
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
        Some(Commands::Provisioning { action }) => run_provisioning(&cli.config, action),
        None => match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => match rt.block_on(run_server(&cli)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("Takuto error: {e}");
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
        takuto_core::config::resolve_config_relative_path(
            config_file_dir,
            &c.general.workflow_definitions_dir,
        )
    };
    // Parse the default per-user work-item flows once at startup. Shared by
    // the startup seeding backfill below and the web layer (seeding new users,
    // the "Re-seed from defaults" action).
    let work_item_flow_defaults = Arc::new(
        takuto_core::workflow::definitions::default_flows_from_dir(&workflows_dir),
    );
    // Initialize the SQLite database for multi-user auth. This happens BEFORE
    // engine construction so the engine can thread the DB handle into the
    // bootstrap driver for per-workspace `worktree_init_commands` overrides,
    // and BEFORE poller construction so we can resolve the poller-owner
    // user_id and pass it into both pollers.
    let resolved_data_dir = takuto_core::workflow::snapshot::resolve_data_dir();
    // Sweep orphan WorkerSecretsBundle directories from a prior
    // run (crash between TempDir creation and drop leaves them around).
    // Safe to run unconditionally — best-effort, no-op when the dir is
    // missing. Runs BEFORE the DB opens because it touches a sibling
    // directory under data_dir, not the DB itself.
    if let Some(dir) = resolved_data_dir.as_deref()
        && let Err(e) = takuto_core::auth::bundle::cleanup_orphan_secrets(dir)
    {
        tracing::warn!(
            data_dir = %dir.display(),
            error = %e,
            "WorkerSecretsBundle orphan sweep failed (continuing); old dirs may persist"
        );
    }
    let (allow_auto_generate_secret_key, mut db_config) = {
        let cfg = config.read().await;
        (
            cfg.general.allow_auto_generate_secret_key,
            cfg.database.clone(),
        )
    };
    // `TAKUTO_DATABASE_CONNECTION` env var overrides
    // `[database].connection` from config.toml. Useful for the
    // docker-compose.{postgres,mariadb}.yml overlays which set the URL
    // per-deployment without touching the user's checked-in config.
    if let Ok(env_url) = std::env::var("TAKUTO_DATABASE_CONNECTION")
        && !env_url.trim().is_empty()
    {
        db_config.connection = env_url;
    }
    let db = match resolved_data_dir.as_deref() {
        Some(data_dir) => match takuto_core::db::Database::connect(
            data_dir,
            &db_config,
            allow_auto_generate_secret_key,
        )
        .await
        {
            Ok(db) => {
                if db_config.is_default_sqlite() {
                    info!(path = %data_dir.join("takuto.db").display(), "Multi-user database initialized (sqlite)");
                } else {
                    info!(
                        backend = %db.adapter().backend(),
                        url = %takuto_core::config::redact_connection_password(db_config.connection_url()),
                        "Multi-user database initialized (external backend)"
                    );
                }
                Some(db)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to open multi-user database (multi-user auth unavailable; legacy auth still works)");
                None
            }
        },
        None => {
            tracing::warn!(
                "No data directory resolved — multi-user database unavailable (set TAKUTO_DATA_DIR, TAKUTO_HOME, or HOME)"
            );
            None
        }
    };

    // Filesystem ↔ DB reconciliation must run AFTER DB open and
    // BEFORE engine.restore_persisted_workflows(). Otherwise restored
    // workflows have no `repositories` row to look up by workspace_name
    // and the workflow filter hides every legacy workflow from its
    // owner's dashboard until an admin manually re-adds.
    if let (Some(db), Some(data_dir)) = (db.as_ref(), resolved_data_dir.as_deref()) {
        let migrate_associations = config.read().await.general.migrate_orphan_repo_associations;

        // Repositories DAO uses the agnostic adapter — no rusqlite
        // MutexGuard needed for the reconciliation path; both helpers
        // are async and take &DbAdapter directly.
        let adapter = db.adapter();

        // Filesystem → `repositories` reconciliation.
        match repo_reconcile::reconcile_repositories(
            adapter,
            takuto_core::workflow::snapshot::WORKSPACES_DIR,
        )
        .await
        {
            Ok(n) if n > 0 => info!(
                count = n,
                workspaces_dir = takuto_core::workflow::snapshot::WORKSPACES_DIR,
                "Reconciliation: registered repositories from on-disk clones"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Repository reconciliation failed (continuing)"),
        }

        // Backfill `user_repositories` from restored snapshot workflows
        // (gated; default on).
        if migrate_associations {
            match repo_reconcile::backfill_user_repositories_from_snapshots(adapter, data_dir).await
            {
                Ok(n) if n > 0 => info!(
                    count = n,
                    "Backfilled user_repositories from restored workflow snapshots"
                ),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "Association backfill failed (continuing)")
                }
            }
        } else {
            info!(
                "[general] migrate_orphan_repo_associations = false — skipping snapshot-driven \
                 user_repositories backfill; existing workflows will be invisible until each user \
                 re-adds the repository from the dashboard"
            );
        }

        // Seed default work-item flows for every existing user against the
        // active workspace, idempotently. Runs before the HTTP listener
        // accepts traffic so the first dashboard load already sees flows for
        // the currently-selected workspace. Best-effort: a failure logs a
        // warning and the user falls back to the empty-state banner.
        let active_workspace = takuto_core::workflow::snapshot::workspace_name_from_repo_path(
            std::path::Path::new(&config.read().await.git.repo_path),
        );
        match takuto_core::db::users::list_users(adapter).await {
            Ok(users) => {
                for user in &users {
                    if let Err(e) = takuto_core::db::user_work_item_flows::seed_if_absent(
                        adapter,
                        &user.id,
                        &active_workspace,
                        &work_item_flow_defaults,
                    )
                    .await
                    {
                        tracing::warn!(
                            user_id = %user.id,
                            workspace = %active_workspace,
                            error = %e,
                            "Failed to seed default work-item flows (continuing)"
                        );
                    }
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                "Failed to list users for work-item flow seeding (continuing)"
            ),
        }

        // active_workspace file cleanup. The active-workspace concept is
        // dead; each workflow carries its own repo association.
        let aw_path = data_dir.join("active_workspace");
        if aw_path.exists() {
            if let Ok(value) = std::fs::read_to_string(&aw_path) {
                tracing::info!(
                    value = %value.trim(),
                    "Removing dead `active_workspace` file"
                );
            }
            let _ = std::fs::remove_file(&aw_path);
        }

        // Deprecation warning for `[git] repo_path` when set and not
        // matching any registered repository.
        let cfg_repo_path = config.read().await.git.repo_path.clone();
        if !cfg_repo_path.is_empty() && cfg_repo_path != "/workspace" {
            let matches_any = takuto_core::db::repositories::get_by_path(adapter, &cfg_repo_path)
                .await
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
            "Skipping repository reconciliation: no multi-user database available — \
             repositories cannot be registered until the data dir is configured"
        );
    }

    // Construct the GitAuthResolver here so we can attach it to the
    // engine via `with_git_auth_resolver` BEFORE wrapping in Arc. The
    // same resolver is later stored on AppState for the web layer.
    let git_auth_resolver: Option<Arc<takuto_core::github::auth_resolver::GitAuthResolver>> =
        db.as_ref().map(|d| {
            Arc::new(takuto_core::github::auth_resolver::GitAuthResolver::new(
                d.clone(),
                github_app_for_token_writer.clone(),
            ))
        });

    let mut engine = WorkflowEngine::new_with_db(
        config.clone(),
        actions.clone(),
        max_concurrent,
        jira_available.clone(),
        ticketing_system,
        workflows_dir,
        db.clone(),
    );
    if let Some(ref resolver) = git_auth_resolver {
        engine = engine.with_git_auth_resolver(resolver.clone());
    }
    // Wire the production GhClient so at-resume PAT revalidation can
    // run. Tests inject a MockGhClient instead.
    engine = engine.with_gh_client(Arc::new(takuto_core::auth::RealGhClient::new()));
    let engine = Arc::new(engine);

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
    if migrate_orphans && let Some(ref owner_id) = resolved_poller_owner {
        let migrated = engine.migrate_orphan_workflows_to_owner(owner_id).await;
        if migrated > 0 {
            // Persist immediately so the migration survives a crash.
            if let Err(e) = engine.sync_workflow_snapshot().await {
                tracing::warn!(error = %e, "Failed to persist workflow snapshot after orphan migration");
            } else {
                info!(
                    count = migrated,
                    "Orphan workflow migration complete and persisted"
                );
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
        Arc::new(takuto_core::jira::RealJiraSourceFactory),
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

    // `db` was initialised above (before poller construction) so we could resolve
    // the poller owner. Move it into the AppState here.

    // Reuse the resolver we built above for the engine. The same Arc
    // lives on both AppState (for HTTP-handler use) and the workflow
    // engine (for driver-task use).

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
