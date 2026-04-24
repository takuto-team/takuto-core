// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Thin `gh` (GitHub CLI) wrapper.
//!
//! Security is delegated to the GitHub token scope (fine-grained PAT or GitHub App
//! installation token). No engine-level argv allowlist is applied.

use std::path::Path;

use tokio_util::sync::CancellationToken;

use crate::error::Result;
use crate::process::{self, CommandOutput};

/// Spawn `gh` with the given argv (no shell — avoids injection).
pub async fn run_gh(
    argv: &[&str],
    cwd: &Path,
    cancel: CancellationToken,
) -> Result<CommandOutput> {
    process::run_command("gh", argv, cwd, cancel).await
}
