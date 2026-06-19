// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Testability seam in front of the Docker calls made by the run-command
//! endpoints.
//!
//! `start_run_command` / `stop_run_command` reach Docker through concrete,
//! static functions in `takuto_core::container`, which a unit test cannot
//! exercise without a real daemon spawning a real container. This trait moves
//! those four calls behind an injectable boundary — production wires
//! [`DockerSpawner`] (pure delegation, behaviour unchanged), tests wire a fake.
//! Mirrors the engine's `ExternalActions` / `DryRunActions` pattern.

use std::path::Path;

use async_trait::async_trait;
use takuto_core::auth::WorkerSecretsBundle;
use takuto_core::container::{self, ContainerRunner};

/// The Docker operations the run-command handlers depend on.
#[async_trait]
pub trait ContainerSpawner: Send + Sync {
    /// Whether a working Docker daemon is reachable.
    fn is_available(&self) -> bool;

    /// Resolve the worker image to run the command in (`None` ⇒ caller falls
    /// back to `takuto:latest`).
    async fn discover_worker_image(&self) -> Option<String>;

    /// Spawn the run-command container, returning the allocated spare host
    /// ports for the caller's port scanner. `extra_env` is owned (rather than
    /// borrowed `&str` pairs) purely to keep the `async_trait` boundary clean.
    #[allow(clippy::too_many_arguments)]
    async fn start_run_command(
        &self,
        ticket_key: &str,
        worktree_path: &Path,
        image: &str,
        command: &str,
        cmd_index: usize,
        dynamic_ports: usize,
        isolate_workspace: bool,
        extra_env: &[(String, String)],
        secrets_bundle: Option<&WorkerSecretsBundle>,
        // Workspace init commands, run when the workspace container is brought up.
        init_commands: &[String],
    ) -> Result<Vec<u16>, String>;

    /// Stop and remove the run-command container (best-effort).
    async fn stop_run_command(&self, ticket_key: &str, cmd_index: usize);
}

/// Production spawner — delegates verbatim to `takuto_core::container`.
pub struct DockerSpawner;

#[async_trait]
impl ContainerSpawner for DockerSpawner {
    fn is_available(&self) -> bool {
        ContainerRunner::is_available()
    }

    async fn discover_worker_image(&self) -> Option<String> {
        ContainerRunner::discover_worker_image().await
    }

    async fn start_run_command(
        &self,
        ticket_key: &str,
        worktree_path: &Path,
        image: &str,
        command: &str,
        cmd_index: usize,
        dynamic_ports: usize,
        isolate_workspace: bool,
        extra_env: &[(String, String)],
        secrets_bundle: Option<&WorkerSecretsBundle>,
        init_commands: &[String],
    ) -> Result<Vec<u16>, String> {
        let env: Vec<(&str, &str)> = extra_env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        container::start_run_command(
            ticket_key,
            worktree_path,
            image,
            command,
            cmd_index,
            dynamic_ports,
            isolate_workspace,
            &env,
            secrets_bundle,
            init_commands,
        )
        .await
    }

    async fn stop_run_command(&self, ticket_key: &str, cmd_index: usize) {
        container::stop_run_command(ticket_key, cmd_index).await
    }
}
