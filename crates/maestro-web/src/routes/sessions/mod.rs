// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! HTTP + WebSocket reverse-proxy that fronts every editor and terminal
//! session under a single dashboard port.
//!
//! Path shape: `/s/{path-token}/{*rest}`
//!  * `path-token` is the 32-char hex token registered in the
//!    [`PathTokenRegistry`] when the session was opened.
//!  * `rest` is forwarded verbatim (with the original query string) to the
//!    upstream backend listener (derived from `DOCKER_HOST` in DinD mode,
//!    or `127.0.0.1` with local Docker).
//!
//! Behaviour requirements:
//!  * When a user database is configured, every request must carry a valid
//!    `maestro_session` cookie (database-backed session). Unauthenticated
//!    requests receive `401 Unauthorized` before the path token is even
//!    checked, so a leaked URL alone is not sufficient to access a session.
//!  * Unknown tokens → `404 Not Found`, empty body, no `kind` echoed back
//!    (anti-info-leak).
//!  * `/s/{token}` with no trailing slash → `308 Permanent Redirect` to
//!    `/s/{token}/` so relative asset URLs from openvscode-server / ttyd
//!    resolve correctly.
//!  * WebSocket upgrade requests are token-validated **before** the 101
//!    handshake completes — an unknown token returns 404, never an upgrade
//!    response.
//!  * Successful WebSocket upgrades are tunnelled bidirectionally so the
//!    backend's existing `Upgrade: websocket` flow keeps working through
//!    the proxy.
//!
//! Split into four files under §7 push-to-A audit (previously a single
//! 1165-LOC `routes/sessions.rs`):
//! - `mod.rs` — top-level [`proxy_session`] + [`proxy_or_static_fallback`]
//!   and the auth/ownership gate
//! - `token_validator.rs` — path parser, token-hash logger, 404/308
//!   builders, WebSocket-upgrade detection
//! - `proxy_forward.rs` — plain HTTP forwarding, upstream URI builder,
//!   header sanitisation, hyper client
//! - `websocket.rs` — `forward_websocket` (101 + bidirectional tunnel)

mod proxy_forward;
mod token_validator;
mod websocket;

pub use token_validator::{parse_session_path, token_hash_prefix};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Request, Response, StatusCode, header};
use axum::response::IntoResponse;

use crate::auth::{session_cookie_from_headers, validate_db_session};
use crate::session_registry::{SessionRoute, SessionRouteKind};
use crate::state::{AuthState, EditorState};

use proxy_forward::forward_http;
use token_validator::{is_websocket_upgrade, not_found, redirect_to_trailing_slash};
use websocket::forward_websocket;

/// Fallback handler for root-relative asset requests from proxied apps.
///
/// Dev servers (Vite, Storybook, etc.) generate JS `import` statements with
/// root-relative paths (e.g. `import "/node_modules/.vite/deps/react.js"`).
/// The browser resolves these against the page origin, bypassing the
/// `/s/{token}/` proxy path. This handler catches those requests by checking
/// the `Referer` header: if it contains `/s/{token}/`, the request is proxied
/// to the same upstream.
///
/// Registered as the `fallback` handler, replacing the static file handler
/// for requests that match a known proxy session via referer.
pub async fn proxy_or_static_fallback(
    State(auth): State<AuthState>,
    State(editor): State<EditorState>,
    req: Request<Body>,
) -> Response<Body> {
    // Unknown `/api/*` paths must NOT fall through to the SPA bundle — a route
    // that has been deleted (e.g. a removed `/api/workspaces`) should
    // return 404, not 200 with `index.html`. The SPA is for client-side routes
    // only; the dashboard's API surface is canonical.
    if req.uri().path().starts_with("/api/") {
        return (StatusCode::NOT_FOUND, "").into_response();
    }

    // Find a DynamicPort route via referer or cookie.
    let proxy_route = find_dynamic_port_route(&editor, req.headers()).await;

    if let Some((token, route)) = proxy_route {
        let auth_user_id = authenticate_request(auth.db.as_ref(), req.headers())
            .await
            .ok()
            .flatten();
        if user_owns_route(&auth_user_id, &route) {
            let path = req.uri().path().to_string();
            return forward_http(req, route, &path, &token).await;
        }
    }
    crate::server::serve_static(req.uri().clone()).await
}

