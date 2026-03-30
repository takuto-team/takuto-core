use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use tracing::{info, warn};

use crate::state::AppState;

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
) -> impl IntoResponse {
    info!("WebSocket upgrade requested");
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let mut rx = state.engine.subscribe();
    let receiver_count = state.engine.event_tx.receiver_count();
    info!(receivers = receiver_count, "WebSocket client connected, subscribed to events");

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
