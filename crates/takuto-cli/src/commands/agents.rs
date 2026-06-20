// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto agents install` — install/refresh the agent + Atlassian CLIs into the
//! shared tools volume. Used by the setup-mode entrypoint (no web server) so the
//! interactive auth steps find `claude` / `agent` / `acli`. Normal-mode runtime
//! installs are driven by the web server (with live progress); this is the
//! headless equivalent.

use std::path::Path;
use std::process::ExitCode;

use takuto_core::agent_install::{Installer, StdoutSink};

use super::load_config_or_default;

/// Directory the runtime CLIs install into (binaries land in `<dir>/bin`, which
/// is on the worker/workspace `PATH`). Overridable for tests / custom layouts.
fn install_dir() -> String {
    std::env::var("TAKUTO_TOOLS_DIR").unwrap_or_else(|_| "/opt/takuto-tools".to_string())
}

pub(crate) async fn run_agents_install(config_path: &Path) -> ExitCode {
    let cfg = match load_config_or_default(config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let installer = Installer::new(install_dir());
    match installer.install_all(&cfg, &StdoutSink).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Dependency install failed: {e}");
            ExitCode::FAILURE
        }
    }
}
