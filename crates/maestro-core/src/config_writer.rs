// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.
//! Atomic, validated config persistence.
//!
//! [`ConfigWriter`] provides the single write path for `config.toml`. All
//! dashboard and wizard writes funnel through this module — request handlers
//! never touch the filesystem directly.
//!
//! Write protocol (preferred — atomic rename):
//! 1. Validate the [`Config`] against the schema (`Config::validate()`).
//! 2. Acquire an advisory file lock (`fs2::FileExt::lock_exclusive`).
//! 3. Serialize to TOML.
//! 4. Write to a temp file in the same directory.
//! 5. `fs::rename` the temp file over the target (atomic on POSIX).
//! 6. Release the lock.
//!
//! Write protocol (fallback — in-place write, task #38):
//! On `EBUSY` from step 5 (Docker single-file bind mounts hold the inode
//! busy → POSIX `rename(2)` refuses to clobber it), the writer falls back
//! to:
//!   a. Best-effort backup: copy current `config.toml` to `config.toml.bak`
//!      so a power loss mid-write doesn't lose the previous valid content.
//!   b. Truncate-and-write `config.toml` in place; `fsync(2)` it.
//!   c. Remove the orphan `.tmp` file (no longer useful).
//!   d. Set [`ConfigWriter::used_inplace_fallback`] so the dashboard banner
//!      can surface an info-level `config_file_bind_mounted` warning.
//! Both paths update `last_write_epoch_ms` identically so `ConfigWatcher`'s
//! self-write dedup keeps working in either mode.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use fs2::FileExt;

use crate::config::{Config, ConfigError};
use crate::error::Result;

/// Linux `EBUSY` raw OS error number. Used in [`ConfigWriter::write_config`]
/// to detect bind-mounted single-file targets and trigger the in-place
/// fallback. Defined here as a named constant rather than `libc::EBUSY` so
/// the module stays portable (Windows/macOS dev builds still compile; the
/// fallback never fires there because `rename(2)` succeeds normally).
const RAW_OS_ERROR_EBUSY: i32 = 16;

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
    /// Task #38: latched `true` the first time `write_config` falls back to
    /// the in-place path because `rename(2)` returned `EBUSY` (the
    /// bind-mounted-single-file case). Read by the dashboard refresh path
    /// to surface an info-level `config_file_bind_mounted` warning so
    /// admins know they're on the fallback. Never reset — the diagnosis
    /// is sticky for the process lifetime.
    used_inplace_fallback: Arc<AtomicBool>,
    /// Test-only knob: when `true`, [`Self::try_rename`] returns a
    /// synthetic `EBUSY` instead of calling `fs::rename`. This is the
    /// cleanest way to exercise the fallback path without actually
    /// constructing a Linux bind mount in a unit test.
    #[cfg(test)]
    force_inplace_for_tests: AtomicBool,
}

impl ConfigWriter {
    /// Create a new writer for the given config file path.
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            last_write_epoch_ms: Arc::new(AtomicU64::new(0)),
            used_inplace_fallback: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            force_inplace_for_tests: AtomicBool::new(false),
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

    /// Task #38: `true` once `write_config` has had to fall back to the
    /// in-place write path (i.e. `config.toml` is bind-mounted as a single
    /// file and atomic rename can't clobber it). The dashboard refresh
    /// path checks this and emits an info-level `config_file_bind_mounted`
    /// warning. The flag latches `true` for the process lifetime.
    pub fn used_inplace_fallback(&self) -> &Arc<AtomicBool> {
        &self.used_inplace_fallback
    }

    /// Try to `rename(2)` the temp file over the target. Indirection point
    /// for testing — in `#[cfg(test)]` builds, the writer can be flipped
    /// to always return a synthetic `EBUSY` so the fallback path is
    /// exercisable without a real bind mount.
    fn try_rename(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        #[cfg(test)]
        if self.force_inplace_for_tests.load(Ordering::Acquire) {
            return Err(std::io::Error::from_raw_os_error(RAW_OS_ERROR_EBUSY));
        }
        fs::rename(src, dst)
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
            .ok_or_else(|| ConfigError::Operational {
                op: "config write",
                detail: "config path has no parent directory".to_string(),
            })?;
        fs::create_dir_all(parent)?;

        let lock_path = self.config_path.with_extension("toml.lock");
        let lock_file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&lock_path)?;
        lock_file.lock_exclusive().map_err(|e| ConfigError::Operational {
            op: "config file lock",
            detail: e.to_string(),
        })?;

