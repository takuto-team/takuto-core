// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Command-line surface: the `clap` parser and subcommand enums.

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

#[derive(Copy, Clone, Debug, ValueEnum)]
pub enum DockerHookPhase {
    Build,
    Startup,
}

impl DockerHookPhase {
    pub fn label(self) -> &'static str {
        match self {
            DockerHookPhase::Build => "build",
            DockerHookPhase::Startup => "startup",
        }
    }
}

#[derive(Subcommand)]
pub enum Commands {
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
    /// Print the egress allowlist hosts for the configured AI provider(s),
    /// one per line. Consumed by `docker/egress-rules.sh` so the firewall
    /// opens exactly the active + admin-allowed providers (and any
    /// self-hosted `base_url`) instead of hard-coding one vendor.
    EgressHosts,
    /// Install/refresh the agent + Atlassian CLIs into the shared tools volume.
    /// These CLIs are not baked into the image; the setup-mode entrypoint runs
    /// this before interactive auth, and the web server runs the equivalent at
    /// startup with live progress.
    Agents {
        #[command(subcommand)]
        action: AgentsAction,
    },
}

#[derive(Subcommand)]
pub enum AgentsAction {
    /// Install/refresh the configured CLIs (claude/codex/opencode/cursor, plus
    /// acli in Jira mode) at their pinned version (or latest) into
    /// `/opt/takuto-tools/bin`.
    Install,
}

#[derive(Subcommand)]
pub enum ProvisioningAction {
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
pub enum KeysAction {
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
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Path to the configuration file (also reads TAKUTO_CONFIG env var)
    #[arg(
        short,
        long,
        default_value = "config.toml",
        env = "TAKUTO_CONFIG",
        global = true
    )]
    pub config: PathBuf,

    /// Enable dry-run mode (overrides config file); only applies to the default server command
    #[arg(long, global = true)]
    pub dry_run: bool,
}
