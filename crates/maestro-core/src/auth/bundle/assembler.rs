// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Glue: orchestrate tempdir creation, secret unsealing, GitHub-token
//! materialisation, and non-secret env-var assembly into a finished
//! [`WorkerSecretsBundle`]. Also: capture an [`AuthPin`] at workflow start
//! and a no-pin helper for ephemeral runners.

use std::sync::Arc;

use crate::config::{AiAgentProvider, Config, ConfigError};
use crate::db::github_credentials;
use crate::db::provider_credentials;
use crate::error::Result;
use crate::github::auth_resolver::{GitAction, GitAuthResolver, TokenSource};
use crate::workflow::snapshot::AuthPin;

use super::tempdir::secrets_dir_for_db;
use super::types::{SECRET_FILE_GH, WorkerSecretsBundle};
use super::unseal::{non_empty, unseal_claude_session, unseal_provider_credential};
use super::write_secret::write_secret_file;

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
