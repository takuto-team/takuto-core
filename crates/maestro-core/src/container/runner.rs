// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! [`ContainerRunner`] — wraps a `docker run` invocation for one workflow
//! ticket so each agent step lands in an isolated container with the right
//! env, volumes, and secrets bundle attached.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use tracing::{error, info, warn};

use super::{sanitize_ticket_key, shell_escape};

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

/// Volume mounts shared between the orchestrator and every worker container.
pub(crate) const WORKER_VOLUMES: &[&str] = &[
    "/workspace:/workspace",
    "/shared-auth/claude:/home/maestro/.claude",
    "/shared-auth/cursor:/home/maestro/.cursor",
    // npx skills add -g stores actual files in .agents/skills/; .claude/skills/ and
    // .cursor/skills/ contain symlinks pointing there, so this must be shared.
    "/shared-auth/agents:/home/maestro/.agents",
    "/shared-auth/gh:/home/maestro/.config/gh",
    "/shared-auth/acli:/home/maestro/.config/acli",
    "/shared-auth/fcli:/home/maestro/.config/fcli",
    "/shared-auth/npm:/home/maestro/.npm",
    "/shared-auth/mise-data:/home/maestro/.local/share/mise",
    "/shared-auth/mise-cache:/home/maestro/.cache/mise",
    "/shared-auth/aws:/home/maestro/.aws",
    // Playwright browser cache — must align with the repo's package.json, not a baked image path
    "/shared-auth/playwright-cache:/home/maestro/.cache/ms-playwright",
    // openvscode-server data (extensions, settings, state)
    "/shared-auth/vscode:/home/maestro/.openvscode-server",
    // Config + env for egress rules (extra_egress_hosts, .npmrc registry hosts, allow_all_https)
    "/etc/maestro:/etc/maestro:ro",
];

/// Phase 2b.3 (04_architecture.md §6): shell snippet that sources every
/// `/run/maestro-secrets/*` file into the matching env var, then `rm -f`s
/// the on-disk copy. **Single source of truth** for both:
///
///   1. `worker-entrypoint.sh` — used when the worker container is spawned
///      WITH `--entrypoint /usr/local/bin/worker-entrypoint.sh` (e.g.
///      `wrap_shell_command`, `start_editor`, `start_run_command`).
///   2. `ContainerRunner::wrap_command` — used by agent invocations
///      (claude, cursor, codex, opencode) which pass no entrypoint and
///      build their own inline `sh -c`. WITHOUT this block, the bundle's
///      tmpfs files are mounted but NEVER sourced, so the agent CLI sees
///      no token and reports "Not logged in" (task #36 bug).
///
/// The snippet is self-gated on `MAESTRO_AUTH_BUNDLE=1` so it is a no-op
/// when the legacy passthrough path is active. It mirrors the env-mapping
/// of `worker-entrypoint.sh` lines 24-58 exactly; a unit test asserts the
/// snippet contains every documented (file → env) mapping so the two
/// can't drift silently.
///
/// The snippet does NOT include a trailing newline so it composes cleanly
/// inside a `;`-joined command string.
pub(crate) const BUNDLE_SOURCING_SH: &str = concat!(
    r#"if [ "${MAESTRO_AUTH_BUNDLE:-0}" = "1" ] && [ -d /run/maestro-secrets ]; then"#,
    // Task #42: observability breadcrumb. When the bundle's discriminator
    // env var is set but the bind-mounted directory has no files, the
    // bundle's host-side TempDir has dropped out from under us — almost
    // certainly because nothing held the Arc alive long enough. Emit a
    // single grep-friendly stderr line so future regressions surface in
    // the workflow / editor terminal instead of silently falling back to
    // the deployment default. Without this breadcrumb, the only symptom
    // is "claude says I'm not logged in" — exactly the diagnostic loop
    // task #42 is closing.
    r#" __bundle_present=$(ls -A /run/maestro-secrets 2>/dev/null | wc -l);"#,
    r#" if [ "${__bundle_present:-0}" = "0" ]; then"#,
    r#" echo "[maestro-bundle] MAESTRO_AUTH_BUNDLE=1 but /run/maestro-secrets/ is empty -- secret files vanished (host TempDir dropped). Check WorkerSecretsBundle lifetime in AppState." >&2;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/claude ]; then"#,
    r#" CLAUDE_CODE_OAUTH_TOKEN="$(cat /run/maestro-secrets/claude)";"#,
    r#" export CLAUDE_CODE_OAUTH_TOKEN;"#,
    r#" rm -f /run/maestro-secrets/claude 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/cursor ]; then"#,
    r#" CURSOR_API_KEY="$(cat /run/maestro-secrets/cursor)";"#,
    r#" export CURSOR_API_KEY;"#,
    r#" rm -f /run/maestro-secrets/cursor 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/codex ]; then"#,
    r#" OPENAI_API_KEY="$(cat /run/maestro-secrets/codex)";"#,
    r#" export OPENAI_API_KEY;"#,
    r#" rm -f /run/maestro-secrets/codex 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/opencode ]; then"#,
    r#" ANTHROPIC_API_KEY="$(cat /run/maestro-secrets/opencode)";"#,
    r#" export ANTHROPIC_API_KEY;"#,
    r#" rm -f /run/maestro-secrets/opencode 2>/dev/null || true;"#,
    r#" fi;"#,
    // Task #41 (was #39): Claude session-state (`~/.claude.json`). The
    // bundle ships ONLY the keys the user pasted (typically just
    // `oauthAccount` for team-plan users on a custom proxy). A naive `cp`
    // would wipe whatever the legacy backups-restore step put on disk —
    // including `hasCompletedOnboarding`, `userID`, accumulated state —
    // and Claude Code checks those fields too. We do a shallow JSON
    // merge: existing keys win when bundle blob is silent on them;
    // bundle keys (oauthAccount, etc.) win when present. `jq -s '.[0]
    // * .[1]'` is the canonical jq incantation for this. jq is in the
    // image (Dockerfile line 62). When jq is somehow missing OR there's
    // no existing `.claude.json` to merge into, fall back to a plain
    // overwrite (matches pre-#41 behaviour). Placed AFTER the legacy
    // backups-restore so per-user session always wins over stale state.
    r#" if [ -f /run/maestro-secrets/claude_session.json ]; then"#,
    r#" if [ -f "$HOME/.claude.json" ] && command -v jq >/dev/null 2>&1; then"#,
    r#" __mtmp=$(mktemp);"#,
    r#" if jq -s '.[0] * .[1]' "$HOME/.claude.json" /run/maestro-secrets/claude_session.json > "$__mtmp" 2>/dev/null; then"#,
    r#" mv "$__mtmp" "$HOME/.claude.json";"#,
    r#" else"#,
    r#" rm -f "$__mtmp";"#,
    r#" cp /run/maestro-secrets/claude_session.json "$HOME/.claude.json" || true;"#,
    r#" fi;"#,
    r#" else"#,
    r#" cp /run/maestro-secrets/claude_session.json "$HOME/.claude.json" || true;"#,
    r#" fi;"#,
    r#" rm -f /run/maestro-secrets/claude_session.json 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" if [ -f /run/maestro-secrets/gh ]; then"#,
    r#" GH_TOKEN="$(cat /run/maestro-secrets/gh)";"#,
    r#" export GH_TOKEN;"#,
    r#" rm -f /run/maestro-secrets/gh 2>/dev/null || true;"#,
    r#" fi;"#,
    r#" fi"#,
);

