// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/// Application version from the VERSION file at the repository root.
pub const VERSION: &str = include_str!("../../../VERSION");

pub mod actions;
pub mod agent_prompt;
pub mod claude;
pub mod config;
pub mod container;
pub mod cursor;
pub mod docker_hooks;
pub mod error;
pub mod git;
pub mod github;
pub mod github_app;
pub mod jira;
pub mod license;
pub mod process;
pub mod skill_resolve;
pub mod workflow;
