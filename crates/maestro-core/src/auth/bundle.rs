// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! Phase 2b.3 (04_architecture.md §6) — per-workflow worker secrets bundle.
//!
//! Builds an opaque container of tmpfs-mounted secret files + non-secret
//! env vars that the worker entrypoint sources, then deletes. The bundle's
//! `TempDir` field RAII-cleans the secrets directory when the workflow
//! teardown drops the value.
//!
//! Threat model: secrets MUST NOT be passed as `-e KEY=value` to `docker
//! run` (visible via `docker inspect`). Instead, the secret files live on
//! a host tmpfs (`mode 0700` parent, `0400` files) and are bind-mounted
//! read-only into the worker at `/run/maestro-secrets/`. The worker
//! entrypoint `source`s each file into an env var then `rm`s the on-disk
//! copy to shrink the blast radius if the worker is later compromised.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tempfile::TempDir;

use crate::auth::{open, MasterKey, SealedBlob};
use crate::config::{AiAgentProvider, Config, ConfigError};
use crate::db::github_credentials;
use crate::db::provider_credentials;
use crate::error::Result;
use crate::github::auth_resolver::{GitAction, GitAuthResolver, TokenSource};
use crate::workflow::snapshot::AuthPin;

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
/// Task #39: Claude session-state file. Carries the contents of a paid /
/// team Claude account's `~/.claude.json` (the OAuth `oauthAccount` block
/// the CLI requires for "logged in"). When present alongside the api_key
/// file, the worker `cp`s this onto `$HOME/.claude.json` before exec'ing
/// the agent so Claude Code accepts the session.
pub const SECRET_FILE_CLAUDE_SESSION: &str = "claude_session.json";

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
    /// Task #39: Claude only. Host-side path to the unsealed
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
    /// `MAESTRO_AUTH_BUNDLE=1` (a discriminator the entrypoint switches on)
    /// or `OPENCODE_PROVIDER_BASE_URL`. NEVER carries token bytes.
    pub extra_env: Vec<(String, String)>,
    /// RAII: dropping the `TempDir` `rm -rf`s the secrets directory.
    _temp_dir: TempDir,
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

    /// Phase 2b.3 (task #36): construct a stub bundle for unit tests that
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
            _temp_dir: dir,
        }
    }
}

/// Task #43: filesystem location for per-workflow secret directories.
/// Relative to the maestro `data_dir`. We deliberately don't expose this
/// as a config knob — the path is referenced by the path-translation
/// logic in `container.rs` (which swaps the data_dir prefix for the
/// DinD-side equivalent), and that translation has to stay in lockstep.
pub const SECRETS_DIR_REL: &str = "runtime/secrets";

/// Task #43: resolve the host-side directory in which per-workflow secret
/// tempdirs live, and create a fresh TempDir inside it.
///
/// Lives at `<data_dir>/runtime/secrets/<random>` when `data_dir` is
/// available; falls back to the process tempdir when it isn't (in-memory
/// test DB). The fallback path is fine for unit tests — they never bind
/// the dir into a DinD container.
fn secrets_dir_for_db(db: &crate::db::Database) -> Result<TempDir> {
    if let Some(data_dir) = db.data_dir() {
        let root = data_dir.join(SECRETS_DIR_REL);
        std::fs::create_dir_all(&root).map_err(|e| ConfigError::BundleSecretFile {
            op: "create-root",
            path: root.clone(),
            detail: e.to_string(),
        })?;
        tempfile::Builder::new()
            .prefix("bundle-")
            .tempdir_in(&root)
            .map_err(|source| ConfigError::BundleTempdir { source }.into())
    } else {
        // No data_dir → in-memory DB → unit test path. Fall back to
        // process tempdir (`/tmp/...`). Tests never reach the bind-mount
        // resolver so this is safe.
        TempDir::new().map_err(|source| ConfigError::BundleTempdir { source }.into())
    }
}

/// Task #43: best-effort startup sweep. `<data_dir>/runtime/secrets/`
/// accumulates orphan bundle dirs when maestro crashes between TempDir
/// creation and drop. Call this once at process boot — every entry is a
/// dead bundle dir from a previous run (the current run hasn't created
/// any yet). Logs at info level so operators see the cleanup happen.
pub fn cleanup_orphan_secrets(data_dir: &Path) -> std::io::Result<usize> {
    let root = data_dir.join(SECRETS_DIR_REL);
    if !root.exists() {
        return Ok(0);
    }
    let mut swept = 0_usize;
    for entry in std::fs::read_dir(&root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(&path) {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "failed to remove orphan secrets dir; will retry on next boot"
                );
                continue;
            }
            swept += 1;
        }
    }
    if swept > 0 {
        tracing::info!(
            data_dir = %data_dir.display(),
            count = swept,
            "Swept orphan WorkerSecretsBundle directories from a prior run"
        );
    }
    Ok(swept)
}

