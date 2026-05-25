// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
#![allow(deprecated)] // Transitional: ConfigStr sites rewritten to ConfigError variants in C2.

//! Phase 2b.3 workflow auth pinning + per-step `WorkerSecretsBundle`
//! construction. Split off `driver.rs` so the bootstrap and step-runner
//! sub-modules can both reach the bundle helper without the bundle
//! plumbing dominating `driver.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::Config;
use crate::db::Database;
use crate::error::{MaestroError, Result};

use super::types::Workflow;

/// Phase 2b.3: pin the workflow's credentials at the first agent step.
/// Idempotent — if the workflow already has an `auth_pin` (snapshot resume,
/// re-entry after pause), the existing pin is preserved so an in-flight
/// workflow survives an admin provider switch.
pub(super) async fn ensure_workflow_auth_pin(
    ticket_key: &str,
    config: &Arc<RwLock<Config>>,
    db: &Database,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    user_id: &str,
) -> Result<()> {
    // Fast path: pin already set.
    {
        let wf = workflows.read().await;
        if wf.get(ticket_key).and_then(|w| w.auth_pin.as_ref()).is_some() {
            return Ok(());
        }
    }

    let cfg_snapshot = config.read().await.clone();
    let pin = crate::auth::bundle::pin_for_workflow(&cfg_snapshot, db, user_id)
        .await
        .map_err(|e| MaestroError::ConfigStr(format!("pin_for_workflow failed: {e}")))?;

    // Write back; concurrent calls collapse — we only write if still None.
    let mut wf = workflows.write().await;
    if let Some(w) = wf.get_mut(ticket_key)
        && w.auth_pin.is_none()
    {
        info!(
            ticket = %ticket_key,
            user_id = %user_id,
            provider = %pin.provider,
            github_mode = %pin.github_mode,
            "Auth pinned at workflow start"
        );
        w.auth_pin = Some(pin);
    }
    Ok(())
}

/// Phase 2b.3 — build a [`WorkerSecretsBundle`] for the workflow and return
/// it as an `Arc` ready to hand to [`ContainerRunner::with_secrets_bundle`].
///
/// Why a helper: the bundle MUST be attached to every runner that spawns an
/// agent worker — both the bootstrap runner (mise install / worktree init)
/// AND the agent-step runner (`run_workflow_def_steps`). When only the
/// bootstrap runner had it, claude / cursor / codex / opencode workers were
/// spawned without `BUNDLE_SOURCING_SH` spliced in, so the agent never
/// exported `CLAUDE_CODE_OAUTH_TOKEN` and `~/.claude.json` never got the
/// `oauthAccount` merge — surfacing as "Not logged in · Please run /login"
/// even though the bundle had built successfully earlier in the workflow.
///
/// Returns `None` (and logs a single line at debug or warn) when the
/// pre-Phase-2b.3 legacy `PASSTHROUGH_ENV` path is the correct fallback:
/// no resolver, no user_id, no db, or a build failure. Callers use
/// `if let Some(bundle) = ... { runner = runner.with_secrets_bundle(bundle); }`.
pub(super) async fn try_attach_secrets_bundle(
    ticket_key: &str,
    config: &Arc<RwLock<Config>>,
    workflows: &Arc<RwLock<HashMap<String, Workflow>>>,
    db: Option<&Database>,
    git_auth_resolver: Option<&Arc<crate::github::auth_resolver::GitAuthResolver>>,
) -> Option<Arc<crate::auth::WorkerSecretsBundle>> {
    let user_id: Option<String> = {
        let wf = workflows.read().await;
        wf.get(ticket_key).and_then(|w| w.user_id.clone())
    };
    let (resolver, uid, db_handle) = match (git_auth_resolver, user_id.as_deref(), db) {
        (Some(r), Some(u), Some(d)) => (r, u, d),
        _ => {
            tracing::debug!(
                ticket = %ticket_key,
                has_resolver = git_auth_resolver.is_some(),
                has_user = user_id.is_some(),
                has_db = db.is_some(),
                "Phase 2b.3 bundle path skipped — using legacy PASSTHROUGH_ENV"
            );
            return None;
        }
    };

    // `ensure_workflow_auth_pin` is idempotent — the fast path returns
    // immediately when the pin already exists (bootstrap usually writes it
    // first). The agent-step call is a no-op in steady state.
    if let Err(e) = ensure_workflow_auth_pin(ticket_key, config, db_handle, workflows, uid).await {
        warn!(
            ticket = %ticket_key,
            user_id = %uid,
            error = %e,
            "auth pin failed — falling back to legacy PASSTHROUGH_ENV path"
        );
        return None;
    }

    let pin = {
        let wf = workflows.read().await;
        wf.get(ticket_key).and_then(|w| w.auth_pin.clone())
    }?;

    let cfg_snapshot = config.read().await.clone();
    match crate::auth::bundle::build(&cfg_snapshot, db_handle, resolver, &pin, uid).await {
        Ok(bundle) => {
            info!(
                ticket = %ticket_key,
                user_id = %uid,
                provider = %pin.provider,
                "Worker secrets bundle attached (Phase 2b.3 path active)"
            );
            Some(Arc::new(bundle))
        }
        Err(e) => {
            warn!(
                ticket = %ticket_key,
                user_id = %uid,
                error = %e,
                "build worker secrets bundle failed — falling back to legacy PASSTHROUGH_ENV path"
            );
            None
        }
    }
}
