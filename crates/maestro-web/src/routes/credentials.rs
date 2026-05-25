// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 2b.1 per-user credential endpoints. Source of truth:
//! tmp/multi-agents/04_architecture.md §3 (storage) + §4 (GitHub PAT).
//!
//! Endpoints (all `api_protected` — require a valid session cookie):
//!
//! - `GET    /api/users/me/credentials`            — combined provider+github status
//! - `POST   /api/users/me/credentials/{provider}` — paste/rotate an AI-provider api_key
//! - `DELETE /api/users/me/credentials/{provider}` — wipe an AI-provider credential
//! - `POST   /api/users/me/github-pat`             — validate + seal + store a GitHub PAT
//! - `DELETE /api/users/me/github-pat`             — wipe the user's PAT
//! - `PATCH  /api/users/me/github`                 — toggle `attribute_commits`
//! - `GET    /api/admin/users/{id}/github-status`  — admin-only read of peer's PAT presence
//!
//! Hard rules upheld by every handler:
//! - Sealed bytes (`ciphertext`, `nonce`, `wrapped_dek`, `wnonce`) and raw
//!   tokens are **never** included in any response.
//! - Every mutation writes a `credential_audit` row inside the same SQLite
//!   transaction as the credential write itself.
//! - Write endpoints return `503 master_key_unavailable` when the master key
//!   resolution failed at boot (degraded mode).
//!
//! Wire-vs-column rename — the JSON body uses `attribute_commits` (A3 — git
//! author/committer attribution; NOT GPG/SSH signing) but the SQLite column
//! is `sign_commits`. The boundary is bridged exactly once via
//! `#[serde(rename = "attribute_commits")]` on [`GithubPatBody::attribute_commits`]
//! and [`PatchGithubBody::attribute_commits`] / [`AdminGithubStatusResponse::attribute_commits`].
//! Everywhere else, the code uses the column name.

use axum::Json;
use axum::extract::{Extension, Path, Query, State};
use axum::http::StatusCode;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use maestro_core::auth::{PatValidationError, SealedBlob, seal, validate_pat};
use maestro_core::config::AiAgentProvider;
use maestro_core::db::credential_audit::{self, CredentialAuditKind};
use maestro_core::db::github_credentials;
use maestro_core::db::provider_credentials::{self, ProviderCredentialKind, UpsertOutcome};

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::state::{AuthState, ConfigState};

/// API key length cap. PATs / OAuth tokens / API keys are all comfortably
/// under 4 KiB; anything bigger is almost certainly a paste mistake (or a
/// gzipped blob someone is trying to smuggle past the validator).
const MAX_API_KEY_LEN: usize = 4096;

/// Task #41: cap for `~/.claude.json` blobs accepted via the `kind=cli_state`
/// flow. Bumped from 64 KiB (the original #39 cap) to **1 MiB** because real
/// `.claude.json` files accumulate `tipsHistory`, `cachedGrowthBookFeatures`,
/// startup-counter state, etc. over time and routinely exceed 64 KiB. The
/// envelope-encryption layer handles any size (no AEAD ceiling); SQLite's
/// BLOB column is unlimited. 1 MiB stays a sane upper bound — anything
/// bigger is almost certainly a paste mistake (file path, dump, …).
const MAX_CLAUDE_SESSION_JSON_LEN: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Response shapes
// ---------------------------------------------------------------------------

/// Returned by `GET /api/users/me/credentials`.
///
/// Task #39: with Claude `kind=cli_state` shipping, one user can have BOTH
/// an `api_key` row AND a `cli_state` row for the same provider. The
/// response now carries a [`ProviderCredentialBundle`] (api_key +
/// cli_state slots) instead of a single status — back-compat is preserved
/// because old clients reading `provider.kind` / `provider.active` can
/// still read those nested under `provider.api_key.*`.
#[derive(Debug, Serialize)]
pub struct UserCredentialsStatus {
    /// `None` when the user has no row at all for the active provider.
    /// `Some` even when only one of (`api_key`, `cli_state`) is set — both
    /// fields inside the bundle are optional independently.
    pub provider: Option<ProviderCredentialBundle>,
    pub github: Option<GithubCredentialStatus>,
}

