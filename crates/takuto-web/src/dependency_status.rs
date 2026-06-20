// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Process-wide status of the runtime agent/CLI install, surfaced to the web UI.
//!
//! The agent + Atlassian CLIs are installed at server startup into the shared
//! tools volume (see `takuto_core::agent_install`). Because that happens while
//! the web server is already listening, the dashboard polls
//! `GET /api/system/dependencies` and shows an "Installing dependencies" overlay
//! with the current step until `phase == Ready`.
//!
//! State lives in a small encapsulated process global (keyed off the binary's
//! single startup install), so it needs no plumbing through `AppState` and its
//! many construction sites. Access is only ever through the functions below.

use std::sync::{Arc, OnceLock, RwLock};

use serde::Serialize;
use tokio::sync::RwLock as AsyncRwLock;

use takuto_core::agent_install::{Installer, ProgressSink};
use takuto_core::config::Config;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    /// No install has been started (e.g. local dev without the tools volume).
    Idle,
    Installing,
    Ready,
    Error,
}

#[derive(Debug, Clone, Serialize)]
pub struct DependencyInstallStatus {
    pub phase: Phase,
    /// Label of the step in progress (e.g. "Claude Code (latest)").
    pub current_step: String,
    /// Steps completed so far / total to install (best-effort; 0 until known).
    pub done: usize,
    pub total: usize,
    /// Present only when `phase == Error`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Default for DependencyInstallStatus {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            current_step: String::new(),
            done: 0,
            total: 0,
            error: None,
        }
    }
}

fn cell() -> &'static RwLock<DependencyInstallStatus> {
    static STATUS: OnceLock<RwLock<DependencyInstallStatus>> = OnceLock::new();
    STATUS.get_or_init(|| RwLock::new(DependencyInstallStatus::default()))
}

/// Current snapshot (cheap clone) for the status endpoint.
pub fn snapshot() -> DependencyInstallStatus {
    cell().read().expect("dependency status lock").clone()
}

fn update(f: impl FnOnce(&mut DependencyInstallStatus)) {
    let mut g = cell().write().expect("dependency status lock");
    f(&mut g);
}

/// [`ProgressSink`] that writes into the process global the endpoint reads.
struct GlobalSink;

impl ProgressSink for GlobalSink {
    fn step(&self, index: usize, total: usize, label: &str) {
        update(|s| {
            s.phase = Phase::Installing;
            s.current_step = label.to_string();
            s.done = index;
            s.total = total;
            s.error = None;
        });
    }
    fn finished(&self) {
        update(|s| {
            s.phase = Phase::Ready;
            s.done = s.total;
            s.current_step.clear();
        });
    }
    fn failed(&self, label: &str, error: &str) {
        update(|s| {
            s.phase = Phase::Error;
            s.error = Some(format!("{label}: {error}"));
        });
    }
}

/// Spawn the startup install in the background so the server keeps serving while
/// it runs. No-op (leaves `phase = Idle`) when the tools volume isn't mounted
/// (local dev), so `cargo run` doesn't try to npm-install into `/opt`.
pub fn spawn_install(config: Arc<AsyncRwLock<Config>>) {
    let install_dir =
        std::env::var("TAKUTO_TOOLS_DIR").unwrap_or_else(|_| "/opt/takuto-tools".to_string());
    if !std::path::Path::new(&install_dir).exists() {
        tracing::info!(
            install_dir = %install_dir,
            "Tools volume absent; skipping runtime agent install (local dev)"
        );
        return;
    }
    update(|s| {
        s.phase = Phase::Installing;
        s.current_step = "Preparing…".to_string();
        s.done = 0;
        s.total = 0;
        s.error = None;
    });
    tokio::spawn(async move {
        let cfg = config.read().await.clone();
        let installer = Installer::new(install_dir);
        match installer.install_all(&cfg, &GlobalSink).await {
            Ok(()) => tracing::info!("Runtime agent install complete"),
            Err(e) => tracing::error!(error = %e, "Runtime agent install failed"),
        }
    });
}
