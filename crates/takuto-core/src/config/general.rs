// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Top-level toggles: ticketing mode, concurrency caps, polling, log level,
//! plus the `[provisioning]`, `[dev]`, and `[docker]` sections that the audit
//! groups with the general configuration block.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Which ticketing system (if any) drives workflow automation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum TicketingSystem {
    /// No ticketing integration — manual description entry only (default).
    #[default]
    None,
    /// Jira via `acli` — current behavior with auto-polling and ticket transitions.
    Jira,
    /// GitHub Issues — poll open issues, no Atlassian auth required.
    GitHub,
}

impl fmt::Display for TicketingSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Jira => f.write_str("jira"),
            Self::GitHub => f.write_str("github"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    #[serde(default)]
    pub dry_mode: bool,
    /// How often (seconds) the single poller loop wakes to poll every
    /// auto-polling-enabled repository. Deployment-global (one cadence for the
    /// shared loop); per-repo enable lives in `user_repo_polling_settings`.
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_workflows: u32,
    /// Max **visible** workflows on the dashboard (rows still in the map: **Done**, paused, stopped, error, in-progress all count). `0` means use **`max_concurrent_workflows`**.
    #[serde(default)]
    pub max_active_workflows: u32,
    /// Max **manual** dashboard-started ticket workflows that still **occupy a slot** (not **Done**, **Stopped**, or **Error**). `0` means no limit.
    #[serde(default)]
    pub max_concurrent_manual_workflows: u32,
    /// When `true`, the per-repo `max_parallel_items` cap is enforced per
    /// workflow owner; `false` enforces it globally. Deployment-global
    /// (cross-repo, per-user concurrency policy).
    #[serde(default)]
    pub max_parallel_per_user: bool,
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// Docker image for workflow worker containers. Empty = auto-detect from running Takuto container.
    #[serde(default)]
    pub worker_image: String,
    /// Which ticketing system drives workflow automation. Default `none` (no ticketing integration).
    #[serde(default)]
    pub ticketing_system: TicketingSystem,
    /// Interval in seconds for polling PR merge status via the GitHub API (`0` disables polling). Default: 60.
    #[serde(default = "default_pr_merge_poll_interval")]
    pub pr_merge_poll_interval_secs: u64,
    /// When `true`, each agent step prompt includes instructions to append findings to
    /// `lore/reports/<item-key>_report.md` and a final consolidation step produces a polished
    /// summary after all custom steps complete. Default `false`.
    #[serde(default)]
    pub generate_report: bool,
    /// Directory containing dynamic workflow definition YAML files. Relative to the config file
    /// directory, or absolute. Default `"workflows"`.
    #[serde(default = "default_workflow_definitions_dir")]
    pub workflow_definitions_dir: String,
    /// Username of the user who owns workflows created automatically by the Jira/GitHub poller.
    /// When `None` (default), the poller falls back to the lexicographically-first non-suspended
    /// admin. When set but the named user is missing or suspended, a warning is logged and the
    /// fallback is used. When neither resolves, polling-created workflows are skipped entirely.
    #[serde(default)]
    pub poller_owner_username: Option<String>,
    /// When `true`, workflows restored from snapshot with `user_id == None` (e.g. pre-multi-user
    /// orphans) are reassigned to the resolved poller owner at startup so they appear on that
    /// user's dashboard. Default `false` — orphan workflows remain invisible until an explicit
    /// migration is requested.
    #[serde(default)]
    pub migrate_orphan_workflows: bool,
    /// When `true` (default), startup reconciliation back-fills
    /// `user_repositories` rows from restored snapshot workflows — every
    /// workflow whose `user_id` is set and whose `workspace_name` matches a
    /// registered repository's name gets a `(user_id, repository_id)`
    /// association created so the dashboard list filter shows the workflow
    /// to its owner. Set to `false` if the operator wants pre-existing
    /// workflows to STAY hidden on their owner's dashboard until the owner
    /// explicitly adds the repository.
    #[serde(default = "default_migrate_orphan_repo_associations")]
    pub migrate_orphan_repo_associations: bool,
    /// Controls whether the server will auto-generate
    /// `${data_dir}/secret.key` on first boot when neither
    /// `TAKUTO_SECRET_KEY` nor an existing keyfile is present. Default
    /// **`true`** so single-tenant + fresh installs Just Work. Set to `false`
    /// in hardened environments where the operator wants to provision the
    /// key out of band; the server then boots in degraded mode until the
    /// keyfile or env var is provided.
    #[serde(default = "default_allow_auto_generate_secret_key")]
    pub allow_auto_generate_secret_key: bool,
    /// How many days of `work_item_log_lines` rows to keep before the
    /// retention task deletes them. `0` disables retention (keep forever).
    /// Default `7` days, which is enough to cover a long weekend of
    /// investigation without growing the DB indefinitely.
    #[serde(default = "default_work_item_log_retention_days")]
    pub work_item_log_retention_days: u32,
}

fn default_migrate_orphan_repo_associations() -> bool {
    true
}

fn default_allow_auto_generate_secret_key() -> bool {
    true
}

fn default_work_item_log_retention_days() -> u32 {
    7
}

fn default_workflow_definitions_dir() -> String {
    "workflows".to_string()
}

fn default_poll_interval() -> u64 {
    60
}
fn default_max_concurrent() -> u32 {
    1
}
fn default_log_level() -> String {
    "info".to_string()
}
fn default_pr_merge_poll_interval() -> u64 {
    60
}

