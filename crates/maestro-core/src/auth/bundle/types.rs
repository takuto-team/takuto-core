// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Public bundle types + the file-name constants the worker entrypoint reads.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::config::AiAgentProvider;

/// In-container mount point. `ContainerRunner` adds `-v <bundle.dir>:<this>:ro`
/// when a bundle is attached.
pub const WORKER_SECRETS_MOUNTPOINT: &str = "/run/maestro-secrets";

/// File names inside the secrets directory. The worker entrypoint reads each
/// of these into the matching env var when `MAESTRO_AUTH_BUNDLE=1` is set.
pub const SECRET_FILE_CLAUDE: &str = "claude";
pub const SECRET_FILE_CURSOR: &str = "cursor";
pub const SECRET_FILE_CODEX: &str = "codex";
pub const SECRET_FILE_OPENCODE: &str = "opencode";
pub const SECRET_FILE_GH: &str = "gh";
/// Claude session-state file. Carries the contents of a paid / team Claude
/// account's `~/.claude.json` (the OAuth `oauthAccount` block the CLI
/// requires for "logged in"). When present alongside the api_key file, the
/// worker `cp`s this onto `$HOME/.claude.json` before exec'ing the agent
/// so Claude Code accepts the session.
pub const SECRET_FILE_CLAUDE_SESSION: &str = "claude_session.json";

/// Filesystem location for per-workflow secret directories. Relative to
/// the maestro `data_dir`. We deliberately don't expose this as a config
/// knob — the path is referenced by the path-translation logic in
/// `container.rs` (which swaps the data_dir prefix for the DinD-side
/// equivalent), and that translation has to stay in lockstep.
pub const SECRETS_DIR_REL: &str = "runtime/secrets";

/// The end-product of `build_for_workflow`. Lives for the duration of the
/// agent step; dropping it removes the underlying tmpfs directory (which is
/// already bind-mounted read-only into the worker — the container's view
/// disappears with the next `docker run` cycle anyway).
pub struct WorkerSecretsBundle {
    pub provider: AiAgentProvider,
    /// Absolute host-side path to the per-provider secret file. `None` only
    /// when the workflow has no provider credential and the deployment-default
    /// fallback is allowed (the worker entrypoint will source nothing for
    /// this provider; the provider CLI then reads ambient
    /// `CLAUDE_CODE_OAUTH_TOKEN` / `CURSOR_API_KEY` from `/etc/maestro/env`).
    pub provider_secret_file: Option<PathBuf>,
    /// Claude only. Host-side path to the unsealed
    /// `claude_session.json` blob (the user's `~/.claude.json` contents
    /// — `oauthAccount` block etc.). `Some` when a `kind=cli_state` row
    /// exists for `(user_id, "claude")`; `None` otherwise. Independent of
    /// `provider_secret_file` — a user can have one, the other, or both.
    /// The worker `cp`s this onto `$HOME/.claude.json` before exec'ing
    /// the agent so Claude Code accepts the session as "logged in".
    pub claude_session_file: Option<PathBuf>,
    /// Absolute host-side path to the GitHub token file. `None` when the
    /// resolver returned `UnauthenticatedGit` (workflow can still spawn a
    /// container; the agent step itself will fail at `git push` time).
    pub github_token_file: Option<PathBuf>,
    /// `Some` for `TokenSource::UserPat` and the App identity ("maestro-bot[bot]"),
    /// `None` only when the resolver failed to materialise any token.
    pub git_author_name: Option<String>,
    pub git_author_email: Option<String>,
    /// Custom AEAD provider base URL (`ANTHROPIC_BASE_URL` etc.). Sourced
    /// from `[agent.providers.<name>].base_url`. Safe to pass as a CLI
    /// `--env` (URLs are not secrets).
    pub base_url: Option<String>,
    /// `extra_args` from the active provider's sub-table. The Claude/Cursor
    /// adapters append these to the agent argv.
    pub extra_args: Vec<String>,
    /// Non-secret env-vars to set on the worker. Used for things like
    /// `MAESTRO_AUTH_BUNDLE=1` (a discriminator the entrypoint switches on).
    /// NEVER carries token bytes.
    pub extra_env: Vec<(String, String)>,
    /// OpenCode self-hosted init-shim output directory (spec
    /// `lore/audits/2026-05-27-opencode-self-hosted-spec.md`). When set,
    /// the container runner bind-mounts this read-only at
    /// `/home/maestro/.config/opencode/` so OpenCode reads its
    /// `opencode.json` config (which embeds the admin `base_url` + `model`
    /// and the user's optional bearer). `Some` only when
    /// `provider == OpenCode`; `None` for every other provider.
    pub opencode_config_dir: Option<PathBuf>,
    /// RAII: dropping the `TempDir` `rm -rf`s the secrets directory.
    pub(super) _temp_dir: TempDir,
}

impl std::fmt::Debug for WorkerSecretsBundle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerSecretsBundle")
            .field("provider", &self.provider.as_str())
            .field("has_provider_secret", &self.provider_secret_file.is_some())
            .field("has_claude_session", &self.claude_session_file.is_some())
            .field("has_github_token", &self.github_token_file.is_some())
            .field("git_author_name", &self.git_author_name)
            .field("git_author_email", &self.git_author_email)
            .field("base_url", &self.base_url)
            .field("extra_args_count", &self.extra_args.len())
            .field("has_opencode_config", &self.opencode_config_dir.is_some())
            .field("temp_dir", &self._temp_dir.path())
            .finish()
    }
}

impl WorkerSecretsBundle {
    /// Host-side directory holding the secret files. The caller bind-mounts
    /// this into the worker at [`WORKER_SECRETS_MOUNTPOINT`] (read-only).
    pub fn host_dir(&self) -> &Path {
        self._temp_dir.path()
    }

    /// Construct a stub bundle for unit tests that
    /// need to exercise the bundle-attached code paths without going
    /// through the full `build()` async + DB pipeline. The `_temp_dir`
    /// field is private to this module so callers in other crate modules
    /// can't construct one by hand; this helper plugs the gap.
    ///
    /// Visibility: `pub(crate)`, `#[doc(hidden)]`, and `#[cfg(test)]` —
    /// only compiled into test builds, never used outside.
    #[cfg(test)]
    #[doc(hidden)]
    pub(crate) fn for_tests(
        provider: AiAgentProvider,
        extra_env: Vec<(String, String)>,
    ) -> Self {
        let dir = tempfile::TempDir::new().expect("tempdir for test bundle");
        let provider_path = dir.path().join("provider");
        let gh_path = dir.path().join("gh");
        Self {
            provider,
            provider_secret_file: Some(provider_path),
            claude_session_file: None,
            github_token_file: Some(gh_path),
            git_author_name: None,
            git_author_email: None,
            base_url: None,
            extra_args: vec![],
            extra_env,
            opencode_config_dir: None,
            _temp_dir: dir,
        }
    }
}
