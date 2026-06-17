// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto github-app-token` — mint a GitHub App installation token to stdout.

use std::process::ExitCode;

use takuto_core::config::Config;

pub(crate) async fn run_github_app_token(config_path: &std::path::Path) -> ExitCode {
    let config = match Config::load(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Failed to load config {}: {e}", config_path.display());
            return ExitCode::FAILURE;
        }
    };

    let mgr = match takuto_core::github_app::try_create_token_manager(&config.github) {
        Some(mgr) => mgr,
        None => {
            eprintln!(
                "GitHub App not configured — set [github] app_id, app_installation_id, and app_private_key/app_private_key_path in config.toml."
            );
            return ExitCode::FAILURE;
        }
    };

    // Use the config file's directory as cwd for the curl invocation; fall back to /tmp.
    let cwd = config_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .filter(|p| p.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"));

    match mgr.get_installation_token(&cwd).await {
        Ok(token) => {
            println!("{token}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("Failed to get GitHub App installation token: {e}");
            ExitCode::FAILURE
        }
    }
}