/// Build a [`WorkerSecretsBundle`] for the workflow.
///
/// Strict dependency injection — every input is passed explicitly so the
/// builder is unit-testable without an `AppState` instance.
pub async fn build(
    config: &Config,
    db: &crate::db::Database,
    resolver: &Arc<GitAuthResolver>,
    auth_pin: &AuthPin,
    workflow_user_id: &str,
) -> Result<WorkerSecretsBundle> {
    let provider = AiAgentProvider::parse(&auth_pin.provider).map_err(|e| {
        ConfigError::BundleProviderInvalid {
            detail: e.to_string(),
        }
    })?;

    let master_key = db
        .master_key()
        .ok_or(ConfigError::MasterKeyUnavailable)?
        .key
        .clone();

    // Task #43: create the host-side secrets dir under
    // `${data_dir}/runtime/secrets/` (per arch doc §3.3) rather than the
    // process `/tmp`. The maestro container shares `${data_dir}` with the
    // DinD sidecar via a docker volume, so DinD can resolve the bind-mount
    // source. `/tmp` is NOT shared — that's the bug task #43 closes.
    // When `data_dir` is None (in-memory test DB), fall back to the
    // process tempdir so unit tests still work.
    let dir = secrets_dir_for_db(db)?;

    // ── Provider secret (api_key) ────────────────────────────────────────
    let provider_secret_file = unseal_provider_credential(
        config,
        db,
        &master_key,
        auth_pin,
        workflow_user_id,
        dir.path(),
    )
    .await?;

    // ── Task #39: Claude `cli_state` row (optional, claude-only) ─────────
    // Independent of `provider_secret_file`. When present, the worker
    // `cp`s the unsealed JSON onto `$HOME/.claude.json` so Claude Code
    // sees a populated `oauthAccount` block and accepts the session.
    let claude_session_file = if matches!(provider, AiAgentProvider::Claude) {
        unseal_claude_session(db, &master_key, workflow_user_id, dir.path()).await?
    } else {
        None
    };

    // ── Provider sub-table: base_url + extra_args (non-secret) ───────────
    let base_url = match provider {
        AiAgentProvider::Claude => {
            non_empty(&config.agent.providers.claude.base_url)
        }
        AiAgentProvider::Cursor => None, // Amendment A1: Cursor has no base_url.
        AiAgentProvider::Codex => non_empty(&config.agent.providers.codex.base_url),
        AiAgentProvider::OpenCode => non_empty(&config.agent.providers.opencode.base_url),
    };
    let extra_args = match provider {
        AiAgentProvider::Claude => config.agent.providers.claude.extra_args.clone(),
        AiAgentProvider::Cursor => config.agent.providers.cursor.extra_args.clone(),
        AiAgentProvider::Codex => config.agent.providers.codex.extra_args.clone(),
        AiAgentProvider::OpenCode => config.agent.providers.opencode.extra_args.clone(),
    };

    // ── GitHub token ─────────────────────────────────────────────────────
    let (github_token_file, git_author_name, git_author_email) = match resolver
        .token_for(GitAction::Push, workflow_user_id)
        .await
    {
        Ok(tok) => {
            let path = dir.path().join(SECRET_FILE_GH);
            write_secret_file(&path, tok.bearer.expose().as_bytes())?;
            tracing::debug!(
                source = ?tok.source,
                "Wrote GitHub secret file for worker bundle"
            );
            (
                Some(path),
                tok.author_name.clone(),
                tok.author_email.clone(),
            )
        }
        Err(crate::github::auth_resolver::GitAuthError::UnauthenticatedGit { .. }) => {
            // No GitHub auth — workflow may still spawn (some workflows only
            // do AI work, no push). The agent step will fail at git push time
            // if it tries. Surface as a debug log, not an error.
            tracing::warn!(
                user_id = %workflow_user_id,
                "no GitHub credential available for worker bundle; git operations will fail at use time"
            );
            (None, None, None)
        }
        Err(e) => {
            return Err(ConfigError::Operational {
                op: "resolver token_for(Push)",
                detail: e.to_string(),
            }
            .into());
        }
    };

    // ── Non-secret env vars the worker entrypoint switches on ────────────
    let mut extra_env: Vec<(String, String)> =
        vec![("MAESTRO_AUTH_BUNDLE".to_string(), "1".to_string())];
    if let Some(ref url) = base_url {
        // Per-provider env-var name for the custom base URL. We use the
        // documented names so the provider CLIs pick them up directly.
        let env_name = match provider {
            AiAgentProvider::Claude => "ANTHROPIC_BASE_URL",
            AiAgentProvider::Codex => "OPENAI_BASE_URL",
            AiAgentProvider::OpenCode => "OPENCODE_PROVIDER_BASE_URL",
            // Cursor has no base_url (A1); the match above already filters it.
            AiAgentProvider::Cursor => "ANTHROPIC_BASE_URL",
        };
        extra_env.push((env_name.to_string(), url.clone()));
    }
    if let Some(ref name) = git_author_name {
        extra_env.push(("GIT_AUTHOR_NAME".to_string(), name.clone()));
        extra_env.push(("GIT_COMMITTER_NAME".to_string(), name.clone()));
    }
    if let Some(ref email) = git_author_email {
        extra_env.push(("GIT_AUTHOR_EMAIL".to_string(), email.clone()));
        extra_env.push(("GIT_COMMITTER_EMAIL".to_string(), email.clone()));
    }

    Ok(WorkerSecretsBundle {
        provider,
        provider_secret_file,
        claude_session_file,
        github_token_file,
        git_author_name,
        git_author_email,
        base_url,
        extra_args,
        extra_env,
        _temp_dir: dir,
    })
}

