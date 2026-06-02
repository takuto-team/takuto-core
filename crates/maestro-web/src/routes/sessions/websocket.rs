// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Forward a WebSocket upgrade to the upstream session listener.
//!
//! Token validation **must** happen before `forward_websocket` is called —
//! that ordering ensures the 101 response is never emitted on an unknown
//! token.

use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::response::IntoResponse;
use http_body_util::BodyExt;

use crate::session_registry::SessionRoute;

use super::proxy_forward::{
    build_upstream_uri, http_client, sanitise_request_headers, upstream_path_for_kind,
};
use super::token_validator::not_found;

/// Forward a WebSocket upgrade. The token has already been validated by
/// `proxy_session` BEFORE entering this function — that ordering ensures
/// the 101 response is never emitted on an unknown token.
pub(super) async fn forward_websocket(
    mut req: Request<Body>,
    route: SessionRoute,
    rest: &str,
    token: &str,
) -> Response<Body> {
    let query = req.uri().query().map(|s| s.to_string());
    let upstream_path = upstream_path_for_kind(route.kind, token, rest);
    let upstream_uri = match build_upstream_uri(route.host_port, &upstream_path, query.as_deref()) {
        Some(uri) => uri,
        None => return not_found(),
    };

    // Take the inbound upgrade future BEFORE forwarding, so we can pair it
    // with the upstream's upgrade once the 101 returns.
    let inbound_upgrade = hyper::upgrade::on(&mut req);

    sanitise_request_headers(&mut req, route.host_port, true);
    *req.uri_mut() = upstream_uri.clone();

    tracing::debug!(
        kind = route.kind.as_str(),
        host_port = route.host_port,
        upstream = %upstream_uri,
        "session proxy forwarding websocket upgrade"
    );

    let mut upstream_resp = match http_client().request(req).await {
        Ok(r) => r,
        Err(err) => {
            tracing::warn!(
                kind = route.kind.as_str(),
                host_port = route.host_port,
                error = %err,
                "session proxy upstream upgrade request failed"
            );
            return (StatusCode::BAD_GATEWAY, "upstream unavailable").into_response();
        }
    };

    if upstream_resp.status() != StatusCode::SWITCHING_PROTOCOLS {
        // Backend refused the upgrade — pass its response through.
        let (parts, body) = upstream_resp.into_parts();
        let body = Body::new(body.map_err(axum::Error::new));
        return Response::from_parts(parts, body);
    }

    // Take the upstream upgrade future BEFORE moving the response into parts
    // so we extract it from the live response's extensions, not from a clone.
    // The 101 response we return below tells the client the upgrade succeeded;
    // once axum hands us the raw socket via `hyper::upgrade::on(&mut req)`,
    // we glue it to this upstream upgrade.
    let upstream_upgrade = hyper::upgrade::on(&mut upstream_resp);
    let (parts, _body) = upstream_resp.into_parts();
    tokio::spawn(async move {
        let (client_io, server_io) = match tokio::join!(inbound_upgrade, upstream_upgrade) {
            (Ok(c), Ok(s)) => (c, s),
            (Err(e), _) | (_, Err(e)) => {
                tracing::debug!(error = %e, "websocket upgrade pairing failed");
                return;
            }
        };
        let mut client_io = hyper_util::rt::TokioIo::new(client_io);
        let mut server_io = hyper_util::rt::TokioIo::new(server_io);
        if let Err(e) = tokio::io::copy_bidirectional(&mut client_io, &mut server_io).await {
            tracing::debug!(error = %e, "websocket tunnel ended with error");
        }
    });

    // Build the 101 response sent back to the client. We must replay the
    // upstream's response headers (Sec-WebSocket-Accept, Upgrade, Connection)
    // verbatim — those headers are what completes the RFC-6455 handshake.
    let mut response = Response::builder().status(StatusCode::SWITCHING_PROTOCOLS);
    if let Some(headers) = response.headers_mut() {
        for (name, value) in parts.headers.iter() {
            headers.insert(name.clone(), value.clone());
        }
    }
    // SAFETY: The builder has only `SWITCHING_PROTOCOLS` set and headers
    // copied from a valid `parts.headers` (already parsed by hyper from a
    // real upstream response). `.body(Body::empty())` finalises an
    // already-valid builder; failure here is impossible.
    response
        .body(Body::empty())
        .expect("status + copied valid headers + empty body is infallible")
}
