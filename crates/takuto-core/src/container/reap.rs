// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Zombie container cleanup and dangling-image pruning.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::{info, warn};

/// Throttle DinD image pruning to at most once every 5 minutes.
static LAST_IMAGE_PRUNE: AtomicU64 = AtomicU64::new(0);
const IMAGE_PRUNE_INTERVAL_SECS: u64 = 300;

/// Hard ceiling on any single `docker` invocation made during best-effort
/// cleanup. Container reaping must never block pause/cancel (or, in tests, the
/// engine harness) on an unresponsive or wedged Docker daemon: a hung daemon
/// would otherwise park the `.output()` future forever. On timeout we kill the
/// orphaned client (`kill_on_drop`), log, and move on — cleanup is best-effort.
const DOCKER_CMD_TIMEOUT: Duration = Duration::from_secs(10);

/// Run a `docker` command, returning its output, or `None` if it failed to
/// spawn or exceeded [`DOCKER_CMD_TIMEOUT`].
async fn run_docker_bounded(args: &[&str]) -> Option<std::process::Output> {
    let fut = tokio::process::Command::new("docker")
        .args(args)
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(DOCKER_CMD_TIMEOUT, fut).await {
        Ok(Ok(out)) => Some(out),
        Ok(Err(e)) => {
            warn!(error = %e, "docker command failed to spawn during cleanup");
            None
        }
        Err(_) => {
            warn!(
                timeout_secs = DOCKER_CMD_TIMEOUT.as_secs(),
                "docker command timed out during cleanup; skipping (daemon unresponsive?)"
            );
            None
        }
    }
}

/// List and force-remove containers whose name matches the prefix.
pub(crate) async fn remove_containers_matching(sanitized_key: &str) {
    let filter = format!("name=takuto-worker-{sanitized_key}-");
    let output = run_docker_bounded(&["ps", "-a", "--filter", &filter, "-q"]).await;

    match output {
        Some(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            let ids: Vec<String> = stdout
                .lines()
                .filter(|l| !l.is_empty())
                .map(str::to_string)
                .collect();
            if ids.is_empty() {
                return;
            }
            info!(
                count = ids.len(),
                key = sanitized_key,
                "Removing worker containers"
            );
            let mut rm_args: Vec<&str> = vec!["rm", "-f"];
            rm_args.extend(ids.iter().map(String::as_str));
            let _ = run_docker_bounded(&rm_args).await;
        }
        Some(out) => {
            warn!(
                stderr = %String::from_utf8_lossy(&out.stderr),
                "docker ps failed while cleaning up worker containers"
            );
        }
        // None → run_docker_bounded already logged the spawn failure/timeout.
        None => {}
    }
}

/// Prune dangling DinD images (throttled to once per 5 minutes).
///
/// Runs `docker image prune -f` to remove dangling image layers that accumulate
/// from rebuilding `takuto:latest`. This is safe because dangling images have no
/// tags and are not referenced by any running container. The `takuto:latest`
/// image itself is always tagged and will never be removed.
pub(crate) async fn prune_dangling_images() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let last = LAST_IMAGE_PRUNE.load(Ordering::Relaxed);
    if now.saturating_sub(last) < IMAGE_PRUNE_INTERVAL_SECS {
        return; // throttled
    }
    LAST_IMAGE_PRUNE.store(now, Ordering::Relaxed);

    let output = run_docker_bounded(&["image", "prune", "-f"]).await;

    match output {
        Some(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.trim().is_empty() {
                info!("Pruned dangling DinD images: {}", stdout.trim());
            }
        }
        Some(out) => warn!(
            stderr = %String::from_utf8_lossy(&out.stderr),
            "docker image prune failed"
        ),
        // None → run_docker_bounded already logged the spawn failure/timeout.
        None => {}
    }
}
