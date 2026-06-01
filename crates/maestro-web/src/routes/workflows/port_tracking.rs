// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Background tasks that subscribe to `WorkflowEvent`s and keep the dashboard's
//! dynamic-port forwarding maps + `PathTokenRegistry` in sync.

use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use maestro_core::container;
use maestro_core::db::Database;
use maestro_core::workflow::engine::WorkflowEvent;

use crate::session_registry::{PathTokenRegistry, SessionRoute, SessionRouteKind};
use crate::state::{DynamicForwardsMap, DynamicPortForward};

/// Listen on the workflow event broadcast channel and keep the dynamic-forwards
/// map in sync for the given ticket.  Runs until `cancel` fires or the channel
/// closes.
///
/// `work_item_id` + `db` are Plan-07 step 4 slice 7 shadow-write
/// inputs. When both are `Some`, every `port_forwarded` event also
/// upserts a row into `work_item_port_mappings`; cleanup is handled
/// by the bulk-delete in `close_editor`, so the unforward path
/// stays unchanged. `None` on either preserves the pre-shadow
/// behaviour (used by unit tests).
#[allow(clippy::too_many_arguments)]
pub async fn track_port_forwards(
    ticket_key: String,
    user_id: String,
    dyn_fwd: DynamicForwardsMap,
    registry: PathTokenRegistry,
    mut rx: broadcast::Receiver<WorkflowEvent>,
    cancel: CancellationToken,
    work_item_id: Option<String>,
    db: Option<Database>,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            msg = rx.recv() => {
                match msg {
                    Ok(evt) if evt.ticket_key == ticket_key => {
                        if evt.event_type == "port_forwarded"
                            && let Some((cp, hp)) = evt.forwarded_port
                        {
                            let mut map = dyn_fwd.write().await;
                            let list = map.entry(ticket_key.clone()).or_default();
                            if !list.iter().any(|f| f.container_port == cp) {
                                let path_token = registry.register(SessionRoute {
                                    kind: SessionRouteKind::DynamicPort,
                                    host_port: hp,
                                    ticket_key: ticket_key.clone(),
                                    user_id: user_id.clone(),
                                }).await;
                                let proxy_url = container::build_session_dynamic_port_url(&path_token);
                                // Plan-07 step 4 slice 7: shadow-write the
                                // scanner-detected Dynamic port row. Cleanup
                                // is handled in bulk by `close_editor`, so
                                // the unforward path below stays unchanged.
                                if let Some(ref wi) = work_item_id {
                                    maestro_core::db::work_items::shadow_upsert_port_mapping(
                                        db.as_ref(),
                                        wi,
                                        cp as i32,
                                        hp as i32,
                                        &proxy_url,
                                        &path_token,
                                        maestro_core::db::work_items::PortMappingKind::Dynamic,
                                        None,
                                        chrono::Utc::now().timestamp(),
                                    )
                                    .await;
                                }
                                list.push(DynamicPortForward {
                                    container_port: cp,
                                    host_port: hp,
                                    proxy_url,
                                    path_token,
                                });
                            }
                        } else if evt.event_type == "port_unforwarded"
                            && let Some((cp, _)) = evt.forwarded_port
                        {
                            let mut map = dyn_fwd.write().await;
                            if let Some(list) = map.get_mut(&ticket_key)
                                && let Some(pos) = list.iter().position(|f| f.container_port == cp)
                            {
                                let removed = list.remove(pos);
                                registry.remove(&removed.path_token).await;
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }
        }
    }
}

/// Track port events for a single run command. Registers the reserved proxy
/// token when a port is detected and cleans up on stop/unforward events.
#[allow(clippy::too_many_arguments)]
pub(super) async fn run_command_port_tracker(
    ticket_key: String,
    cmd_index: usize,
    user_id: String,
    reserved_token: String,
    proxy_base: String,
    run_cmds_map: crate::state::RunCommandsMap,
    registry: PathTokenRegistry,
    mut rx: broadcast::Receiver<WorkflowEvent>,
    cancel: CancellationToken,
) {
    loop {
        tokio::select! {
            _ = cancel.cancelled() => break,
            result = rx.recv() => {
                match result {
                    Ok(event) => {
                        if event.ticket_key != ticket_key {
                            continue;
                        }
                        let evt_cmd_index: usize = match event.step_name.as_deref().unwrap_or("").parse() {
                            Ok(i) => i,
                            Err(_) => continue,
                        };
                        if evt_cmd_index != cmd_index {
                            continue;
                        }
                        match event.event_type.as_str() {
                            "run_command_port_forwarded" => {
                                if let Some((cp, hp)) = event.forwarded_port {
                                    registry.register_with_token(
                                        reserved_token.clone(),
                                        SessionRoute {
                                            kind: SessionRouteKind::DynamicPort,
                                            host_port: hp,
                                            ticket_key: ticket_key.clone(),
                                            user_id: user_id.clone(),
                                        },
                                    ).await;
                                    let mut map = run_cmds_map.write().await;
                                    if let Some(cmd) = map.get_mut(&ticket_key)
                                        .and_then(|cmds| cmds.iter_mut().find(|c| c.cmd_index == cmd_index))
                                    {
                                        cmd.forwarded_port = Some(DynamicPortForward {
                                            container_port: cp,
                                            host_port: hp,
                                            proxy_url: proxy_base.clone(),
                                            path_token: reserved_token.clone(),
                                        });
                                    }
                                }
                            }
                            "run_command_port_unforwarded" => {
                                let mut map = run_cmds_map.write().await;
                                if let Some(cmd) = map.get_mut(&ticket_key)
                                    .and_then(|cmds| cmds.iter_mut().find(|c| c.cmd_index == cmd_index))
                                    && let Some((gone_cp, _)) = event.forwarded_port
                                    && cmd.forwarded_port.as_ref().map(|f| f.container_port) == Some(gone_cp)
                                {
                                    if let Some(ref fwd) = cmd.forwarded_port {
                                        registry.remove(&fwd.path_token).await;
                                    }
                                    cmd.forwarded_port = None;
                                }
                            }
                            "run_command_stopped" => {
                                let mut map = run_cmds_map.write().await;
                                if let Some(cmds) = map.get_mut(&ticket_key) {
                                    if let Some(cmd) = cmds.iter().find(|c| c.cmd_index == cmd_index)
                                        && let Some(ref fwd) = cmd.forwarded_port
                                    {
                                        registry.remove(&fwd.path_token).await;
                                    }
                                    cmds.retain(|c| c.cmd_index != cmd_index);
                                    if cmds.is_empty() {
                                        map.remove(&ticket_key);
                                    }
                                }
                                break;
                            }
                            _ => {}
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    use crate::session_registry::PathTokenRegistry;

    /// Create a minimal `WorkflowEvent` for port-forwarding tests.
    fn port_event(
        event_type: &str,
        ticket_key: &str,
        container_port: u16,
        host_port: u16,
    ) -> WorkflowEvent {
        WorkflowEvent {
            event_type: event_type.to_string(),
            workflow_id: String::new(),
            ticket_key: ticket_key.to_string(),
            state: String::new(),
            timestamp: chrono::Utc::now(),
            error: None,
            step_name: None,
            output_line: None,
            stream: None,
            progress_percent: None,
            progress_steps_total: None,
            forwarded_port: Some((container_port, host_port)),
            pr_merged: None,
            user_id: None,
            ..Default::default()
        }
    }

    /// Helper: extract `(container_port, host_port)` pairs from the map.
    fn port_pairs(fwd: &[DynamicPortForward]) -> Vec<(u16, u16)> {
        fwd.iter().map(|f| (f.container_port, f.host_port)).collect()
    }

    /// `track_port_forwards` adds ports on `port_forwarded` events and
    /// registers proxy tokens.
    #[tokio::test]
    async fn track_port_forwards_adds_on_forwarded() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c, None, None));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(port_pairs(ports), vec![(3000, 9100)]);
            assert!(ports[0].proxy_url.starts_with("/s/"));
            assert!(!ports[0].path_token.is_empty());
            // Token should be registered in the registry.
            assert!(registry.lookup(&ports[0].path_token).await.is_some());
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` removes ports on `port_unforwarded` events and
    /// deregisters proxy tokens.
    #[tokio::test]
    async fn track_port_forwards_removes_on_unforwarded() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c, None, None));

        // Forward two ports.
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tx.send(port_event("port_forwarded", "T-1", 5000, 9101)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Capture the token for port 3000 before removal.
        let token_3000 = {
            let fwd = map.read().await;
            fwd.get("T-1").unwrap().iter().find(|f| f.container_port == 3000).unwrap().path_token.clone()
        };

        // Unforward port 3000.
        tx.send(port_event("port_unforwarded", "T-1", 3000, 9100)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(port_pairs(ports), vec![(5000, 9101)]);
            // Token for 3000 should be deregistered.
            assert!(registry.lookup(&token_3000).await.is_none());
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` ignores events for other tickets.
    #[tokio::test]
    async fn track_port_forwards_ignores_other_tickets() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c, None, None));

        tx.send(port_event("port_forwarded", "T-2", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            assert!(fwd.get("T-1").is_none(), "should not add ports for T-1");
            assert!(fwd.get("T-2").is_none(), "should not add ports for T-2");
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` deduplicates by container port.
    #[tokio::test]
    async fn track_port_forwards_deduplicates_by_container_port() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c, None, None));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports.len(), 1, "duplicate container port should not be added twice");
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` handles multiple ports for the same ticket.
    #[tokio::test]
    async fn track_port_forwards_multiple_ports() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let m = map.clone();
        let r = registry.clone();
        let c = cancel.clone();
        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), m, r, rx, c, None, None));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100)).unwrap();
        tx.send(port_event("port_forwarded", "T-1", 5173, 9101)).unwrap();
        tx.send(port_event("port_forwarded", "T-1", 8080, 9102)).unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        {
            let fwd = map.read().await;
            let ports = fwd.get("T-1").expect("should have entry");
            assert_eq!(ports.len(), 3);
            let pairs = port_pairs(ports);
            assert!(pairs.contains(&(3000, 9100)));
            assert!(pairs.contains(&(5173, 9101)));
            assert!(pairs.contains(&(8080, 9102)));
        }

        cancel.cancel();
        let _ = handle.await;
    }

    /// `track_port_forwards` exits when the cancellation token is cancelled.
    #[tokio::test]
    async fn track_port_forwards_exits_on_cancel() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), map, registry, rx, cancel.clone(), None, None));

        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("task should exit within 1 second")
            .expect("task should not panic");
    }

    /// `track_port_forwards` exits when the broadcast channel is closed.
    #[tokio::test]
    async fn track_port_forwards_exits_on_channel_close() {
        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards("T-1".into(), "test-user".into(), map, registry, rx, cancel, None, None));

        drop(tx);
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("task should exit within 1 second")
            .expect("task should not panic");
    }

    /// Plan-07 step 4 slice 7 — when `track_port_forwards` is wired
    /// with `Some(work_item_id)` + `Some(db)`, every detected port
    /// also lands as a `Dynamic` row in `work_item_port_mappings`.
    /// Unforward events DO NOT delete the row — cleanup is bulk via
    /// `close_editor` (slice 6) so we deliberately do not test that
    /// path here.
    #[tokio::test]
    async fn track_port_forwards_shadow_writes_dynamic_port_row() {
        use maestro_core::db::adapter::DbValue;
        use maestro_core::db::work_items;

        let db = crate::test_helpers::temp_db();
        // Seed minimum prerequisites: user + a work_items row whose
        // id is what the tracker will be told to write under.
        let _ = db
            .adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES ('u-1', 'a', 'admin')",
                Vec::<DbValue>::new(),
            )
            .await
            .expect("seed user");
        let _ = db
            .adapter()
            .execute(
                "INSERT INTO work_items (\
                    id, ticket_key, workspace_name, user_id, private, \
                    started_manually, counts_toward_manual_cap, driver_started, \
                    jira_available, state_kind, started_at, created_at, updated_at\
                 ) VALUES (\
                    'wf-pt', 'T-1', 'ws', 'u-1', 0, \
                    0, 0, 0, \
                    0, 'pending', 100, 100, 100\
                 )",
                Vec::<DbValue>::new(),
            )
            .await
            .expect("seed work_items");

        let map: DynamicForwardsMap = Arc::new(RwLock::new(HashMap::new()));
        let registry = PathTokenRegistry::new();
        let (tx, _) = broadcast::channel(16);
        let cancel = CancellationToken::new();
        let rx = tx.subscribe();

        let handle = tokio::spawn(track_port_forwards(
            "T-1".into(),
            "test-user".into(),
            map.clone(),
            registry.clone(),
            rx,
            cancel.clone(),
            Some("wf-pt".into()),
            Some(db.clone()),
        ));

        tx.send(port_event("port_forwarded", "T-1", 3000, 9100))
            .unwrap();
        tx.send(port_event("port_forwarded", "T-1", 5173, 9101))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;

        let rows = work_items::list_port_mappings(db.adapter(), "wf-pt")
            .await
            .expect("list");
        assert_eq!(rows.len(), 2, "one row per detected port");

        // Sort by container_port for stable assertions.
        let mut by_cp: Vec<_> = rows.into_iter().collect();
        by_cp.sort_by_key(|r| r.container_port);
        assert_eq!(by_cp[0].container_port, 3000);
        assert_eq!(by_cp[0].host_port, 9100);
        assert_eq!(by_cp[0].kind, work_items::PortMappingKind::Dynamic);
        assert!(
            by_cp[0].proxy_url.starts_with("/s/"),
            "proxy URL preserved in DB"
        );
        assert!(!by_cp[0].path_token.is_empty());
        assert_eq!(by_cp[1].container_port, 5173);
        assert_eq!(by_cp[1].host_port, 9101);
        assert_eq!(by_cp[1].kind, work_items::PortMappingKind::Dynamic);

        // Unforward MUST NOT delete the row — cleanup is bulk.
        tx.send(port_event("port_unforwarded", "T-1", 3000, 9100))
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(80)).await;
        let rows = work_items::list_port_mappings(db.adapter(), "wf-pt")
            .await
            .expect("list");
        assert_eq!(
            rows.len(),
            2,
            "unforward keeps the row; close_editor bulk-deletes"
        );

        cancel.cancel();
        let _ = handle.await;
    }
}
