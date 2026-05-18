// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

/// Application version from the VERSION file at the repository root.
pub const VERSION: &str = include_str!("../../../VERSION");

pub mod actions;
pub mod agent_prompt;
pub mod auth;
pub mod claude;
pub mod config;
pub mod config_watcher;
pub mod config_writer;
pub mod container;
pub mod cursor;
pub mod db;
pub mod dev_mock;
pub mod docker_hooks;
pub mod error;
pub mod git;
pub mod github;
pub mod github_app;
pub mod jira;
pub mod license;
pub mod process;
pub mod repo_reconcile;
pub mod skill_resolve;
pub mod workflow;
