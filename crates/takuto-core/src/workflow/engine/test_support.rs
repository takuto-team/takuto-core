// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Shared fixtures for the workflow-engine unit tests.
//!
//! Everything here is in-process: an in-memory database, [`DryRunActions`]
//! (logs Jira writes, no network), a canned [`MockGhClient`], and the
//! `dev_mock` mock-agent. No Docker daemon or AI agent is ever touched, so
//! these fixtures stay deterministic with the daemon stopped.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use async_trait::async_trait;
use tokio::sync::RwLock;

use crate::actions::dry_run::DryRunActions;
use crate::actions::traits::ExternalActions;
use crate::auth::{GhClient, GhResponse};
use crate::config::{Config, TicketingSystem};
use crate::db::user_work_item_flows::{UserFlow, UserFlowStep};
use crate::db::{Database, DbValue};
use crate::workflow::engine::{Workflow, WorkflowEngine};
use crate::workflow::state::WorkflowState;

/// A canned [`GhClient`] — always reports a live `repo`-scoped user. Engine
/// state/persistence tests rarely consult it, but `with_gh_client` requires
/// some impl.
#[derive(Default)]
pub(crate) struct MockGhClient;

#[async_trait]
impl GhClient for MockGhClient {
    async fn api_user(&self, _pat: &str) -> Result<GhResponse, String> {
        Ok(GhResponse {
            status: 200,
            headers: vec![("X-OAuth-Scopes".to_string(), "repo".to_string())],
            body: r#"{"login":"tester"}"#.to_string(),
        })
    }
    async fn api_org(&self, _pat: &str, _org: &str) -> Result<GhResponse, String> {
        Ok(GhResponse {
            status: 200,
            headers: vec![],
            body: "{}".to_string(),
        })
    }
}

/// An in-memory-DB engine wired with `DryRunActions` + [`MockGhClient`].
/// `max_concurrent_workflows = 4` so concurrency-slot tests have headroom.
pub(crate) fn test_engine(workflows_dir: &Path) -> (WorkflowEngine, Database) {
    let db = Database::open_in_memory().expect("in-memory db");
    let config = Arc::new(RwLock::new(Config::default()));
    let actions: Arc<dyn ExternalActions> =
        Arc::new(DryRunActions::new("origin".to_string(), None));
    let engine = WorkflowEngine::new_with_db(
        config,
        actions,
        4,
        Arc::new(AtomicBool::new(false)),
        TicketingSystem::None,
        workflows_dir.to_path_buf(),
        Some(db.clone()),
    )
    .with_gh_client(Arc::new(MockGhClient));
    (engine, db)
}

/// Insert a `users` row (role `"admin"` or `"user"`).
pub(crate) async fn seed_user(db: &Database, id: &str, role: &str) {
    db.adapter()
        .execute(
            "INSERT INTO users (id, username, role) VALUES (?, ?, ?)",
            vec![
                DbValue::Text(id.to_string()),
                DbValue::Text(id.to_string()),
                DbValue::Text(role.to_string()),
            ],
        )
        .await
        .expect("seed user");
}

/// `Paused { source_state: Pending }` — the common paused fixture.
pub(crate) fn paused() -> WorkflowState {
    WorkflowState::Paused {
        source_state: Box::new(WorkflowState::Pending),
    }
}

/// `Error { source_state: Pending, message }` — the common errored fixture.
pub(crate) fn errored() -> WorkflowState {
    WorkflowState::Error {
        source_state: Box::new(WorkflowState::Pending),
        message: "boom".to_string(),
    }
}

/// Build a [`Workflow`] in `state` for `key`, optionally owned by `user`.
/// Uses [`Workflow::new`] so new fields pick up their defaults automatically.
pub(crate) fn seed_workflow(state: WorkflowState, key: &str, user: Option<&str>) -> Workflow {
    let mut wf = Workflow::new(
        key.to_string(),
        format!("{key} summary"),
        false,
        false,
        TicketingSystem::None,
        None,
        "ws".to_string(),
    );
    wf.state = state;
    wf.user_id = user.map(str::to_string);
    wf
}

/// Insert a workflow into the engine's in-memory map (keyed by ticket_key).
pub(crate) async fn insert(engine: &WorkflowEngine, wf: Workflow) {
    let key = wf.ticket_key.clone();
    engine.workflows_arc().write().await.insert(key, wf);
}

/// Build a [`UserFlow`] named `name`, depending on `deps`, with one trivial
/// agent step per entry in `steps`.
pub(crate) fn flow(name: &str, deps: &[&str], steps: &[&str]) -> UserFlow {
    UserFlow {
        name: name.to_string(),
        depends_on: deps.iter().map(|s| s.to_string()).collect(),
        steps: steps
            .iter()
            .map(|s| UserFlowStep {
                name: s.to_string(),
                prompt: format!("do {s}"),
                skills: Vec::new(),
            })
            .collect(),
    }
}
