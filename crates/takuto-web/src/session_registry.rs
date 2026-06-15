// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Session path-token registry for the shared-port reverse proxy.
//!
//! Maps an unguessable 32-char hex path token (≥128 bits of CSPRNG entropy,
//! produced by [`takuto_core::container::generate_session_path_token`]) to
//! the backend it fronts:
//!
//!  * the kind of session (editor or terminal) — purely informational, used
//!    for tracing and the deregister-by-ticket helper;
//!  * the host port the backend listens on (always `127.0.0.1:port` per the
//!    loopback-binding requirement);
//!  * the owning workflow ticket key — used so closing a workflow tears down
//!    every route it owns in one call;
//!  * (optional) the in-process auth secret the backend itself enforces
//!    (`?tkn=` for openvscode-server, `-b /TOKEN` for ttyd) — kept as defence
//!    in depth and as a place to record ownership metadata for future
//!    rotations.
//!
//! Lookups are cheap and read-mostly; the registry uses a `RwLock<HashMap>`
//! shared via `Arc` so it can be cloned into the [`crate::state::AppState`].

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use takuto_core::container::generate_session_path_token;

/// What kind of backend a registered token routes to.
///
/// The variant is *not* echoed back in 404 responses (see `routes::sessions`)
/// to avoid leaking which token slot was attempted, but it IS used in
/// structured tracing fields and to scope the deregister-by-ticket helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRouteKind {
    Editor,
    Terminal,
    /// A dynamically forwarded application port (e.g. a dev server started by
    /// the user). Proxy strips the `/s/{token}` prefix so the upstream app
    /// receives requests at root `/`. A `takuto_dynamic_port` cookie is set
    /// on HTML responses so the referer-based fallback can route root-relative
    /// JS imports to the correct upstream.
    DynamicPort,
}

impl SessionRouteKind {
    /// Stable lowercase identifier for tracing fields.
    pub fn as_str(self) -> &'static str {
        match self {
            SessionRouteKind::Editor => "editor",
            SessionRouteKind::Terminal => "terminal",
            SessionRouteKind::DynamicPort => "dynamic_port",
        }
    }
}

/// A backend reachable through the shared-port proxy.
#[derive(Debug, Clone)]
pub struct SessionRoute {
    pub kind: SessionRouteKind,
    /// The host port the backend listens on. The proxy connects to
    /// `127.0.0.1:host_port` — never to `0.0.0.0` — because backends MUST
    /// bind to loopback (enforced upstream in `container::start_editor` via
    /// `session_publish_arg`).
    pub host_port: u16,
    /// Workflow this route belongs to — used by `remove_for_ticket` so
    /// `close_editor` / `close_terminal` can drop every route they own
    /// without iterating the whole registry.
    pub ticket_key: String,
    /// The user who owns this session. The proxy verifies the authenticated
    /// user matches before forwarding, so one user cannot access another's
    /// editor, terminal, or dynamically forwarded ports.
    pub user_id: String,
}

/// Thread-safe map from path token → [`SessionRoute`].
///
/// Construct with `PathTokenRegistry::new()` and clone freely — the inner
/// `Arc<RwLock<_>>` makes clones share a single map.
#[derive(Clone, Default)]
pub struct PathTokenRegistry {
    inner: Arc<RwLock<HashMap<String, SessionRoute>>>,
}

