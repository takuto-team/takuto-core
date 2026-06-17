// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto preflight [--strict]` — collect the boot SystemStatus, seed the
//! GitHub App token file, and report. Exits 0 in degraded mode unless `--strict`.

use std::process::ExitCode;

use takuto_core::config::TicketingSystem;
use takuto_core::docker_hooks;

use super::load_config_or_default;

pub(crate) fn run_preflight(config_path: &std::path::Path, strict: bool) -> ExitCode {
    let config = match load_config_or_default(config_path) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };

    // Phase 0 (04_architecture.md §1): collect_system_status never returns Err
    // — every former hard-error becomes a structured warning. The CLI prints
    // them to stderr (so docker logs surface them) and exits 0 unless the
    // operator passed --strict.
    let status = docker_hooks::collect_system_status(&config);

    for w in &status.warnings {
        eprintln!(
            "[takuto preflight] {sev}: {code} — {msg}",
            sev = w.severity,
            code = w.code,
            msg = w.message
        );
    }

    // Seed the GitHub App token file so it is available before the main server
    // starts. The server's background task handles subsequent refreshes. This
    // runs even on degraded boots — it's a no-op when the App is not
    // configured.
    if let Some(mgr) = takuto_core::github_app::try_create_token_manager(&config.github) {
        let cwd = config_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .filter(|p| p.exists())
            .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            match rt.block_on(mgr.get_installation_token(&cwd)) {
                Ok(token) => {
                    let token_path = std::path::Path::new(takuto_core::github_app::TOKEN_FILE_PATH);
                    match takuto_core::github_app::write_token_file(token_path, &token) {
                        Ok(()) => {
                            eprintln!(
                                "[takuto preflight] GitHub App token written to {}",
                                token_path.display()
                            );
                        }
                        Err(e) => {
                            eprintln!(
                                "[takuto preflight] WARNING: failed to write token file: {e}"
                            );
                        }
                    }
                }
                Err(e) => {
                    eprintln!("[takuto preflight] WARNING: failed to fetch GitHub App token: {e}");
                }
            }
        }
    }

    match config.general.ticketing_system {
        TicketingSystem::Jira => {
            if status.ticketing.acli_ok {
                eprintln!("[takuto preflight] ticketing_system = jira, acli authenticated.");
            } else {
                eprintln!(
                    "[takuto preflight] ticketing_system = jira but acli is not authenticated — Jira integration disabled, manual entry only."
                );
            }
        }
        TicketingSystem::GitHub => {
            eprintln!(
                "[takuto preflight] ticketing_system = github — polling GitHub issues, no Atlassian auth required."
            );
        }
        TicketingSystem::None => {
            eprintln!(
                "[takuto preflight] ticketing_system = none — manual description entry only."
            );
        }
    }

    if status.has_critical() {
        eprintln!(
            "[takuto preflight] {n} critical warning(s) present — the dashboard will boot in degraded mode (see GET /api/onboarding/status).",
            n = status
                .warnings
                .iter()
                .filter(|w| w.severity == "critical")
                .count()
        );
        if strict {
            eprintln!("[takuto preflight] --strict was passed — exiting FAILURE for CI.");
            return ExitCode::FAILURE;
        }
    } else {
        eprintln!("[takuto preflight] OK.");
    }

    ExitCode::SUCCESS
}
