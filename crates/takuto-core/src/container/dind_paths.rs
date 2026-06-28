// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Docker-in-Docker (DinD) path translation helpers.
//!
//! When `DOCKER_HOST=tcp://...` is set, the docker daemon resolves
//! bind-mount sources against its OWN filesystem, not takuto's. The
//! `takuto-data` volume is mounted at a different prefix in each side;
//! the helpers here swap the prefix so `docker run -v <src>` uses the
//! path the DinD daemon understands.

use std::path::{Path, PathBuf};

/// Env var name for the DinD-side mount prefix of the takuto `data_dir`
/// volume. Defaults to `/shared-auth/takuto-data` (the standard
/// `docker-compose.dind.yml` layout). Operators with a custom compose can
/// override.
pub(crate) const DIND_DATA_PREFIX_ENV: &str = "TAKUTO_DIND_DATA_PREFIX";

/// Takuto-side prefix of the data_dir bind mount. Hard-coded
/// because `TAKUTO_HOME` / `HOME` is the canonical path baked into
/// `docker/entrypoint.sh` and the compose volume mapping; if a deployment
/// changes this they must also update the compose file and rebuild.
pub(crate) const TAKUTO_DATA_DIR_HOST_PREFIX: &str = "/home/takuto/.takuto";

/// Translate a takuto-side absolute path to its DinD-side equivalent.
/// Used for the `WorkerSecretsBundle` bind-mount source.
///
/// When `DOCKER_HOST` is `tcp://...` (DinD mode), the daemon resolves
/// bind-mount sources against its OWN filesystem, NOT takuto's. The
/// secrets directory under `<data_dir>/runtime/secrets/` lives in the
/// `takuto-data` docker volume which is mounted at `<takuto-prefix>`
/// in takuto and `<dind-prefix>` in DinD — we swap the prefix so the
/// `-v` flag uses the path DinD understands.
///
/// When `DOCKER_HOST` is unset OR points at a unix socket (local-Docker
/// development), the takuto container IS the host, so its paths and the
/// daemon's paths agree — translation is a no-op.
pub(crate) fn translate_path_for_dind(takuto_path: &Path) -> PathBuf {
    if !is_remote_docker_daemon() {
        return takuto_path.to_path_buf();
    }
    let dind_prefix = std::env::var(DIND_DATA_PREFIX_ENV)
        .unwrap_or_else(|_| "/shared-auth/takuto-data".to_string());
    translate_path_for_dind_inner(takuto_path, TAKUTO_DATA_DIR_HOST_PREFIX, &dind_prefix)
}

/// Pure swap-prefix helper; testable without mutating process env.
/// Returns the translated path, OR the original when it doesn't lie
/// under the takuto-side prefix (logs a warning in that case because
/// the bind mount will likely fail in DinD mode).
pub(crate) fn translate_path_for_dind_inner(
    takuto_path: &Path,
    takuto_prefix: &str,
    dind_prefix: &str,
) -> PathBuf {
    match takuto_path.strip_prefix(takuto_prefix) {
        Ok(rel) => PathBuf::from(dind_prefix).join(rel),
        Err(_) => {
            tracing::warn!(
                path = %takuto_path.display(),
                takuto_prefix,
                "translate_path_for_dind: path is not under the takuto data_dir prefix; \
                 bind mount may fail in DinD mode"
            );
            takuto_path.to_path_buf()
        }
    }
}

/// Detect whether the docker daemon is on the OTHER end of a
/// network socket (i.e. DinD via `tcp://`) — in which case the daemon
/// resolves bind-mount sources in its own filesystem, and takuto must
/// translate paths. Distinct from a plain "is `DOCKER_HOST` set?" check
/// (which is true for ANY `DOCKER_HOST` including unix sockets — those
/// still share the filesystem with takuto and don't need path
/// translation).
pub(crate) fn is_remote_docker_daemon() -> bool {
    std::env::var("DOCKER_HOST")
        .map(|v| v.starts_with("tcp://"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Happy path: a takuto-side path under `<takuto-prefix>/runtime/secrets/abc`
    /// translates to `<dind-prefix>/runtime/secrets/abc`.
    #[test]
    fn translate_path_for_dind_swaps_known_prefix() {
        let got = translate_path_for_dind_inner(
            std::path::Path::new("/home/takuto/.takuto/runtime/secrets/bundle-xyz"),
            "/home/takuto/.takuto",
            "/shared-auth/takuto-data",
        );
        assert_eq!(
            got.to_string_lossy(),
            "/shared-auth/takuto-data/runtime/secrets/bundle-xyz"
        );
    }

    /// Path outside the shared volume → passed through unchanged.
    /// Lets local-Docker dev (where takuto IS the host) stay working
    /// without translation, and surfaces a warning when a DinD setup
    /// accidentally feeds an untranslatable path.
    #[test]
    fn translate_path_for_dind_returns_unchanged_when_outside_shared_volume() {
        let got = translate_path_for_dind_inner(
            std::path::Path::new("/tmp/something/outside"),
            "/home/takuto/.takuto",
            "/shared-auth/takuto-data",
        );
        assert_eq!(got.to_string_lossy(), "/tmp/something/outside");
    }

    /// Operators can supply a custom DinD-side prefix via the helper's
    /// `dind_prefix` arg (the public function reads it from the
    /// `TAKUTO_DIND_DATA_PREFIX` env var).
    #[test]
    fn translate_path_for_dind_honors_custom_prefix() {
        let got = translate_path_for_dind_inner(
            std::path::Path::new("/home/takuto/.takuto/runtime/secrets/abc"),
            "/home/takuto/.takuto",
            "/custom/dind/mount",
        );
        assert_eq!(
            got.to_string_lossy(),
            "/custom/dind/mount/runtime/secrets/abc"
        );
    }
}