impl GeneralConfig {
    /// Effective cap on **visible** dashboard workflows for the Jira poller. **`max_active_workflows == 0`** mirrors **`max_concurrent_workflows`**.
    pub fn effective_max_active_workflows(&self) -> u32 {
        if self.max_active_workflows == 0 {
            self.max_concurrent_workflows
        } else {
            self.max_active_workflows
        }
    }
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            dry_mode: false,
            poll_interval_secs: default_poll_interval(),
            max_concurrent_workflows: default_max_concurrent(),
            max_active_workflows: 0,
            max_concurrent_manual_workflows: 0,
            max_parallel_per_user: false,
            log_level: default_log_level(),
            worker_image: String::new(),
            ticketing_system: TicketingSystem::None,
            pr_merge_poll_interval_secs: default_pr_merge_poll_interval(),
            generate_report: false,
            workflow_definitions_dir: default_workflow_definitions_dir(),
            poller_owner_username: None,
            migrate_orphan_workflows: false,
            migrate_orphan_repo_associations: default_migrate_orphan_repo_associations(),
            allow_auto_generate_secret_key: default_allow_auto_generate_secret_key(),
            work_item_log_retention_days: default_work_item_log_retention_days(),
        }
    }
}

/// Tool-provisioning block. List of shell commands that run as
/// **root** in the takuto container at startup (before `setpriv` to the
/// `takuto` user) to populate the shared `takuto-tools` Docker volume
/// at `/opt/takuto-tools/bin`. The volume is bind-mounted **read-only**
/// into every spawned worker / editor / run-command via
/// `container.rs::base_docker_args`, so anything installed here is
/// available to claude / cursor / scripts on `$PATH`.
///
/// **SHA-gated**: the canonical sha256 of the (sorted, JSON-encoded)
/// `install_commands` list is written to
/// `/opt/takuto-tools/.provisioning-sha` on full success. Subsequent
/// boots compare the live config's SHA against the file; if they match
/// the install pass is skipped (fast path). Edit the list (add, remove,
/// reorder, or tweak a command) → SHA changes → install pass runs again.
///
/// **Per-command idempotency** is the admin's responsibility — guard
/// each command with `[ -f "$TAKUTO_TOOLS_BIN/<name>" ] || …` (matching
/// the defaults shipped in `config.toml.example`) so re-runs after
/// adding an unrelated tool don't re-fetch the unchanged ones.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvisioningConfig {
    /// One shell command per element. Each runs via `bash -c "$cmd"` as
    /// root, with `TAKUTO_TOOLS_BIN=/opt/takuto-tools/bin` exported.
    /// Empty list (the default) → no-op fast path; the install pass
    /// records its empty-list SHA and skips on subsequent boots.
    #[serde(default)]
    pub install_commands: Vec<String>,
}

/// Dev-only knobs. Off by default in production. Never read inside any code path
/// that runs against real users without an explicit `[dev]` opt-in.
///
/// See `crates/takuto-core/src/dev_mock.rs` for the mock-agent behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DevConfig {
    /// When `true`, `ClaudeSession::run_prompt` and `CursorSession::run_prompt`
    /// short-circuit to a scripted mock session. **No Claude/Cursor process is spawned.**
    /// Honors the env override `TAKUTO_DEV_MOCK_AGENT=1`.
    #[serde(default)]
    pub mock_agent: bool,

    /// Optional path to a text file used as the mock's line script (one emit per line).
    /// Relative paths resolve against the config file directory.
    /// When `None` (default), the built-in `DEFAULT_MOCK_SCRIPT` is used.
    #[serde(default)]
    pub mock_agent_script_path: Option<String>,

    /// Delay between emitted lines in ms. Default 75.
    #[serde(default = "default_mock_line_delay_ms")]
    pub mock_agent_line_delay_ms: u64,

    /// Total mock session duration cap in ms. The mock will stop emitting after this
    /// even if the script has more lines. Default 5000.
    #[serde(default = "default_mock_total_ms")]
    pub mock_agent_total_ms: u64,
}

impl Default for DevConfig {
    fn default() -> Self {
        Self {
            mock_agent: false,
            mock_agent_script_path: None,
            mock_agent_line_delay_ms: default_mock_line_delay_ms(),
            mock_agent_total_ms: default_mock_total_ms(),
        }
    }
}

fn default_mock_line_delay_ms() -> u64 {
    75
}
fn default_mock_total_ms() -> u64 {
    5000
}

/// Docker-specific hooks (see README). `build_commands` run at image build time; `compose_up_commands` on each container start.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DockerConfig {
    /// Shell commands (`bash -c`) executed once while building the image, after tools are installed.
    #[serde(default)]
    pub build_commands: Vec<String>,
    /// Shell commands executed on every `docker compose up` as the takuto user, after auth preflight, before the server.
    #[serde(default)]
    pub compose_up_commands: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_max_active_workflows_returns_max_active_when_nonzero() {
        let general = GeneralConfig {
            max_active_workflows: 5,
            max_concurrent_workflows: 3,
            ..Default::default()
        };
        assert_eq!(general.effective_max_active_workflows(), 5);
    }

    #[test]
    fn effective_max_active_workflows_falls_back_to_concurrent_when_zero() {
        let general = GeneralConfig {
            max_active_workflows: 0,
            max_concurrent_workflows: 4,
            ..Default::default()
        };
        assert_eq!(general.effective_max_active_workflows(), 4);
    }

    #[test]
    fn ticketing_system_display_returns_lowercase_names() {
        assert_eq!(TicketingSystem::None.to_string(), "none");
        assert_eq!(TicketingSystem::Jira.to_string(), "jira");
        assert_eq!(TicketingSystem::GitHub.to_string(), "github");
    }
}
