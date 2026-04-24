// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Atomic, validated config persistence.
//!
//! [`ConfigWriter`] provides the single write path for `config.toml`. All
//! dashboard and wizard writes funnel through this module — request handlers
//! never touch the filesystem directly.
//!
//! Write protocol:
//! 1. Validate the [`Config`] against the schema (`Config::validate()`).
//! 2. Acquire an advisory file lock (`fs2::FileExt::lock_exclusive`).
//! 3. Serialize to TOML.
//! 4. Write to a temp file in the same directory.
//! 5. `fs::rename` the temp file over the target (atomic on POSIX).
//! 6. Release the lock.

use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;

use crate::config::Config;
use crate::error::{MaestroError, Result};

/// Manages atomic config file persistence with advisory locking.
///
/// Shared via `Arc` between the web layer and background tasks.
#[derive(Debug)]
pub struct ConfigWriter {
    /// Absolute path to `config.toml`.
    config_path: PathBuf,
    /// Epoch-millis timestamp of the last API-initiated write. The config
    /// watcher checks this to skip reload events triggered by its own writes.
    last_write_epoch_ms: Arc<AtomicU64>,
}

impl ConfigWriter {
    /// Create a new writer for the given config file path.
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            last_write_epoch_ms: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Path to the managed config file.
    pub fn config_path(&self) -> &Path {
        &self.config_path
    }

    /// Epoch-millis of the most recent successful API write.
    ///
    /// The config file watcher reads this to decide whether to skip a change
    /// event that was triggered by our own write (avoids a redundant reload).
    pub fn last_write_epoch_ms(&self) -> &Arc<AtomicU64> {
        &self.last_write_epoch_ms
    }

