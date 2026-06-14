// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Read-side trait seams over [`JiraClient`] so the poll->start path and
//! ticket-detail retrieval are testable without a live `acli` binary.
//!
//! The two traits are deliberately segregated (Interface Segregation): the
//! poller only lists **To Do** tickets ([`TicketLister`]); the bootstrap
//! driver only reads a single ticket's details ([`TicketReader`]). A caller
//! depends on the one it uses, not on a fat client. [`JiraClient`] implements
//! both by delegating to its inherent methods.
//!
//! [`TicketListerFactory`] builds a per-repo [`TicketLister`]. The poller
//! resolves `repo_path` fresh on every poll (it changes on workspace switch),
//! so it holds a factory and depends on a trait object rather than
//! constructing a concrete [`JiraClient`] inline.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;

use super::client::{JiraClient, JiraTicket};

/// Lists **To Do** tickets for the poll->start path.
#[async_trait]
pub trait TicketLister: Send + Sync {
    /// List **To Do** tickets across `project_keys`, restricted to `item_types`.
    async fn list_todo_tickets(
        &self,
        project_keys: &[String],
        item_types: &[String],
    ) -> Result<Vec<JiraTicket>>;
}

/// Reads a single ticket's details (summary/description/type + linked items)
/// for the bootstrap "Retrieve Details" step.
#[async_trait]
pub trait TicketReader: Send + Sync {
    /// Fetch full details for `key`, including linked items from `project_keys`.
    async fn get_ticket_details(&self, key: &str, project_keys: &[String]) -> Result<JiraTicket>;
}

#[async_trait]
impl TicketLister for JiraClient {
    async fn list_todo_tickets(
        &self,
        project_keys: &[String],
        item_types: &[String],
    ) -> Result<Vec<JiraTicket>> {
        // Inherent method takes priority over the trait method of the same name.
        JiraClient::list_todo_tickets(self, project_keys, item_types).await
    }
}

#[async_trait]
impl TicketReader for JiraClient {
    async fn get_ticket_details(&self, key: &str, project_keys: &[String]) -> Result<JiraTicket> {
        JiraClient::get_ticket_details(self, key, project_keys).await
    }
}

/// Builds a per-repo [`TicketLister`]. Lets the poller depend on a trait object
/// while still resolving `repo_path` from config on each poll.
pub trait TicketListerFactory: Send + Sync {
    /// Construct a lister bound to `repo_path` (the clone `acli` runs against).
    fn lister(&self, repo_path: PathBuf) -> Arc<dyn TicketLister>;
}

/// Production factory: builds real [`JiraClient`]s that shell out to `acli`.
pub struct RealJiraSourceFactory;

impl TicketListerFactory for RealJiraSourceFactory {
    fn lister(&self, repo_path: PathBuf) -> Arc<dyn TicketLister> {
        Arc::new(JiraClient::new(repo_path))
    }
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// In-memory [`TicketLister`] returning a fixed list — no `acli` process.
    pub(crate) struct FakeTicketLister {
        pub tickets: Vec<JiraTicket>,
    }

    #[async_trait]
    impl TicketLister for FakeTicketLister {
        async fn list_todo_tickets(
            &self,
            _project_keys: &[String],
            _item_types: &[String],
        ) -> Result<Vec<JiraTicket>> {
            Ok(self.tickets.clone())
        }
    }

    /// Factory yielding a clone of a preset ticket list for any repo path.
    pub(crate) struct FakeJiraSourceFactory {
        pub tickets: Vec<JiraTicket>,
    }

    impl TicketListerFactory for FakeJiraSourceFactory {
        fn lister(&self, _repo_path: PathBuf) -> Arc<dyn TicketLister> {
            Arc::new(FakeTicketLister {
                tickets: self.tickets.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::FakeTicketLister;
    use super::*;

    fn ticket(key: &str) -> JiraTicket {
        JiraTicket {
            key: key.to_string(),
            summary: format!("summary {key}"),
            description: String::new(),
            item_type: "Task".to_string(),
            status: "To Do".to_string(),
            linked_items: Vec::new(),
        }
    }

    #[tokio::test]
    async fn fake_lister_returns_preset_tickets() {
        let lister = FakeTicketLister {
            tickets: vec![ticket("PROJ-1"), ticket("PROJ-2")],
        };
        let out = lister
            .list_todo_tickets(&["PROJ".to_string()], &["Task".to_string()])
            .await
            .expect("fake never errors");
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].key, "PROJ-1");
    }

    #[test]
    fn real_factory_builds_a_lister() {
        let factory = RealJiraSourceFactory;
        // Construction must not require a live `acli`; it only stores the path.
        let _lister: Arc<dyn TicketLister> = factory.lister(PathBuf::from("/tmp/repo"));
    }
}
