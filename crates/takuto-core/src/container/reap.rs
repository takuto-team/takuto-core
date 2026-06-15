// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Zombie container cleanup and dangling-image pruning.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use tracing::{info, warn};

/// Throttle DinD image pruning to at most once every 5 minutes.
static LAST_IMAGE_PRUNE: AtomicU64 = AtomicU64::new(0);
const IMAGE_PRUNE_INTERVAL_SECS: u64 = 300;

/// List and force-remove containers whose name matches the prefix.
pub(crate) async fn remove_containers_matching(sanitized_key: &str) {
    let filter = format!("name=takuto-worker-{sanitized_key}-");
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

    let output = tokio::process::Command::new("docker")
        .args(["image", "prune", "-f"])
        .output()
        .await;

    match output {
        Ok(out) if out.status.success() => {
            let stdout = String::from_utf8_lossy(&out.stdout);
            if !stdout.trim().is_empty() {
                info!("Pruned dangling DinD images: {}", stdout.trim());
            }
        }
        Ok(out) => warn!(
            stderr = %String::from_utf8_lossy(&out.stderr),
            "docker image prune failed"
        ),
        Err(e) => warn!(error = %e, "Failed to run docker image prune"),
    }
}
