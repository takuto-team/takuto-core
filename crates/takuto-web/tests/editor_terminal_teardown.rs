// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Editor / terminal session-teardown isolation.
//
// The editor (openvscode-server) and the web terminal (ttyd) share ONE
// per-item workspace container but are independent sessions, each with its own
// proxy path token. Stopping one must never tear down the other: "Stop editor"
// stops the editor only, "Stop terminal" stops the terminal only.
//
// `close_editor` / `close_terminal` are thin wrappers that call these in-memory
// teardown methods and then kill the relevant process in the container. The
// process kill needs Docker, but the routing/state teardown — the part that
// regressed (an editor stop was dropping the terminal's token) — is pure
// in-memory bookkeeping and is exercised directly here.

use takuto_web::session_registry::{SessionRoute, SessionRouteKind};
use takuto_web::state::{DynamicPortForward, EditorState};
use takuto_web::test_helpers::test_state_with_db;
use tokio_util::sync::CancellationToken;

const TICKET: &str = "GH-1";
const USER: &str = "user-1";

/// Register one route of each kind for the ticket and seed the scanner,
/// dynamic-forwards, and terminal-port maps — the full state an item with both
/// an open editor and an open terminal (plus a forwarded dev server) would have.
/// Returns the three path tokens `(editor, terminal, dynamic_port)`.
async fn seed_both_open(editor: &EditorState) -> (String, String, String) {
    let reg = &editor.path_token_registry;
    let editor_tok = reg
        .register(SessionRoute {
            kind: SessionRouteKind::Editor,
            host_port: 9100,
            ticket_key: TICKET.to_string(),
            user_id: USER.to_string(),
        })
        .await
        .expect("register editor route");
    let terminal_tok = reg
        .register(SessionRoute {
            kind: SessionRouteKind::Terminal,
            host_port: 9300,
            ticket_key: TICKET.to_string(),
            user_id: USER.to_string(),
        })
        .await
        .expect("register terminal route");
    let dynamic_tok = reg
        .register(SessionRoute {
            kind: SessionRouteKind::DynamicPort,
            host_port: 9110,
            ticket_key: TICKET.to_string(),
            user_id: USER.to_string(),
        })
        .await
        .expect("register dynamic-port route");

    editor
        .editor_scanners
        .write()
        .await
        .insert(TICKET.to_string(), CancellationToken::new());
    editor.dynamic_forwards.write().await.insert(
        TICKET.to_string(),
        vec![DynamicPortForward {
            container_port: 5173,
            host_port: 9110,
            proxy_url: format!("/s/{dynamic_tok}/"),
            path_token: dynamic_tok.clone(),
        }],
    );
    editor
        .terminal_ports
        .write()
        .await
        .insert(TICKET.to_string(), (9300, "ttyd-token".to_string()));

    (editor_tok, terminal_tok, dynamic_tok)
}

#[tokio::test]
async fn stop_editor_leaves_the_terminal_intact() {
    let state = test_state_with_db();
    let editor = state.editor();
    let (editor_tok, terminal_tok, dynamic_tok) = seed_both_open(editor).await;
    let scanner = editor
        .editor_scanners
        .read()
        .await
        .get(TICKET)
        .cloned()
        .unwrap();

    editor.teardown_editor_state(TICKET).await;

    // Terminal is untouched — its token still resolves and its port remains.
    assert!(
        editor
            .path_token_registry
            .lookup(&terminal_tok)
            .await
            .is_some(),
        "stopping the editor must not drop the terminal's path token"
    );
    assert!(
        editor.terminal_ports.read().await.contains_key(TICKET),
        "stopping the editor must not drop the terminal's port"
    );

    // Editor routing + port-forwarding state is gone, and the scanner cancelled.
    assert!(
        editor
            .path_token_registry
            .lookup(&editor_tok)
            .await
            .is_none()
    );
    assert!(
        editor
            .path_token_registry
            .lookup(&dynamic_tok)
            .await
            .is_none()
    );
    assert!(!editor.editor_scanners.read().await.contains_key(TICKET));
    assert!(editor.dynamic_forwards.read().await.get(TICKET).is_none());
    assert!(scanner.is_cancelled(), "the port scanner must be cancelled");
}

#[tokio::test]
async fn stop_terminal_leaves_the_editor_intact() {
    let state = test_state_with_db();
    let editor = state.editor();
    let (editor_tok, terminal_tok, dynamic_tok) = seed_both_open(editor).await;

    editor.teardown_terminal_state(TICKET).await;

    // Terminal routing is gone.
    assert!(
        editor
            .path_token_registry
            .lookup(&terminal_tok)
            .await
            .is_none()
    );
    assert!(!editor.terminal_ports.read().await.contains_key(TICKET));

    // Editor + its forwarded ports are untouched.
    assert!(
        editor
            .path_token_registry
            .lookup(&editor_tok)
            .await
            .is_some(),
        "stopping the terminal must not drop the editor's path token"
    );
    assert!(
        editor
            .path_token_registry
            .lookup(&dynamic_tok)
            .await
            .is_some(),
        "stopping the terminal must not drop the editor's forwarded ports"
    );
    assert!(editor.editor_scanners.read().await.contains_key(TICKET));
    assert!(editor.dynamic_forwards.read().await.get(TICKET).is_some());
}

#[tokio::test]
async fn teardown_is_scoped_to_the_ticket() {
    // A second item with its own open editor must be unaffected when the first
    // item's editor is stopped (no cross-ticket teardown).
    let state = test_state_with_db();
    let editor = state.editor();
    let (_e1, _t1, _d1) = seed_both_open(editor).await;
    let other_tok = editor
        .path_token_registry
        .register(SessionRoute {
            kind: SessionRouteKind::Editor,
            host_port: 9101,
            ticket_key: "GH-2".to_string(),
            user_id: USER.to_string(),
        })
        .await
        .unwrap();

    editor.teardown_editor_state(TICKET).await;

    assert!(
        editor
            .path_token_registry
            .lookup(&other_tok)
            .await
            .is_some(),
        "tearing down GH-1's editor must not touch GH-2's editor"
    );
}

#[tokio::test]
async fn teardown_on_unknown_ticket_is_a_noop() {
    let state = test_state_with_db();
    let editor = state.editor();
    // No panic, no error when nothing is registered.
    editor.teardown_editor_state("does-not-exist").await;
    editor.teardown_terminal_state("does-not-exist").await;
}
