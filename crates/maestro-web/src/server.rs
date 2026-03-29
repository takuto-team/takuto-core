use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use rust_embed::Embed;
use tower_http::cors::CorsLayer;

use crate::routes;
use crate::state::AppState;

#[derive(Embed)]
#[folder = "src/assets/"]
struct Assets;

pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        .route("/workflows", get(routes::workflows::list_workflows))
        .route("/workflows/{id}", get(routes::workflows::get_workflow))
        .route(
            "/workflows/{id}/pause",
            post(routes::workflows::pause_workflow),
        )
        .route(
            "/workflows/{id}/resume",
            post(routes::workflows::resume_workflow),
        )
        .route(
            "/workflows/{id}/stop",
            post(routes::workflows::stop_workflow),
        )
        .route("/config", get(routes::config::get_config))
        .route("/config", put(routes::config::update_config))
        .route("/health", get(health));

    Router::new()
        .nest("/api", api)
        .route("/ws", get(routes::ws::ws_handler))
        .fallback(static_handler)
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Map root to index.html
    let path = if path.is_empty() { "index.html" } else { path };

    // Serve from assets/ prefix or directly
    let asset_path = if let Some(stripped) = path.strip_prefix("assets/") {
        stripped
    } else {
        path
    };

    match Assets::get(asset_path) {
        Some(content) => {
            let mime = mime_guess::from_path(asset_path)
                .first_or_octet_stream()
                .to_string();
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime)
                .body(Body::from(content.data.to_vec()))
                .unwrap()
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not Found"))
            .unwrap(),
    }
}
