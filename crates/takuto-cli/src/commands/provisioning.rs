// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto provisioning <sha|commands>` — read-only accessors over
//! `[provisioning].install_commands` for the Docker entrypoint shell.
//!
//! The entrypoint can't parse TOML itself, so this subcommand exposes the
//! canonical SHA and the command list. Both write to stdout so the entrypoint
//! can capture via `$(takuto provisioning sha)` / a `while read` loop.
//!
//! Exit code is 0 on success even when the install_commands list is empty (the
//! entrypoint takes the no-op fast path).

use std::process::ExitCode;

use super::load_config_or_default;
use crate::cli::ProvisioningAction;

pub(crate) fn run_provisioning(
    config_path: &std::path::Path,
    action: &ProvisioningAction,
) -> ExitCode {
    let config = match load_config_or_default(config_path) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    match action {
        ProvisioningAction::Sha => {
            println!("{}", config.provisioning_sha());
        }
        ProvisioningAction::Commands => {
            // NUL-terminated stream — `0x00` is the only byte that can't
            // appear inside a POSIX shell command, so the entrypoint can
            // iterate via `while IFS= read -r -d '' cmd; do ... done`.
            use std::io::Write;
            let mut out = std::io::stdout().lock();
            for cmd in &config.provisioning.install_commands {
                if out.write_all(cmd.as_bytes()).is_err() || out.write_all(&[0u8]).is_err() {
                    return ExitCode::FAILURE;
                }
            }
            let _ = out.flush();
        }
    }
    ExitCode::SUCCESS
}
