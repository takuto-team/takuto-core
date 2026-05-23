// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `docker run` argv assembly — the env curation + base flag prefix shared
//! by every `ContainerRunner` invocation.
//!
//! Holds the fixed [`WORKER_ENV`] / [`PASSTHROUGH_ENV`] lists and the
//! [`base_docker_args`] helper that builds the
//! `run --rm --name … -e … -v … -w … --entrypoint …` prefix consumed by
//! `wrap_command` and `wrap_shell_command`.

use std::path::Path;
use std::sync::Arc;

use super::dind_paths::translate_path_for_dind;
use super::secrets_bundle::SECRET_PASSTHROUGH;
use super::volumes::build_volume_args;

/// Fixed environment variables injected into every worker container.
pub(crate) const WORKER_ENV: &[(&str, &str)] = &[
    ("HOME", "/home/maestro"),
    ("MAESTRO_HOME", "/home/maestro"),
    ("CURSOR_CONFIG_DIR", "/home/maestro/.cursor"),
    ("MISE_DATA_DIR", "/home/maestro/.local/share/mise"),
    ("MISE_CACHE_DIR", "/home/maestro/.cache/mise"),
    ("MISE_CONFIG_DIR", "/home/maestro/.config/mise"),
    ("MISE_TRUST_ALL_CONFIGS", "1"),
    ("MISE_YES", "1"),
    (
        "PATH",
        "/home/maestro/.local/share/mise/shims:/usr/local/cargo/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    ),
    ("MAESTRO_CONFIG", "/etc/maestro/config.toml"),
    // Persist user-level .npmrc across worker containers (aws codeartifact login writes here)
    ("NPM_CONFIG_USERCONFIG", "/workspace/.maestro/.npmrc"),
    // Deterministic text rendering in screenshots / snapshots (Playwright, Storybook, etc.)
    ("TZ", "UTC"),
    ("LANG", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
];

/// Host environment variables forwarded into the worker when set.
pub(crate) const PASSTHROUGH_ENV: &[&str] = &[
    // Claude Code auth (token + optional base URL override)
    "CLAUDE_CODE_OAUTH_TOKEN",
    "ANTHROPIC_BASE_URL",
    "FIGMA_API_TOKEN",
    // figma-cli (`fcli`) personal access token; takes priority over stored auth.
    "FIGMA_ACCESS_TOKEN",
    // Lokalise CLI v2 (`lokalise2`) — the tool itself reads `--token`; exporting a
    // var lets users wrap invocations (e.g. `lokalise2 --token "$LOKALISE_API_TOKEN"`)
    // or write a thin shell alias in maestro.env.
    "LOKALISE_API_TOKEN",
    "CURSOR_API_KEY",
    // Ambient GH_TOKEN fallback for local development (no GitHub App / no token file).
    // When the centralized token file exists, workers read from that instead.
    "GH_TOKEN",
    // Optional: force a fixed browser bundle (must match the project's @playwright/test version).
    "PLAYWRIGHT_BROWSERS_PATH",
    // Match CI behaviour when needed (some tools tweak output when CI is set).
    "CI",
    // Override defaults above when the host sets them.
    "TZ",
    "LANG",
    "LC_ALL",
];

/// Build the common `docker run` prefix (flags, env, volumes, workdir, entrypoint)
/// before the image name and user command.
///
/// Lifted from `ContainerRunner::base_docker_args` so it can be shared
/// without leaking new public accessors on the struct. Reads exactly the
/// `&ContainerRunner` fields it needs as plain arguments.
///
/// Phase 2b.3: when `secrets_bundle` is `Some`, the legacy `PASSTHROUGH_ENV`
/// token forwarding is suppressed for the AI-provider auth env vars
/// (`CLAUDE_CODE_OAUTH_TOKEN`, `CURSOR_API_KEY`, `GH_TOKEN`,
/// `ANTHROPIC_BASE_URL`) — the worker entrypoint sources them from the
/// tmpfs files at `/run/maestro-secrets/*` instead. Non-secret env vars
/// (`CI`, `TZ`, `LANG`, `LC_ALL`, `FIGMA_*`, `LOKALISE_*`,
/// `PLAYWRIGHT_BROWSERS_PATH`) keep flowing through `-e` because they
/// aren't in the threat model. The bundle's `extra_env` (non-secret
/// names like `MAESTRO_AUTH_BUNDLE`, base URLs, `GIT_AUTHOR_*`) is
/// appended after the passthrough block.
pub(crate) fn base_docker_args(
    container_name: &str,
    entrypoint: Option<&str>,
    worktree_path: &Path,
    isolate_workspace: bool,
    secrets_bundle: Option<&Arc<crate::auth::WorkerSecretsBundle>>,
) -> Vec<String> {
    let mut args = vec![
        "run".into(),
        "--rm".into(),
        "--name".into(),
        container_name.into(),
        "--cap-add=NET_ADMIN".into(),
    ];

    for (k, v) in WORKER_ENV {
        args.push("-e".into());
        args.push(format!("{k}={v}"));
    }

    // Tokens we hide when a bundle is attached. These names must mirror
    // the keys the worker entrypoint reads from `/run/maestro-secrets/*`.
    // Single source of truth lives in `super::secrets_bundle::SECRET_PASSTHROUGH`.
    let bundle_attached = secrets_bundle.is_some();
    for key in PASSTHROUGH_ENV {
        if bundle_attached && SECRET_PASSTHROUGH.contains(key) {
            // Suppress: a bundle is in charge of this secret. Passing
            // the host's ambient value would defeat the
            // `docker inspect` mitigation.
            continue;
        }
        if let Ok(val) = std::env::var(key)
            && !val.is_empty()
        {
            args.push("-e".into());
            args.push(format!("{key}={val}"));
        }
    }

    for mount in build_volume_args(worktree_path, isolate_workspace) {
        args.push("-v".into());
        args.push(mount);
    }

    // Phase 2b.3: bundle-driven secret mount + non-secret env vars.
    if let Some(bundle) = secrets_bundle {
        // Bind-mount the per-workflow secrets dir read-only into the
        // worker. Path bytes ARE fine in `docker inspect`; secret bytes
        // are not.
        // Task #43: translate the host-side path for DinD mode (no-op
        // for local Docker). See `translate_path_for_dind`.
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

    args.push("-w".into());
    args.push(worktree_path.to_string_lossy().into_owned());

    args.push("--entrypoint".into());
    args.push(entrypoint.unwrap_or("").into());

    args
}
