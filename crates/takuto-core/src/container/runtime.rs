// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! [`ContainerRuntime`] — a trait seam over the **process-spawning** docker
//! operations that back container orchestration: availability probing, worker
//! image discovery, and worker-container cleanup.
//!
//! Why this exists: the pure `docker run` argument assembly in
//! [`super::runner::ContainerRunner`] (`wrap_command` / `wrap_shell_command`)
//! is already unit-testable because it only builds strings. The remaining
//! docker touch-points actually spawn `docker info` / `docker inspect` /
//! `docker rm`, so any code path that branches on them could not be exercised
//! without a live daemon. This trait abstracts exactly those calls so callers
//! can depend on `Arc<dyn ContainerRuntime>` and tests can inject a fake.
//!
//! [`DockerRuntime`] is the production implementation; it delegates to the
//! existing [`ContainerRunner`] associated functions so behavior is unchanged.

use async_trait::async_trait;

use super::runner::ContainerRunner;

/// The process-spawning docker operations used during container orchestration.
#[async_trait]
pub trait ContainerRuntime: Send + Sync {
    /// `true` when a docker daemon is reachable (DinD configured + `docker info`
    /// succeeds). Implementations may cache the result.
    async fn is_available(&self) -> bool;

    /// Resolve the worker image to run agent steps in, or `None` when it cannot
    /// be auto-detected.
    async fn discover_worker_image(&self) -> Option<String>;

    /// Force-remove all worker containers for `ticket_key` (and prune dangling
    /// images). Best-effort; never fails the caller.
    async fn cleanup_for_ticket(&self, ticket_key: &str);
}

/// Production [`ContainerRuntime`] backed by the real docker CLI via
/// [`ContainerRunner`].
pub struct DockerRuntime;

#[async_trait]
impl ContainerRuntime for DockerRuntime {
    async fn is_available(&self) -> bool {
        ContainerRunner::is_available()
    }

    async fn discover_worker_image(&self) -> Option<String> {
        ContainerRunner::discover_worker_image().await
    }

    async fn cleanup_for_ticket(&self, ticket_key: &str) {
        ContainerRunner::cleanup_for_ticket(ticket_key).await;
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::Mutex;

    /// Configurable in-memory [`ContainerRuntime`] for tests — never spawns a
    /// process. Records the ticket keys passed to `cleanup_for_ticket`.
    pub(crate) struct FakeContainerRuntime {
        pub available: bool,
        pub worker_image: Option<String>,
        pub cleaned: Mutex<Vec<String>>,
    }

    impl FakeContainerRuntime {
        pub fn unavailable() -> Self {
            Self {
                available: false,
                worker_image: None,
                cleaned: Mutex::new(Vec::new()),
            }
        }

        pub fn available_with_image(image: &str) -> Self {
            Self {
                available: true,
                worker_image: Some(image.to_string()),
                cleaned: Mutex::new(Vec::new()),
            }
        }

        pub fn cleaned_tickets(&self) -> Vec<String> {
            self.cleaned.lock().expect("cleaned lock poisoned").clone()
        }
    }

    #[async_trait]
    impl ContainerRuntime for FakeContainerRuntime {
        async fn is_available(&self) -> bool {
            self.available
        }

        async fn discover_worker_image(&self) -> Option<String> {
            self.worker_image.clone()
        }

        async fn cleanup_for_ticket(&self, ticket_key: &str) {
            self.cleaned
                .lock()
                .expect("cleaned lock poisoned")
                .push(ticket_key.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeContainerRuntime;
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn fake_reports_configured_availability_and_image() {
        let rt = FakeContainerRuntime::available_with_image("takuto:latest");
        assert!(rt.is_available().await);
        assert_eq!(
            rt.discover_worker_image().await.as_deref(),
            Some("takuto:latest")
        );

        let down = FakeContainerRuntime::unavailable();
        assert!(!down.is_available().await);
        assert!(down.discover_worker_image().await.is_none());
    }

    #[tokio::test]
    async fn fake_records_cleanup_calls_behind_trait_object() {
        let rt: Arc<dyn ContainerRuntime> =
            Arc::new(FakeContainerRuntime::available_with_image("img"));
        rt.cleanup_for_ticket("PROJ-1").await;
        rt.cleanup_for_ticket("PROJ-2").await;

        // Downcast-free check: hold a concrete handle too.
        let concrete = Arc::new(FakeContainerRuntime::unavailable());
        let dyn_handle: Arc<dyn ContainerRuntime> = concrete.clone();
        dyn_handle.cleanup_for_ticket("PROJ-3").await;
        assert_eq!(concrete.cleaned_tickets(), vec!["PROJ-3".to_string()]);
    }
}