/// Find a DynamicPort route for a root-relative asset request.
///
/// Strategy 1: `Referer` header contains `/s/{token}/` → use that token.
/// Strategy 2: `maestro_dynamic_port` cookie → set when the user last
///   visited a DynamicPort HTML page, catches deep JS import chains.
async fn find_dynamic_port_route(
    editor: &EditorState,
    headers: &axum::http::HeaderMap,
) -> Option<(String, SessionRoute)> {
    // Try referer first (most reliable — carries the exact token).
    if let Some(referer) = headers.get(header::REFERER).and_then(|v| v.to_str().ok())
        && let Some(token) = extract_token_from_referer(referer)
        && let Some(route) = editor.path_token_registry.lookup(&token).await
        && route.kind == SessionRouteKind::DynamicPort
    {
        return Some((token, route));
    }
    // Fall back to cookie (covers deep dependency chains without referer).
    let token = extract_dynamic_port_cookie(headers)?;
    let route = editor.path_token_registry.lookup(&token).await?;
    (route.kind == SessionRouteKind::DynamicPort).then_some((token, route))
}

/// Authenticate the request and return the user ID if valid.
///
/// Returns `Some(user_id)` when a valid `db-` session cookie is present.
/// Returns `None` when no database is configured (auth skipped).
/// Returns `Err(())` when auth is required but the cookie is missing/invalid.
async fn authenticate_request(
    db: Option<&maestro_core::db::Database>,
    headers: &axum::http::HeaderMap,
) -> Result<Option<String>, ()> {
    let Some(db) = db else {
        return Ok(None);
    };
    match session_cookie_from_headers(headers) {
        Some(raw) if raw.starts_with("db-") => {
            let uid = validate_db_session(db.adapter(), raw).await;
            uid.map(Some).ok_or(())
        }
        _ => Err(()),
    }
}

/// Check that the authenticated user owns the given route.
fn user_owns_route(auth_user_id: &Option<String>, route: &SessionRoute) -> bool {
    match auth_user_id {
        Some(uid) => route.user_id == *uid,
        None => true, // no DB configured, skip ownership check
    }
}

