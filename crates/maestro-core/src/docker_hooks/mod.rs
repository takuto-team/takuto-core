// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Config-driven shell hooks for Docker image build and container startup.

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
    GitHubStatus, PreflightResult, ProviderStatus, StructuredWarning, SystemStatus,
    TicketingStatus,
};
