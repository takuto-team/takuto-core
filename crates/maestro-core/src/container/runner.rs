// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! [`ContainerRunner`] — wraps a `docker run` invocation for one workflow
//! ticket so each agent step lands in an isolated container with the right
//! env, volumes, and secrets bundle attached.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::OnceLock;

use tracing::{error, info, warn};

use super::sanitize_ticket_key;

// `pub(crate) use` re-exports of items now living in sibling modules
// (`docker_args`, `volumes`, `secrets_bundle`) so external callers
// reaching `super::runner::*` (e.g. `editor.rs`, `run_command.rs`) keep
// compiling unchanged. Shell snippets live in `super::wrap_command`.
pub(crate) use super::docker_args::{PASSTHROUGH_ENV, WORKER_ENV};
pub(crate) use super::secrets_bundle::{apply_secrets_bundle_to_args, passthrough_is_bundled};
pub(crate) use super::volumes::build_volume_args;

static DOCKER_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Runs AI agent commands inside isolated Docker containers so each workflow
/// gets its own filesystem and network namespace.
pub struct ContainerRunner {
    ticket_key: String,
    image: String,
    worktree_path: PathBuf,
    step_counter: std::sync::atomic::AtomicU32,
    /// When `true`, replace `/workspace:/workspace` with targeted mounts
    /// for the worktree, `.git`, and `.maestro` — see `super::volumes`.
    isolate_workspace: bool,
    /// Optional per-workflow secrets bundle. When `Some`,
    /// `super::docker_args::base_docker_args` bind-mounts the bundle's
    /// tmpfs at `/run/maestro-secrets:ro` and `super::wrap_command`
    /// sources it. When `None`, ambient `PASSTHROUGH_ENV` carries tokens.
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

    /// Attach a per-workflow secrets bundle. The runner bind-mounts the
    /// bundle's tmpfs at `/run/maestro-secrets:ro`, sets
    /// `MAESTRO_AUTH_BUNDLE=1`, and exports the bundle's non-secret env
    /// vars. Token bytes are NEVER passed via `-e`. See
    /// `super::secrets_bundle`.
    pub fn with_secrets_bundle(mut self, bundle: Arc<crate::auth::WorkerSecretsBundle>) -> Self {
        self.secrets_bundle = Some(bundle);
        self
    }

    /// `true` when this runner has a secrets bundle attached. Callers
    /// consult this to log "legacy auth path" vs "bundle path" without
    /// exposing the bundle itself.
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

    /// Bundle's `extra_args` (provider sub-table). `None` when no bundle attached.
    pub fn provider_extra_args(&self) -> Option<&[String]> {
        self.secrets_bundle
            .as_ref()
            .map(|b| b.extra_args.as_slice())
    }

    /// Returns a unique container name for this ticket, incrementing an internal counter.
    pub fn next_container_name(&self) -> String {
        let n = self
            .step_counter
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let sanitized = sanitize_ticket_key(&self.ticket_key);
        format!("maestro-worker-{sanitized}-{n}")
    }

    /// Common `docker run` prefix (flags, env, volumes, workdir, entrypoint)
    /// before the image and user command. Thin wrapper around the free function
    /// in [`super::docker_args`] — passes runner state through without accessors.
    fn base_docker_args(&self, container_name: &str, entrypoint: Option<&str>) -> Vec<String> {
        super::docker_args::base_docker_args(
            container_name,
            entrypoint,
            &self.worktree_path,
            self.isolate_workspace,
            self.secrets_bundle.as_ref(),
        )
    }

    /// Wrap a direct command (`program` + `args`) into a `docker run`
    /// invocation. The `sh -c` payload (restore / fix-perms / gh-token /
    /// bundle-source / exec) is assembled by
    /// [`super::wrap_command::build_sh_payload`].
    pub fn wrap_command(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
        let name = self.next_container_name();
        let mut docker_args = self.base_docker_args(&name, None);
        // `--user`: without it, `docker run` defaults to root and would write
        // root-owned files on the bind-mounted worktree that maestro can't remove.
        docker_args.push("--user".into());
        docker_args.push("maestro:maestro".into());
        docker_args.push(self.image.clone());

        let cmd = super::wrap_command::build_sh_payload(self.has_secrets_bundle(), program, args);
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
        // Hostname is the container ID (Docker default); works regardless of compose project.
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
            // Verify the image is in DinD before using it — `docker inspect` may name
            // a registry tag that was never pulled into DinD (local dev builds).
            if !image.is_empty() && image_exists_in_dind(&image).await {
                info!(image = %image, "Discovered worker image from running Maestro container");
                return Some(image);
            }
        }

        // maestro:latest in DinD (e.g. via `make load-worker`) — local dev builds.
        if image_exists_in_dind("maestro:latest").await {
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

/// `docker image inspect <name>` → does the image exist in the current daemon?
/// Stderr / stdout silenced; falsy on any error.
async fn image_exists_in_dind(image: &str) -> bool {
    tokio::process::Command::new("docker")
        .args(["image", "inspect", image])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false)
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

        for mount in super::super::volumes::WORKER_VOLUMES {
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
        for mount in super::super::volumes::WORKER_VOLUMES {
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
}
