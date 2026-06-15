// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for WebSocket per-user event isolation.
//
// The full WS upgrade path requires a real TCP socket. This test focuses on
// the **filter logic** (`should_deliver_event`) and exercises it end-to-end
// against the live `WorkflowEngine` event bus so we have confidence the
// wiring is correct.
//
// A full end-to-end test with `tokio_tungstenite` + `axum::serve` on an
// ephemeral port is left to the Playwright E2E suite.
//
// Verifies:
//   - G/W/T 1.1: Cross-user isolation — when an event carrying alice's
//     `user_id` is published, only alice's filter accepts it.
//   - G/W/T 1.2: Broadcast events (`user_id = None`) reach every viewer.
//   - G/W/T 1.4: Admin does NOT bypass — the role is irrelevant; only
//     `user_id` equality decides delivery.

use std::time::Duration;

use chrono::Utc;
use takuto_core::workflow::engine::WorkflowEvent;
use takuto_web::routes::ws::should_deliver_event;
use takuto_web::test_helpers::test_state_with_db;

/// Build a WorkflowEvent shaped like a typical `workflow_updated` payload.
fn make_event(ticket: &str, user_id: Option<&str>) -> WorkflowEvent {
    WorkflowEvent {
        event_type: "work_item_updated".to_string(),
        workflow_id: "wf-1".to_string(),
        ticket_key: ticket.to_string(),
        state: "Pending".to_string(),
        timestamp: Utc::now(),
        error: None,
        step_name: None,
        output_line: None,
        stream: None,
        progress_percent: None,
        progress_steps_total: None,
        forwarded_port: None,
        pr_merged: None,
        user_id: user_id.map(|s| s.to_string()),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// Pure filter tests (extracted free function per spec fallback B.7)
// ---------------------------------------------------------------------------

#[test]
fn filter_passes_event_for_owning_user() {
    let evt = make_event("PROJ-A1", Some("alice-id"));
    assert!(should_deliver_event(&evt, "alice-id"));
}

#[test]
fn filter_drops_event_targeted_at_other_user() {
    // Bob must not see alice's events.
    let evt = make_event("PROJ-A1", Some("alice-id"));
    assert!(!should_deliver_event(&evt, "bob-id"));
}

#[test]
fn filter_passes_broadcast_event_to_everyone() {
    // `user_id == None` events reach every viewer
    // (e.g. `workflow_definitions_changed`).
    let evt = make_event("", None);
    assert!(should_deliver_event(&evt, "alice-id"));
    assert!(should_deliver_event(&evt, "bob-id"));
    assert!(should_deliver_event(&evt, ""));
}

#[test]
fn filter_does_not_match_admin_role() {
    // Admin does NOT bypass ownership — only `user_id` matters. (The filter
    // does not consult the role at all; the test asserts the contract: a
    // user named "admin" still cannot read alice's events.)
    let evt = make_event("PROJ-A1", Some("alice-id"));
    assert!(!should_deliver_event(&evt, "admin-id"));
}

// ---------------------------------------------------------------------------
// Engine-level wiring test: publish through the broadcast bus and confirm the
// filter still produces the expected output. This catches regressions where
// the WS handler subscribes to the wrong channel or the WorkflowEvent loses
// its `user_id` on the wire.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn engine_broadcast_round_trips_user_id_through_filter() {
    let state = test_state_with_db();

    // Two independent subscribers — modelling alice's and bob's WS loops.
    let mut alice_rx = state.engine().engine.subscribe();
    let mut bob_rx = state.engine().engine.subscribe();

    // Emit an event scoped to alice. We bypass start_workflow and just push
    // a raw event onto the bus to keep the test focused on filter semantics.
    let event_tx = state.engine().engine.event_sender();
    let alice_event = make_event("PROJ-A1", Some("alice-id"));
    event_tx
        .send(alice_event.clone())
        .expect("broadcast should accept events with at least one subscriber");

    // Alice receives it; her filter passes it.
    let received = tokio::time::timeout(Duration::from_millis(200), alice_rx.recv())
        .await
        .expect("alice should receive event within 200 ms")
        .expect("alice's receiver should not be closed");
    assert_eq!(received.ticket_key, "PROJ-A1");
    assert_eq!(received.user_id.as_deref(), Some("alice-id"));
    assert!(should_deliver_event(&received, "alice-id"));

    // Bob also receives the broadcast (broadcast channels deliver to all
    // subscribers), but his filter drops it.
    let received = tokio::time::timeout(Duration::from_millis(200), bob_rx.recv())
        .await
        .expect("bob's broadcast receiver should still get the raw event")
        .expect("bob's receiver should not be closed");
    assert!(
        !should_deliver_event(&received, "bob-id"),
        "bob's filter must drop alice's event"
    );

    // Now emit a broadcast event (`user_id = None`) and confirm both filters
    // pass it.
    let broadcast_event = make_event("", None);
    event_tx
        .send(broadcast_event)
        .expect("broadcast should be accepted");

    let alice_msg = tokio::time::timeout(Duration::from_millis(200), alice_rx.recv())
        .await
        .expect("alice should receive broadcast")
        .expect("alice's receiver should be open");
    let bob_msg = tokio::time::timeout(Duration::from_millis(200), bob_rx.recv())
        .await
        .expect("bob should receive broadcast")
        .expect("bob's receiver should be open");

    assert!(should_deliver_event(&alice_msg, "alice-id"));
    assert!(should_deliver_event(&bob_msg, "bob-id"));
}

#[tokio::test]
async fn engine_event_carries_user_id_serialised_to_wire() {
    // Regression guard: serde_json::to_string must include the user_id field
    // when present, and elide it when None (it is `skip_serializing_if`).
    let scoped = make_event("PROJ-A1", Some("alice-id"));
    let json = serde_json::to_string(&scoped).expect("serializes");
    assert!(
        json.contains("\"user_id\":\"alice-id\""),
        "user_id must appear in scoped event JSON: {json}"
    );

    let broadcast = make_event("", None);
    let json = serde_json::to_string(&broadcast).expect("serializes");
    assert!(
        !json.contains("user_id"),
        "user_id must be skipped on broadcast events to avoid noise: {json}"
    );
}
