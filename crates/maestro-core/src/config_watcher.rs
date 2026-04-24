// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Live config reload via filesystem polling.
//!
//! [`ConfigWatcher`] periodically checks whether `config.toml` has been
//! modified on disk (by comparing the file's mtime or content hash). When a
//! valid change is detected, the in-memory [`Config`] is hot-swapped.
//!
//! **Why polling, not inotify?**  Docker Desktop for macOS uses VirtioFS /
//! gRPC-FUSE for bind mounts, so kernel-level inotify events fired by host
//! edits are **not** delivered inside the container. A polling watcher works
//! universally — at the cost of a small, configurable delay.
//!
//! Self-authored writes (from [`ConfigWriter`]) are skipped via a shared
//! timestamp: if the file was last modified within
//! [`SELF_WRITE_SKIP_WINDOW_MS`] of the most recent API write, the event is
//! treated as self-triggered and suppressed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::config::Config;

/// Default interval between filesystem polls (seconds).
pub const DEFAULT_POLL_INTERVAL_SECS: u64 = 5;

/// How long after an API-initiated write we suppress watcher-triggered
/// reloads (milliseconds). Prevents a write→detect→reload cycle.
const SELF_WRITE_SKIP_WINDOW_MS: u64 = 2_000;

/// Watches a config file and hot-swaps the shared `Config` when valid
/// changes are detected.
pub struct ConfigWatcher {
    config_path: PathBuf,
    config: Arc<RwLock<Config>>,
    /// Epoch-millis of the last API-initiated write (shared with
    /// [`ConfigWriter`]).
    last_api_write_ms: Arc<AtomicU64>,
    poll_interval: Duration,
    cancel: CancellationToken,
}

impl ConfigWatcher {
    pub fn new(
        config_path: PathBuf,
        config: Arc<RwLock<Config>>,
        last_api_write_ms: Arc<AtomicU64>,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            config_path,
            config,
            last_api_write_ms,
            poll_interval: Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS),
            cancel,
        }
    }

    /// Override the default poll interval.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Run the watcher loop until cancelled. This is intended to be
    /// `tokio::spawn`ed as a background task.
    pub async fn run(self) {
        let mut last_mtime = file_mtime(&self.config_path);

        tracing::info!(
            path = %self.config_path.display(),
            poll_secs = self.poll_interval.as_secs(),
            "Config file watcher started"
        );

        let mut interval = tokio::time::interval(self.poll_interval);
        // The first tick fires immediately; skip it so we don't reload on startup.
        interval.tick().await;

        loop {
            tokio::select! {
                _ = interval.tick() => {}
                _ = self.cancel.cancelled() => {
                    tracing::info!("Config watcher shutting down");
                    break;
                }
            }

            let current_mtime = file_mtime(&self.config_path);
            if current_mtime == last_mtime {
                continue;
            }
            last_mtime = current_mtime;

            // Check if this change was self-authored (API write).
            if self.is_self_authored_write() {
                tracing::debug!("Skipping config reload (self-authored write)");
                continue;
            }

            tracing::info!(
                path = %self.config_path.display(),
                "Config file change detected, reloading"
            );

            match self.try_reload().await {
                Ok(()) => {
                    tracing::info!("Config reloaded successfully from disk");
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "Config reload failed (keeping current config): invalid config on disk"
                    );
                }
            }
        }
    }

    /// Attempt to reload config from disk, validate, and swap.
    ///
    /// Public so it can be called from `POST /api/config/reload`.
    pub async fn try_reload(&self) -> crate::error::Result<()> {
        self.reload_from_path(&self.config_path).await
    }

    /// Reload config from a specific path, validate, and swap the in-memory config.
    async fn reload_from_path(&self, path: &Path) -> crate::error::Result<()> {
        let new_config = Config::load(path)?;
        self.log_restart_required_changes(&new_config).await;

        let mut config = self.config.write().await;
        *config = new_config;
        Ok(())
    }

    /// Log warnings for fields that changed but require a restart to take effect.
    async fn log_restart_required_changes(&self, new: &Config) {
        let current = self.config.read().await;
        log_restart_required_field_changes(&current, new);
    }

    fn is_self_authored_write(&self) -> bool {
        let last_write = self.last_api_write_ms.load(Ordering::Acquire);
        if last_write == 0 {
            return false;
        }
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now.saturating_sub(last_write) < SELF_WRITE_SKIP_WINDOW_MS
    }
}

