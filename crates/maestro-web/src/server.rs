use axum::Router;
use axum::body::Body;
use axum::http::{StatusCode, Uri, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use rust_embed::Embed;
use tower_http::cors::CorsLayer;

use crate::auth::dashboard_auth_middleware;
use crate::routes;
use crate::state::AppState;

#[derive(Embed)]
#[folder = "src/assets/"]
struct Assets;

pub fn build_router(state: AppState) -> Router {
    let api_public = Router::new()
        .route("/health", get(health))
        .route("/auth/status", get(routes::auth::auth_status))
        .route("/auth/login", post(routes::auth::login))
        .route("/auth/logout", post(routes::auth::logout));

    let api_protected = Router::new()
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
        .route(
            "/workflows/{id}/retry",
            post(routes::workflows::retry_workflow),
        )
        .route(
            "/workflows/{id}/resume-from-error",
            post(routes::workflows::resume_from_error),
        )
        .route(
            "/workflows/{id}/address-pr-comments",
            post(routes::workflows::address_pr_comments),
        )
        .route(
            "/workflows/{id}/merge-base-branch",
            post(routes::workflows::merge_base_branch),
        )
        .route(
            "/workflows/{id}/mark-done",
            post(routes::workflows::mark_work_done),
        )
        .route(
            "/workflows/{id}/delete",
            post(routes::workflows::delete_workflow),
        )
        .route(
            "/workflows/{id}/open-editor",
            post(routes::workflows::open_editor),
        )
        .route(
            "/workflows/{id}/close-editor",
            post(routes::workflows::close_editor),
        )
        .route(
            "/workflows/{id}/open-terminal",
            post(routes::workflows::open_terminal),
        )
        .route(
            "/workflows/{id}/close-terminal",
            post(routes::workflows::close_terminal),
        )
        .route(
            "/workflows/start-manual",
            post(routes::workflows::start_manual_workflow),
        )
        .route(
            "/jira/todo-tickets-manual",
            get(routes::jira::list_todo_tickets_manual),
        )
        .route(
            "/jira/tickets/{key}/preview",
            get(routes::jira::get_ticket_preview),
        )
        .route("/github/issues", get(routes::github::list_github_issues))
        .route(
            "/tickets/{key}/improve",
            post(routes::tickets::improve_ticket),
        )
        .route(
            "/tickets/{key}/update-description",
            post(routes::tickets::update_ticket_description),
        )
        .route("/config", get(routes::config::get_config))
        .route("/config", put(routes::config::update_config))
        .route("/polling", get(routes::polling::get_polling_status))
        .route("/polling/pause", post(routes::polling::pause_polling))
        .route("/polling/resume", post(routes::polling::resume_polling))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            dashboard_auth_middleware,
        ));

    let api = Router::new().merge(api_public).merge(api_protected);

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
            let mut res = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime);
            // Avoid stale dashboard JS/HTML/CSS after upgrades (embedded assets otherwise get heuristic browser cache).
            if asset_path.ends_with(".html")
                || asset_path.ends_with(".js")
                || asset_path.ends_with(".css")
            {
                res = res.header(header::CACHE_CONTROL, "no-store, max-age=0");
            }
            res.body(Body::from(content.data.to_vec())).unwrap()
        }
        None => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::from("Not Found"))
            .unwrap(),
    }
}
