// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use maestro_core::workflow::engine::WorkflowEvent;
use tracing::{info, warn};

use crate::auth::{session_cookie_from_headers, validate_db_session};
use crate::state::{AuthState, EngineState};

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(auth): State<AuthState>,
    State(engine): State<EngineState>,
) -> impl IntoResponse {
    let Some(ref db) = auth.db else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if let Some(raw_cookie) = session_cookie_from_headers(&headers)
        && raw_cookie.starts_with("db-")
    {
        // Plan-11 step 3 cluster Sessions: sessions on the adapter.
        // Validate the session AND capture the owning user_id so the WS event
        // loop can filter per-user (AC-1: cross-user event isolation).
        let viewer_user_id = validate_db_session(db.adapter(), raw_cookie).await;

        let Some(viewer_user_id) = viewer_user_id else {
            return StatusCode::UNAUTHORIZED.into_response();
        };
        info!(user = %viewer_user_id, "WebSocket upgrade requested");
        return ws.on_upgrade(move |socket| handle_socket(socket, engine, viewer_user_id));
    }

    StatusCode::UNAUTHORIZED.into_response()
}

/// Per-socket filter: pass through broadcast events (`user_id == None`); drop
/// events scoped to a different user.
///
/// Exposed for the integration test in `tests/ws_isolation.rs` so the filter
/// can be exercised against the live engine event bus without standing up a
/// real WebSocket upgrade. See `tmp/plan-01-acceptance.md` AC-1 for the
/// behavioural contract.
pub fn should_deliver_event(evt: &WorkflowEvent, viewer_user_id: &str) -> bool {
    match evt.user_id.as_deref() {
        None => true,
        Some(owner) => owner == viewer_user_id,
    }
}

async fn handle_socket(mut socket: WebSocket, engine: EngineState, viewer_user_id: String) {
    let mut rx = engine.engine.subscribe();
    let receiver_count = engine.engine.event_subscriber_count();
    info!(
        receivers = receiver_count,
        user = %viewer_user_id,
        "WebSocket client connected, subscribed to events"
    );

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(evt) => {
                        // AC-1: per-user isolation — drop events that target a
                        // different user. `user_id == None` is a broadcast and
                        // is delivered to every subscriber.
                        if !should_deliver_event(&evt, &viewer_user_id) {
                            continue;
                        }
                        let event_type = evt.event_type.clone();
                        let ticket = evt.ticket_key.clone();
                        match serde_json::to_string(&evt) {
                            Ok(json) => {
                                info!(event_type = %event_type, ticket = %ticket, json_len = json.len(), "Sending WS event");
                                if socket.send(Message::Text(json.into())).await.is_err() {
                                    warn!("WebSocket send failed, closing");
                                    break;
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "Failed to serialize event");
                            }
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(skipped = n, "WebSocket receiver lagged, missed events");
                        continue;
                    }
                    Err(e) => {
                        warn!(error = %e, "Broadcast channel error, closing WebSocket");
                        break;
                    }
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(_)) => {} // ignore client messages
                    _ => {
                        info!(user = %viewer_user_id, "WebSocket client disconnected");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::test_helpers::{register_and_login, test_state_with_db};

    /// WebSocket upgrade requires a real TCP connection. With tower's `oneshot()`,
    /// the handler is reached but returns 426 (Upgrade Required) because the upgrade
    /// cannot complete without a socket. We verify the route exists and the handler
    /// activates (not 404, not 401).
    #[tokio::test]
    async fn ws_route_is_reachable() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/ws")
                    .header("Host", "localhost")
                    .header("Connection", "Upgrade")
                    .header("Upgrade", "websocket")
                    .header("Sec-WebSocket-Version", "13")
                    .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // 426 confirms the WebSocket handler was reached; the upgrade can't complete
        // through oneshot (no real TCP socket). The key thing is it's not 404 or 500.
        assert!(
            resp.status() == StatusCode::SWITCHING_PROTOCOLS
                || resp.status() == StatusCode::from_u16(426).unwrap(),
            "expected 101 or 426, got: {}",
            resp.status()
        );
    }
}