/// Extract the `maestro_dynamic_port` cookie value (a proxy token).
fn extract_dynamic_port_cookie(headers: &axum::http::HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some((name, value)) = part.split_once('=')
            && name.trim() == "maestro_dynamic_port"
        {
            let token = value.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// Extract a session token from a Referer URL like
/// `http://host:port/s/{token}/…`.
fn extract_token_from_referer(referer: &str) -> Option<String> {
    // Find "/s/" in the referer URL and extract the token segment.
    let after_s = referer.find("/s/").map(|i| &referer[i + 3..])?;
    let token = after_s.split('/').next()?;
    if token.is_empty() {
        return None;
    }
    Some(token.to_string())
}

/// Top-level handler registered at `/s/{*rest}`.
///
/// When a user database is configured (`state.auth.db` is `Some`), the handler
/// requires a valid `maestro_session` cookie before proceeding. This prevents
/// access if the URL leaks — the unguessable path token remains a defence in
/// depth, but is no longer the sole gatekeeper.
pub async fn proxy_session(
    State(auth): State<AuthState>,
    State(editor): State<EditorState>,
    req: Request<Body>,
) -> Response<Body> {
    let auth_user_id = match authenticate_request(auth.db.as_ref(), req.headers()).await {
        Ok(uid) => uid,
        Err(()) => {
            // SAFETY: Response::builder() with only a `StatusCode` set + an
            // empty body cannot fail — no header validation involved.
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Body::empty())
                .expect("status + empty body is infallible");
        }
    };

    let path = req.uri().path().to_string();
    let query = req.uri().query().map(|s| s.to_string());
    let (token, rest) = match parse_session_path(&path) {
        Some(parts) => parts,
        None => return not_found(),
    };
    let rest = match rest {
        None => return redirect_to_trailing_slash(token, query.as_deref()),
        Some(r) => r,
    };
    let route = match editor.path_token_registry.lookup(token).await {
        Some(r) => r,
        None => {
            tracing::warn!(
                token_hash = %token_hash_prefix(token),
                "session path token not found"
            );
            return not_found();
        }
    };
    if !user_owns_route(&auth_user_id, &route) {
        tracing::warn!(
            token_hash = %token_hash_prefix(token),
            "session route user_id mismatch"
        );
        return not_found();
    }

    if is_websocket_upgrade(&req) {
        forward_websocket(req, route, rest, token).await
    } else {
        forward_http(req, route, rest, token).await
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use maestro_core::actions::dry_run::DryRunActions;
    use maestro_core::config::{Config, TicketingSystem};
    use maestro_core::workflow::engine::WorkflowEngine;

    use crate::server::build_router;
    use crate::session_registry::{PathTokenRegistry, SessionRoute, SessionRouteKind};
    use crate::state::{
        AppState, AuthState, ConfigState, EditorState, EngineState, RunCommandState,
    };

    fn test_state() -> AppState {
        let config = Arc::new(RwLock::new(Config::default()));
        let actions: Arc<dyn maestro_core::actions::traits::ExternalActions> =
            Arc::new(DryRunActions::new("origin".to_string(), None));
        let jira_available = Arc::new(AtomicBool::new(false));
        let engine = Arc::new(WorkflowEngine::new(
            config.clone(),
            actions,
            1,
            jira_available.clone(),
            TicketingSystem::None,
            std::env::temp_dir(),
        ));
        AppState::new(
            EngineState {
                engine,
                polling_paused: Arc::new(AtomicBool::new(false)),
                clone_in_progress: Arc::new(AtomicBool::new(false)),
                system_status: Arc::new(RwLock::new(
                    maestro_core::docker_hooks::SystemStatus::default(),
                )),
            },
            AuthState {
                db: None,
                gh_client: Arc::new(maestro_core::auth::RealGhClient::new()),
                git_auth_resolver: None,
            },
            ConfigState {
                config,
                config_path: std::env::temp_dir().join("config.toml"),
                config_writer: None,
                ticketing_system: TicketingSystem::None,
                jira_available,
                preflight_error: None,
                work_item_flow_defaults: std::sync::Arc::new(Vec::new()),
            },
            EditorState {
                editor_scanners: Arc::new(RwLock::new(HashMap::new())),
                dynamic_forwards: Arc::new(RwLock::new(HashMap::new())),
                terminal_ports: Arc::new(RwLock::new(HashMap::new())),
                editor_bundles: Arc::new(RwLock::new(HashMap::new())),
                path_token_registry: PathTokenRegistry::new(),
            },
            RunCommandState {
                run_commands: Arc::new(RwLock::new(HashMap::new())),
                run_command_bundles: Arc::new(RwLock::new(HashMap::new())),
            },
        )
    }

    /// Registered token with no upstream listener → 502 Bad Gateway.
    /// This proves the handler resolved the token, built the upstream URI,
    /// and attempted forwarding (connection refused ≈ 502).
    #[tokio::test]
    async fn proxy_known_token_no_upstream_returns_502() {
        let state = test_state();
        let token = state
            .editor
            .path_token_registry
            .register_with_token(
                "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                SessionRoute {
                    kind: SessionRouteKind::Editor,
                    host_port: 19999, // nothing listens here
                    ticket_key: "TEST-1".to_string(),
                    user_id: "test-user".to_string(),
                },
            )
            .await;
        assert!(token);

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                    .header("Host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    /// Unknown token → 404 with empty body (anti-info-leak).
    #[tokio::test]
    async fn proxy_unknown_token_returns_404() {
        let state = test_state();
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/s/deadbeefdeadbeefdeadbeefdeadbeef/foo")
                    .header("Host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        assert_eq!(body.len(), 0, "404 body must be empty (no info leak)");
    }

    /// Bare `/s/{token}` (no trailing slash) → 308 redirect to `/s/{token}/`.
    #[tokio::test]
    async fn proxy_bare_token_redirects_with_trailing_slash() {
        let state = test_state();
        let token_str = "eeee5555ffff6666aaaa7777bbbb8888";
        state
            .editor
            .path_token_registry
            .register_with_token(
                token_str.to_string(),
                SessionRoute {
                    kind: SessionRouteKind::Terminal,
                    host_port: 19998,
                    ticket_key: "TEST-2".to_string(),
                    user_id: "test-user".to_string(),
                },
            )
            .await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get(format!("/s/{token_str}"))
                    .header("Host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
        let loc = resp
            .headers()
            .get(header::LOCATION)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(loc, format!("/s/{token_str}/"));
    }

    /// When a database is present, requests without a session cookie must
    /// be rejected with 401 — even if the path token is valid.
    #[tokio::test]
    async fn proxy_returns_401_when_db_present_but_no_cookie() {
        let state = crate::test_helpers::test_state_with_db();
        state
            .editor
            .path_token_registry
            .register_with_token(
                "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                SessionRoute {
                    kind: SessionRouteKind::Editor,
                    host_port: 19999,
                    ticket_key: "TEST-1".to_string(),
                    user_id: "test-user".to_string(),
                },
            )
            .await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                    .header("Host", "localhost")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// An invalid session cookie must also be rejected with 401.
    #[tokio::test]
    async fn proxy_returns_401_when_db_present_with_invalid_cookie() {
        let state = crate::test_helpers::test_state_with_db();
        state
            .editor
            .path_token_registry
            .register_with_token(
                "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                SessionRoute {
                    kind: SessionRouteKind::Editor,
                    host_port: 19999,
                    ticket_key: "TEST-1".to_string(),
                    user_id: "test-user".to_string(),
                },
            )
            .await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                    .header("Host", "localhost")
                    .header("Cookie", "maestro_session=db-nonexistent-session-id")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// A valid session cookie must pass through to the proxy logic.
    /// Since there's no upstream listener, we expect 502 (Bad Gateway) —
    /// proving we got past the auth gate AND ownership check.
    #[tokio::test]
    async fn proxy_allows_with_valid_cookie() {
        let state = crate::test_helpers::test_state_with_db();
        let cookie = crate::test_helpers::register_and_login(&state).await;
        // Look up the admin user's ID so the route ownership check passes.
        let admin_user_id = {
            let db = state.auth.db.as_ref().unwrap();
            maestro_core::db::users::get_user_by_username(db.adapter(), "admin")
                .await
                .unwrap()
                .unwrap()
                .id
        };
        state
            .editor
            .path_token_registry
            .register_with_token(
                "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                SessionRoute {
                    kind: SessionRouteKind::Editor,
                    host_port: 19999,
                    ticket_key: "TEST-1".to_string(),
                    user_id: admin_user_id,
                },
            )
            .await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                    .header("Host", "localhost")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // 502 = upstream unreachable, which means auth + ownership passed.
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    /// A valid session cookie for user A must NOT grant access to user B's
    /// session route — the proxy returns 404 (same as unknown token).
    #[tokio::test]
    async fn proxy_returns_404_when_user_mismatch() {
        let state = crate::test_helpers::test_state_with_db();
        let cookie = crate::test_helpers::register_and_login(&state).await;
        // Register a route owned by a different user.
        state
            .editor
            .path_token_registry
            .register_with_token(
                "aaaa1111bbbb2222cccc3333dddd4444".to_string(),
                SessionRoute {
                    kind: SessionRouteKind::Editor,
                    host_port: 19999,
                    ticket_key: "TEST-1".to_string(),
                    user_id: "other-user-id".to_string(),
                },
            )
            .await;

        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::get("/s/aaaa1111bbbb2222cccc3333dddd4444/")
                    .header("Host", "localhost")
                    .header("Cookie", &cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }
}