/// Task #39: per-provider credential bundle. One slot per `kind`. UI uses
/// `provider.api_key.is_some()` and `provider.cli_state.is_some()` to
/// render the two-pill state independently.
#[derive(Debug, Serialize)]
pub struct ProviderCredentialBundle {
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<ProviderCredentialStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli_state: Option<ProviderCredentialStatus>,
}

#[derive(Debug, Serialize)]
pub struct ProviderCredentialStatus {
    pub provider: String,
    pub kind: String,
    /// True when the row is currently active. Inactive rows survive provider
    /// switches for audit/restore (04_architecture.md §2.4).
    pub active: bool,
    pub last_validated_at: Option<String>,
    pub last_used_at: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct GithubCredentialStatus {
    pub login: String,
    pub scopes: Vec<String>,
    /// Wire name — see file-level docs for the column-rename note.
    #[serde(rename = "attribute_commits")]
    pub sign_commits: bool,
    pub last_validated_at: Option<String>,
}

/// `GET /api/admin/users/{id}/github-status` — never returns tokens.
#[derive(Debug, Serialize)]
pub struct AdminGithubStatusResponse {
    pub has_pat: bool,
    pub login: Option<String>,
    pub scopes: Vec<String>,
    /// Wire name — see file-level docs for the column-rename note.
    #[serde(rename = "attribute_commits")]
    pub sign_commits: bool,
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiKeyBody {
    /// Bearer / API key string. Required when `kind` is `api_key` (default).
    /// Forbidden when `kind = cli_state`.
    #[serde(default)]
    pub api_key: Option<String>,
    /// Task #39: Claude `~/.claude.json` blob (full JSON string).
    /// Required when `kind = cli_state`. Forbidden otherwise.
    #[serde(default)]
    pub claude_session_json: Option<String>,
    /// Discriminator. `None` defaults to `"api_key"` for back-compat with
    /// pre-task-#39 clients that only ever wrote bearer keys.
    #[serde(default)]
    pub kind: Option<String>,
}

/// Task #39: `DELETE /api/users/me/credentials/{provider}?kind=cli_state`
/// query string. `kind = None` (omitted) deletes EVERY row for
/// `(user, provider)` (back-compat).
#[derive(Debug, Deserialize, Default)]
pub struct DeleteProviderCredentialQuery {
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GithubPatBody {
    pub pat: String,
    /// Wire name — bridges to the `sign_commits` SQLite column per A3.
    /// `None` defaults to `true` (commit attribution ON).
    #[serde(default, rename = "attribute_commits")]
    pub sign_commits: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PatchGithubBody {
    /// Wire name — bridges to the `sign_commits` SQLite column per A3.
    #[serde(rename = "attribute_commits")]
    pub sign_commits: bool,
}

// ---------------------------------------------------------------------------
// Common helpers
// ---------------------------------------------------------------------------

/// Map any 4xx-style error body into `{ "error": "<code>" }` + the supplied
/// status code. Keeps every handler's failure shape identical so the UI can
/// `switch()` on `error` without scraping the status line.
fn err(status: StatusCode, code: &str) -> (StatusCode, Json<serde_json::Value>) {
    (status, Json(serde_json::json!({ "error": code })))
}

fn err_with(
    status: StatusCode,
    code: &str,
    extra: serde_json::Value,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut obj = serde_json::Map::new();
    obj.insert("error".into(), serde_json::Value::String(code.into()));
    if let serde_json::Value::Object(m) = extra {
        for (k, v) in m {
            obj.insert(k, v);
        }
    }
    (status, Json(serde_json::Value::Object(obj)))
}

/// Reject empty / oversized API keys before they hit the seal layer.
fn validate_api_key_shape(api_key: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "api_key_empty"));
    }
    if api_key.len() > MAX_API_KEY_LEN {
        return Err(err(StatusCode::BAD_REQUEST, "api_key_too_long"));
    }
    if api_key.bytes().any(|b| b == 0) {
        return Err(err(StatusCode::BAD_REQUEST, "api_key_invalid_nul"));
    }
    Ok(())
}

/// Verify the supplied provider name parses to a known [`AiAgentProvider`]
/// variant. Returns the lower-case canonical form on success.
fn normalise_provider(provider: &str) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    AiAgentProvider::parse(provider)
        .map(|p| p.as_str().to_string())
        .map_err(|_| err(StatusCode::BAD_REQUEST, "unknown_provider"))
}