    /// Validate, lock, and atomically write `config` to disk.
    ///
    /// Returns `Ok(())` on success. Returns `Err` with a descriptive message
    /// if validation fails or the I/O operation cannot complete.
    pub fn write_config(&self, config: &Config) -> Result<()> {
        // 1. Validate before any I/O.
        config.validate()?;

        // 2. Serialize to TOML.
        let toml_string = config.to_toml_string()?;

        // 3. Acquire advisory lock.
        let parent = self
            .config_path
            .parent()
            .ok_or_else(|| MaestroError::Config("config path has no parent directory".into()))?;
        fs::create_dir_all(parent)?;

        let lock_path = self.config_path.with_extension("toml.lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)?;
        lock_file.lock_exclusive().map_err(|e| {
            MaestroError::Config(format!("Failed to acquire config file lock: {e}"))
        })?;

        // 4. Write to temp file.
        let tmp_path = self.config_path.with_extension("toml.tmp");
        let write_result = fs::write(&tmp_path, &toml_string);
        if let Err(e) = write_result {
            let _ = lock_file.unlock();
            return Err(e.into());
        }

        // 5. Atomic rename.
        let rename_result = fs::rename(&tmp_path, &self.config_path);
        if let Err(e) = rename_result {
            // Clean up the temp file on failure.
            let _ = fs::remove_file(&tmp_path);
            let _ = lock_file.unlock();
            return Err(e.into());
        }

        // 6. Record write timestamp and release lock.
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_write_epoch_ms.store(now_ms, Ordering::Release);

        let _ = lock_file.unlock();

        tracing::info!(path = %self.config_path.display(), "Config persisted to disk");
        Ok(())
    }

    /// Read and parse config from disk, applying workflow step files and
    /// validation — the same pipeline as [`Config::load`].
    pub fn read_config(&self) -> Result<Config> {
        Config::load(&self.config_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid TOML for a `Config` that passes `validate()`.
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

    /// Helper: create a temp dir with a valid `config.toml` inside.
    fn setup_temp_config() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, minimal_valid_toml()).unwrap();
        (dir, path)
    }

    #[test]
    fn write_creates_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let writer = ConfigWriter::new(path.clone());

        let config = Config::load_from_str(minimal_valid_toml()).unwrap();
        writer.write_config(&config).unwrap();

        assert!(path.exists(), "config file should exist after write");
        let content = fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("poll_interval_secs"),
            "written file should contain config fields"
        );
    }

    #[test]
    fn write_replaces_existing_file() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path.clone());

        let mut config = Config::load(&path).unwrap();
        config.general.max_concurrent_workflows = 42;
        writer.write_config(&config).unwrap();

        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.general.max_concurrent_workflows, 42);
    }

    #[test]
    fn write_rejects_invalid_config() {
        let (_dir, path) = setup_temp_config();
        let original_content = fs::read_to_string(&path).unwrap();
        let writer = ConfigWriter::new(path.clone());

        let mut config = Config::load(&path).unwrap();
        config.general.max_concurrent_workflows = 0; // Invalid: must be >= 1
        let result = writer.write_config(&config);

        assert!(result.is_err(), "should reject invalid config");
        // Original file should be unchanged.
        let after_content = fs::read_to_string(&path).unwrap();
        assert_eq!(original_content, after_content);
    }

    #[test]
    fn write_is_atomic_no_partial_file() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path.clone());

        let config = Config::load(&path).unwrap();
        writer.write_config(&config).unwrap();

        // Temp file should not exist after successful write.
        let tmp_path = path.with_extension("toml.tmp");
        assert!(
            !tmp_path.exists(),
            "temp file should be removed after rename"
        );
    }

    #[test]
    fn write_updates_last_write_timestamp() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path.clone());

        assert_eq!(
            writer.last_write_epoch_ms().load(Ordering::Acquire),
            0,
            "initial timestamp should be 0"
        );

        let config = Config::load(&path).unwrap();
        writer.write_config(&config).unwrap();

        let ts = writer.last_write_epoch_ms().load(Ordering::Acquire);
        assert!(ts > 0, "timestamp should be updated after write");
    }

    #[test]
    fn read_config_returns_valid_config() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path);

        let config = writer.read_config().unwrap();
        assert_eq!(config.general.poll_interval_secs, 30);
    }

    #[test]
    fn read_config_fails_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nonexistent.toml");
        let writer = ConfigWriter::new(path);

        let result = writer.read_config();
        assert!(result.is_err());
    }

    #[test]
    fn config_round_trip_preserves_fields() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path.clone());

        let original = Config::load(&path).unwrap();
        writer.write_config(&original).unwrap();
        let reloaded = Config::load(&path).unwrap();

        // Spot-check key fields.
        assert_eq!(
            original.general.poll_interval_secs,
            reloaded.general.poll_interval_secs
        );
        assert_eq!(
            original.general.max_concurrent_workflows,
            reloaded.general.max_concurrent_workflows
        );
        assert_eq!(original.web.port, reloaded.web.port);
        assert_eq!(original.git.base_branch, reloaded.git.base_branch);
        assert_eq!(original.jira.project_keys, reloaded.jira.project_keys);
        assert_eq!(
            original.agent.step_timeout_secs,
            reloaded.agent.step_timeout_secs
        );
    }

    #[test]
    fn concurrent_writes_do_not_corrupt() {
        let (_dir, path) = setup_temp_config();
        let writer = std::sync::Arc::new(ConfigWriter::new(path.clone()));

        let handles: Vec<_> = (0..10)
            .map(|i| {
                let w = writer.clone();
                let p = path.clone();
                std::thread::spawn(move || {
                    let mut config = Config::load(&p).unwrap_or_default();
                    config.general.poll_interval_secs = 30;
                    config.general.max_concurrent_workflows = (i % 5) + 1;
                    config.jira.item_types = vec!["Task".to_string()];
                    config.agent.step_timeout_secs = 600;
                    w.write_config(&config).unwrap();
                })
            })
            .collect();

        for h in handles {
            h.join().unwrap();
        }

        // File should be valid after all concurrent writes.
        let final_config = Config::load(&path).unwrap();
        assert!(final_config.general.max_concurrent_workflows >= 1);
        assert!(final_config.general.max_concurrent_workflows <= 5);
    }
}
