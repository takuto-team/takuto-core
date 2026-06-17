// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `takuto egress-hosts` — print the provider-aware egress allowlist (one host
//! per line) for `docker/egress-rules.sh` to feed into its `allow_host` helper.

use std::process::ExitCode;

use super::load_config_or_default;

pub(crate) fn run_egress_hosts(config_path: &std::path::Path) -> ExitCode {
    let config = match load_config_or_default(config_path) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("{msg}");
            return ExitCode::FAILURE;
        }
    };
    for host in config.provider_egress_hosts() {
        println!("{host}");
    }
    ExitCode::SUCCESS
}