/// Return `Err((503, master_key_unavailable))` when the master key did not
/// load at boot. Read endpoints don't need this guard because they never
/// seal — only writes do.
fn require_master_key(
    auth_state: &AuthState,
) -> Result<std::sync::Arc<maestro_core::auth::MasterKey>, (StatusCode, Json<serde_json::Value>)> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?;
    db.master_key()
        .map(|s| s.key.clone())
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "master_key_unavailable"))
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /api/users/me/credentials` — read-only status. Returns NO sealed
/// bytes and NO tokens; only enough metadata for the dashboard to render
/// "credential set / not set / last validated" UI.
pub async fn get_my_credentials(
    State(auth_state): State<AuthState>,
    State(cfg_state): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<UserCredentialsStatus>, (StatusCode, Json<serde_json::Value>)> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?
        .clone();

    let active_provider = {
        let cfg = cfg_state.config.read().await;
        cfg.agent.provider.as_str().to_string()
    };
    let user_id = auth.user_id.clone();
    let active_provider_clone = active_provider.clone();
    let status = tokio::task::spawn_blocking(move || -> rusqlite::Result<UserCredentialsStatus> {
        let conn = db.conn().blocking_lock();
        // Task #39: fetch the two kinds independently. We return a bundle
        // (api_key + cli_state slots) so the UI can render the two-pill
        // state per-kind. Bundle is `None` only when BOTH are absent.
        let fetch = |kind: ProviderCredentialKind| -> rusqlite::Result<Option<ProviderCredentialStatus>> {
            let r = provider_credentials::find_active_with_kind(
                &conn,
                &user_id,
                &active_provider_clone,
                kind,
            )
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Null,
                    format!("find_active_with_kind({kind:?}) failed: {e}").into(),
                )
            })?;
            Ok(r.map(|row| ProviderCredentialStatus {
                provider: row.provider,
                kind: row.kind.as_str().to_string(),
                active: !row.inactive,
                last_validated_at: row.last_validated_at,
                last_used_at: row.last_used_at,
            }))
        };
        let api_key = fetch(ProviderCredentialKind::ApiKey)?;
        let cli_state = fetch(ProviderCredentialKind::CliState)?;
        let provider = if api_key.is_none() && cli_state.is_none() {
            None
        } else {
            Some(ProviderCredentialBundle {
                provider: active_provider_clone.clone(),
                api_key,
                cli_state,
            })
        };
        let github = github_credentials::find(&conn, &user_id)
            .map_err(|e| {
                rusqlite::Error::FromSqlConversionFailure(
                    0,
                    rusqlite::types::Type::Null,
                    format!("github_credentials::find failed: {e}").into(),
                )
            })?
            .map(|row| GithubCredentialStatus {
                login: row.github_login,
                scopes: serde_json::from_str(&row.scopes_json).unwrap_or_default(),
                sign_commits: row.sign_commits,
                last_validated_at: row.last_validated_at,
            });
        Ok(UserCredentialsStatus { provider, github })
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?
    .map_err(|e| {
        tracing::warn!(error = %e, "credential read failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "read_failed")
    })?;

    Ok(Json(status))
}

/// `POST /api/users/me/credentials/{provider}` — paste/rotate a credential.
///
/// Task #39: body grows two new fields. `kind` defaults to `"api_key"`
/// when absent (back-compat). Validation matrix:
///
///   - `kind = "api_key"` → `api_key` field required, `claude_session_json`
///     forbidden. Any provider accepts this.
///   - `kind = "cli_state"` → `claude_session_json` field required (must
///     parse as JSON AND contain `oauthAccount.{accountUuid, emailAddress,
///     organizationUuid}`), `api_key` forbidden. **Only Claude** accepts
///     this kind — every other provider rejects with `kind_not_supported`.
pub async fn post_provider_credential(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(provider): Path<String>,
    Json(body): Json<ApiKeyBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    let provider = normalise_provider(&provider)?;
    let kind_str = body.kind.as_deref().unwrap_or("api_key");
    let (kind, plaintext): (ProviderCredentialKind, Vec<u8>) = match kind_str {
        "api_key" => {
            if body.claude_session_json.is_some() {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "claude_session_json_not_allowed_for_api_key_kind",
                ));
            }
            let key = body
                .api_key
                .as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "api_key_required"))?;
            validate_api_key_shape(key)?;
            (ProviderCredentialKind::ApiKey, key.as_bytes().to_vec())
        }
        "cli_state" => {
            // Only Claude accepts cli_state today; every other provider
            // gets a structured rejection so future-proofing is clear.
            if provider != "claude" {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "kind_not_supported_for_provider",
                ));
            }
            if body.api_key.is_some() {
                return Err(err(
                    StatusCode::BAD_REQUEST,
                    "api_key_not_allowed_for_cli_state_kind",
                ));
            }
            let blob = body
                .claude_session_json
                .as_deref()
                .ok_or_else(|| err(StatusCode::BAD_REQUEST, "claude_session_json_required"))?;
            validate_claude_session_blob(blob)?;
            (ProviderCredentialKind::CliState, blob.as_bytes().to_vec())
        }
        _ => return Err(err(StatusCode::BAD_REQUEST, "unknown_kind")),
    };

    let master = require_master_key(&auth_state)?;
    // SAFETY: `require_master_key` returns `Err` when the DB is missing
    // (no master key without a DB), so reaching this point guarantees
    // `auth_state.db.is_some()`.
    let db = auth_state
        .db
        .as_ref()
        .expect("require_master_key gated db.is_some()")
        .clone();

    let sealed = seal(&master, &plaintext).map_err(|e| {
        tracing::warn!(error = %e, "seal failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "seal_failed")
    })?;
    drop(master);

    let user_id = auth.user_id.clone();
    let provider_for_blocking = provider.clone();
    // Per-kind metadata so audit-log reviewers can distinguish api_key
    // rotations from cli_state rotations. Non-secret JSON only.
    let metadata = serde_json::json!({ "kind": kind.as_str() }).to_string();
    let outcome = tokio::task::spawn_blocking(move || -> Result<UpsertOutcome, String> {
        let conn = db.conn().blocking_lock();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let outcome = provider_credentials::upsert(
            &tx,
            &user_id,
            &provider_for_blocking,
            kind,
            &sealed,
            &metadata,
        )
        .map_err(|e| e.to_string())?;
        // Task #39: the kind ("api_key" / "cli_state") is recorded in the
        // user_provider_credentials.metadata_json column above, NOT in
        // credential_audit (which has no metadata slot). Admins reviewing
        // audit can join by (user_id, provider) to confirm which kind
        // rotated. The `error_code` column stays for actual error codes
        // only.
        credential_audit::log(
            &tx,
            &user_id,
            Some(&user_id),
            CredentialAuditKind::AiProvider,
            Some(&provider_for_blocking),
            outcome.audit_event(),
            "ok",
            None,
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(outcome)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?
    .map_err(|e| {
        tracing::warn!(error = %e, "provider credential upsert failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "write_failed")
    })?;

    let status = match outcome {
        UpsertOutcome::Created => StatusCode::CREATED,
        UpsertOutcome::Rotated => StatusCode::OK,
    };
    Ok((
        status,
        Json(serde_json::json!({
            "provider": provider,
            "kind": kind.as_str(),
        })),
    ))
}

/// Task #39: validate a Claude session-state JSON blob.
///
/// Requirements (per Anthropic's `~/.claude.json` schema):
///   - Must parse as JSON.
///   - Must contain a top-level `oauthAccount` object.
///   - Must contain `oauthAccount.accountUuid`, `oauthAccount.emailAddress`,
///     `oauthAccount.organizationUuid` (the three keys Claude Code itself
///     requires before treating the session as authenticated).
///
/// We accept extra fields silently — Anthropic adds keys over time and we
/// don't want to break paste flows when they ship a new release. The blob
/// is stored verbatim.
fn validate_claude_session_blob(
    blob: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if blob.trim().is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "claude_session_json_empty"));
    }
    if blob.len() > MAX_CLAUDE_SESSION_JSON_LEN {
        // Task #41: human-readable hint pointing at the most common cause
        // (pasted a file path or a stale dump). Stable code preserved for
        // the UI's error-toast switch.
        return Err(err_with(
            StatusCode::BAD_REQUEST,
            "claude_session_json_too_long",
            serde_json::json!({
                "message": "Session JSON is larger than 1 MiB. Are you sure \
                            you pasted only the ~/.claude.json contents and \
                            not a file path or full disk dump?",
                "max_bytes": MAX_CLAUDE_SESSION_JSON_LEN,
            }),
        ));
    }
    let parsed: serde_json::Value = serde_json::from_str(blob)
        .map_err(|_| err(StatusCode::BAD_REQUEST, "claude_session_json_invalid"))?;
    let oauth = parsed
        .get("oauthAccount")
        .and_then(|v| v.as_object())
        .ok_or_else(|| err(StatusCode::BAD_REQUEST, "claude_session_invalid"))?;
    for key in ["accountUuid", "emailAddress", "organizationUuid"] {
        let value = oauth.get(key);
        let ok = match value {
            Some(serde_json::Value::String(s)) => !s.trim().is_empty(),
            _ => false,
        };
        if !ok {
            return Err(err(StatusCode::BAD_REQUEST, "claude_session_invalid"));
        }
    }
    Ok(())
}

/// `DELETE /api/users/me/credentials/{provider}` — hard delete + audit row.
///
/// Task #39: when `?kind=api_key` or `?kind=cli_state` is supplied, only
/// that kind is wiped; the other-kind row stays intact. When `kind` is
/// absent (legacy / "Wipe everything" UI), every row for `(user, provider)`
/// is deleted (back-compat).
pub async fn delete_provider_credential(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(provider): Path<String>,
    Query(query): Query<DeleteProviderCredentialQuery>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let provider = normalise_provider(&provider)?;
    let kind = match query.kind.as_deref() {
        None => None,
        Some("api_key") => Some(ProviderCredentialKind::ApiKey),
        Some("cli_state") => Some(ProviderCredentialKind::CliState),
        Some(_) => return Err(err(StatusCode::BAD_REQUEST, "unknown_kind")),
    };
    let db = auth_state
        .db
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?
        .clone();

    let user_id = auth.user_id.clone();
    let provider_for_blocking = provider.clone();
    let deleted = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = db.conn().blocking_lock();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let was_present = match kind {
            Some(k) => provider_credentials::delete_with_kind(
                &tx,
                &user_id,
                &provider_for_blocking,
                k,
            )
            .map_err(|e| e.to_string())?,
            None => provider_credentials::delete(&tx, &user_id, &provider_for_blocking)
                .map_err(|e| e.to_string())?,
        };
        if was_present {
            credential_audit::log(
                &tx,
                &user_id,
                Some(&user_id),
                CredentialAuditKind::AiProvider,
                Some(&provider_for_blocking),
                "deleted",
                "ok",
                None,
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(was_present)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?
    .map_err(|e| {
        tracing::warn!(error = %e, "provider credential delete failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "write_failed")
    })?;

    // 204 on hit AND on idempotent miss — the dashboard "delete" button is
    // safe to click twice.
    let _ = deleted;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/users/me/github-pat` — validate PAT with the injected GhClient,
/// seal, upsert, audit-log. On validation failure: 400 + structured error
/// + audit row with `outcome = "error"`.
pub async fn post_github_pat(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<GithubPatBody>,
) -> Result<(StatusCode, Json<serde_json::Value>), (StatusCode, Json<serde_json::Value>)> {
    if body.pat.trim().is_empty() || body.pat.len() > MAX_API_KEY_LEN {
        return Err(err(StatusCode::BAD_REQUEST, "invalid_pat"));
    }
    let master = require_master_key(&auth_state)?;
    // SAFETY: `require_master_key` returns `Err` when the DB is missing
    // (no master key without a DB), so reaching this point guarantees
    // `auth_state.db.is_some()`.
    let db = auth_state
        .db
        .as_ref()
        .expect("require_master_key gated db.is_some()")
        .clone();

    // Phase 2b.1 does not yet parse remote URLs — pass an empty org list so
    // the SSO check is a no-op until Phase 2b.2 wires it to [git].repo_path.
    let orgs: Vec<String> = Vec::new();

    let validated = match validate_pat(auth_state.gh_client.as_ref(), &body.pat, &orgs).await {
        Ok(v) => v,
        Err(e) => {
            let code = e.code();
            // Audit-log the validation failure (best-effort — the bad PAT
            // never reaches the seal layer).
            let user_id = auth.user_id.clone();
            let db_audit = db.clone();
            let code_owned = code.to_string();
            let _ = tokio::task::spawn_blocking(move || {
                let conn = db_audit.conn().blocking_lock();
                let _ = credential_audit::log(
                    &conn,
                    &user_id,
                    Some(&user_id),
                    CredentialAuditKind::GithubPat,
                    None,
                    "validation_failed",
                    "error",
                    Some(&code_owned),
                );
            })
            .await;

            let extra = match &e {
                PatValidationError::SsoAuthorizationRequired { org, sso_url } => {
                    serde_json::json!({ "org": org, "org_sso_url": sso_url })
                }
                PatValidationError::InsufficientScopes { missing } => {
                    serde_json::json!({ "missing_scopes": missing })
                }
                PatValidationError::InvalidPat | PatValidationError::Transport(_) => {
                    serde_json::json!({})
                }
            };
            // Transport errors → 502, validation errors → 400 (the dashboard
            // distinguishes "your PAT is bad" from "the upstream check
            // failed transiently").
            let status = match e {
                PatValidationError::Transport(_) => StatusCode::BAD_GATEWAY,
                _ => StatusCode::BAD_REQUEST,
            };
            return Err(err_with(status, code, extra));
        }
    };

    let sealed = seal(&master, body.pat.as_bytes()).map_err(|e| {
        tracing::warn!(error = %e, "PAT seal failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "seal_failed")
    })?;
    drop(master);

    let sign_commits = body.sign_commits.unwrap_or(true);
    let scopes_json =
        serde_json::to_string(&validated.scopes).unwrap_or_else(|_| "[]".to_string());
    let login = validated.login.clone();
    let user_id = auth.user_id.clone();
    let now = Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = db.conn().blocking_lock();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let already_present = github_credentials::find(&tx, &user_id)
            .map_err(|e| e.to_string())?
            .is_some();
        github_credentials::upsert(
            &tx,
            &user_id,
            &sealed,
            &login,
            &scopes_json,
            sign_commits,
        )
        .map_err(|e| e.to_string())?;
        github_credentials::touch_last_validated(&tx, &user_id, &now)
            .map_err(|e| e.to_string())?;
        let event = if already_present { "rotated" } else { "created" };
        credential_audit::log(
            &tx,
            &user_id,
            Some(&user_id),
            CredentialAuditKind::GithubPat,
            None,
            event,
            "ok",
            None,
        )
        .map_err(|e| e.to_string())?;
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?
    .map_err(|e| {
        tracing::warn!(error = %e, "github PAT upsert failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "write_failed")
    })?;

    Ok((
        StatusCode::OK,
        Json(serde_json::json!({
            "login": validated.login,
            "scopes": validated.scopes,
            "attribute_commits": sign_commits,
        })),
    ))
}

/// `DELETE /api/users/me/github-pat` — hard delete + audit.
pub async fn delete_github_pat(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?
        .clone();
    let user_id = auth.user_id.clone();

    tokio::task::spawn_blocking(move || -> Result<(), String> {
        let conn = db.conn().blocking_lock();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let was_present =
            github_credentials::delete(&tx, &user_id).map_err(|e| e.to_string())?;
        if was_present {
            credential_audit::log(
                &tx,
                &user_id,
                Some(&user_id),
                CredentialAuditKind::GithubPat,
                None,
                "deleted",
                "ok",
                None,
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(())
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?
    .map_err(|e| {
        tracing::warn!(error = %e, "github PAT delete failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "write_failed")
    })?;

    Ok(StatusCode::NO_CONTENT)
}

/// `PATCH /api/users/me/github` — toggle the commit-attribution flag.
pub async fn patch_github_attribution(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<PatchGithubBody>,
) -> Result<StatusCode, (StatusCode, Json<serde_json::Value>)> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?
        .clone();
    let user_id = auth.user_id.clone();
    let value = body.sign_commits;

    let updated = tokio::task::spawn_blocking(move || -> Result<bool, String> {
        let conn = db.conn().blocking_lock();
        let tx = conn.unchecked_transaction().map_err(|e| e.to_string())?;
        let hit =
            github_credentials::set_sign_commits(&tx, &user_id, value).map_err(|e| e.to_string())?;
        if hit {
            credential_audit::log(
                &tx,
                &user_id,
                Some(&user_id),
                CredentialAuditKind::GithubPat,
                None,
                "rotated",
                "ok",
                None,
            )
            .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())?;
        Ok(hit)
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?
    .map_err(|e| {
        tracing::warn!(error = %e, "github attribution patch failed");
        err(StatusCode::INTERNAL_SERVER_ERROR, "write_failed")
    })?;

    if updated {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(err(StatusCode::NOT_FOUND, "no_github_pat"))
    }
}

/// `GET /api/admin/users/{id}/github-status` — admin-only.
pub async fn get_admin_github_status(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(target_user_id): Path<String>,
) -> Result<Json<AdminGithubStatusResponse>, (StatusCode, Json<serde_json::Value>)> {
    require_admin_for(&auth_state, &auth)
        .await
        .map_err(|s| err(s, "forbidden"))?;
    let db = auth_state
        .db
        .as_ref()
        .ok_or_else(|| err(StatusCode::SERVICE_UNAVAILABLE, "database_unavailable"))?
        .clone();

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        github_credentials::find(&conn, &target_user_id).ok().flatten()
    })
    .await
    .map_err(|_| err(StatusCode::INTERNAL_SERVER_ERROR, "join_failed"))?;

    let resp = match row {
        Some(r) => AdminGithubStatusResponse {
            has_pat: true,
            login: Some(r.github_login),
            scopes: serde_json::from_str(&r.scopes_json).unwrap_or_default(),
            sign_commits: r.sign_commits,
        },
        None => AdminGithubStatusResponse {
            has_pat: false,
            login: None,
            scopes: Vec::new(),
            sign_commits: false,
        },
    };
    Ok(Json(resp))
}

// `SealedBlob` is referenced by handlers indirectly through the seal helpers;
// pulling the symbol into scope keeps rustc's "unused import" check quiet
// without needing a glob.
#[allow(dead_code)]
fn _seal_in_scope(_b: SealedBlob) {}
