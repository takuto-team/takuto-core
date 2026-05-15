// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderValue, Method, StatusCode, Uri, header};
use axum::middleware;
use axum::response::Response;
use axum::routing::{any, get, post, put};
use rust_embed::Embed;
use tower_http::cors::{AllowOrigin, CorsLayer};

use crate::auth::dashboard_auth_middleware;
use crate::middleware::csrf::csrf_middleware;
use crate::middleware::security_headers::security_headers_middleware;
use crate::routes;
use crate::state::AppState;

use tower_governor::GovernorLayer;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::key_extractor::KeyExtractor;

/// Key extractor used by the per-IP login/recover rate limiter.
///
/// Prefers `X-Forwarded-For` / `X-Real-IP` (Maestro normally runs behind a
/// reverse proxy or terminator), falls back to the connection peer address
/// when present, and finally to a static sentinel string when neither is
/// available (e.g. during in-process integration tests where `oneshot` does
/// not populate `ConnectInfo`). Production deployments always have one of
/// the first two; the sentinel only fires in test mode.
#[derive(Clone, Copy, Debug, Default)]
pub struct LoginKeyExtractor;

impl KeyExtractor for LoginKeyExtractor {
    type Key = String;

    fn extract<T>(&self, req: &axum::http::Request<T>) -> Result<Self::Key, tower_governor::GovernorError> {
        // 1. X-Forwarded-For: first comma-separated entry.
        if let Some(v) = req.headers().get("x-forwarded-for")
            && let Ok(s) = v.to_str()
            && let Some(first) = s.split(',').next()
        {
            let ip = first.trim();
            if !ip.is_empty() {
                return Ok(ip.to_string());
            }
        }
        // 2. X-Real-IP.
        if let Some(v) = req.headers().get("x-real-ip")
            && let Ok(s) = v.to_str()
            && !s.trim().is_empty()
        {
            return Ok(s.trim().to_string());
        }
        // 3. Peer addr via Axum's ConnectInfo, if it was wired in.
        if let Some(ci) =
            req.extensions().get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
        {
            return Ok(ci.0.ip().to_string());
        }
        // 4. Static sentinel — keeps in-process tests deterministic. Production
        //    always reaches one of the prior cases because the listener
        //    captures the peer address.
        Ok("anon".to_string())
    }
}

#[derive(Embed)]
#[folder = "../../ui/dist/"]
struct Assets;

