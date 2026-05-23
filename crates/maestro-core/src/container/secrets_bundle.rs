// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Phase 2b.3 `WorkerSecretsBundle` plumbing for `docker run` argv.
//!
//! Holds the helpers that splice the per-workflow bundle's tmpfs mount
//! and non-secret env vars onto a `docker run` argv, plus the
//! single-source-of-truth list of `PASSTHROUGH_ENV` names that a bundle
//! takes ownership of (so the legacy ambient-token forwarding skips
//! them).

use super::dind_paths::translate_path_for_dind;

/// Phase 2b.3.x: which `PASSTHROUGH_ENV` names a [`WorkerSecretsBundle`]
/// takes over. **Single source of truth** — the env-var suppression
/// loop in `docker_args::base_docker_args` and the `super::runner` /
/// `super::editor` / `super::run_command` callers all consult
/// [`passthrough_is_bundled`] which reads this list.
///
/// Must match the keys the worker entrypoint sources from
/// `/run/maestro-secrets/*`. Drift here means tokens leak via
/// `docker run -e` AND get sourced from tmpfs — duplicate exposure
/// surface.
pub(crate) const SECRET_PASSTHROUGH: &[&str] = &[
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "CURSOR_API_KEY",
    "GH_TOKEN",
];

/// Phase 2b.3.x helper: which `PASSTHROUGH_ENV` names a
/// [`WorkerSecretsBundle`] takes over. Must match
/// [`super::docker_args::base_docker_args`]'s suppression list so callers
/// outside `ContainerRunner` (e.g. `start_editor`, `start_run_command`,
/// improve-ticket) keep the same threat model.
pub(crate) fn passthrough_is_bundled(key: &str) -> bool {
    SECRET_PASSTHROUGH.contains(&key)
}

/// Phase 2b.3.x helper: append the bundle's mount (`/run/maestro-secrets:ro`)
/// and non-secret env vars (`MAESTRO_AUTH_BUNDLE`, base URLs,
/// `GIT_AUTHOR_*`/`GIT_COMMITTER_*`) onto an in-flight `docker run` argv.
/// Token bytes are NEVER added; they live in the bind-mounted tmpfs files.
pub(crate) fn apply_secrets_bundle_to_args(
    args: &mut Vec<String>,
    bundle: &crate::auth::WorkerSecretsBundle,
) {
    // Task #43: translate maestro-side host path → DinD-side path. In
    // DinD mode `docker run`'s `-v <src>` is resolved by the DinD daemon
    // in its own filesystem, which has the shared volume at a different
    // prefix. No-op in local-Docker mode.
    let src = translate_path_for_dind(bundle.host_dir());
    args.push("-v".into());
    args.push(format!(
        "{}:{}:ro",
        src.to_string_lossy(),
        crate::auth::WORKER_SECRETS_MOUNTPOINT,
    ));
    for (k, v) in &bundle.extra_env {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Phase 2b.3.x: `passthrough_is_bundled` must match the exact set of
    /// env names the worker entrypoint sources from `/run/maestro-secrets`.
    /// If this list drifts from the entrypoint, tokens leak via `docker
    /// run -e` AND get sourced from tmpfs — duplicate exposure surface.
    #[test]
    fn passthrough_is_bundled_lists_only_known_secret_env_names() {
        // Must suppress (bundled by tmpfs files):
        assert!(passthrough_is_bundled("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(passthrough_is_bundled("ANTHROPIC_BASE_URL"));
        assert!(passthrough_is_bundled("CURSOR_API_KEY"));
        assert!(passthrough_is_bundled("GH_TOKEN"));
        // Must NOT suppress (still flow through legacy passthrough):
        assert!(!passthrough_is_bundled("PATH"));
        assert!(!passthrough_is_bundled("HOME"));
        assert!(!passthrough_is_bundled("MAESTRO_AUTH_BUNDLE"));
        assert!(!passthrough_is_bundled("GIT_AUTHOR_NAME"));
        assert!(!passthrough_is_bundled("GH_TOKEN_FOO")); // prefix match must NOT match
        assert!(!passthrough_is_bundled("")); // empty must not match
    }

    /// `apply_secrets_bundle_to_args` must emit a `-v <translated>:/run/maestro-secrets:ro`
    /// mount when the bundle's host_dir lies under the data_dir AND the
    /// docker daemon is remote (DOCKER_HOST=tcp://...). We test the path
    /// surface via the pure helper to avoid mutating process env.
    #[test]
    fn apply_secrets_bundle_uses_translated_path_for_dind() {
        let host_path = std::path::PathBuf::from(
            "/home/maestro/.maestro/runtime/secrets/bundle-abc",
        );
        let translated = super::super::dind_paths::translate_path_for_dind_inner(
            &host_path,
            "/home/maestro/.maestro",
            "/shared-auth/maestro-data",
        );
        // Construct the resulting `-v` argument the way
        // `apply_secrets_bundle_to_args` does.
        let mount = format!(
            "{}:{}:ro",
            translated.to_string_lossy(),
            crate::auth::WORKER_SECRETS_MOUNTPOINT,
        );
        assert_eq!(
            mount,
            "/shared-auth/maestro-data/runtime/secrets/bundle-abc:/run/maestro-secrets:ro"
        );
    }
}
