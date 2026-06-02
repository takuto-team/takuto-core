// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Atomic, mode-0400 write of a secret file. Defined twice so the Unix
//! build sets `O_CREAT | O_EXCL` + `mode 0400` via `OpenOptionsExt`; the
//! cfg(not(unix)) variant relies on the parent tempdir already being 0700.

use std::path::Path;

use crate::config::ConfigError;
use crate::error::Result;

/// Write a secret file with mode 0400 (owner-read-only on Unix). The parent
/// is already 0700 because `TempDir::new()` uses `OsRng` + the kernel's
/// secure-temp helpers.
#[cfg(unix)]
pub(super) fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o400)
        .open(path)
        .map_err(|e| ConfigError::BundleSecretFile {
            op: "create",
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    f.write_all(bytes)
        .map_err(|e| ConfigError::BundleSecretFile {
            op: "write",
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    f.sync_all().ok();
    Ok(())
}

#[cfg(not(unix))]
pub(super) fn write_secret_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|e| ConfigError::BundleSecretFile {
            op: "create",
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    f.write_all(bytes)
        .map_err(|e| ConfigError::BundleSecretFile {
            op: "write",
            path: path.to_path_buf(),
            detail: e.to_string(),
        })?;
    f.sync_all().ok();
    Ok(())
}
