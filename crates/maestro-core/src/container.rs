use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tracing::{info, warn};

/// Sanitize a ticket key for use in container names (lowercase, replace non-alphanumeric with `-`).
fn sanitize_ticket_key(key: &str) -> String {
    key.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

/// Runs AI agent commands inside isolated Docker containers so each workflow
/// gets its own filesystem and network namespace.
pub struct ContainerRunner {
    ticket_key: String,
    image: String,
    worktree_path: PathBuf,
    step_counter: std::sync::atomic::AtomicU32,
}

static DOCKER_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Fixed environment variables injected into every worker container.
const WORKER_ENV: &[(&str, &str)] = &[
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
        "/home/maestro/.local/share/mise/shims:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    ),
    ("DOCKER_HOST", "tcp://dind:2375"),
    ("MAESTRO_CONFIG", "/etc/maestro/config.toml"),
    // Persist user-level .npmrc across worker containers (aws codeartifact login writes here)
    ("NPM_CONFIG_USERCONFIG", "/workspace/.maestro/.npmrc"),
    // Deterministic text rendering in screenshots / snapshots (Playwright, Storybook, etc.)
    ("TZ", "UTC"),
    ("LANG", "C.UTF-8"),
    ("LC_ALL", "C.UTF-8"),
];

/// Host environment variables forwarded into the worker when set.
const PASSTHROUGH_ENV: &[&str] = &[
    "FIGMA_API_TOKEN",
    "CURSOR_API_KEY",
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
const WORKER_VOLUMES: &[&str] = &[
    "/workspace:/workspace",
    "/shared-auth/claude:/home/maestro/.claude",
    "/shared-auth/cursor:/home/maestro/.cursor",
    "/shared-auth/gh:/home/maestro/.config/gh",
    "/shared-auth/acli:/home/maestro/.config/acli",
    "/shared-auth/npm:/home/maestro/.npm",
    "/shared-auth/mise-data:/home/maestro/.local/share/mise",
    "/shared-auth/mise-cache:/home/maestro/.cache/mise",
    "/shared-auth/aws:/home/maestro/.aws",
    // Playwright browser cache — must align with the repo's package.json, not a baked image path
    "/shared-auth/playwright-cache:/home/maestro/.cache/ms-playwright",
    // Config + env for egress rules (extra_egress_hosts, .npmrc registry hosts, allow_all_https)
    "/etc/maestro:/etc/maestro:ro",
];

impl ContainerRunner {
    pub fn new(ticket_key: &str, worktree_path: &Path, image: &str) -> Self {
        Self {
            ticket_key: ticket_key.to_string(),
            image: image.to_string(),
            worktree_path: worktree_path.to_path_buf(),
            step_counter: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Check if Docker is available (`DOCKER_HOST` set and `docker info` succeeds).
    /// The result is cached for the process lifetime.
    pub fn is_available() -> bool {
        *DOCKER_AVAILABLE.get_or_init(|| {
            if std::env::var("DOCKER_HOST").unwrap_or_default().is_empty() {
                info!("DOCKER_HOST not set — container isolation disabled");
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
                warn!("docker info failed — container isolation disabled");
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

        for key in PASSTHROUGH_ENV {
            if let Ok(val) = std::env::var(key) {
                if !val.is_empty() {
                    args.push("-e".into());
                    args.push(format!("{key}={val}"));
                }
            }
        }

        for v in WORKER_VOLUMES {
            args.push("-v".into());
            args.push((*v).into());
        }

        args.push("-w".into());
        args.push(self.worktree_path.to_string_lossy().into_owned());

        args.push("--entrypoint".into());
        args.push(entrypoint.unwrap_or("").into());

        args
    }

    /// Wrap a direct command (`program` + `args`) into a `docker run` invocation.
    ///
    /// Uses `--entrypoint ""` so the container runs the program directly (bypassing
    /// the Maestro image entrypoint).
    pub fn wrap_command(&self, program: &str, args: &[&str]) -> (String, Vec<String>) {
        let name = self.next_container_name();
        let mut docker_args = self.base_docker_args(&name, None);
        // Without `--user`, `docker run` defaults to root and writes root-owned files on the
        // bind-mounted repo/worktree; the Maestro process (user `maestro`) cannot remove them later.
        docker_args.push("--user".into());
        docker_args.push("maestro:maestro".into());
        docker_args.push(self.image.clone());
        docker_args.push(program.into());
        for a in args {
            docker_args.push((*a).into());
        }
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
        remove_containers_matching(&sanitized).await;
    }

    /// Force-remove all worker containers for a given ticket key (no instance needed).
    pub async fn cleanup_for_ticket(ticket_key: &str) {
        let sanitized = sanitize_ticket_key(ticket_key);
        remove_containers_matching(&sanitized).await;
    }

    /// Auto-detect the worker image by inspecting the running Maestro container.
    pub async fn discover_worker_image() -> Option<String> {
        let output = tokio::process::Command::new("docker")
            .args(["inspect", "maestro", "--format", "{{.Config.Image}}"])
            .output()
            .await
            .ok()?;

        if !output.status.success() {
            warn!("docker inspect maestro failed — cannot auto-detect worker image");
            return None;
        }

        let image = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if image.is_empty() {
            None
        } else {
            info!(image = %image, "Discovered worker image from running Maestro container");
            Some(image)
        }
    }
}

/// List and force-remove containers whose name matches the prefix.
async fn remove_containers_matching(sanitized_key: &str) {
    let filter = format!("name=maestro-worker-{sanitized_key}-");
    let output = tokio::process::Command::new("docker")
        .args(["ps", "-a", "--filter", &filter, "-q"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let ids: Vec<&str> = stdout.lines().filter(|l| !l.is_empty()).collect();
            if ids.is_empty() {
                return;
            }
            info!(
                count = ids.len(),
                key = sanitized_key,
                "Removing worker containers"
            );
            let mut rm_args: Vec<&str> = vec!["rm", "-f"];
            rm_args.extend(ids.iter());
            let _ = tokio::process::Command::new("docker")
                .args(&rm_args)
                .output()
                .await;
        }
        Ok(out) => {
            warn!(
                stderr = %String::from_utf8_lossy(&out.stderr),
                "docker ps failed while cleaning up worker containers"
            );
        }
        Err(e) => {
            warn!(error = %e, "Failed to list worker containers for cleanup");
        }
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
    fn sanitize_ticket_key_lowercases_and_replaces() {
        assert_eq!(sanitize_ticket_key("PROJ-123"), "proj-123");
        assert_eq!(sanitize_ticket_key("My_Ticket.1"), "my-ticket-1");
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
        assert!(has_env(&args, "DOCKER_HOST", "tcp://dind:2375"));
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

        // After --entrypoint "" comes: --user maestro:maestro, image, program, args...
        let entrypoint_idx = args.iter().position(|a| a == "--entrypoint").unwrap();
        let tail = &args[entrypoint_idx + 2..];
        assert_eq!(tail[0], "--user");
        assert_eq!(tail[1], "maestro:maestro");
        assert_eq!(tail[2], "maestro:latest");
        assert_eq!(tail[3], "claude");
        assert_eq!(tail[4], "--print");
        assert_eq!(tail[5], "-p");
        assert_eq!(tail[6], "hello");
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
}