impl PathTokenRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a route by token. Returns `None` for unknown tokens — callers
    /// must respond with `404 Not Found` (no body, no info leak).
    pub async fn lookup(&self, token: &str) -> Option<SessionRoute> {
        self.inner.read().await.get(token).cloned()
    }

    /// Generate a fresh path token, register the given route under it, and
    /// return the token.
    ///
    /// Internally retries on the (vanishingly rare) collision with an
    /// existing entry. `HashMap::entry` is used so the check-and-insert is
    /// atomic under the held write guard — no TOCTOU between lookup and
    /// insert.
    ///
    /// Returns `None` only if every attempt collides — astronomically
    /// unlikely with 128 bits of entropy, and only reachable under a
    /// degraded CSPRNG. Callers treat `None` as a transient failure (skip the
    /// route or respond `500`) rather than aborting the process.
    pub async fn register(&self, route: SessionRoute) -> Option<String> {
        let mut guard = self.inner.write().await;
        // 128 bits of entropy: collisions are negligible. The bounded loop
        // is belt-and-braces so a runaway loop is impossible even under
        // pathological RNG behaviour.
        for _ in 0..8 {
            let token = generate_session_path_token();
            if let std::collections::hash_map::Entry::Vacant(slot) = guard.entry(token.clone()) {
                slot.insert(route);
                return Some(token);
            }
        }
        tracing::error!(
            "PathTokenRegistry::register: 8 consecutive 128-bit token collisions; CSPRNG may be degraded"
        );
        None
    }

    /// Register with a caller-supplied token. Returns `true` on insert,
    /// `false` if the slot was already taken (idempotent for the same token).
    ///
    /// Used in production by `open_editor` for restart recovery: the editor
    /// container stores its path token as a Docker label, and on reconnect
    /// the same token is re-registered rather than minting a new one.
    pub async fn register_with_token(&self, token: String, route: SessionRoute) -> bool {
        let mut guard = self.inner.write().await;
        match guard.entry(token) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(route);
                true
            }
            std::collections::hash_map::Entry::Occupied(_) => false,
        }
    }

    /// Remove a single token. No-op for unknown tokens. Callers MUST invoke
    /// this BEFORE tearing down the underlying port so in-flight requests
    /// get a clean 404 instead of a hung connection.
    pub async fn remove(&self, token: &str) -> Option<SessionRoute> {
        self.inner.write().await.remove(token)
    }

    /// Drop every route owned by `ticket_key`. Returns the dropped tokens so
    /// callers can log them at debug level (logging the truncated SHA-256,
    /// not the raw token).
    pub async fn remove_for_ticket(&self, ticket_key: &str) -> Vec<String> {
        let mut guard = self.inner.write().await;
        let to_remove: Vec<String> = guard
            .iter()
            .filter(|(_, r)| r.ticket_key == ticket_key)
            .map(|(t, _)| t.clone())
            .collect();
        for t in &to_remove {
            guard.remove(t);
        }
        to_remove
    }

    /// Drop every route owned by `ticket_key` whose kind matches `kind`.
    /// Used by `close_terminal` so the editor's path token is preserved
    /// even when the terminal is closed independently.
    pub async fn remove_for_ticket_kind(
        &self,
        ticket_key: &str,
        kind: SessionRouteKind,
    ) -> Vec<String> {
        let mut guard = self.inner.write().await;
        let to_remove: Vec<String> = guard
            .iter()
            .filter(|(_, r)| r.ticket_key == ticket_key && r.kind == kind)
            .map(|(t, _)| t.clone())
            .collect();
        for t in &to_remove {
            guard.remove(t);
        }
        to_remove
    }

    /// Find an existing path token for `(ticket_key, kind)`. Returns the
    /// first match, or `None` if no such route exists. Used by
    /// `open_terminal` so reopening the same terminal re-uses the existing
    /// token instead of leaking a new one.
    pub async fn find_token_for(&self, ticket_key: &str, kind: SessionRouteKind) -> Option<String> {
        let guard = self.inner.read().await;
        guard
            .iter()
            .find(|(_, r)| r.ticket_key == ticket_key && r.kind == kind)
            .map(|(t, _)| t.clone())
    }

    /// Number of registered routes — used by tests and by debug telemetry.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// `true` when no routes are registered.
    pub async fn is_empty(&self) -> bool {
        self.inner.read().await.is_empty()
    }

    /// Acquire a read guard on the inner map for bulk lookups.
    ///
    /// Callers can iterate the map once under a single lock acquisition
    /// instead of calling `find_token_for` N times (each of which acquires
    /// its own read guard and does a linear scan).
    pub async fn inner_read(
        &self,
    ) -> tokio::sync::RwLockReadGuard<'_, HashMap<String, SessionRoute>> {
        self.inner.read().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor_route(ticket: &str, port: u16) -> SessionRoute {
        SessionRoute {
            kind: SessionRouteKind::Editor,
            host_port: port,
            ticket_key: ticket.to_string(),
            user_id: "test-user".to_string(),
        }
    }

    fn terminal_route(ticket: &str, port: u16) -> SessionRoute {
        SessionRoute {
            kind: SessionRouteKind::Terminal,
            host_port: port,
            ticket_key: ticket.to_string(),
            user_id: "test-user".to_string(),
        }
    }

    fn dynamic_port_route(ticket: &str, port: u16) -> SessionRoute {
        SessionRoute {
            kind: SessionRouteKind::DynamicPort,
            host_port: port,
            ticket_key: ticket.to_string(),
            user_id: "test-user".to_string(),
        }
    }

    #[tokio::test]
    async fn register_returns_token_present_in_lookup() {
        let reg = PathTokenRegistry::new();
        let token = reg
            .register(editor_route("PROJ-1", 9101))
            .await
            .expect("register");
        let route = reg.lookup(&token).await.expect("just registered");
        assert_eq!(route.kind, SessionRouteKind::Editor);
        assert_eq!(route.host_port, 9101);
        assert_eq!(route.ticket_key, "PROJ-1");
    }

    #[tokio::test]
    async fn register_returns_32_char_hex_token() {
        let reg = PathTokenRegistry::new();
        let token = reg
            .register(editor_route("PROJ-1", 9101))
            .await
            .expect("register");
        assert_eq!(token.len(), 32);
        assert!(token.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[tokio::test]
    async fn lookup_unknown_token_returns_none() {
        let reg = PathTokenRegistry::new();
        let _ = reg
            .register(editor_route("PROJ-1", 9101))
            .await
            .expect("register");
        assert!(reg.lookup("not-a-real-token").await.is_none());
        // Even a well-formed-looking but unknown token returns None.
        assert!(
            reg.lookup("ffffffffffffffffffffffffffffffff")
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn register_produces_distinct_tokens_per_call() {
        let reg = PathTokenRegistry::new();
        let t1 = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");
        let t2 = reg
            .register(terminal_route("A", 9102))
            .await
            .expect("register");
        assert_ne!(t1, t2);
        assert!(reg.lookup(&t1).await.is_some());
        assert!(reg.lookup(&t2).await.is_some());
    }

    #[tokio::test]
    async fn remove_drops_route() {
        let reg = PathTokenRegistry::new();
        let token = reg
            .register(editor_route("PROJ-1", 9101))
            .await
            .expect("register");
        assert!(reg.lookup(&token).await.is_some());
        let dropped = reg.remove(&token).await;
        assert!(dropped.is_some());
        assert!(reg.lookup(&token).await.is_none());
    }

    #[tokio::test]
    async fn remove_unknown_token_is_noop() {
        let reg = PathTokenRegistry::new();
        assert!(reg.remove("nope").await.is_none());
        // No panic, no state mutation:
        assert!(reg.is_empty().await);
    }

    #[tokio::test]
    async fn remove_for_ticket_drops_only_owned_routes() {
        let reg = PathTokenRegistry::new();
        let _ = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");
        let _ = reg
            .register(terminal_route("A", 9102))
            .await
            .expect("register");
        let b_token = reg
            .register(editor_route("B", 9103))
            .await
            .expect("register");
        let dropped = reg.remove_for_ticket("A").await;
        assert_eq!(dropped.len(), 2);
        assert_eq!(reg.len().await, 1);
        assert!(reg.lookup(&b_token).await.is_some());
    }

    #[tokio::test]
    async fn concurrent_register_threadsafe() {
        let reg = PathTokenRegistry::new();
        let mut handles = Vec::new();
        for i in 0..50 {
            let r = reg.clone();
            handles.push(tokio::spawn(async move {
                r.register(editor_route(&format!("T-{i}"), 9100 + i))
                    .await
                    .expect("register")
            }));
        }
        let mut tokens = Vec::new();
        for h in handles {
            tokens.push(h.await.unwrap());
        }
        // All 50 inserted tokens must be distinct AND findable.
        let unique: std::collections::HashSet<_> = tokens.iter().cloned().collect();
        assert_eq!(unique.len(), 50);
        for t in &tokens {
            assert!(reg.lookup(t).await.is_some());
        }
    }

    #[tokio::test]
    async fn kind_is_preserved() {
        let reg = PathTokenRegistry::new();
        let e = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");
        let t = reg
            .register(terminal_route("A", 9102))
            .await
            .expect("register");
        assert_eq!(reg.lookup(&e).await.unwrap().kind, SessionRouteKind::Editor);
        assert_eq!(
            reg.lookup(&t).await.unwrap().kind,
            SessionRouteKind::Terminal
        );
    }

    #[tokio::test]
    async fn register_with_token_rejects_duplicates() {
        let reg = PathTokenRegistry::new();
        let token = "0123456789abcdef0123456789abcdef".to_string();
        assert!(
            reg.register_with_token(token.clone(), editor_route("A", 9101))
                .await
        );
        // Second insert with same token must fail (covers TOCTOU-safe
        // semantics in `register`).
        assert!(
            !reg.register_with_token(token.clone(), editor_route("B", 9102))
                .await
        );
        // Original entry untouched.
        assert_eq!(reg.lookup(&token).await.unwrap().ticket_key, "A");
    }

    #[test]
    fn session_route_kind_as_str() {
        assert_eq!(SessionRouteKind::Editor.as_str(), "editor");
        assert_eq!(SessionRouteKind::Terminal.as_str(), "terminal");
    }

    // -----------------------------------------------------------------
    // remove_for_ticket_kind
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn remove_for_ticket_kind_drops_only_matching_kind() {
        let reg = PathTokenRegistry::new();
        let editor_token = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");
        let terminal_token = reg
            .register(terminal_route("A", 9102))
            .await
            .expect("register");
        let _ = reg
            .register(editor_route("B", 9103))
            .await
            .expect("register");

        // Remove only terminal routes for ticket A.
        let dropped = reg
            .remove_for_ticket_kind("A", SessionRouteKind::Terminal)
            .await;
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0], terminal_token);

        // Editor route for A must be preserved.
        assert!(reg.lookup(&editor_token).await.is_some());
        // Terminal route for A must be gone.
        assert!(reg.lookup(&terminal_token).await.is_none());
        // B is untouched.
        assert_eq!(reg.len().await, 2);
    }

    #[tokio::test]
    async fn remove_for_ticket_kind_preserves_other_ticket() {
        let reg = PathTokenRegistry::new();
        let _ = reg
            .register(terminal_route("A", 9101))
            .await
            .expect("register");
        let b_term = reg
            .register(terminal_route("B", 9102))
            .await
            .expect("register");

        let dropped = reg
            .remove_for_ticket_kind("A", SessionRouteKind::Terminal)
            .await;
        assert_eq!(dropped.len(), 1);
        // B's terminal must survive.
        assert!(reg.lookup(&b_term).await.is_some());
    }

    #[tokio::test]
    async fn remove_for_ticket_kind_noop_when_no_match() {
        let reg = PathTokenRegistry::new();
        let editor_token = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");

        // No terminal route for A exists — should be a no-op.
        let dropped = reg
            .remove_for_ticket_kind("A", SessionRouteKind::Terminal)
            .await;
        assert!(dropped.is_empty());
        assert!(reg.lookup(&editor_token).await.is_some());
    }

    // -----------------------------------------------------------------
    // find_token_for
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn find_token_for_returns_matching_token() {
        let reg = PathTokenRegistry::new();
        let editor_token = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");
        let _ = reg
            .register(terminal_route("A", 9102))
            .await
            .expect("register");

        let found = reg.find_token_for("A", SessionRouteKind::Editor).await;
        assert_eq!(found, Some(editor_token));
    }

    #[tokio::test]
    async fn find_token_for_returns_none_when_kind_mismatch() {
        let reg = PathTokenRegistry::new();
        let _ = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");

        let found = reg.find_token_for("A", SessionRouteKind::Terminal).await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_token_for_returns_none_when_ticket_mismatch() {
        let reg = PathTokenRegistry::new();
        let _ = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");

        let found = reg.find_token_for("B", SessionRouteKind::Editor).await;
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn find_token_for_returns_none_on_empty_registry() {
        let reg = PathTokenRegistry::new();
        assert!(
            reg.find_token_for("A", SessionRouteKind::Editor)
                .await
                .is_none()
        );
    }

    // -----------------------------------------------------------------
    // DynamicPort variant
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn dynamic_port_kind_preserved_and_scoped() {
        let reg = PathTokenRegistry::new();
        let editor = reg
            .register(editor_route("A", 9101))
            .await
            .expect("register");
        let terminal = reg
            .register(terminal_route("A", 9102))
            .await
            .expect("register");
        let dyn_tok = reg
            .register(dynamic_port_route("A", 9103))
            .await
            .expect("register");

        assert_eq!(
            reg.lookup(&dyn_tok).await.unwrap().kind,
            SessionRouteKind::DynamicPort
        );

        // remove_for_ticket_kind(DynamicPort) drops only dynamic port routes.
        let dropped = reg
            .remove_for_ticket_kind("A", SessionRouteKind::DynamicPort)
            .await;
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0], dyn_tok);
        assert!(reg.lookup(&editor).await.is_some());
        assert!(reg.lookup(&terminal).await.is_some());
    }

    #[test]
    fn dynamic_port_kind_as_str() {
        assert_eq!(SessionRouteKind::DynamicPort.as_str(), "dynamic_port");
    }
}