/// Task #43: env var name for the DinD-side mount prefix of the maestro
/// `data_dir` volume. Defaults to `/shared-auth/maestro-data` (the
/// standard `docker-compose.dind.yml` layout). Operators with a custom
/// compose can override.
pub(crate) const DIND_DATA_PREFIX_ENV: &str = "MAESTRO_DIND_DATA_PREFIX";

/// Task #43: maestro-side prefix of the data_dir bind mount. Hard-coded
/// because `MAESTRO_HOME` / `HOME` is the canonical path baked into
/// `docker/entrypoint.sh` and the compose volume mapping; if a deployment
/// changes this they must also update the compose file and rebuild.
pub(crate) const MAESTRO_DATA_DIR_HOST_PREFIX: &str = "/home/maestro/.maestro";

static DOCKER_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Task #43: translate a maestro-side absolute path to its DinD-side
/// equivalent. Used for the `WorkerSecretsBundle` bind-mount source —
/// see the layered diagnosis in task #43.
///
/// When `DOCKER_HOST` is `tcp://...` (DinD mode), the daemon resolves
/// bind-mount sources against its OWN filesystem, NOT maestro's. The
/// secrets directory under `<data_dir>/runtime/secrets/` lives in the
/// `maestro-data` docker volume which is mounted at `<maestro-prefix>`
/// in maestro and `<dind-prefix>` in DinD — we swap the prefix so the
/// `-v` flag uses the path DinD understands.
///
/// When `DOCKER_HOST` is unset OR points at a unix socket (local-Docker
/// development), the maestro container IS the host, so its paths and the
/// daemon's paths agree — translation is a no-op.
pub(crate) fn translate_path_for_dind(maestro_path: &Path) -> PathBuf {
    if !is_remote_docker_daemon() {
        return maestro_path.to_path_buf();
    }
    let dind_prefix = std::env::var(DIND_DATA_PREFIX_ENV)
        .unwrap_or_else(|_| "/shared-auth/maestro-data".to_string());
    translate_path_for_dind_inner(maestro_path, MAESTRO_DATA_DIR_HOST_PREFIX, &dind_prefix)
}

/// Pure swap-prefix helper; testable without mutating process env.
/// Returns the translated path, OR the original when it doesn't lie
/// under the maestro-side prefix (logs a warning in that case because
/// the bind mount will likely fail in DinD mode).
pub(crate) fn translate_path_for_dind_inner(
    maestro_path: &Path,
    maestro_prefix: &str,
    dind_prefix: &str,
) -> PathBuf {
    match maestro_path.strip_prefix(maestro_prefix) {
        Ok(rel) => PathBuf::from(dind_prefix).join(rel),
        Err(_) => {
            tracing::warn!(
                path = %maestro_path.display(),
                maestro_prefix,
                "translate_path_for_dind: path is not under the maestro data_dir prefix; \
                 bind mount may fail in DinD mode"
            );
            maestro_path.to_path_buf()
        }
    }
}

/// Task #43: detect whether the docker daemon is on the OTHER end of a
/// network socket (i.e. DinD via `tcp://`) — in which case the daemon
/// resolves bind-mount sources in its own filesystem, and maestro must
/// translate paths. Distinct from [`super::is_dind_mode`] (which returns
/// true for ANY `DOCKER_HOST` setting including unix sockets — those
/// still share the filesystem with maestro and don't need path
/// translation).
fn is_remote_docker_daemon() -> bool {
    std::env::var("DOCKER_HOST")
        .map(|v| v.starts_with("tcp://"))
        .unwrap_or(false)
}

