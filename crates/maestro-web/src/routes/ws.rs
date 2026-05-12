// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use tracing::{info, warn};

use crate::auth::{session_cookie_from_headers, validate_db_session};
use crate::state::AppState;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let Some(ref db) = state.db else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    if let Some(raw_cookie) = session_cookie_from_headers(&headers)
        && raw_cookie.starts_with("db-")
    {
        let db = db.clone();
        let cookie = raw_cookie.to_string();
        let valid = tokio::task::spawn_blocking(move || {
            let conn = db.conn().blocking_lock();
            validate_db_session(&conn, &cookie).is_some()
        })
        .await
        .unwrap_or(false);

        if !valid {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        info!("WebSocket upgrade requested");
        return ws.on_upgrade(move |socket| handle_socket(socket, state));
    }

    StatusCode::UNAUTHORIZED.into_response()
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

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.engine.subscribe();
    let receiver_count = state.engine.event_subscriber_count();
    info!(
        receivers = receiver_count,
        "WebSocket client connected, subscribed to events"
    );

    loop {
        tokio::select! {
            event = rx.recv() => {
                match event {
                    Ok(evt) => {
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
                        info!("WebSocket client disconnected");
                        break;
                    }
                }
            }
        }
    }
}