        // 4. Write to temp file.
        let tmp_path = self.config_path.with_extension("toml.tmp");
        let write_result = fs::write(&tmp_path, &toml_string);
        if let Err(e) = write_result {
            let _ = lock_file.unlock();
            return Err(e.into());
        }

        // 5. Atomic rename — fast path. On EBUSY (`config.toml` is
        //    bind-mounted as a single file under Docker), fall back to an
        //    in-place truncate+write with a best-effort `.bak` first.
        match self.try_rename(&tmp_path, &self.config_path) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(RAW_OS_ERROR_EBUSY) => {
                tracing::info!(
                    path = %self.config_path.display(),
                    "config.toml is bind-mounted as a single file; \
                     using in-place write fallback (atomic rename returned EBUSY)"
                );
                if let Err(write_err) = Self::write_in_place_with_backup(
                    &self.config_path,
                    toml_string.as_bytes(),
                ) {
                    let _ = fs::remove_file(&tmp_path);
                    let _ = lock_file.unlock();
                    return Err(write_err.into());
                }
                // Orphan tmp no longer useful.
                let _ = fs::remove_file(&tmp_path);
                // Latch the diagnostic flag so the dashboard surfaces an
                // info-level warning. Sticky for process lifetime.
                self.used_inplace_fallback.store(true, Ordering::Release);
            }
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                let _ = lock_file.unlock();
                return Err(e.into());
            }
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

    /// Task #38 in-place fallback: truncate and rewrite `target` in place.
    /// Before rewriting, copy the existing content to `target.bak` so a
    /// power loss between `truncate` and `write_all` doesn't lose the
    /// previous valid config. The backup write is **best-effort** — when
    /// it fails we log and continue rather than blocking the save (e.g.
    /// `.bak` already exists with restrictive perms).
    ///
    /// Caller is responsible for the advisory lock; this helper assumes
    /// the lock is held.
    fn write_in_place_with_backup(target: &Path, new_contents: &[u8]) -> std::io::Result<()> {
        // Best-effort backup. fs::copy is atomic-replace at the FS layer
        // and won't fail if the bak path is regular.
        let bak_path = target.with_extension("toml.bak");
        if let Err(e) = fs::copy(target, &bak_path) {
            tracing::warn!(
                bak = %bak_path.display(),
                error = %e,
                "config.toml.bak write failed; continuing with in-place rewrite \
                 (config is still valid on disk pre-rewrite)"
            );
        }
        // Truncate-and-write in place. Open with truncate=true so the file
        // contents are zeroed before write_all — required because the bind
        // mount may already be open by another reader. fsync to make the
        // new contents durable.
        let mut f = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(target)?;
        f.write_all(new_contents)?;
        f.sync_all()?;
        Ok(())
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

    // ─── Task #38: in-place fallback on EBUSY ───────────────────────────

    /// When `try_rename` returns synthetic EBUSY (forced via the test
    /// knob), `write_config` must take the in-place path: new contents
    /// land in the target, `.bak` carries the previous contents, the
    /// `.tmp` orphan is removed, and `used_inplace_fallback` is set.
    #[test]
    fn write_config_falls_back_to_in_place_on_ebusy() {
        let (_dir, path) = setup_temp_config();
        let original_content = fs::read_to_string(&path).unwrap();
        let writer = ConfigWriter::new(path.clone());

        // Force the synthetic EBUSY before writing.
        writer
            .force_inplace_for_tests
            .store(true, Ordering::Release);

        let mut config = Config::load(&path).unwrap();
        config.general.max_concurrent_workflows = 42;
        writer
            .write_config(&config)
            .expect("in-place fallback must succeed when atomic rename fails with EBUSY");

        // Target file now reflects the new config.
        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.general.max_concurrent_workflows, 42);

        // .bak file exists and carries the PRE-write contents.
        let bak_path = path.with_extension("toml.bak");
        assert!(bak_path.exists(), ".bak file must exist after in-place write");
        let bak_content = fs::read_to_string(&bak_path).unwrap();
        assert_eq!(
            bak_content, original_content,
            ".bak must preserve the previous valid content"
        );

        // .tmp orphan was cleaned up.
        let tmp_path = path.with_extension("toml.tmp");
        assert!(
            !tmp_path.exists(),
            "in-place fallback must remove the orphan .tmp"
        );

        // Diagnostic flag latched.
        assert!(
            writer.used_inplace_fallback().load(Ordering::Acquire),
            "used_inplace_fallback must be set after EBUSY fallback"
        );
    }

    /// The diagnostic flag must NOT be set when the fast path (atomic
    /// rename) succeeds. Otherwise the dashboard would emit a misleading
    /// "bind-mounted" banner on plain deployments.
    #[test]
    fn write_config_does_not_set_inplace_flag_on_fast_path() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path.clone());
        // No forced EBUSY — should take the rename fast path.

        let config = Config::load(&path).unwrap();
        writer.write_config(&config).unwrap();

        assert!(
            !writer.used_inplace_fallback().load(Ordering::Acquire),
            "fast-path write must not flip the in-place diagnostic flag"
        );
    }

    /// In-place fallback must keep `last_write_epoch_ms` updated so the
    /// `ConfigWatcher` self-write dedup keeps working in either mode.
    #[test]
    fn in_place_fallback_updates_last_write_timestamp() {
        let (_dir, path) = setup_temp_config();
        let writer = ConfigWriter::new(path.clone());
        writer
            .force_inplace_for_tests
            .store(true, Ordering::Release);

        assert_eq!(
            writer.last_write_epoch_ms().load(Ordering::Acquire),
            0,
            "initial timestamp should be 0"
        );

        let config = Config::load(&path).unwrap();
        writer.write_config(&config).unwrap();

        let ts = writer.last_write_epoch_ms().load(Ordering::Acquire);
        assert!(
            ts > 0,
            "timestamp must be updated by the in-place fallback too \
             (otherwise ConfigWatcher would replay our own write)"
        );
    }

    /// Corruption-safety smoke test: if the `.bak` write fails (here we
    /// pre-create a 0-byte read-only file at that path), the in-place
    /// rewrite must still proceed — `.bak` is best-effort. The target
    /// file ends up with the new contents.
    #[cfg(unix)]
    #[test]
    fn in_place_fallback_tolerates_bak_write_failure() {
        use std::os::unix::fs::PermissionsExt;

        let (_dir, path) = setup_temp_config();
        // Make the `.bak` slot uncopyable by pre-creating it with mode 0.
        // (When running as root, mode 0 doesn't actually block writes —
        // CAP_DAC_OVERRIDE bypasses POSIX perms — so we skip the assertion
        // there. The in-place write must still succeed.)
        let bak_path = path.with_extension("toml.bak");
        fs::write(&bak_path, b"").unwrap();
        let mut perms = fs::metadata(&bak_path).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&bak_path, perms).unwrap();

        let writer = ConfigWriter::new(path.clone());
        writer
            .force_inplace_for_tests
            .store(true, Ordering::Release);

        let mut config = Config::load(&path).unwrap();
        config.general.max_concurrent_workflows = 7;
        let result = writer.write_config(&config);

        // Restore perms so tempdir cleanup succeeds regardless of outcome.
        let mut restore = fs::metadata(&bak_path).unwrap().permissions();
        restore.set_mode(0o600);
        fs::set_permissions(&bak_path, restore).unwrap();

        result.expect("in-place write must succeed even when .bak copy fails");
        let reloaded = Config::load(&path).unwrap();
        assert_eq!(reloaded.general.max_concurrent_workflows, 7);
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
