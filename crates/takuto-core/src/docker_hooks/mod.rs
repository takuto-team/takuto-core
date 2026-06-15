// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Config-driven shell hooks for Docker image build and container startup,
//! plus the boot-time `SystemStatus` collector consumed by the dashboard.
//!
//! The implementation is split across six sibling modules; this file only
//! wires them up and re-exports the public surface so every existing
//! `crate::docker_hooks::*` / `takuto_core::docker_hooks::*` path keeps
//! resolving:
//!
//! - [`process`] — `auth_cmd_ok`, `preflight_home`, unix process-group setup
//!   and best-effort cleanup.
//! - [`cursor_auth`] — Cursor on-disk auth heuristics.
//! - [`gh_auth`] — `gh auth switch` recovery for expired App-installation tokens.
//! - [`hook_runner`] — `bash -c` hook executor for Docker build/up phases.
//! - [`status_types`] — public `SystemStatus` shape (HTTP contract for
//!   `GET /api/onboarding/status`) plus `StructuredWarning`, `PreflightResult`.
//! - [`status`] — `collect_system_status[_with_db]`, `check_config_dir_writable`,
//!   `check_acli_auth`, and the `#[deprecated]` `preflight()` shim.

mod cursor_auth;
mod gh_auth;
mod hook_runner;
mod process;
mod status;
mod status_types;

pub use hook_runner::run_hook_commands;
#[allow(deprecated)]
pub use status::preflight;
pub use status::{
    check_acli_auth, check_config_dir_writable, collect_system_status,
    collect_system_status_with_db,
};
pub use status_types::{
    GitHubStatus, PreflightResult, ProviderStatus, StructuredWarning, SystemStatus, TicketingStatus,
};