pub fn build_router(state: AppState) -> Router {
    // Plan-02 AC-3 Layer A: per-IP rate limit on /auth/login + /auth/recover.
    //
    // 10 requests per minute per source IP, burst 10. Returns 429 + a
    // `Retry-After` header on overflow. We deliberately scope this to the two
    // brute-force-prone endpoints — `auth/status`, `auth/logout`, and the
    // protected API stay unthrottled to keep idle dashboard polling and
    // long-running editor sessions out of the bucket.
    //
    // `SmartIpKeyExtractor` consults `Forwarded`, `X-Forwarded-For`, and falls
    // back to the connection peer address, so reverse-proxy deployments don't
    // funnel every request into one bucket.
    let login_governor_conf = std::sync::Arc::new(
        GovernorConfigBuilder::default()
            // 1 permit every 6 seconds → 10 / minute steady state.
            .period(std::time::Duration::from_secs(6))
            // Allow short bursts up to 10 — matches the per-minute steady rate
            // so a fresh attacker burns through 10 attempts before the throttle
            // engages, and the 11th lands as 429 (G/W/T 3.1).
            .burst_size(10)
            .key_extractor(LoginKeyExtractor)
            .finish()
            .expect("static governor config"),
    );
    let login_rate_limited = Router::new()
        .route("/auth/login", post(routes::auth::login))
        .route("/auth/recover", post(routes::auth::recover))
        .layer(GovernorLayer::new(login_governor_conf));

    let api_public = Router::new()
        .route("/health", get(health))
        .route("/version", get(routes::config::get_version))
        .route("/auth/status", get(routes::auth::auth_status))
        .route("/auth/logout", post(routes::auth::logout))
        .route("/auth/register", post(routes::auth::register))
        .merge(login_rate_limited)
        // CSRF: reject cross-origin mutating requests before they hit the
        // login/register/recover handlers. Safe methods short-circuit inside
        // the middleware, so `/health`, `/version`, `/auth/status` stay open.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ));

    let api_protected = Router::new()
        .route("/auth/me", get(routes::auth::me))
        .route(
            "/auth/change-password",
            post(routes::auth::change_password),
        )
        .route(
            "/auth/recovery-codes",
            post(routes::auth::regenerate_recovery_codes),
        )
        // User management routes (admin only).
        .route("/users/export", get(routes::admin::export_users))
        .route("/users/import", post(routes::admin::import_users))
        .route(
            "/users",
            get(routes::admin::list_users).post(routes::admin::create_user),
        )
        .route(
            "/users/{id}",
            get(routes::admin::get_user)
                .patch(routes::admin::update_user)
                .delete(routes::admin::delete_user),
        )
        .route("/users/{id}/suspend", post(routes::admin::suspend_user))
        .route("/users/{id}/unsuspend", post(routes::admin::unsuspend_user))
        .route("/users/{id}/unlock", post(routes::admin::unlock_user))
        .route("/workflows", get(routes::workflows::list_workflows))
        .route("/workflows/counts", get(routes::workflows::workflow_counts))
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
            "/workflows/{id}/run-commands",
            get(routes::workflows::list_run_commands),
        )
        .route(
            "/workflows/{id}/run-commands/{index}/start",
            post(routes::workflows::start_run_command),
        )
        .route(
            "/workflows/{id}/run-commands/{index}/stop",
            post(routes::workflows::stop_run_command),
        )
        .route(
            "/workflows/{id}/report",
            get(routes::workflows::get_workflow_report),
        )
        .route(
            "/workflow-definitions",
            get(routes::workflows::list_workflow_definitions),
        )
        .route(
            "/workflows/{id}/run-workflow/{def}",
            post(routes::workflows::run_workflow_def),
        )
        .route(
            "/workflows/{id}/retry-workflow/{def}",
            post(routes::workflows::retry_workflow_def),
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
        .route("/github/repos", get(routes::repos::list_github_repos))
        .route("/repos/clone", post(routes::repos::clone_repo))
        .route("/workspaces", get(routes::repos::list_workspaces))
        .route("/workspaces/switch", post(routes::repos::switch_workspace))
        .route(
            "/tickets/{key}/improve",
            post(routes::tickets::improve_ticket),
        )
        .route(
            "/tickets/{key}/prompt",
            post(routes::tickets::prompt_ticket),
        )
        .route(
            "/tickets/{key}/update-description",
            post(routes::tickets::update_ticket_description),
        )
        .route("/config", get(routes::config::get_config))
        .route("/config", put(routes::config::update_config))
        .route("/config/reload", post(routes::config::reload_config))
        .route("/polling", get(routes::polling::get_polling_status))
        .route("/polling/pause", post(routes::polling::pause_polling))
        .route("/polling/resume", post(routes::polling::resume_polling))
        // Plan-09 Step 5: per-user-per-workspace init + run commands. No
        // admin gate — every authenticated user manages their own rows
        // and only their own (the URL never carries a `user_id`). The
        // `_workspaces` route must be registered BEFORE the `{workspace}`
        // parameter route so it doesn't get captured as a workspace name.
        .route(
            "/worktree-commands/_workspaces",
            get(routes::worktree_commands::list_workspaces_with_has_commands),
        )
        .route(
            "/worktree-commands",
            get(routes::worktree_commands::list_my_rows),
        )
        .route(
            "/worktree-commands/{workspace}",
            get(routes::worktree_commands::get_my_row)
                .put(routes::worktree_commands::put_my_row)
                .delete(routes::worktree_commands::delete_my_row),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            dashboard_auth_middleware,
        ))
        // CSRF runs OUTSIDE the auth middleware so a planted cross-origin POST
        // is rejected before any DB lookup. Tower layers wrap inside-out, so
        // the LAST `.layer()` call here is the outermost — runs first on
        // requests, last on responses.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            csrf_middleware,
        ));

    let api = Router::new().merge(api_public).merge(api_protected);

    // Read the config outside of an async context to avoid panicking on
    // tokio::sync::RwLock::blocking_read() inside the runtime.
    // Safety: build_router is called once at startup; the try_read() will
    // succeed because no writer is active during router construction.
    let cors_layer = {
        let config = state
            .config
            .try_read()
            .expect("config lock should be available during router construction");
        build_cors_layer(&config.web)
    };

    Router::new()
        .nest("/api", api)
        .route("/ws", get(routes::ws::ws_handler))
        // GH-45: shared-port reverse proxy for editor and terminal sessions.
        // `any` so all HTTP methods AND WebSocket upgrades dispatch to the
        // same handler — `proxy_session` decides HTTP vs WS internally based
        // on the `Upgrade` / `Connection` headers. The `{*rest}` greedy
        // wildcard also matches bare `/s/{token}` (single segment) and
        // `/s/{token}/...` (deeper paths); `proxy_session::parse_session_path`
        // distinguishes the two cases and emits a 308 redirect for the
        // trailing-slash-missing form so relative asset URLs from the backend
        // resolve correctly.
        .route("/s/{*rest}", any(routes::sessions::proxy_session))
        .fallback(routes::sessions::proxy_or_static_fallback)
        .layer(cors_layer)
        // Security headers run as the OUTERMOST layer so every response —
        // including 401 from auth, 403 from CSRF, 404 from the static
        // fallback, and `/s/*` proxy responses — carries the defence-in-depth
        // headers. The middleware branches on `request.uri().path()` to apply
        // a looser policy to `/s/*` proxy responses.
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_headers_middleware,
        ))
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
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
        ])
        .allow_headers([header::CONTENT_TYPE])
        .allow_credentials(true)
}