/// Task #39: unseal the user's optional `kind = cli_state` row for Claude
/// (the user's `~/.claude.json` blob) and write it to
/// `<dir>/claude_session.json` (mode 0400). Returns `None` when the row
/// doesn't exist — callers treat that as "API-key-only setup, no session
/// state needed" and don't error.
async fn unseal_claude_session(
    db: &crate::db::Database,
    master_key: &Arc<MasterKey>,
    workflow_user_id: &str,
    dir: &Path,
) -> Result<Option<PathBuf>> {
    let row = {
        let conn = db.conn().lock().await;
        provider_credentials::find_active_with_kind(
            &conn,
            workflow_user_id,
            AiAgentProvider::Claude.as_str(),
            provider_credentials::ProviderCredentialKind::CliState,
        )
        .map_err(|e| ConfigError::BundleDbLookup {
            op: "find_active_with_kind(cli_state)",
            detail: e.to_string(),
        })?
    };
    let Some(r) = row else {
        return Ok(None);
    };
    let sealed = SealedBlob {
        ciphertext: r.ciphertext,
        nonce: r.nonce,
        wrapped_dek: r.wrapped_dek,
        wnonce: r.wnonce,
    };
    let plaintext = open(master_key, &sealed).map_err(|e| ConfigError::BundleClaudeState {
        op: "open seal",
        detail: e.to_string(),
    })?;
    // Defense in depth: the validator already requires the four
    // `oauthAccount` keys at save time, but we re-validate parseable JSON
    // here so a corrupted seal doesn't deliver garbage to the worker.
    if serde_json::from_slice::<serde_json::Value>(&plaintext).is_err() {
        return Err(ConfigError::BundleClaudeState {
            op: "json validate",
            detail: "cli_state blob is not valid JSON (corrupt seal or schema drift)".to_string(),
        }
        .into());
    }
    let path = dir.join(SECRET_FILE_CLAUDE_SESSION);
    write_secret_file(&path, &plaintext)?;
    Ok(Some(path))
}

fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

async fn unseal_provider_credential(
    config: &Config,
    db: &crate::db::Database,
    master_key: &Arc<MasterKey>,
    auth_pin: &AuthPin,
    workflow_user_id: &str,
    dir: &Path,
) -> Result<Option<PathBuf>> {
    let provider = AiAgentProvider::parse(&auth_pin.provider).map_err(|e| {
        ConfigError::BundleProviderInvalid {
            detail: e.to_string(),
        }
    })?;

    // Find the row by the pinned id when set; otherwise by (user, provider).
    // Pre-Phase-2b.3 we don't yet have an `id`-keyed lookup helper, so the
    // pinned row id is informational for now — we always do the
    // (user_id, provider, kind=api_key) lookup. Task #39: switched to the
    // kind-explicit query so the lookup is unambiguous when a Claude user
    // also has a `cli_state` row (UNIQUE(user_id, provider, kind)).
    let row = {
        let conn = db.conn().lock().await;
        provider_credentials::find_active_with_kind(
            &conn,
            workflow_user_id,
            &auth_pin.provider,
            provider_credentials::ProviderCredentialKind::ApiKey,
        )
        .map_err(|e| ConfigError::BundleDbLookup {
            op: "find_active_with_kind(provider_credential)",
            detail: e.to_string(),
        })?
    };

    let plaintext = match row {
        Some(r) => {
            let sealed = SealedBlob {
                ciphertext: r.ciphertext,
                nonce: r.nonce,
                wrapped_dek: r.wrapped_dek,
                wnonce: r.wnonce,
            };
            open(master_key, &sealed).map_err(|e| ConfigError::Operational {
                op: "open(provider_credential)",
                detail: e.to_string(),
            })?
        }
        None => {
            // No per-user credential. The deployment-default fallback is
            // valid only when `allow_shared_default = true` for the active
            // provider; in that case the worker entrypoint will source the
            // ambient env var (`CLAUDE_CODE_OAUTH_TOKEN` etc.) and we write
            // no file.
            let allow_default = match provider {
                AiAgentProvider::Claude => config.agent.providers.claude.allow_shared_default,
                AiAgentProvider::Cursor => config.agent.providers.cursor.allow_shared_default,
                AiAgentProvider::Codex => config.agent.providers.codex.allow_shared_default,
                AiAgentProvider::OpenCode => config.agent.providers.opencode.allow_shared_default,
            };
            if allow_default {
                tracing::debug!(
                    provider = %provider.as_str(),
                    "no per-user credential, falling back to deployment default"
                );
                return Ok(None);
            } else {
                return Err(ConfigError::Operational {
                    op: "provider_credential_missing",
                    detail: format!(
                        "user {workflow_user_id} has no {} credential and allow_shared_default = false",
                        provider.as_str()
                    ),
                }
                .into());
            }
        }
    };

    let filename = match provider {
        AiAgentProvider::Claude => SECRET_FILE_CLAUDE,
        AiAgentProvider::Cursor => SECRET_FILE_CURSOR,
        AiAgentProvider::Codex => SECRET_FILE_CODEX,
        AiAgentProvider::OpenCode => SECRET_FILE_OPENCODE,
    };
    let path = dir.join(filename);
    write_secret_file(&path, &plaintext)?;
    Ok(Some(path))
}

