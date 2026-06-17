// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 2: open the multi-user database and run filesystem ↔ DB
//! reconciliation + per-user work-item-flow seeding.
//!
//! Ordering matters: this runs AFTER config/logging bootstrap and BEFORE engine
//! construction, so restored workflows have a `repositories` row to resolve by
//! workspace name and the engine can thread the DB handle into the bootstrap
//! driver.

use tracing::info;

use takuto_core::db::Database;
use takuto_core::repo_reconcile;

use super::Bootstrap;

pub(super) async fn open_and_reconcile(boot: &Bootstrap) -> Option<Database> {
    let config = &boot.config;

    // Initialize the SQLite database for multi-user auth. This happens BEFORE
    // engine construction so the engine can thread the DB handle into the
    // bootstrap driver for per-workspace `worktree_init_commands` overrides,
    // and BEFORE poller construction so we can resolve the poller-owner
    // user_id and pass it into both pollers.
    let resolved_data_dir = takuto_core::workflow::snapshot::resolve_data_dir();
    // Sweep orphan WorkerSecretsBundle directories from a prior
    // run (crash between TempDir creation and drop leaves them around).
    // Safe to run unconditionally — best-effort, no-op when the dir is
    // missing. Runs BEFORE the DB opens because it touches a sibling
    // directory under data_dir, not the DB itself.
    if let Some(dir) = resolved_data_dir.as_deref()
        && let Err(e) = takuto_core::auth::bundle::cleanup_orphan_secrets(dir)
    {
        tracing::warn!(
            data_dir = %dir.display(),
            error = %e,
            "WorkerSecretsBundle orphan sweep failed (continuing); old dirs may persist"
        );
    }
    let (allow_auto_generate_secret_key, mut db_config) = {
        let cfg = config.read().await;
        (
            cfg.general.allow_auto_generate_secret_key,
            cfg.database.clone(),
        )
    };
    // `TAKUTO_DATABASE_CONNECTION` env var overrides
    // `[database].connection` from config.toml. Useful for the
    // docker-compose.{postgres,mariadb}.yml overlays which set the URL
    // per-deployment without touching the user's checked-in config.
    if let Ok(env_url) = std::env::var("TAKUTO_DATABASE_CONNECTION")
        && !env_url.trim().is_empty()
    {
        db_config.connection = env_url;
    }
    let db = match resolved_data_dir.as_deref() {
        Some(data_dir) => match takuto_core::db::Database::connect(
            data_dir,
            &db_config,
            allow_auto_generate_secret_key,
        )
        .await
        {
            Ok(db) => {
                if db_config.is_default_sqlite() {
                    info!(path = %data_dir.join("takuto.db").display(), "Multi-user database initialized (sqlite)");
                } else {
                    info!(
                        backend = %db.adapter().backend(),
                        url = %takuto_core::config::redact_connection_password(db_config.connection_url()),
                        "Multi-user database initialized (external backend)"
                    );
                }
                Some(db)
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to open multi-user database (multi-user auth unavailable; legacy auth still works)");
                None
            }
        },
        None => {
            tracing::warn!(
                "No data directory resolved — multi-user database unavailable (set TAKUTO_DATA_DIR, TAKUTO_HOME, or HOME)"
            );
            None
        }
    };

    // Filesystem ↔ DB reconciliation must run AFTER DB open and
    // BEFORE engine.restore_persisted_workflows(). Otherwise restored
    // workflows have no `repositories` row to look up by workspace_name
    // and the workflow filter hides every legacy workflow from its
    // owner's dashboard until an admin manually re-adds.
    if let (Some(db), Some(data_dir)) = (db.as_ref(), resolved_data_dir.as_deref()) {
        let migrate_associations = config.read().await.general.migrate_orphan_repo_associations;

        // Repositories DAO uses the agnostic adapter — no rusqlite
        // MutexGuard needed for the reconciliation path; both helpers
        // are async and take &DbAdapter directly.
        let adapter = db.adapter();

        // Filesystem → `repositories` reconciliation.
        match repo_reconcile::reconcile_repositories(
            adapter,
            takuto_core::workflow::snapshot::WORKSPACES_DIR,
        )
        .await
        {
            Ok(n) if n > 0 => info!(
                count = n,
                workspaces_dir = takuto_core::workflow::snapshot::WORKSPACES_DIR,
                "Reconciliation: registered repositories from on-disk clones"
            ),
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, "Repository reconciliation failed (continuing)"),
        }

        // Backfill `user_repositories` from restored snapshot workflows
        // (gated; default on).
        if migrate_associations {
            match repo_reconcile::backfill_user_repositories_from_snapshots(adapter, data_dir).await
            {
                Ok(n) if n > 0 => info!(
                    count = n,
                    "Backfilled user_repositories from restored workflow snapshots"
                ),
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "Association backfill failed (continuing)")
                }
            }
        } else {
            info!(
                "[general] migrate_orphan_repo_associations = false — skipping snapshot-driven \
                 user_repositories backfill; existing workflows will be invisible until each user \
                 re-adds the repository from the dashboard"
            );
        }

        // Seed default work-item flows for every existing user against the
        // active workspace, idempotently. Runs before the HTTP listener
        // accepts traffic so the first dashboard load already sees flows for
        // the currently-selected workspace. Best-effort: a failure logs a
        // warning and the user falls back to the empty-state banner.
        let active_workspace = takuto_core::workflow::snapshot::workspace_name_from_repo_path(
            std::path::Path::new(&config.read().await.git.repo_path),
        );
        match takuto_core::db::users::list_users(adapter).await {
            Ok(users) => {
                for user in &users {
                    if let Err(e) = takuto_core::db::user_work_item_flows::seed_if_absent(
                        adapter,
                        &user.id,
                        &active_workspace,
                        &boot.work_item_flow_defaults,
                    )
                    .await
                    {
                        tracing::warn!(
                            user_id = %user.id,
                            workspace = %active_workspace,
                            error = %e,
                            "Failed to seed default work-item flows (continuing)"
                        );
                    }
                }
            }
            Err(e) => tracing::warn!(
                error = %e,
                "Failed to list users for work-item flow seeding (continuing)"
            ),
        }

        // active_workspace file cleanup. The active-workspace concept is
        // dead; each workflow carries its own repo association.
        let aw_path = data_dir.join("active_workspace");
        if aw_path.exists() {
            if let Ok(value) = std::fs::read_to_string(&aw_path) {
                tracing::info!(
                    value = %value.trim(),
                    "Removing dead `active_workspace` file"
                );
            }
            let _ = std::fs::remove_file(&aw_path);
        }

        // Deprecation warning for `[git] repo_path` when set and not
        // matching any registered repository.
        let cfg_repo_path = config.read().await.git.repo_path.clone();
        if !cfg_repo_path.is_empty() && cfg_repo_path != "/workspace" {
            let matches_any = takuto_core::db::repositories::get_by_path(adapter, &cfg_repo_path)
                .await
                .ok()
                .flatten()
                .is_some();
            if !matches_any {
                tracing::warn!(
                    repo_path = %cfg_repo_path,
                    "[git] repo_path is deprecated and ignored; configure repositories via the dashboard's My Repositories tab."
                );
            }
        }
    } else {
        tracing::warn!(
            "Skipping repository reconciliation: no multi-user database available — \
             repositories cannot be registered until the data dir is configured"
        );
    }

    db
}
