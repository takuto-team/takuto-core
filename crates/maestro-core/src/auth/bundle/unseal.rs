// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Open the user's sealed provider credential + (claude-only) sealed
//! cli_state row, validate, and write each to its mode-0400 secret file.
//!
//! Returns `Ok(Some(path))` when a file was written, `Ok(None)` when the
//! deployment-default fallback is allowed, and `Err` when the user has no
//! credential AND the fallback is forbidden.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::auth::{open, MasterKey, SealedBlob};
use crate::config::{AiAgentProvider, Config, ConfigError};
use crate::db::provider_credentials;
use crate::error::Result;
use crate::workflow::snapshot::AuthPin;

use super::types::{
    SECRET_FILE_CLAUDE, SECRET_FILE_CLAUDE_SESSION, SECRET_FILE_CODEX, SECRET_FILE_CURSOR,
    SECRET_FILE_OPENCODE,
};
use super::write_secret::write_secret_file;

/// Unseal the user's optional `kind = cli_state` row for Claude (the
/// user's `~/.claude.json` blob) and write it to
/// `<dir>/claude_session.json` (mode 0400). Returns `None` when the row
/// doesn't exist — callers treat that as "API-key-only setup, no session
/// state needed" and don't error.
pub(super) async fn unseal_claude_session(
    db: &crate::db::Database,
    master_key: &Arc<MasterKey>,
    workflow_user_id: &str,
    dir: &Path,
) -> Result<Option<PathBuf>> {
    // provider_credentials uses the agnostic adapter; no rusqlite
    // MutexGuard needed.
    let row = provider_credentials::find_active_with_kind(
        db.adapter(),
        workflow_user_id,
        AiAgentProvider::Claude.as_str(),
        provider_credentials::ProviderCredentialKind::CliState,
    )
    .await
    .map_err(|e| ConfigError::BundleDbLookup {
        op: "find_active_with_kind(cli_state)",
        detail: e.to_string(),
    })?;
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

pub(super) fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// OpenCode self-hosted spec (2026-05-27 §2.3): unseal the user's
/// `kind = api_key` row for the active provider and return the raw
/// plaintext bytes — no on-disk secret file. The caller embeds the
/// bytes inside the OpenCode init-shim's `opencode.json` instead of
/// sourcing a `/run/maestro-secrets/<provider>` env file (which the
/// OpenCode CLI does not consume).
///
/// Returns `Ok(None)` when the user has no credential AND the active
/// provider allows the deployment-default fallback; `Err` when the user
/// has no credential AND the fallback is forbidden. Identical
/// semantics to [`unseal_provider_credential`] except the byte path.
pub(super) async fn unseal_provider_plaintext_bytes(
    config: &Config,
    db: &crate::db::Database,
    master_key: &Arc<MasterKey>,
    auth_pin: &AuthPin,
    workflow_user_id: &str,
) -> Result<Option<Vec<u8>>> {
    let provider = AiAgentProvider::parse(&auth_pin.provider).map_err(|e| {
        ConfigError::BundleProviderInvalid {
            detail: e.to_string(),
        }
    })?;
    let row = provider_credentials::find_active_with_kind(
        db.adapter(),
        workflow_user_id,
        &auth_pin.provider,
        provider_credentials::ProviderCredentialKind::ApiKey,
    )
    .await
    .map_err(|e| ConfigError::BundleDbLookup {
        op: "find_active_with_kind(provider_credential)",
        detail: e.to_string(),
    })?;
    match row {
        Some(r) => {
            let sealed = SealedBlob {
                ciphertext: r.ciphertext,
                nonce: r.nonce,
                wrapped_dek: r.wrapped_dek,
                wnonce: r.wnonce,
            };
            let plaintext = open(master_key, &sealed).map_err(|e| ConfigError::Operational {
                op: "open(provider_credential)",
                detail: e.to_string(),
            })?;
            Ok(Some(plaintext))
        }
        None => {
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
                Ok(None)
            } else {
                Err(ConfigError::Operational {
                    op: "provider_credential_missing",
                    detail: format!(
                        "user {workflow_user_id} has no {} credential and allow_shared_default = false",
                        provider.as_str()
                    ),
                }
                .into())
            }
        }
    }
}

pub(super) async fn unseal_provider_credential(
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
    // We don't yet have an `id`-keyed lookup helper, so the pinned row id
    // is informational for now — we always do the (user_id, provider,
    // kind=api_key) lookup. The kind-explicit query keeps the lookup
    // unambiguous when a Claude user also has a `cli_state` row
    // (UNIQUE(user_id, provider, kind)).
    let row = provider_credentials::find_active_with_kind(
        db.adapter(),
        workflow_user_id,
        &auth_pin.provider,
        provider_credentials::ProviderCredentialKind::ApiKey,
    )
    .await
    .map_err(|e| ConfigError::BundleDbLookup {
        op: "find_active_with_kind(provider_credential)",
        detail: e.to_string(),
    })?;

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