async fn health() -> &'static str {
    "ok"
}

/// Serve embedded static files (dashboard UI assets). Public so the
/// session proxy referer-fallback handler can delegate to it.
pub async fn serve_static(uri: Uri) -> Response<Body> {
    let path = uri.path().trim_start_matches('/');

    // Map root to index.html
    let path = if path.is_empty() { "index.html" } else { path };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path)
                .first_or_octet_stream()
                .to_string();
            let mut res = Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime);
            // Avoid stale dashboard JS/HTML/CSS after upgrades (embedded assets otherwise get heuristic browser cache).
            if path.ends_with(".html") || path.ends_with(".js") || path.ends_with(".css") {
                res = res.header(header::CACHE_CONTROL, "no-store, max-age=0");
            }
            res.body(Body::from(content.data.to_vec())).unwrap()
        }
        None => {
            // SPA fallback: serve index.html for routes handled by React Router
            // (e.g. /config.html, /login.html) unless it looks like a real file request.
            if (!path.contains('.') || path.ends_with(".html"))
                && let Some(index) = Assets::get("index.html")
            {
                return Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "text/html")
                    .header(header::CACHE_CONTROL, "no-store, max-age=0")
                    .body(Body::from(index.data.to_vec()))
                    .unwrap();
            }
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::from("Not Found"))
                .unwrap()
        }
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
        for m in &["GET", "POST", "PUT", "PATCH", "DELETE"] {
            assert!(
                methods.contains(m),
                "Allow-Methods should include {m}, got: {methods}"
            );
        }
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
        // Default: host="0.0.0.0", port=8080, cors_origins=[] →
        // http://localhost:8080, http://127.0.0.1:8080, http://0.0.0.0:8080
        let web = WebConfig::default();

        // All three auto-computed variants should be allowed
        for origin in &[
            "http://localhost:8080",
            "http://127.0.0.1:8080",
            "http://0.0.0.0:8080",
        ] {
            let app = test_router(&web);
            let resp = app
                .oneshot(
                    Request::get("/api/health")
                        .header("Origin", *origin)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(resp.status(), StatusCode::OK);
            let acao = resp
                .headers()
                .get("access-control-allow-origin")
                .unwrap_or_else(|| panic!("missing ACAO header for auto-computed origin {origin}"));
            assert_eq!(acao, *origin);
        }
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
                    .header("Access-Control-Request-Method", "TRACE")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // TRACE is not in the allowed methods list; the preflight should either
        // return 403 or omit the allow-methods header that includes TRACE.
        let methods = resp
            .headers()
            .get("access-control-allow-methods")
            .map(|v| v.to_str().unwrap_or("").to_string())
            .unwrap_or_default();
        assert!(
            !methods.contains("TRACE"),
            "TRACE should not be in allowed methods: {methods}"
        );
    }
}
