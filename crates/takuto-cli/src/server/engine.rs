// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 3: build the workflow engine (attaching the auth resolver + gh
//! client), restore persisted workflows, resolve the poller owner, and run the
//! one-shot orphan-workflow migration.

use std::sync::Arc;

use tracing::info;

use takuto_core::db::Database;
use takuto_core::github::auth_resolver::GitAuthResolver;
use takuto_core::workflow::engine::WorkflowEngine;

use super::poller_owner::resolve_poller_owner;
use super::{Bootstrap, EngineSetup};

pub(super) async fn build(boot: &Bootstrap, db: &Option<Database>) -> EngineSetup {
    // Construct the GitAuthResolver here so we can attach it to the
    // engine via `with_git_auth_resolver` BEFORE wrapping in Arc. The
    // same resolver is later stored on AppState for the web layer.
    let git_auth_resolver: Option<Arc<GitAuthResolver>> = db
        .as_ref()
        .map(|d| Arc::new(GitAuthResolver::new(d.clone(), boot.github_app_mgr.clone())));

    let mut engine = WorkflowEngine::new_with_db(
        boot.config.clone(),
        boot.actions.clone(),
        boot.max_concurrent,
        boot.jira_available.clone(),
        boot.ticketing_system,
        boot.workflows_dir.clone(),
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
        let cfg = boot.config.read().await;
        let username = cfg.general.poller_owner_username.clone();
        let migrate = cfg.general.migrate_orphan_workflows;
        let owner = match db {
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

    EngineSetup {
        engine,
        git_auth_resolver,
        resolved_poller_owner,
    }
}
