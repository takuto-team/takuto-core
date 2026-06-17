// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto keys reset` (04_architecture.md §3.2, A5).
//!
//! Clears every credential / audit / onboarding row and rewrites the master
//! keyfile under `${data_dir}/secret.key`. Lossy by design: every user must
//! re-paste their credentials afterwards.

use std::process::ExitCode;

use takuto_core::config::Config;

/// Refuses to run when:
/// - `--yes-i-am-sure` not passed
/// - any workflow in a non-terminal, non-paused state per the snapshot file
///   (graceful-shutdown the server first; see QA T-KEYS-002)
/// - config load failure
/// - data_dir cannot be resolved
pub(crate) fn run_keys_reset(config_path: &std::path::Path, yes_i_am_sure: bool) -> ExitCode {
    if !yes_i_am_sure {
        eprintln!(
            "takuto keys reset: refuses to run without --yes-i-am-sure (this command is destructive)."
        );
        return ExitCode::FAILURE;
    }

    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let Some(data_dir) = takuto_core::workflow::snapshot::resolve_data_dir() else {
        eprintln!(
            "takuto keys reset: cannot resolve data directory (set TAKUTO_DATA_DIR, TAKUTO_HOME, or HOME)."
        );
        return ExitCode::FAILURE;
    };

    // Refuse when an in-flight workflow is present in any per-workspace
    // snapshot. The helper lives in takuto-core::workflow::snapshot so it
    // can be unit-tested without spawning the CLI.
    match takuto_core::workflow::snapshot::scan_in_flight_workflow_keys(&data_dir) {
        Ok(active) if !active.is_empty() => {
            eprintln!(
                "takuto keys reset: refuses to run — {} active workflow(s) present in the snapshot: {}.",
                active.len(),
                active.join(", ")
            );
            eprintln!("Stop or finish them, then retry.");
            return ExitCode::FAILURE;
        }
        Ok(_) => {}
        Err(e) => {
            eprintln!("takuto keys reset: failed to read workflow snapshots: {e}");
            return ExitCode::FAILURE;
        }
    }

    // Open the DB so we can clear the credential / audit / onboarding tables.
    // Pass `allow_auto_generate_secret_key = false` here — we're about to
    // rewrite the keyfile ourselves; we don't want Database::open creating
    // a new one mid-reset.
    let db = match takuto_core::db::Database::open(&data_dir, false) {
        Ok(db) => db,
        Err(e) => {
            eprintln!(
                "takuto keys reset: failed to open database at {}: {e}",
                data_dir.display()
            );
            return ExitCode::FAILURE;
        }
    };

    // Spin up a one-shot runtime to drive the async adapter call. The CLI
    // dispatcher is sync, so we need a local runtime for this single
    // transaction — keep it scoped tight so the runtime drops with the
    // transaction.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("takuto keys reset: failed to start async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(e) = rt.block_on(clear_credential_tables(&db)) {
        eprintln!("takuto keys reset: failed to clear credential tables: {e}");
        return ExitCode::FAILURE;
    }

    // Regenerate (or wipe) the master keyfile.
    let keyfile = takuto_core::auth::master_key::keyfile_path(&data_dir);
    let env_set = std::env::var("TAKUTO_SECRET_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if env_set {
        // Operator manages the master key via env var; we only nuked the rows.
        // Don't touch the keyfile (if any) — it may not even exist. Warn so
        // the operator knows to rotate the env var manually.
        eprintln!(
            "takuto keys reset: credential rows cleared. TAKUTO_SECRET_KEY is set — rotate it manually before bringing the server back up."
        );
    } else {
        // Best-effort delete + regenerate.
        let _ = std::fs::remove_file(&keyfile);
        match takuto_core::auth::master_key::load_or_init_master_key(
            &data_dir,
            config.general.allow_auto_generate_secret_key,
        ) {
            Ok(_) => {
                eprintln!(
                    "takuto keys reset: credential rows cleared and {} regenerated.",
                    keyfile.display()
                );
            }
            Err(e) => {
                eprintln!(
                    "takuto keys reset: credential rows cleared but failed to regenerate {}: {e}",
                    keyfile.display()
                );
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

/// Wipe every credential / audit / onboarding row. Caller has already
/// opened the database and verified no workflows are in flight.
///
/// Goes through the agnostic database adapter. The CLI is sync at the
/// dispatch layer, so the call site spins up a one-shot current-thread
/// runtime to drive the async transaction.
async fn clear_credential_tables(
    db: &takuto_core::db::Database,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut tx = db.adapter().begin().await?;
    for table in [
        "user_provider_credentials",
        "user_github_credentials",
        "credential_audit",
        "onboarding_state",
    ] {
        tx.execute(&format!("DELETE FROM {table}"), vec![]).await?;
    }
    tx.commit().await?;
    Ok(())
}