/// Write a secret file with mode 0400 (owner-read-only on Unix). The parent
/// is already 0700 because `TempDir::new()` uses `OsRng` + the kernel's
/// secure-temp helpers.
#[cfg(unix)]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(path)
        .map_err(|e| ConfigError::BundleSecretFile {
            op: "create",
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    f.write_all(bytes).map_err(|e| ConfigError::BundleSecretFile {
        op: "write",
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    f.sync_all().ok();
    Ok(())
}

#[cfg(not(unix))]
fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| ConfigError::BundleSecretFile {
            op: "create",
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    f.write_all(bytes).map_err(|e| ConfigError::BundleSecretFile {
        op: "write",
        path: path.to_path_buf(),
        detail: e.to_string(),
    })?;
    f.sync_all().ok();
    Ok(())
}

// ---------------------------------------------------------------------------
// auth_pin helpers: capture a pin at workflow start
// ---------------------------------------------------------------------------

/// Phase 2b.3.x — build a [`WorkerSecretsBundle`] from the user's **current**
/// credentials (no workflow / no pin involved). Used by the ephemeral
/// runners that aren't part of a workflow auth-pin chain:
///
/// - `improve_ticket` / `prompt_ticket` (one-shot Improve / Ask AI sessions)
/// - `open_editor` (browser VS Code container, when the workflow has no pin)
/// - `start_run_command` (dev-server / preview containers)
///
/// Reads the active provider from `[agent].provider`, looks up the user's
/// active credential row for that provider, and builds the bundle the same
/// way [`build`] does — same RAII `TempDir`, same secret-file layout, same
/// `extra_env` discriminator.
pub async fn build_for_endpoint(
    config: &Config,
    db: &crate::db::Database,
    resolver: &Arc<GitAuthResolver>,
    user_id: &str,
) -> Result<WorkerSecretsBundle> {
    // Synthesise a pin from the current config + DB. We don't write it
    // anywhere; it's only used by `build`'s row-lookup logic.
    let ephemeral_pin = AuthPin {
        provider: config.agent.provider.as_str().to_string(),
        provider_credential_row_id: None,
        github_mode: "unknown".to_string(),
        github_credential_row_id: None,
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    build(config, db, resolver, &ephemeral_pin, user_id).await
}

/// Build an [`AuthPin`] from the current state of the DB / config. Called
/// once at the workflow's first agent step.
pub async fn pin_for_workflow(
    config: &Config,
    db: &crate::db::Database,
    workflow_user_id: &str,
) -> Result<AuthPin> {
    let provider = config.agent.provider.as_str().to_string();

    let (provider_credential_row_id, github_credential_row_id, github_mode) = {
        let conn = db.conn().lock().await;
        let p = provider_credentials::find_active(&conn, workflow_user_id, &provider).map_err(
            |e| ConfigError::BundleDbLookup {
                op: "provider_credentials::find_active",
                detail: e.to_string(),
            },
        )?;
        let g = github_credentials::find(&conn, workflow_user_id).map_err(|e| {
            ConfigError::BundleDbLookup {
                op: "github_credentials::find",
                detail: e.to_string(),
            }
        })?;
        let github_mode = if g.is_some() {
            TokenSource::UserPat.as_str().to_string()
        } else {
            TokenSource::App.as_str().to_string()
        };
        (p.map(|r| r.id), g.map(|_| 0_i64), github_mode)
    };

    Ok(AuthPin {
        provider,
        provider_credential_row_id,
        github_mode,
        github_credential_row_id,
        started_at: chrono::Utc::now().to_rfc3339(),
    })
}

// ---------------------------------------------------------------------------
// Tests — unit tests cover the happy + error paths
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{seal, MasterKey};
    use crate::config::AiAgentProvider;
    use crate::db::Database;

    fn fixed_config(provider: AiAgentProvider) -> Config {
        let mut cfg = Config::default();
        cfg.agent.provider = provider;
        cfg
    }

    fn fixed_pin(provider: AiAgentProvider) -> AuthPin {
        AuthPin {
            provider: provider.as_str().to_string(),
            provider_credential_row_id: None,
            github_mode: "app".to_string(),
            github_credential_row_id: None,
            started_at: "2026-05-18T00:00:00Z".to_string(),
        }
    }

    fn db_with_master_key() -> Database {
        Database::open_in_memory()
            .unwrap()
            .with_test_master_key(MasterKey::from_bytes([0x42; 32]))
    }

    async fn seed_user(db: &Database, user_id: &str) {
        let conn = db.conn().lock().await;
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES (?1, ?2, 'user')",
            rusqlite::params![user_id, user_id],
        )
        .unwrap();
    }

    async fn seed_provider_credential(
        db: &Database,
        user_id: &str,
        provider: &str,
        plaintext: &[u8],
    ) {
        let mk = db.master_key().unwrap().key.clone();
        let sealed = seal(&mk, plaintext).unwrap();
        let conn = db.conn().lock().await;
        provider_credentials::upsert(
            &conn,
            user_id,
            provider,
            provider_credentials::ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .unwrap();
    }

    fn make_resolver(db: Database) -> Arc<GitAuthResolver> {
        Arc::new(GitAuthResolver::new(db, None))
    }

    /// Task #39: seed a cli_state row carrying a minimal valid Claude
    /// session blob. Uses the test DB's master key so `seal()`/`open()`
    /// round-trip cleanly.
    async fn seed_claude_cli_state(db: &Database, user_id: &str, json: &[u8]) {
        let mk = db.master_key().unwrap().key.clone();
        let sealed = seal(&mk, json).unwrap();
        let conn = db.conn().lock().await;
        provider_credentials::upsert(
            &conn,
            user_id,
            "claude",
            provider_credentials::ProviderCredentialKind::CliState,
            &sealed,
            r#"{"kind":"cli_state"}"#,
        )
        .unwrap();
    }

    #[tokio::test]
    async fn build_writes_provider_secret_file_when_credential_present() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant-test-token").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let secret_path = bundle
            .provider_secret_file
            .as_ref()
            .expect("provider secret file");
        let bytes = std::fs::read(secret_path).expect("read secret file");
        assert_eq!(bytes, b"sk-ant-test-token");

        // host_dir is the parent of the secret file.
        assert!(secret_path.starts_with(bundle.host_dir()));
    }

    #[tokio::test]
    async fn build_returns_none_secret_when_no_credential_and_default_allowed() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let mut cfg = fixed_config(AiAgentProvider::Claude);
        cfg.agent.providers.claude.allow_shared_default = true;
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build with shared default fallback");
        assert!(bundle.provider_secret_file.is_none());
    }

    #[tokio::test]
    async fn build_errors_when_no_credential_and_default_disallowed() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude); // allow_shared_default defaults to false
        let pin = fixed_pin(AiAgentProvider::Claude);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("no credential + no fallback must error");
        assert!(err.to_string().contains("provider_credential_missing"));
    }

    #[tokio::test]
    async fn build_errors_when_master_key_unavailable() {
        let db = Database::open_in_memory().unwrap();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("must error when master key not loaded");
        assert!(err.to_string().contains("master_key_unavailable"));
    }

    #[tokio::test]
    async fn temp_dir_cleanup_removes_secret_files_on_drop() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .unwrap();
        let secret_path = bundle.provider_secret_file.clone().unwrap();
        assert!(secret_path.exists());

        // Drop the bundle; RAII should remove the directory.
        drop(bundle);
        assert!(
            !secret_path.exists(),
            "secret file must be cleaned up when WorkerSecretsBundle drops"
        );
    }

    #[tokio::test]
    async fn build_emits_anthropic_base_url_env_for_claude() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let mut cfg = fixed_config(AiAgentProvider::Claude);
        cfg.agent.providers.claude.base_url = "https://proxy.example.com".into();
        cfg.agent.providers.claude.extra_args =
            vec!["--max-turns".into(), "50".into()];
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .unwrap();
        assert!(
            bundle
                .extra_env
                .iter()
                .any(|(k, v)| k == "ANTHROPIC_BASE_URL"
                    && v == "https://proxy.example.com")
        );
        assert_eq!(bundle.extra_args, vec!["--max-turns", "50"]);
        // MAESTRO_AUTH_BUNDLE is the worker entrypoint's discriminator.
        assert!(
            bundle
                .extra_env
                .iter()
                .any(|(k, v)| k == "MAESTRO_AUTH_BUNDLE" && v == "1")
        );
    }

    #[tokio::test]
    async fn pin_for_workflow_captures_provider_and_github_mode() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let cfg = fixed_config(AiAgentProvider::Claude);

        let pin = pin_for_workflow(&cfg, &db, "u-alice")
            .await
            .expect("pin");
        assert_eq!(pin.provider, "claude");
        assert!(pin.provider_credential_row_id.is_some());
        assert_eq!(pin.github_mode, "app"); // No GitHub PAT seeded.
        assert!(!pin.started_at.is_empty());
    }

    #[tokio::test]
    async fn pin_for_workflow_with_no_credential_returns_none_id() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let cfg = fixed_config(AiAgentProvider::Claude);

        let pin = pin_for_workflow(&cfg, &db, "u-alice").await.unwrap();
        assert_eq!(pin.provider, "claude");
        assert!(pin.provider_credential_row_id.is_none());
    }

    // ─── build_for_endpoint (Phase 2b.3.x) ────────────────────────────────
    //
    // The endpoint-side wrapper synthesizes an ephemeral pin internally.
    // It must behave identically to `build` for credential lookup, but
    // requires no caller-supplied pin (so improve_ticket / open_editor
    // / start_run_command can be wired without first computing a pin).

    #[tokio::test]
    async fn build_for_endpoint_returns_bundle_when_credential_present() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant-endpoint").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);

        let bundle = build_for_endpoint(&cfg, &db, &resolver, "u-alice")
            .await
            .expect("endpoint bundle build");
        let secret_path = bundle
            .provider_secret_file
            .as_ref()
            .expect("provider secret file");
        let bytes = std::fs::read(secret_path).expect("read secret file");
        assert_eq!(bytes, b"sk-ant-endpoint");
    }

    #[tokio::test]
    async fn build_for_endpoint_surfaces_credential_required_for_no_cred_and_no_default() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        // Default config has `allow_shared_default = false` for every
        // provider — so a user with no credential MUST surface the
        // structured `provider_credential_missing` error so the dashboard
        // can prompt them to paste an API key.
        let cfg = fixed_config(AiAgentProvider::Claude);

        let err = build_for_endpoint(&cfg, &db, &resolver, "u-alice")
            .await
            .expect_err("must error when caller has no credential");
        assert!(err.to_string().contains("provider_credential_missing"));
    }

    /// Phase 2b.3.x: `apply_secrets_bundle_to_args` (defined in
    /// `container.rs`) must:
    ///   1. Bind-mount the bundle's host_dir RO at /run/maestro-secrets, AND
    ///   2. Copy every `extra_env` pair as `-e KEY=VALUE`, AND
    ///   3. NEVER write secret bytes into the argv (those live in tmpfs).
    ///
    /// We exercise the helper from bundle.rs because `WorkerSecretsBundle`'s
    /// `_temp_dir` field is private to this module, so the by-hand
    /// constructor is only reachable here.
    #[test]
    fn apply_secrets_bundle_to_args_mounts_ro_and_copies_extra_env_only() {
        use std::path::Path;
        let dir = TempDir::new().unwrap();
        let host_dir_path = dir.path().to_path_buf();
        let bundle = WorkerSecretsBundle {
            provider: AiAgentProvider::Claude,
            provider_secret_file: Some(host_dir_path.join("claude")),
            claude_session_file: None,
            github_token_file: Some(host_dir_path.join("gh")),
            git_author_name: Some("alice".into()),
            git_author_email: Some("alice@noreply".into()),
            base_url: Some("https://proxy.example".into()),
            extra_args: vec![],
            extra_env: vec![
                ("MAESTRO_AUTH_BUNDLE".into(), "1".into()),
                ("ANTHROPIC_BASE_URL".into(), "https://proxy.example".into()),
                ("GIT_AUTHOR_NAME".into(), "alice".into()),
            ],
            _temp_dir: dir,
        };

        let mut args: Vec<String> = Vec::new();
        crate::container::apply_secrets_bundle_to_args(&mut args, &bundle);

        // The mount must be RO and point at the bundle's host_dir.
        let mount_expected = format!(
            "{}:/run/maestro-secrets:ro",
            Path::new(&host_dir_path).to_string_lossy()
        );
        let has_volume = args
            .windows(2)
            .any(|w| w[0] == "-v" && w[1] == mount_expected);
        assert!(
            has_volume,
            "expected RO mount {mount_expected:?} in args = {args:?}"
        );

        // All extra_env entries must be present as -e KEY=VALUE.
        let has_env = |k: &str, v: &str| -> bool {
            let needle = format!("{k}={v}");
            args.windows(2).any(|w| w[0] == "-e" && w[1] == needle)
        };
        assert!(has_env("MAESTRO_AUTH_BUNDLE", "1"));
        assert!(has_env("ANTHROPIC_BASE_URL", "https://proxy.example"));
        assert!(has_env("GIT_AUTHOR_NAME", "alice"));

        // CRITICAL: argv must NOT carry the bundled secret env names —
        // those flow through tmpfs files only. Token bytes never appear
        // in argv at all because the bundle exposes only file paths,
        // not byte slices.
        let argv_joined = args.join(" ");
        assert!(
            !argv_joined.contains("CLAUDE_CODE_OAUTH_TOKEN"),
            "secret env name must not appear in argv"
        );
        assert!(
            !argv_joined.contains("CURSOR_API_KEY"),
            "secret env name must not appear in argv"
        );
        assert!(
            !argv_joined.contains("GH_TOKEN="),
            "secret env name must not appear in argv"
        );
    }

    #[test]
    fn debug_does_not_leak_token_bytes() {
        // Build a stub bundle by hand so we can inspect Debug without going
        // through the async builder.
        let dir = TempDir::new().unwrap();
        let bundle = WorkerSecretsBundle {
            provider: AiAgentProvider::Claude,
            provider_secret_file: Some(dir.path().join("claude")),
            claude_session_file: None,
            github_token_file: Some(dir.path().join("gh")),
            git_author_name: Some("alice".into()),
            git_author_email: Some("alice@noreply".into()),
            base_url: Some("https://proxy".into()),
            extra_args: vec![],
            extra_env: vec![("MAESTRO_AUTH_BUNDLE".into(), "1".into())],
            _temp_dir: dir,
        };
        let s = format!("{bundle:?}");
        // Paths can appear; token bytes cannot — they're never set as fields.
        assert!(s.contains("WorkerSecretsBundle"));
        assert!(s.contains("has_provider_secret"));
        assert!(s.contains("has_claude_session"));
        assert!(s.contains("has_github_token"));
    }

    // ─── Task #39: claude cli_state in bundle ────────────────────────────

    /// Minimal valid Claude session blob — three required oauthAccount
    /// keys + a couple of harmless extras the validator must ignore.
    fn fixture_session_json() -> Vec<u8> {
        serde_json::json!({
            "oauthAccount": {
                "accountUuid": "00000000-0000-0000-0000-000000000001",
                "emailAddress": "alice@example.com",
                "organizationUuid": "11111111-1111-1111-1111-111111111111",
                "organizationType": "claude_team",
                "seatTier": "team_standard",
            },
            "lastUpdateCheck": "2026-05-19T00:00:00Z",
        })
        .to_string()
        .into_bytes()
    }

    /// Happy path: both api_key AND cli_state rows present → bundle has
    /// BOTH files populated and the session file contents round-trip.
    #[tokio::test]
    async fn build_writes_claude_session_file_when_cli_state_row_present() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let session_blob = fixture_session_json();
        seed_claude_cli_state(&db, "u-alice", &session_blob).await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let session_path = bundle
            .claude_session_file
            .as_ref()
            .expect("claude_session_file must be Some when cli_state row exists");
        let on_disk = std::fs::read(session_path).expect("read session file");
        assert_eq!(on_disk, session_blob);
        // The mount filename must match the documented constant so
        // BUNDLE_SOURCING_SH's `cp` reads it.
        assert!(
            session_path.ends_with(SECRET_FILE_CLAUDE_SESSION),
            "session file must use the SECRET_FILE_CLAUDE_SESSION name; got {session_path:?}"
        );
        // API key file is also present (the user has both rows).
        assert!(bundle.provider_secret_file.is_some());
    }

    /// No cli_state row → `claude_session_file` is None (api_key path
    /// unchanged). Saves succeed without it; this is the most common
    /// case for direct-API users.
    #[tokio::test]
    async fn build_omits_session_file_when_no_cli_state_row() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        assert!(
            bundle.claude_session_file.is_none(),
            "claude_session_file must be None when no cli_state row exists"
        );
    }

    /// Non-Claude provider with a (somehow) seeded cli_state row → bundle
    /// MUST NOT write the session file (the unseal helper is gated on
    /// `provider == Claude`). Defence-in-depth.
    #[tokio::test]
    async fn build_does_not_emit_session_file_for_non_claude_provider() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "cursor", b"sk-curs").await;
        // Sneak a claude cli_state row in (UI rejects this at POST time,
        // but the bundle builder must not key off non-active providers).
        seed_claude_cli_state(&db, "u-alice", &fixture_session_json()).await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Cursor);
        let pin = fixed_pin(AiAgentProvider::Cursor);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        assert!(
            bundle.claude_session_file.is_none(),
            "non-Claude bundle must NOT include claude_session_file even when \
             a rogue cli_state row exists in the DB"
        );
    }

    /// Corrupt cli_state blob (valid bytes through `seal`, but the
    /// plaintext isn't valid JSON) → typed error at unseal time. Defence
    /// in depth — the validator catches this at save time, but the
    /// bundle builder must not silently ship garbage to the worker.
    #[tokio::test]
    async fn build_errors_when_cli_state_blob_is_not_json() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        // Skip the validator: write garbage bytes via the seed helper.
        seed_claude_cli_state(&db, "u-alice", b"this is not json {[").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("invalid JSON must surface a typed error");
        assert!(
            err.to_string().contains("cli_state blob is not valid JSON"),
            "error must explain the cli_state JSON problem; got: {err}"
        );
    }

    // ─── Task #43: data_dir-based secrets dir + cleanup ────────────────

    /// Build a real Database backed by a temp data_dir (not in-memory) so
    /// `data_dir()` returns a real path the bundle can sit under.
    fn db_with_master_key_and_disk_data_dir() -> (Database, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("disk-backed tempdir");
        let db = Database::open(dir.path(), true).expect("open disk DB")
            .with_test_master_key(MasterKey::from_bytes([0xAA; 32]));
        (db, dir)
    }

    /// When a disk-backed `data_dir` is available, the bundle's TempDir
    /// is created under `<data_dir>/runtime/secrets/<random>` — NOT
    /// under the process `/tmp` (the task #43 bug).
    #[tokio::test]
    async fn bundle_temp_dir_is_under_data_dir_runtime_secrets() {
        let (db, data_dir_keepalive) = db_with_master_key_and_disk_data_dir();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let host_dir = bundle.host_dir().to_path_buf();
        let expected_root = data_dir_keepalive.path().join(SECRETS_DIR_REL);
        assert!(
            host_dir.starts_with(&expected_root),
            "bundle's host_dir must live under {} (got: {})",
            expected_root.display(),
            host_dir.display()
        );
        // The dir is a real tempfile-style random child of secrets/.
        assert!(host_dir.is_dir());
    }

    /// `cleanup_orphan_secrets` is best-effort: missing dir → Ok(0).
    #[test]
    fn cleanup_orphan_secrets_returns_zero_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does-not-exist");
        let n = cleanup_orphan_secrets(&nonexistent).expect("must not error");
        assert_eq!(n, 0);
    }

    /// Pre-seed `<data_dir>/runtime/secrets/` with two fake orphan
    /// directories and a stray file; the sweep removes both dirs and
    /// leaves the file alone (real-world it'd be empty anyway).
    #[test]
    fn cleanup_orphan_secrets_removes_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(SECRETS_DIR_REL);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("orphan-a")).unwrap();
        std::fs::write(root.join("orphan-a/claude"), "stale-token").unwrap();
        std::fs::create_dir_all(root.join("orphan-b")).unwrap();
        // A stray file at the same depth — sweep skips files (its `is_dir()`
        // guard) so callers don't accidentally lose metadata.
        std::fs::write(root.join("stray-file"), "metadata").unwrap();

        let n = cleanup_orphan_secrets(dir.path()).expect("sweep ok");
        assert_eq!(n, 2, "must remove both orphan dirs");
        assert!(!root.join("orphan-a").exists());
        assert!(!root.join("orphan-b").exists());
        assert!(root.join("stray-file").exists(), "files outside dir entries survive");
    }

    /// Task #42: Arc-storage strategy proof. The route handlers stash an
    /// `Arc<WorkerSecretsBundle>` clone in AppState so the bundle's
    /// `TempDir` outlives the route's stack scope. This test asserts the
    /// expected lifetime semantics: cloning the Arc does NOT trigger
    /// cleanup, only dropping the LAST clone does. If anyone refactors
    /// the bundle storage to a non-Arc shape (e.g. `Box<Bundle>`),
    /// this test fails loudly.
    #[test]
    fn bundle_temp_dir_survives_clone_drop_until_last_arc_released() {
        let bundle = Arc::new(WorkerSecretsBundle::for_tests(
            AiAgentProvider::Claude,
            vec![("MAESTRO_AUTH_BUNDLE".into(), "1".into())],
        ));
        let host_dir = bundle.host_dir().to_path_buf();
        assert!(
            host_dir.exists(),
            "bundle's TempDir must exist immediately after construction"
        );

        // Clone the Arc twice — simulates AppState stashing one clone
        // and the route handler passing a second to the container runner.
        let arc1 = bundle.clone();
        let arc2 = bundle.clone();
        // Drop the original — TempDir must still survive.
        drop(bundle);
        assert!(
            host_dir.exists(),
            "TempDir must survive while clones are held"
        );
        // Drop arc1 — still one clone alive.
        drop(arc1);
        assert!(
            host_dir.exists(),
            "TempDir must survive while at least one Arc clone exists"
        );
        // Drop the last clone — RAII fires now.
        drop(arc2);
        assert!(
            !host_dir.exists(),
            "TempDir must be rm-rf'd when the last Arc clone drops"
        );
    }
}
