use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use rust_embed::Embed;
use tower_http::cors::{AllowOrigin, CorsLayer};

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

    let cors_layer = {
        let config = state.config.blocking_read();
        build_cors_layer(&config.web)
    };

    Router::new()
        .nest("/api", api)
        .route("/ws", get(routes::ws::ws_handler))
        .fallback(static_handler)
        .layer(cors_layer)
        .with_state(state)
}

/// Build the CORS layer from the web config. Uses `cors_origins` (or auto-computes
/// from host/port) and restricts methods and headers to the set used by the dashboard.
pub fn build_cors_layer(web_config: &maestro_core::config::WebConfig) -> CorsLayer {
    let origins = web_config.resolved_cors_origins();
    tracing::info!(
        cors_origins = ?origins,
        auto_computed = web_config.cors_origins.is_empty(),
        "CORS origin allowlist configured"
    );
    if web_config.cors_origins.is_empty() {
        tracing::warn!(
            "No explicit [web] cors_origins configured; using auto-computed origins. \
             If behind a reverse proxy or TLS terminator, set cors_origins explicitly."
        );
    }

    let header_values: Vec<HeaderValue> = origins
        .iter()
        .filter_map(|o| match HeaderValue::from_str(o) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!(origin = %o, error = %e, "Skipping CORS origin: not a valid HTTP header value");
                None
            }
        })
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(header_values))
        .allow_methods([Method::GET, Method::POST, Method::PUT, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE])
        .allow_credentials(true)
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use maestro_core::config::WebConfig;
    use tower::ServiceExt;

    /// Build a minimal router with just the CORS layer and a health endpoint for testing.
    fn test_router(web_config: &WebConfig) -> Router {
        let cors = build_cors_layer(web_config);
        Router::new().route("/api/health", get(health)).layer(cors)
    }

    #[tokio::test]
    async fn cors_allows_configured_origin() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::get("/api/health")
                    .header("Origin", "http://localhost:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let acao = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("missing ACAO header");
        // Must be the exact origin, NOT the wildcard "*"
        assert_eq!(acao, "http://localhost:3000");
        let acac = resp
            .headers()
            .get("access-control-allow-credentials")
            .expect("missing ACAC header");
        assert_eq!(acac, "true");
    }

    #[tokio::test]
    async fn cors_rejects_unconfigured_origin() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::get("/api/health")
                    .header("Origin", "http://evil.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // The request itself succeeds (CORS doesn't block server-side),
        // but the Access-Control-Allow-Origin header is absent.
        assert_eq!(resp.status(), StatusCode::OK);
        assert!(
            resp.headers().get("access-control-allow-origin").is_none(),
            "ACAO header should be absent for unlisted origin"
        );
    }

    #[tokio::test]
    async fn cors_preflight_returns_allowed_methods() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/health")
                    .header("Origin", "http://localhost:3000")
                    .header("Access-Control-Request-Method", "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let methods = resp
            .headers()
            .get("access-control-allow-methods")
            .expect("missing Allow-Methods header")
            .to_str()
            .unwrap()
            .to_string();
        for m in &["GET", "POST", "PUT", "DELETE"] {
            assert!(
                methods.contains(m),
                "Allow-Methods should include {m}, got: {methods}"
            );
        }
        // PATCH should not be allowed
        assert!(
            !methods.contains("PATCH"),
            "Allow-Methods should NOT include PATCH, got: {methods}"
        );
    }

    #[tokio::test]
    async fn cors_preflight_returns_allowed_headers() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/health")
                    .header("Origin", "http://localhost:3000")
                    .header("Access-Control-Request-Method", "POST")
                    .header("Access-Control-Request-Headers", "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let headers = resp
            .headers()
            .get("access-control-allow-headers")
            .expect("missing Allow-Headers header")
            .to_str()
            .unwrap()
            .to_lowercase();
        assert!(
            headers.contains("content-type"),
            "Allow-Headers should include content-type, got: {headers}"
        );
    }

    #[tokio::test]
    async fn cors_auto_computed_origin_when_empty() {
        // Default: host="0.0.0.0", port=8080, cors_origins=[] → http://localhost:8080
        let web = WebConfig::default();
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::get("/api/health")
                    .header("Origin", "http://localhost:8080")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::OK);
        let acao = resp
            .headers()
            .get("access-control-allow-origin")
            .expect("missing ACAO header for auto-computed origin");
        assert_eq!(acao, "http://localhost:8080");
    }

    #[tokio::test]
    async fn cors_multiple_origins_both_allowed() {
        let web = WebConfig {
            cors_origins: vec![
                "http://localhost:3000".into(),
                "https://prod.example.com".into(),
            ],
            ..Default::default()
        };

        // First origin
        let app1 = test_router(&web);
        let resp1 = app1
            .oneshot(
                Request::get("/api/health")
                    .header("Origin", "http://localhost:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp1.headers().get("access-control-allow-origin").unwrap(),
            "http://localhost:3000"
        );

        // Second origin
        let app2 = test_router(&web);
        let resp2 = app2
            .oneshot(
                Request::get("/api/health")
                    .header("Origin", "https://prod.example.com")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            resp2.headers().get("access-control-allow-origin").unwrap(),
            "https://prod.example.com"
        );
    }

    #[tokio::test]
    async fn cors_credentials_header_present() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::get("/api/health")
                    .header("Origin", "http://localhost:3000")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let acac = resp
            .headers()
            .get("access-control-allow-credentials")
            .expect("missing ACAC header");
        assert_eq!(acac, "true");
    }

    #[tokio::test]
    async fn cors_preflight_rejects_disallowed_method() {
        let web = WebConfig {
            cors_origins: vec!["http://localhost:3000".into()],
            ..Default::default()
        };
        let app = test_router(&web);

        let resp = app
            .oneshot(
                Request::builder()
                    .method(Method::OPTIONS)
                    .uri("/api/health")
                    .header("Origin", "http://localhost:3000")
                    .header("Access-Control-Request-Method", "PATCH")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // PATCH is not in the allowed methods list; the preflight should either
        // return 403 or omit the allow-methods header that includes PATCH.
        let methods = resp
            .headers()
            .get("access-control-allow-methods")
            .map(|v| v.to_str().unwrap_or("").to_string())
            .unwrap_or_default();
        assert!(
            !methods.contains("PATCH"),
            "PATCH should not be in allowed methods: {methods}"
        );
    }
}
