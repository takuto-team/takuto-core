// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use tracing::{info, warn};

use crate::auth::session_authorized;
use crate::state::AppState;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let web = {
        let cfg = state.config.read().await;
        cfg.web.clone()
    };
    if web.dashboard_auth_enabled() && !session_authorized(&headers, &web) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    info!("WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::workflow::engine::WorkflowEngine;

    use crate::server::build_router;
    use crate::state::AppState;

    fn test_state() -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> = Arc::new(
            DryRunActions::new(std::env::temp_dir(), "origin".to_string(), None),
        );
        let jira_available = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(WorkflowEngine::new(
            config.clone(),
            actions,
            1,
            jira_available.clone(),
            TicketingSystem::None,
            std::env::temp_dir(),
        ));
        AppState {
            engine,
            config,
            polling_paused: Arc::new(AtomicBool::new(false)),
            jira_available,
            ticketing_system: TicketingSystem::None,
            editor_scanners: Arc::new(RwLock::new(HashMap::new())),
            dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
            terminal_ports: Arc::new(RwLock::new(HashMap::new())),
            run_commands: Arc::new(RwLock::new(HashMap::new())),
            preflight_error: None,
            config_path: std::env::temp_dir().join("config.toml"),
            config_writer: None,
            clone_in_progress: Arc::new(AtomicBool::new(false)),
            path_token_registry: crate::session_registry::PathTokenRegistry::new(),
        }
    }

    /// WebSocket upgrade requires a real TCP connection. With tower's `oneshot()`,
    /// the handler is reached but returns 426 (Upgrade Required) because the upgrade
    /// cannot complete without a socket. We verify the route exists and the handler
    /// activates (not 404, not 401).
    #[tokio::test]
    async fn ws_route_is_reachable() {
        let state = test_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/ws")
                    .header("Host", "localhost")
                    .header("Connection", "Upgrade")
                    .header("Upgrade", "websocket")
                    .header("Sec-WebSocket-Version", "13")
                    .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
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
