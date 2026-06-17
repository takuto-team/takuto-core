// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! One module per `takuto <subcommand>` handler invoked by the Docker
//! entrypoint and operators. The default (no subcommand) server path lives
//! under [`crate::server`].

mod docker_hooks;
mod egress_hosts;
mod github_app_token;
mod keys;
mod preflight;
mod provisioning;

pub(crate) use docker_hooks::run_docker_hooks;
pub(crate) use egress_hosts::run_egress_hosts;
pub(crate) use github_app_token::run_github_app_token;
pub(crate) use keys::run_keys_reset;
pub(crate) use preflight::run_preflight;
pub(crate) use provisioning::run_provisioning;

use takuto_core::config::Config;

/// Load config from `config_path`, falling back to [`Config::default`] when the
/// file is simply **absent** — the no-config-needed first-run bootstrap: the
/// container (and these helper subcommands the Docker entrypoint invokes) must
/// start without a `config.toml`, matching `run_server`. Any other load error
/// (malformed TOML, failed validation) still propagates so a broken file is not
/// silently masked.
pub(crate) fn load_config_or_default(
    config_path: &std::path::Path,
) -> std::result::Result<Config, String> {
    match Config::load(config_path) {
        Ok(c) => Ok(c),
        Err(takuto_core::error::TakutoError::ConfigNotFound(_)) => Ok(Config::default()),
        Err(e) => Err(format!(
            "Failed to load config {}: {e}",
            config_path.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::load_config_or_default;
    use takuto_core::config::Config;

    /// An absent config file is the first-run bootstrap path: fall back to
    /// defaults rather than erroring, so the entrypoint subcommands run before
    /// any `config.toml` exists.
    #[test]
    fn absent_config_falls_back_to_default() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.toml");
        let cfg = load_config_or_default(&missing).expect("absent file must yield defaults");
        // A defaulted config matches Config::default()'s ticketing default.
        assert_eq!(
            cfg.general.ticketing_system,
            Config::default().general.ticketing_system
        );
    }

    /// A malformed file is a real error and must propagate (not be masked as a
    /// default) so a broken config surfaces instead of silently booting.
    #[test]
    fn malformed_config_propagates_error() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("config.toml");
        std::fs::write(&bad, "this = is = not = toml").unwrap();
        let err = load_config_or_default(&bad).expect_err("malformed config must error");
        assert!(err.contains("Failed to load config"), "got: {err}");
    }
}
