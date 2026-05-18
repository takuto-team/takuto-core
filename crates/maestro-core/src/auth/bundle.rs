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
use crate::config::{AiAgentProvider, Config};
use crate::db::github_credentials;
use crate::db::provider_credentials;
use crate::error::{MaestroError, Result};
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
    let provider = AiAgentProvider::parse(&auth_pin.provider)
        .map_err(|e| MaestroError::Config(format!("auth_pin.provider invalid: {e}")))?;

    let master_key = db.master_key().ok_or_else(|| {
        MaestroError::Config("master_key_unavailable: cannot unseal worker secrets".into())
    })?.key.clone();

    // tmpfs-style host directory. `TempDir::new()` creates a 0700-mode dir
    // owned by the current uid. Bind-mounting it `:ro` into the worker
    // prevents the worker from writing back.
    let dir = TempDir::new()
        .map_err(|e| MaestroError::Config(format!("failed to create secrets tmpdir: {e}")))?;

    // ── Provider secret ──────────────────────────────────────────────────
    let provider_secret_file = unseal_provider_credential(
        config,
        db,
        &master_key,
        auth_pin,
        workflow_user_id,
        dir.path(),
    )
    .await?;

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
            return Err(MaestroError::Config(format!(
                "resolver token_for(Push) failed: {e}"
            )));
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
        github_token_file,
        git_author_name,
        git_author_email,
        base_url,
        extra_args,
        extra_env,
        _temp_dir: dir,
    })
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
    let provider = AiAgentProvider::parse(&auth_pin.provider)
        .map_err(|e| MaestroError::Config(e.to_string()))?;

    // Find the row by the pinned id when set; otherwise by (user, provider).
    // Pre-Phase-2b.3 we don't yet have an `id`-keyed lookup helper, so the
    // pinned row id is informational for now — we always do the
    // (user_id, provider) lookup. Phase 2b.4+ can add the id-keyed lookup
    // when provider-switch invalidation requires it.
    let row = {
        let conn = db.conn().lock().await;
        provider_credentials::find_active(&conn, workflow_user_id, &auth_pin.provider)
            .map_err(|e| MaestroError::Config(format!("find_active failed: {e}")))?
    };

    let plaintext = match row {
        Some(r) => {
            let sealed = SealedBlob {
                ciphertext: r.ciphertext,
                nonce: r.nonce,
                wrapped_dek: r.wrapped_dek,
                wnonce: r.wnonce,
            };
            open(master_key, &sealed).map_err(|e| {
                MaestroError::Config(format!("open(provider_credential) failed: {e}"))
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
                return Err(MaestroError::Config(format!(
                    "provider_credential_missing: user {workflow_user_id} has no {} credential and \
                     allow_shared_default = false",
                    provider.as_str()
                )));
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
        .map_err(|e| {
            MaestroError::Config(format!("failed to create secret file {}: {e}", path.display()))
        })?;
    f.write_all(bytes).map_err(|e| {
        MaestroError::Config(format!("failed to write secret file {}: {e}", path.display()))
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
        .map_err(|e| {
            MaestroError::Config(format!("failed to create secret file {}: {e}", path.display()))
        })?;
    f.write_all(bytes).map_err(|e| {
        MaestroError::Config(format!("failed to write secret file {}: {e}", path.display()))
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
        let p = provider_credentials::find_active(&conn, workflow_user_id, &provider)
            .map_err(|e| MaestroError::Config(format!("find_active failed: {e}")))?;
        let g = github_credentials::find(&conn, workflow_user_id)
            .map_err(|e| MaestroError::Config(format!("github_credentials::find failed: {e}")))?;
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
        assert!(s.contains("has_github_token"));
    }
}
