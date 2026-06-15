// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use tokio::sync::broadcast;

use super::types::WorkflowEvent;

pub(crate) struct WorkflowEventBus {
    tx: broadcast::Sender<WorkflowEvent>,
}

impl WorkflowEventBus {
    pub fn new() -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { tx }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<WorkflowEvent> {
        self.tx.subscribe()
    }

    pub fn send(&self, event: WorkflowEvent) {
        let _ = self.tx.send(event);
    }

    /// Expose the raw sender for cases that need to pass it to free functions
    /// (transitional — remove once driver.rs is fully refactored).
    pub fn sender(&self) -> &broadcast::Sender<WorkflowEvent> {
        &self.tx
    }
}