/// Best-effort file modification time. Returns `None` if the file doesn't
/// exist or metadata cannot be read.
fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

/// Log warnings for config fields that changed but require a restart to take
/// effect. Called from both the watcher loop and the manual reload endpoint.
fn log_restart_required_field_changes(current: &Config, new: &Config) {
    if current.web.host != new.web.host {
        tracing::warn!(
            old = %current.web.host,
            new = %new.web.host,
            "web.host changed — restart required to take effect"
        );
    }
    if current.web.port != new.web.port {
        tracing::warn!(
            old = current.web.port,
            new = new.web.port,
            "web.port changed — restart required to take effect"
        );
    }
    if current.web.cors_origins != new.web.cors_origins {
        tracing::warn!("web.cors_origins changed — restart required to take effect");
    }
}

/// Standalone reload function for use from route handlers (e.g., `POST
/// /api/config/reload`). Reads, parses, validates, and swaps the in-memory
/// config. Logs warnings for fields that require a restart.
pub async fn reload_config_from_disk(
    config_path: &Path,
    config: &Arc<RwLock<Config>>,
) -> crate::error::Result<()> {
    let new_config = Config::load(config_path)?;
    let mut guard = config.write().await;
    log_restart_required_field_changes(&guard, &new_config);
    *guard = new_config;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn minimal_valid_toml() -> &'static str {
        r#"
[general]
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"
repo_path = "/workspace"

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#
    }

    #[tokio::test]
    async fn reload_valid_config_updates_in_memory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, minimal_valid_toml()).unwrap();

        let config = Arc::new(RwLock::new(Config::load(&path).unwrap()));
        assert_eq!(config.read().await.general.max_concurrent_workflows, 1);

        // Write a modified config to disk.
        let new_toml = minimal_valid_toml().replace(
            "[general]\npoll_interval_secs = 30",
            "[general]\npoll_interval_secs = 30\nmax_concurrent_workflows = 5",
        );
        fs::write(&path, new_toml).unwrap();

        // Reload.
        reload_config_from_disk(&path, &config).await.unwrap();
        assert_eq!(config.read().await.general.max_concurrent_workflows, 5);
    }

    #[tokio::test]
    async fn reload_invalid_config_keeps_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, minimal_valid_toml()).unwrap();

        let config = Arc::new(RwLock::new(Config::load(&path).unwrap()));

        // Write invalid TOML to disk.
        fs::write(&path, "this is not valid toml {{{{").unwrap();

        let result = reload_config_from_disk(&path, &config).await;
        assert!(result.is_err(), "should fail for invalid TOML");

        // In-memory config should be unchanged.
        assert_eq!(config.read().await.general.poll_interval_secs, 30);
    }

    #[tokio::test]
    async fn reload_missing_file_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");

        let config = Arc::new(RwLock::new(Config::default()));

        let result = reload_config_from_disk(&path, &config).await;
        assert!(result.is_err());
    }

    #[test]
    fn self_authored_write_detection() {
        let last_write = Arc::new(AtomicU64::new(0));

        // No prior write — should not be self-authored.
        let watcher = ConfigWatcher::new(
            PathBuf::from("/tmp/test.toml"),
            Arc::new(RwLock::new(Config::default())),
            last_write.clone(),
            CancellationToken::new(),
        );
        assert!(!watcher.is_self_authored_write());

        // Set a recent write timestamp.
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        last_write.store(now, Ordering::Release);
        assert!(watcher.is_self_authored_write());

        // Set an old write timestamp (well outside the skip window).
        last_write.store(now - 10_000, Ordering::Release);
        assert!(!watcher.is_self_authored_write());
    }

    #[tokio::test]
    async fn watcher_stops_on_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, minimal_valid_toml()).unwrap();

        let config = Arc::new(RwLock::new(Config::load(&path).unwrap()));
        let cancel = CancellationToken::new();

        let watcher = ConfigWatcher::new(path, config, Arc::new(AtomicU64::new(0)), cancel.clone())
            .with_poll_interval(Duration::from_millis(50));

        let handle = tokio::spawn(async move { watcher.run().await });

        // Give the watcher a moment to start, then cancel.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel.cancel();

        // Should complete within a reasonable time.
        tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("watcher should stop within timeout")
            .expect("watcher task should not panic");
    }
}