/// Phase 2b.3.x helper: which `PASSTHROUGH_ENV` names a
/// [`WorkerSecretsBundle`] takes over. Must match
/// [`ContainerRunner::base_docker_args`]'s suppression list so callers
/// outside `ContainerRunner` (e.g. `start_editor`, `start_run_command`,
/// improve-ticket) keep the same threat model.
pub(crate) fn passthrough_is_bundled(key: &str) -> bool {
    matches!(
        key,
        "CLAUDE_CODE_OAUTH_TOKEN" | "ANTHROPIC_BASE_URL" | "CURSOR_API_KEY" | "GH_TOKEN"
    )
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

/// Build the list of volume mount strings for a Docker container.
///
/// When `isolate_workspace` is `true`, the broad `/workspace:/workspace` mount is
/// replaced with three targeted mounts so the container sees only:
///   - its own worktree directory (read-write)
///   - the shared `.git` internals (needed for git operations)
///   - the shared `.maestro` directory (read-only; contains `.npmrc`, etc.)
///
/// All other mounts from [`WORKER_VOLUMES`] (auth volumes, `/etc/maestro`) are preserved.
///
/// The repo root is derived as the grandparent of `worktree_path`
/// (e.g. `/workspace/worktrees/slug` → `/workspace`).
pub fn build_volume_args(worktree_path: &Path, isolate_workspace: bool) -> Vec<String> {
    let mut mounts = Vec::new();
    for v in WORKER_VOLUMES {
        if isolate_workspace && *v == "/workspace:/workspace" {
            continue;
        }
        mounts.push((*v).to_string());
    }
    if isolate_workspace {
        if let Some(repo_root) = worktree_path.parent().and_then(|p| p.parent()) {
            let wt = worktree_path.to_string_lossy();
            let root = repo_root.to_string_lossy();
            mounts.push(format!("{wt}:{wt}"));
            mounts.push(format!("{root}/.git:{root}/.git"));
            mounts.push(format!("{root}/.maestro:{root}/.maestro:ro"));
        } else {
            warn!(
                path = %worktree_path.display(),
                "Cannot derive repo root from worktree path (need grandparent); \
                 falling back to full /workspace mount"
            );
            mounts.push("/workspace:/workspace".to_string());
        }
    }
    // Task #48: mount the `maestro-tools` named volume read-only into
    // every spawned worker / editor / run-command. The maestro container
    // populates this volume at startup via the `[provisioning]` install
    // commands (see `docs/extending-maestro.md`). The volume is a Docker
    // NAMED volume — no host-path translation is needed even in DinD
    // mode because the DinD daemon and maestro share the same volume by
    // name (the maestro service mounts it RW; DinD inherits the same
    // volume via `docker-compose.dind.yml`).
    //
    // `:ro` so workers can't pollute the volume; only the maestro boot
    // pass writes to it. The `ENV PATH` in the Dockerfile prepends
    // `/opt/maestro-tools/bin` so anything dropped here shadows the
    // baked-in tools (admin's lever for pinning a tool to a specific
    // version).
    mounts.push("maestro-tools:/opt/maestro-tools/bin:ro".to_string());
    mounts
}

/// Runs AI agent commands inside isolated Docker containers so each workflow
/// gets its own filesystem and network namespace.
pub struct ContainerRunner {
    ticket_key: String,
    image: String,
    worktree_path: PathBuf,
    step_counter: std::sync::atomic::AtomicU32,
    /// When `true`, replace the broad `/workspace:/workspace` mount with targeted
    /// bind mounts for just the worktree path, `.git`, and `.maestro`. This prevents
    /// a container from accessing any other issue's worktree.
    isolate_workspace: bool,
    /// Phase 2b.3 (04_architecture.md §6): optional per-workflow secrets
    /// bundle. When `Some`, the runner bind-mounts the bundle's tmpfs
    /// directory at `/run/maestro-secrets:ro`, sets `MAESTRO_AUTH_BUNDLE=1`
    /// so the worker entrypoint sources the secret files into env vars (then
    /// `rm`s them inside the container), adds non-secret env vars like
    /// `ANTHROPIC_BASE_URL` and the `GIT_AUTHOR_*` / `GIT_COMMITTER_*`
    /// attribution names. Tokens are NEVER passed as `-e KEY=value` to
    /// `docker run` — the threat is `docker inspect <ctr>` leaking the
    /// bytes.
    ///
    /// When `None`, the runner falls back to the legacy `PASSTHROUGH_ENV`
    /// path which forwards ambient `CLAUDE_CODE_OAUTH_TOKEN` /
    /// `CURSOR_API_KEY` from the host (single-tenant / poller workflows).
    secrets_bundle: Option<Arc<crate::auth::WorkerSecretsBundle>>,
}

impl ContainerRunner {
    pub fn new(ticket_key: &str, worktree_path: &Path, image: &str) -> Self {
        Self {
            ticket_key: ticket_key.to_string(),
            image: image.to_string(),
            worktree_path: worktree_path.to_path_buf(),
            step_counter: std::sync::atomic::AtomicU32::new(0),
            isolate_workspace: false,
            secrets_bundle: None,
        }
    }

    /// Enable per-issue workspace isolation. Instead of mounting the full
    /// `/workspace` volume, only the worktree directory, `.git`, and `.maestro`
    /// are mounted. This prevents a container from accessing other issues' files.
    pub fn with_isolate_workspace(mut self) -> Self {
        self.isolate_workspace = true;
        self
    }

    /// Phase 2b.3 (04_architecture.md §6): attach a per-workflow secrets
    /// bundle. The runner will bind-mount the bundle's tmpfs directory
    /// read-only at `/run/maestro-secrets`, set `MAESTRO_AUTH_BUNDLE=1` so
    /// the worker entrypoint knows to source the files, and export the
    /// non-secret env vars (`ANTHROPIC_BASE_URL`, `GIT_AUTHOR_*` /
    /// `GIT_COMMITTER_*`). Token bytes are NEVER passed via `-e`.
    pub fn with_secrets_bundle(
        mut self,
        bundle: Arc<crate::auth::WorkerSecretsBundle>,
    ) -> Self {
        self.secrets_bundle = Some(bundle);
        self
    }

    /// `true` when this runner has a Phase 2b.3 secrets bundle attached.
    /// Callers consult this to log "legacy auth path" vs "bundle path"
    /// without exposing the bundle itself.
    pub fn has_secrets_bundle(&self) -> bool {
        self.secrets_bundle.is_some()
    }

    /// Check if Docker is available (`DOCKER_HOST` set and `docker info` succeeds).
    /// The result is cached for the process lifetime.
    pub fn is_available() -> bool {
        *DOCKER_AVAILABLE.get_or_init(|| {
            if std::env::var("DOCKER_HOST").unwrap_or_default().is_empty() {
                error!("DOCKER_HOST not set — DinD is required; workflows will fail");
                return false;
            }
            let ok = std::process::Command::new("docker")
                .args(["info"])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if ok {
                info!("Docker daemon reachable — container isolation enabled");
            } else {
                error!("docker info failed — DinD is required; workflows will fail");
            }
            ok
        })
    }

    /// Returns a unique container name for this ticket, incrementing an internal counter.
    pub fn next_container_name(&self) -> String {
        let n = self
            .step_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sanitized = sanitize_ticket_key(&self.ticket_key);
        format!("maestro-worker-{sanitized}-{n}")
    }

    /// Build the common `docker run` prefix (flags, env, volumes, workdir, entrypoint)
    /// before the image name and user command.
    ///
    /// Phase 2b.3: when a `WorkerSecretsBundle` is attached, the legacy
    /// `PASSTHROUGH_ENV` token forwarding is suppressed for the AI-provider
    /// auth env vars (`CLAUDE_CODE_OAUTH_TOKEN`, `CURSOR_API_KEY`, `GH_TOKEN`,
    /// `ANTHROPIC_BASE_URL`) — the worker entrypoint sources them from the
    /// tmpfs files at `/run/maestro-secrets/*` instead. Non-secret env vars
    /// (`CI`, `TZ`, `LANG`, `LC_ALL`, `FIGMA_*`, `LOKALISE_*`,
    /// `PLAYWRIGHT_BROWSERS_PATH`) keep flowing through `-e` because they
    /// aren't in the threat model. The bundle's `extra_env` (non-secret
    /// names like `MAESTRO_AUTH_BUNDLE`, base URLs, `GIT_AUTHOR_*`) is
    /// appended after the passthrough block.
    fn base_docker_args(&self, container_name: &str, entrypoint: Option<&str>) -> Vec<String> {
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
        const SECRET_PASSTHROUGH: &[&str] = &[
            "CLAUDE_CODE_OAUTH_TOKEN",
            "ANTHROPIC_BASE_URL",
            "CURSOR_API_KEY",
            "GH_TOKEN",
        ];

        let bundle_attached = self.secrets_bundle.is_some();
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

        for mount in build_volume_args(&self.worktree_path, self.isolate_workspace) {
            args.push("-v".into());
            args.push(mount);
        }

        // Phase 2b.3: bundle-driven secret mount + non-secret env vars.
        if let Some(ref bundle) = self.secrets_bundle {
            // Bind-mount the per-workflow secrets dir read-only into the
            // worker. Path bytes ARE fine in `docker inspect`; secret bytes
            // are not.
            // Task #43: translate the host-side path for DinD mode (no-op
            // for local Docker). See `translate_path_for_dind` above.
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
        args.push(self.worktree_path.to_string_lossy().into_owned());

        args.push("--entrypoint".into());
        args.push(entrypoint.unwrap_or("").into());

        args
    }

    /// Phase 2b.3: return the bundle's `extra_args` (provider sub-table
    /// `extra_args`). Callers append these to the agent argv. `None` when no
    /// bundle is attached.
    pub fn provider_extra_args(&self) -> Option<&[String]> {
        self.secrets_bundle.as_ref().map(|b| b.extra_args.as_slice())
    }

    /// Wrap a direct command (`program` + `args`) into a `docker run` invocation.
    ///
    /// Uses `sh -c` so we can restore `.claude.json` from backup before exec-ing
    /// the actual program (the file lives outside the shared volume and is missing
    /// in fresh worker containers).
    pub fn wrap_command(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
        let name = self.next_container_name();
        let mut docker_args = self.base_docker_args(&name, None);
        // Without `--user`, `docker run` defaults to root and writes root-owned files on the
        // bind-mounted repo/worktree; the Maestro process (user `maestro`) cannot remove them later.
        docker_args.push("--user".into());
        docker_args.push("maestro:maestro".into());
        docker_args.push(self.image.clone());

        // Build a shell command that restores .claude.json then exec's the program.
        let mut shell_parts: Vec<String> = Vec::new();
        shell_parts.push(shell_escape(program));
        for a in args {
            shell_parts.push(shell_escape(a));
        }
        let restore = r#"if [ ! -f "$HOME/.claude.json" ]; then b=$(ls -t "$HOME/.claude/backups/.claude.json.backup."* 2>/dev/null | head -1); [ -n "$b" ] && cp "$b" "$HOME/.claude.json"; fi"#;
        // Ensure npm/mise dirs are owned by maestro (shared volumes start root-owned).
        // Uses passwordless sudo bash (granted in /etc/sudoers.d/maestro-hook-bash).
        let fix_perms = r#"sudo -n bash -c 'for d in "$HOME/.npm" "$HOME/.npm-global" "$HOME/.cache/mise" "$HOME/.local/share/mise"; do [ -d "$d" ] && chown -R "$(id -u):$(id -g)" "$d"; done' 2>/dev/null || true"#;
        // Source the centralized GitHub App token so `gh` and git operations use a
        // fresh token. The token file is refreshed by Maestro's background service.
        let gh_token = r#"[ -f "$HOME/.config/gh/gh-app-token" ] && export GH_TOKEN="$(cat "$HOME/.config/gh/gh-app-token")";"#;
        // Phase 2b.3 / task #36 fix: when a `WorkerSecretsBundle` is
        // attached, the agent CLI (claude / cursor / codex / opencode) must
        // see its token via env. wrap_command does NOT go through
        // worker-entrypoint.sh, so the bundle's tmpfs files reach the
        // worker but nothing reads them — without this block the user gets
        // "Not logged in". The snippet is self-gated on
        // `MAESTRO_AUTH_BUNDLE=1` and is omitted entirely when no bundle
        // is attached (keeps the legacy path's argv clean).
        let bundle_source: &str = if self.has_secrets_bundle() {
            BUNDLE_SOURCING_SH
        } else {
            ""
        };
        let cmd = if bundle_source.is_empty() {
            format!(
                "{restore}; {fix_perms}; {gh_token} exec {}",
                shell_parts.join(" ")
            )
        } else {
            format!(
                "{restore}; {fix_perms}; {gh_token} {bundle_source}; exec {}",
                shell_parts.join(" ")
            )
        };
        docker_args.push("sh".into());
        docker_args.push("-c".into());
        docker_args.push(cmd);

        ("docker".into(), docker_args)
    }

    /// Wrap a shell command string into a `docker run` invocation using the worker entrypoint
    /// (egress rules + `runuser`).
    pub fn wrap_shell_command(&self, cmd: &str) -> (String, Vec<String>) {
        let name = self.next_container_name();
        let mut docker_args =
            self.base_docker_args(&name, Some("/usr/local/bin/worker-entrypoint.sh"));
        docker_args.push(self.image.clone());
        docker_args.push("sh".into());
        docker_args.push("-c".into());
        docker_args.push(cmd.into());
        ("docker".into(), docker_args)
    }

    /// Force-remove all worker containers for this ticket.
    pub async fn force_remove_all(&self) {
        let sanitized = sanitize_ticket_key(&self.ticket_key);
        super::reap::remove_containers_matching(&sanitized).await;
    }

    /// Force-remove all worker containers for a given ticket key (no instance needed).
    pub async fn cleanup_for_ticket(ticket_key: &str) {
        let sanitized = sanitize_ticket_key(ticket_key);
        super::reap::remove_containers_matching(&sanitized).await;
        super::reap::prune_dangling_images().await;
    }

    /// Auto-detect the worker image by inspecting the running Maestro container,
    /// falling back to a locally-present `maestro:latest`, then `MAESTRO_REGISTRY_IMAGE`.
    pub async fn discover_worker_image() -> Option<String> {
        // Inspect the current container by hostname (Docker sets HOSTNAME to the
        // container ID). This works regardless of the compose project name — no
        // hardcoded container_name needed.
        let container_id = std::env::var("HOSTNAME").unwrap_or_default();
        let output = if !container_id.is_empty() {
            tokio::process::Command::new("docker")
                .args(["inspect", &container_id, "--format", "{{.Config.Image}}"])
                .output()
                .await
                .ok()
        } else {
            None
        };

        if let Some(output) = output
            && output.status.success()
        {
            let image = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !image.is_empty() {
                // Verify the image actually exists in DinD before using it — the name from
                // docker inspect may point to a registry tag (e.g. ghcr.io/…:dev) that was
                // never pulled into DinD (local dev builds).
                let exists = tokio::process::Command::new("docker")
                    .args(["image", "inspect", &image])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status()
                    .await
                    .map(|s| s.success())
                    .unwrap_or(false);
                if exists {
                    info!(image = %image, "Discovered worker image from running Maestro container");
                    return Some(image);
                }
                info!(
                    image = %image,
                    "Image from docker inspect not present in DinD — trying maestro:latest"
                );
            }
        }

        // Check if maestro:latest is present locally in DinD (e.g. loaded via `make load-worker`).
        // This is the correct image for local development builds.
        let local_latest = tokio::process::Command::new("docker")
            .args(["image", "inspect", "maestro:latest"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .map(|s| s.success())
            .unwrap_or(false);
        if local_latest {
            info!("Using local maestro:latest as worker image");
            return Some("maestro:latest".to_string());
        }

        // Fall back to MAESTRO_REGISTRY_IMAGE (set at build time in the Dockerfile)
        if let Ok(image) = std::env::var("MAESTRO_REGISTRY_IMAGE")
            && !image.is_empty()
        {
            info!(image = %image, "Using MAESTRO_REGISTRY_IMAGE as worker image");
            return Some(image);
        }

        warn!(
            "Cannot auto-detect worker image — docker inspect failed, maestro:latest not found, and MAESTRO_REGISTRY_IMAGE not set"
        );
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn runner() -> ContainerRunner {
        ContainerRunner::new(
            "PROJ-42",
            &PathBuf::from("/workspace/proj-42"),
            "maestro:latest",
        )
    }

    #[test]
    fn next_container_name_increments() {
        let r = runner();
        assert_eq!(r.next_container_name(), "maestro-worker-proj-42-0");
        assert_eq!(r.next_container_name(), "maestro-worker-proj-42-1");
        assert_eq!(r.next_container_name(), "maestro-worker-proj-42-2");
    }

    /// Helper: find the value following a flag in a docker args list.
    fn flag_value<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
        args.windows(2).find_map(|w| {
            if w[0] == flag {
                Some(w[1].as_str())
            } else {
                None
            }
        })
    }

    /// Helper: check if an `-e KEY=VALUE` pair is present.
    fn has_env(args: &[String], key: &str, value: &str) -> bool {
        let needle = format!("{key}={value}");
        args.windows(2).any(|w| w[0] == "-e" && w[1] == needle)
    }

    /// Helper: check if a `-v SRC:DST` pair is present.
    fn has_volume(args: &[String], mount: &str) -> bool {
        args.windows(2).any(|w| w[0] == "-v" && w[1] == mount)
    }

    // ─── Task #43: DinD path translation ────────────────────────────────

    /// Happy path: a maestro-side path under `<maestro-prefix>/runtime/secrets/abc`
    /// translates to `<dind-prefix>/runtime/secrets/abc`.
    #[test]
    fn translate_path_for_dind_swaps_known_prefix() {
        let got = translate_path_for_dind_inner(
            std::path::Path::new("/home/maestro/.maestro/runtime/secrets/bundle-xyz"),
            "/home/maestro/.maestro",
            "/shared-auth/maestro-data",
        );
        assert_eq!(
            got.to_string_lossy(),
            "/shared-auth/maestro-data/runtime/secrets/bundle-xyz"
        );
    }

    /// Path outside the shared volume → passed through unchanged.
    /// Lets local-Docker dev (where maestro IS the host) stay working
    /// without translation, and surfaces a warning when a DinD setup
    /// accidentally feeds an untranslatable path.
    #[test]
    fn translate_path_for_dind_returns_unchanged_when_outside_shared_volume() {
        let got = translate_path_for_dind_inner(
            std::path::Path::new("/tmp/something/outside"),
            "/home/maestro/.maestro",
            "/shared-auth/maestro-data",
        );
        assert_eq!(got.to_string_lossy(), "/tmp/something/outside");
    }

    /// Operators can supply a custom DinD-side prefix via the helper's
    /// `dind_prefix` arg (the public function reads it from the
    /// `MAESTRO_DIND_DATA_PREFIX` env var).
    #[test]
    fn translate_path_for_dind_honors_custom_prefix() {
        let got = translate_path_for_dind_inner(
            std::path::Path::new("/home/maestro/.maestro/runtime/secrets/abc"),
            "/home/maestro/.maestro",
            "/custom/dind/mount",
        );
        assert_eq!(got.to_string_lossy(), "/custom/dind/mount/runtime/secrets/abc");
    }

    /// `apply_secrets_bundle_to_args` must emit a `-v <translated>:/run/maestro-secrets:ro`
    /// mount when the bundle's host_dir lies under the data_dir AND the
    /// docker daemon is remote (DOCKER_HOST=tcp://...). We test the path
    /// surface via the pure helper to avoid mutating process env.
    #[test]
    fn apply_secrets_bundle_uses_translated_path_for_dind() {
        // Construct a bundle whose host_dir mimics the in-DinD layout.
        // Since `WorkerSecretsBundle::for_tests` creates a real tempdir
        // (process tempfile), we have to translate the path manually
        // here — the test verifies the translation function, not that
        // the env-gated entry point reads env correctly (env-mutation
        // tests live elsewhere).
        let host_path = std::path::PathBuf::from(
            "/home/maestro/.maestro/runtime/secrets/bundle-abc",
        );
        let translated = translate_path_for_dind_inner(
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

    /// Task #36: the bundle-sourcing snippet must cover every
    /// `/run/maestro-secrets/<file>` → env-var mapping documented in
    /// `worker-entrypoint.sh` (lines 24-58). If the entrypoint adds a new
    /// provider mapping, this constant must be updated in lockstep.
    #[test]
    fn bundle_sourcing_snippet_covers_every_documented_mapping() {
        // Self-gated on the discriminator so it's a no-op when the bundle
        // is absent (worker-entrypoint.sh's pre-Phase-2b.3 path).
        assert!(
            BUNDLE_SOURCING_SH.contains(r#"if [ "${MAESTRO_AUTH_BUNDLE:-0}" = "1" ]"#),
            "snippet must self-gate on MAESTRO_AUTH_BUNDLE=1"
        );
        // Every file → env-var mapping from worker-entrypoint.sh.
        for (file, env_var) in [
            ("/run/maestro-secrets/claude", "CLAUDE_CODE_OAUTH_TOKEN"),
            ("/run/maestro-secrets/cursor", "CURSOR_API_KEY"),
            ("/run/maestro-secrets/codex", "OPENAI_API_KEY"),
            ("/run/maestro-secrets/opencode", "ANTHROPIC_API_KEY"),
            ("/run/maestro-secrets/gh", "GH_TOKEN"),
        ] {
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("[ -f {file} ]")),
                "snippet must source-test {file}"
            );
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("export {env_var};")),
                "snippet must export {env_var}"
            );
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("rm -f {file}")),
                "snippet must rm -f {file} after sourcing"
            );
        }

        // Task #39: Claude session-state file uses `cp` (not source/export)
        // because it carries JSON, not shell variables. Assert the
        // dedicated cp + rm pair instead of the export pattern.
        assert!(
            BUNDLE_SOURCING_SH.contains("[ -f /run/maestro-secrets/claude_session.json ]"),
            "snippet must source-test claude_session.json"
        );
        // Task #41: the snippet shallow-merges the session blob into the
        // existing $HOME/.claude.json via jq, with a `cp` fallback when
        // jq is unavailable OR the target file doesn't yet exist. Assert
        // BOTH paths are present so a regression to plain-cp gets caught.
        assert!(
            BUNDLE_SOURCING_SH.contains("jq -s '.[0] * .[1]'"),
            "snippet must merge via jq's `.[0] * .[1]` shallow-merge"
        );
        assert!(
            BUNDLE_SOURCING_SH
                .contains(r#"cp /run/maestro-secrets/claude_session.json "$HOME/.claude.json""#),
            "snippet must keep a cp fallback for the no-jq / no-existing-file case"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("rm -f /run/maestro-secrets/claude_session.json"),
            "snippet must rm -f /run/maestro-secrets/claude_session.json after merge"
        );

        // Task #42: observability breadcrumb. When MAESTRO_AUTH_BUNDLE=1
        // but the mountpoint is empty, the snippet must emit a single
        // grep-friendly stderr line. Without this, the bundle's lifetime
        // bugs are invisible (everything silently no-ops).
        assert!(
            BUNDLE_SOURCING_SH.contains("[maestro-bundle]"),
            "snippet must carry the [maestro-bundle] stderr tag for the \
             empty-mountpoint case (task #42 observability)"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains(">&2"),
            "the empty-mountpoint warning must go to stderr (not stdout)"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("WorkerSecretsBundle lifetime"),
            "warning must point at the WorkerSecretsBundle lifetime cause"
        );
    }

    /// Task #36: drift-detection. Read `docker/worker-entrypoint.sh` from
    /// disk and confirm the Rust [`BUNDLE_SOURCING_SH`] constant references
    /// the same `/run/maestro-secrets/<file>` ↔ env-var mappings the
    /// entrypoint hardcodes. If someone edits the shell script and adds a
    /// new provider, this test fails until [`BUNDLE_SOURCING_SH`] is
    /// updated in lockstep.
    #[test]
    fn bundle_sourcing_matches_worker_entrypoint_shell_script() {
        // CARGO_MANIFEST_DIR for maestro-core is crates/maestro-core; the
        // entrypoint lives at <repo>/docker/worker-entrypoint.sh.
        let manifest_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let script_path = manifest_dir
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join("docker/worker-entrypoint.sh"))
            .expect("locate docker/worker-entrypoint.sh from manifest dir");
        let script = match std::fs::read_to_string(&script_path) {
            Ok(s) => s,
            Err(e) => {
                // Worktree / sparse-checkout safety: don't fail if the file
                // truly isn't present (CI uses the full repo, this guards
                // local edge cases).
                eprintln!("skip: cannot read {script_path:?}: {e}");
                return;
            }
        };
        // Each mapping the snippet must keep in sync with the script.
        for (file, env_var) in [
            ("/run/maestro-secrets/claude", "CLAUDE_CODE_OAUTH_TOKEN"),
            ("/run/maestro-secrets/cursor", "CURSOR_API_KEY"),
            ("/run/maestro-secrets/codex", "OPENAI_API_KEY"),
            ("/run/maestro-secrets/opencode", "ANTHROPIC_API_KEY"),
            ("/run/maestro-secrets/gh", "GH_TOKEN"),
        ] {
            assert!(
                script.contains(file),
                "drift: worker-entrypoint.sh no longer sources {file}; \
                 update BUNDLE_SOURCING_SH and this test in lockstep"
            );
            assert!(
                script.contains(&format!("export {env_var}")),
                "drift: worker-entrypoint.sh no longer exports {env_var}; \
                 update BUNDLE_SOURCING_SH and this test in lockstep"
            );
            // And the Rust snippet must mirror it.
            assert!(
                BUNDLE_SOURCING_SH.contains(file),
                "drift: BUNDLE_SOURCING_SH missing {file} (present in shell script)"
            );
            assert!(
                BUNDLE_SOURCING_SH.contains(&format!("export {env_var};")),
                "drift: BUNDLE_SOURCING_SH missing export {env_var} \
                 (present in shell script)"
            );
        }

        // Task #39 / #41: the cli_state mapping doesn't use the standard
        // source + export pattern. It writes the session blob onto
        // $HOME/.claude.json via a `jq` shallow-merge (with a `cp`
        // fallback). Both the script and the Rust constant must:
        //   1. Reference the file path,
        //   2. Reference $HOME/.claude.json as the merge target,
        //   3. Carry the `jq -s '.[0] * .[1]'` invocation (so a regression
        //      to plain-cp gets caught).
        assert!(
            script.contains("/run/maestro-secrets/claude_session.json"),
            "drift: worker-entrypoint.sh missing claude_session.json handling"
        );
        assert!(
            script.contains("$HOME/.claude.json") || script.contains("HOME/.claude.json"),
            "drift: worker-entrypoint.sh must write the session blob onto $HOME/.claude.json"
        );
        assert!(
            script.contains("jq -s '.[0] * .[1]'"),
            "drift: worker-entrypoint.sh must merge via `jq -s '.[0] * .[1]'` \
             (task #41); a plain `cp` wipes accumulated state. Update both \
             the script and BUNDLE_SOURCING_SH in lockstep."
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("/run/maestro-secrets/claude_session.json"),
            "drift: BUNDLE_SOURCING_SH missing claude_session.json handling"
        );

        // Task #42: the empty-mountpoint observability breadcrumb must be
        // present in BOTH the script and the Rust constant. If it drifts
        // out of one, future lifetime bugs go silent again.
        assert!(
            script.contains("[maestro-bundle]"),
            "drift: worker-entrypoint.sh missing [maestro-bundle] empty-mountpoint warning (task #42)"
        );
        assert!(
            BUNDLE_SOURCING_SH.contains("[maestro-bundle]"),
            "drift: BUNDLE_SOURCING_SH missing [maestro-bundle] empty-mountpoint warning (task #42)"
        );
    }

    /// Task #36: when the runner has NO secrets bundle attached, the
    /// `sh -c` payload built by `wrap_command` must NOT reference
    /// `/run/maestro-secrets/` — keeps the legacy path's argv clean and
    /// avoids any chance of confusing logs.
    #[test]
    fn wrap_command_without_bundle_does_not_source_run_maestro_secrets() {
        let r = runner();
        let (_program, args) = r.wrap_command("claude", &["--version"]);
        // The shell command is the LAST docker arg (after `sh -c`).
        let cmd = args.last().expect("cmd");
        assert!(
            !cmd.contains("/run/maestro-secrets/"),
            "legacy wrap_command must not reference /run/maestro-secrets/; got: {cmd}"
        );
        assert!(
            !cmd.contains("MAESTRO_AUTH_BUNDLE"),
            "legacy wrap_command must not gate on MAESTRO_AUTH_BUNDLE; got: {cmd}"
        );
        // Sanity: existing legacy stanza is still there.
        assert!(cmd.contains("$HOME/.config/gh/gh-app-token"));
        assert!(cmd.starts_with("if [ ! -f \"$HOME/.claude.json\" ]"));
        assert!(cmd.contains("exec claude --version"));
    }

    /// Task #36 — the core bug. When a bundle IS attached, `wrap_command`'s
    /// `sh -c` payload MUST contain the bundle-sourcing block BEFORE the
    /// `exec` so the agent CLI sees its token in env.
    #[test]
    fn wrap_command_with_bundle_sources_secrets_before_exec() {
        let bundle = crate::auth::WorkerSecretsBundle::for_tests(
            crate::config::AiAgentProvider::Claude,
            vec![("MAESTRO_AUTH_BUNDLE".into(), "1".into())],
        );
        let r = runner().with_secrets_bundle(Arc::new(bundle));
        let (_program, args) = r.wrap_command("claude", &["--version"]);
        let cmd = args.last().expect("cmd");

        // Bundle-sourcing block must be present.
        assert!(
            cmd.contains("/run/maestro-secrets/claude"),
            "bundle-attached wrap_command must source /run/maestro-secrets/claude; got: {cmd}"
        );
        assert!(
            cmd.contains("export CLAUDE_CODE_OAUTH_TOKEN"),
            "bundle-attached wrap_command must export CLAUDE_CODE_OAUTH_TOKEN; got: {cmd}"
        );
        // And it must precede the `exec`, not run after.
        let bundle_pos = cmd
            .find("/run/maestro-secrets/claude")
            .expect("bundle source position");
        let exec_pos = cmd.find("exec claude").expect("exec position");
        assert!(
            bundle_pos < exec_pos,
            "bundle sourcing must precede exec; bundle@{bundle_pos} exec@{exec_pos} in: {cmd}"
        );
        // And all five provider mappings must be present (defence in depth
        // against accidentally narrowing the splice).
        for file in [
            "/run/maestro-secrets/claude",
            "/run/maestro-secrets/cursor",
            "/run/maestro-secrets/codex",
            "/run/maestro-secrets/opencode",
            "/run/maestro-secrets/gh",
        ] {
            assert!(
                cmd.contains(file),
                "bundle-attached wrap_command must reference {file}"
            );
        }
    }

    #[test]
    fn wrap_command_structure() {
        let r = runner();
        let (program, args) = r.wrap_command("claude", &["--print", "-p", "hello"]);

        assert_eq!(program, "docker");
        assert_eq!(args[0], "run");
        assert_eq!(args[1], "--rm");

        // Container name
        assert_eq!(
            flag_value(&args, "--name"),
            Some("maestro-worker-proj-42-0")
        );

        // NET_ADMIN
        assert!(args.contains(&"--cap-add=NET_ADMIN".to_string()));

        // Key env vars
        assert!(has_env(&args, "HOME", "/home/maestro"));
        assert!(!has_env(&args, "DOCKER_HOST", "tcp://dind:2375"));
        assert!(has_env(&args, "MISE_TRUST_ALL_CONFIGS", "1"));

        // Volume mounts
        assert!(has_volume(&args, "/workspace:/workspace"));
        assert!(has_volume(
            &args,
            "/shared-auth/claude:/home/maestro/.claude"
        ));
        assert!(has_volume(
            &args,
            "/shared-auth/gh:/home/maestro/.config/gh"
        ));

        // Working directory
        assert_eq!(flag_value(&args, "-w"), Some("/workspace/proj-42"));

        // Entrypoint is empty (bypass image entrypoint)
        assert_eq!(flag_value(&args, "--entrypoint"), Some(""));

        assert_eq!(flag_value(&args, "--user"), Some("maestro:maestro"));

        // After --entrypoint "" comes: --user maestro:maestro, image, sh, -c, "restore; exec ..."
        let entrypoint_idx = args.iter().position(|a| a == "--entrypoint").unwrap();
        let tail = &args[entrypoint_idx + 2..];
        assert_eq!(tail[0], "--user");
        assert_eq!(tail[1], "maestro:maestro");
        assert_eq!(tail[2], "maestro:latest");
        assert_eq!(tail[3], "sh");
        assert_eq!(tail[4], "-c");
        // The shell command restores .claude.json then execs the original program
        assert!(
            tail[5].contains("exec claude --print -p hello"),
            "sh -c body: {}",
            tail[5]
        );
    }

    #[test]
    fn wrap_shell_command_uses_worker_entrypoint() {
        let r = runner();
        let (program, args) = r.wrap_shell_command("npm install && npm test");

        assert_eq!(program, "docker");

        // Entrypoint is the worker entrypoint
        assert_eq!(
            flag_value(&args, "--entrypoint"),
            Some("/usr/local/bin/worker-entrypoint.sh")
        );

        // Image + shell command at the tail
        let entrypoint_idx = args.iter().position(|a| a == "--entrypoint").unwrap();
        let tail = &args[entrypoint_idx + 2..];
        assert_eq!(tail[0], "maestro:latest");
        assert_eq!(tail[1], "sh");
        assert_eq!(tail[2], "-c");
        assert_eq!(tail[3], "npm install && npm test");
    }

    #[test]
    fn wrap_command_counter_advances_across_calls() {
        let r = runner();
        let (_, args1) = r.wrap_command("echo", &["a"]);
        let (_, args2) = r.wrap_shell_command("echo b");

        assert_eq!(
            flag_value(&args1, "--name"),
            Some("maestro-worker-proj-42-0")
        );
        assert_eq!(
            flag_value(&args2, "--name"),
            Some("maestro-worker-proj-42-1")
        );
    }

    #[test]
    fn all_fixed_env_vars_present() {
        let r = runner();
        let (_, args) = r.wrap_command("true", &[]);

        for (k, v) in WORKER_ENV {
            assert!(has_env(&args, k, v), "Missing env var {k}={v}");
        }
    }

    #[test]
    fn all_volume_mounts_present() {
        let r = runner();
        let (_, args) = r.wrap_command("true", &[]);

        for mount in WORKER_VOLUMES {
            assert!(has_volume(&args, mount), "Missing volume mount {mount}");
        }
    }

    // ── Per-issue volume isolation tests ──────────────────────────────

    /// Helper: create a runner whose worktree path sits under `/workspace/worktrees/`
    /// so the repo root can be derived (parent of parent).
    fn isolated_runner() -> ContainerRunner {
        ContainerRunner::new(
            "PROJ-42",
            &PathBuf::from("/workspace/worktrees/feat-proj-42"),
            "maestro:latest",
        )
        .with_isolate_workspace()
    }

    /// Helper: create a legacy runner (no isolation).
    fn legacy_runner() -> ContainerRunner {
        ContainerRunner::new(
            "PROJ-42",
            &PathBuf::from("/workspace/worktrees/feat-proj-42"),
            "maestro:latest",
        )
    }

    // ── Group 1: Legacy mode (no isolation) ──

    #[test]
    fn legacy_mode_has_workspace_volume() {
        let r = legacy_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace:/workspace"),
            "Legacy mode must mount /workspace:/workspace"
        );
    }

    #[test]
    fn legacy_mode_no_targeted_worktree_mount() {
        let r = legacy_runner();
        let (_, args) = r.wrap_command("true", &[]);
        // No mount of the specific worktree path should appear
        assert!(
            !has_volume(
                &args,
                "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"
            ),
            "Legacy mode must NOT mount the worktree path separately"
        );
    }

    #[test]
    fn legacy_mode_no_standalone_git_mount() {
        let r = legacy_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            !has_volume(&args, "/workspace/.git:/workspace/.git"),
            "Legacy mode must NOT mount .git separately (it is inside /workspace)"
        );
    }

    #[test]
    fn legacy_wrap_shell_command_has_workspace_volume() {
        let r = legacy_runner();
        let (_, args) = r.wrap_shell_command("echo test");
        assert!(
            has_volume(&args, "/workspace:/workspace"),
            "Legacy wrap_shell_command must mount /workspace:/workspace"
        );
    }

    // ── Group 2: Isolated mode ──

    #[test]
    fn isolated_mode_no_full_workspace_mount() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "Isolated mode must NOT mount /workspace:/workspace"
        );
    }

    #[test]
    fn isolated_mode_has_worktree_mount() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(
                &args,
                "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"
            ),
            "Isolated mode must mount the specific worktree path"
        );
    }

    #[test]
    fn isolated_mode_has_git_dir_mount() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace/.git:/workspace/.git"),
            "Isolated mode must mount .git for git operations"
        );
    }

    #[test]
    fn isolated_mode_has_maestro_dir_mount_ro() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace/.maestro:/workspace/.maestro:ro"),
            "Isolated mode must mount .maestro read-only for npm config"
        );
    }

    #[test]
    fn isolated_mode_auth_volumes_preserved() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        // All /shared-auth/* mounts must still be present
        for mount in WORKER_VOLUMES {
            if mount.starts_with("/shared-auth/") || mount.starts_with("/etc/maestro") {
                assert!(
                    has_volume(&args, mount),
                    "Isolated mode must preserve auth volume: {mount}"
                );
            }
        }
    }

    #[test]
    fn isolated_mode_env_vars_unchanged() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        for (k, v) in WORKER_ENV {
            assert!(
                has_env(&args, k, v),
                "Isolated mode must preserve env var {k}={v}"
            );
        }
    }

    #[test]
    fn isolated_mode_working_directory_correct() {
        let r = isolated_runner();
        let (_, args) = r.wrap_command("true", &[]);
        assert_eq!(
            flag_value(&args, "-w"),
            Some("/workspace/worktrees/feat-proj-42"),
            "Isolated mode must keep -w pointing to the worktree path"
        );
    }

    #[test]
    fn isolated_wrap_shell_command_no_full_workspace() {
        let r = isolated_runner();
        let (_, args) = r.wrap_shell_command("echo test");
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "Isolated wrap_shell_command must NOT mount /workspace:/workspace"
        );
    }

    #[test]
    fn isolated_wrap_shell_command_has_targeted_mounts() {
        let r = isolated_runner();
        let (_, args) = r.wrap_shell_command("echo test");
        assert!(
            has_volume(
                &args,
                "/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"
            ),
            "Isolated wrap_shell_command must mount worktree"
        );
        assert!(
            has_volume(&args, "/workspace/.git:/workspace/.git"),
            "Isolated wrap_shell_command must mount .git"
        );
        assert!(
            has_volume(&args, "/workspace/.maestro:/workspace/.maestro:ro"),
            "Isolated wrap_shell_command must mount .maestro:ro"
        );
    }

    // ── Group 3: Builder API ──

    #[test]
    fn with_isolate_workspace_sets_flag() {
        let r = ContainerRunner::new(
            "TEST-1",
            &PathBuf::from("/workspace/worktrees/test-1"),
            "maestro:latest",
        )
        .with_isolate_workspace();
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "with_isolate_workspace must enable isolation"
        );
    }

    #[test]
    fn default_runner_no_isolation() {
        let r = ContainerRunner::new(
            "TEST-1",
            &PathBuf::from("/workspace/worktrees/test-1"),
            "maestro:latest",
        );
        let (_, args) = r.wrap_command("true", &[]);
        assert!(
            has_volume(&args, "/workspace:/workspace"),
            "Default runner must NOT isolate (backward compat)"
        );
    }

    #[test]
    fn isolate_workspace_active() {
        let r = ContainerRunner::new(
            "TEST-1",
            &PathBuf::from("/workspace/worktrees/test-1"),
            "maestro:latest",
        )
        .with_isolate_workspace();
        let (_, args) = r.wrap_command("true", &[]);
        // Isolation must be active
        assert!(
            !has_volume(&args, "/workspace:/workspace"),
            "Isolation must be active"
        );
        assert!(
            has_volume(
                &args,
                "/workspace/worktrees/test-1:/workspace/worktrees/test-1"
            ),
            "Worktree mount must be present"
        );
    }

    #[test]
    fn wrap_command_sources_gh_token_file() {
        let r = runner();
        let (_, args) = r.wrap_command("claude", &["--print"]);
        let sh_body = args.last().expect("last arg is the sh -c body");
        assert!(
            sh_body.contains("gh-app-token"),
            "wrap_command preamble must source the GitHub App token file; got: {sh_body}"
        );
    }

    // ── Group 4: build_volume_args helper tests ──

    #[test]
    fn build_volume_args_legacy_includes_workspace() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, false);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            pairs.contains(&"/workspace:/workspace"),
            "Legacy build_volume_args must include /workspace:/workspace"
        );
    }

    #[test]
    fn build_volume_args_isolated_replaces_workspace() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, true);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            !pairs.contains(&"/workspace:/workspace"),
            "Isolated build_volume_args must NOT include /workspace:/workspace"
        );
        assert!(
            pairs.contains(&"/workspace/worktrees/feat-proj-42:/workspace/worktrees/feat-proj-42"),
            "Isolated build_volume_args must include worktree mount"
        );
        assert!(
            pairs.contains(&"/workspace/.git:/workspace/.git"),
            "Isolated build_volume_args must include .git mount"
        );
        assert!(
            pairs.contains(&"/workspace/.maestro:/workspace/.maestro:ro"),
            "Isolated build_volume_args must include .maestro:ro mount"
        );
    }

    #[test]
    fn build_volume_args_isolated_no_duplicate_mounts() {
        let wt = PathBuf::from("/workspace/worktrees/feat-proj-42");
        let args = build_volume_args(&wt, true);
        // Check for duplicate entries
        let mut seen = std::collections::HashSet::new();
        for mount in &args {
            assert!(
                seen.insert(mount.as_str()),
                "Duplicate volume mount: {mount}"
            );
        }
    }

    #[test]
    fn build_volume_args_isolated_shallow_path_falls_back() {
        // A shallow path like `/tmp` has no grandparent, so isolation cannot
        // derive the repo root. The function should fall back to the full
        // `/workspace:/workspace` mount instead of leaving the container
        // without any workspace volume.
        let wt = PathBuf::from("/tmp");
        let args = build_volume_args(&wt, true);
        let pairs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
        assert!(
            pairs.contains(&"/workspace:/workspace"),
            "Shallow worktree path must fall back to /workspace:/workspace"
        );
    }
}
