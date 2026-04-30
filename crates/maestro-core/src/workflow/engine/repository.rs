// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use super::types::Workflow;
use crate::workflow::state::WorkflowState;

pub(crate) struct WorkflowRepository {
    workflows: Arc<RwLock<HashMap<String, Workflow>>>,
}

impl WorkflowRepository {
    pub fn new() -> Self {
        Self {
            workflows: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    #[allow(dead_code)]
    pub async fn get(&self, key: &str) -> Option<Workflow> {
        self.workflows.read().await.get(key).cloned()
    }

    pub async fn get_ids(&self) -> Vec<String> {
        self.workflows.read().await.keys().cloned().collect()
    }

    #[allow(dead_code)]
    pub async fn insert(&self, w: Workflow) {
        self.workflows.write().await.insert(w.ticket_key.clone(), w);
    }

    /// Apply a mutation to a workflow. Returns true if found.
    #[allow(dead_code)]
    pub async fn update(&self, key: &str, f: impl FnOnce(&mut Workflow)) -> bool {
        let mut guard = self.workflows.write().await;
        if let Some(w) = guard.get_mut(key) {
            f(w);
            true
        } else {
            false
        }
    }

    #[allow(dead_code)]
    pub async fn remove(&self, key: &str) -> Option<Workflow> {
        self.workflows.write().await.remove(key)
    }

    /// Snapshot: returns all workflows as a Vec (cloned, for persistence).
    #[allow(dead_code)]
    pub async fn snapshot(&self) -> Vec<Workflow> {
        self.workflows.read().await.values().cloned().collect()
    }

    // ── Count queries (used by WorkflowEngine public API) ──────────────────

    pub async fn count_all(&self) -> usize {
        self.workflows.read().await.len()
    }

    pub async fn count_active(&self) -> usize {
        self.workflows
            .read()
            .await
            .values()
            .filter(|w| w.state.is_active())
            .count()
    }

    pub async fn count_occupying_slot(&self) -> usize {
        self.workflows
            .read()
            .await
            .values()
            .filter(|w| w.state.occupies_concurrency_slot())
            .count()
    }

    #[allow(dead_code)]
    pub async fn count_manual_toward_cap(&self) -> usize {
        self.workflows
            .read()
            .await
            .values()
            .filter(|w| {
                w.started_manually
                    && !matches!(
                        w.state,
                        WorkflowState::Done | WorkflowState::Stopped | WorkflowState::Error { .. }
                    )
            })
            .count()
    }

    /// Read-locked access for operations that need to inspect many workflows.
    #[allow(dead_code)]
    pub async fn with_read<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&HashMap<String, Workflow>) -> R,
    {
        let guard = self.workflows.read().await;
        f(&guard)
    }

    /// Write-locked access for batch operations.
    #[allow(dead_code)]
    pub async fn with_write<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut HashMap<String, Workflow>) -> R,
    {
        let mut guard = self.workflows.write().await;
        f(&mut guard)
    }

    /// Temporary: exposes the inner Arc for free functions that take
    /// `Arc<RwLock<HashMap<...>>>`.  Remove once driver.rs is fully refactored
    /// to use a proper WorkflowContext.
    pub fn inner_arc(&self) -> Arc<RwLock<HashMap<String, Workflow>>> {
        self.workflows.clone()
    }
}
