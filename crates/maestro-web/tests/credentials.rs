// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Phase 2b.1 integration tests for the per-user credential surface.
// Source: tmp/multi-agents/04_architecture.md §3 + §4.
//
// Every test uses an in-process MockGhClient — the real `gh` CLI is never
// invoked.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use maestro_core::auth::gh_client::{GhClient, GhResponse};
use maestro_web::server::build_router;
use maestro_web::state::AppState;
use maestro_web::test_helpers::register_and_login;

// ---------------------------------------------------------------------------
// Mock gh client
// ---------------------------------------------------------------------------

/// In-memory `GhClient` used by every test in this file. Holds one canned
/// response for `api_user` and one for `api_org` (the latter defaults to a
/// permissive 200). Counts call invocations so a test can assert "the gh
/// client was hit exactly N times".
#[derive(Default)]
struct MockGh {
    user_resp: Mutex<Option<Result<GhResponse, String>>>,
    org_resp: Mutex<Option<Result<GhResponse, String>>>,
    user_calls: Mutex<u32>,
    org_calls: Mutex<u32>,
}

impl MockGh {
    fn user(resp: GhResponse) -> Arc<Self> {
        Arc::new(Self {
            user_resp: Mutex::new(Some(Ok(resp))),
            org_resp: Mutex::new(Some(Ok(GhResponse {
                status: 200,
                headers: vec![],
                body: "{}".into(),
            }))),
            user_calls: Mutex::new(0),
            org_calls: Mutex::new(0),
        })
    }

    fn user_ok(login: &str, scopes: &str) -> Arc<Self> {
        Self::user(GhResponse {
            status: 200,
            headers: vec![("X-OAuth-Scopes".into(), scopes.into())],
            body: format!("{{\"login\":\"{login}\"}}"),
        })
    }
}

#[async_trait::async_trait]
impl GhClient for MockGh {
    async fn api_user(&self, _pat: &str) -> Result<GhResponse, String> {
        *self.user_calls.lock().unwrap() += 1;
        self.user_resp
            .lock()
            .unwrap()
            .clone()
            .expect("no canned user response")
    }
    async fn api_org(&self, _pat: &str, _org: &str) -> Result<GhResponse, String> {
        *self.org_calls.lock().unwrap() += 1;
        self.org_resp
            .lock()
            .unwrap()
            .clone()
            .expect("no canned org response")
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a state with a fresh DB + the supplied mock gh client + a logged-in admin.
async fn state_with_mock(gh: Arc<dyn GhClient>) -> (AppState, String) {
    let mut state = maestro_web::test_helpers::test_state_with_db();
    state.gh_client = gh;
    let cookie = register_and_login(&state).await;
    (state, cookie)
}

/// Disable the master key on the state so write endpoints return 503.
/// We swap the DB for a fresh on-disk one with `allow_auto_generate=false`
/// AND no pre-existing keyfile — `master_key()` resolves to `None`.
fn break_master_key(state: &mut AppState) {
    let dir = std::env::temp_dir().join(format!("maestro-cred-degraded-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    state.db = Some(
        maestro_core::db::Database::open(&dir, false)
            .expect("open DB with auto-gen disabled"),
    );
}

/// Count `credential_audit` rows in the DB.
async fn audit_row_count(state: &AppState) -> i64 {
    let db = state.db.as_ref().unwrap().clone();
    tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        conn.query_row("SELECT COUNT(*) FROM credential_audit", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
    })
    .await
    .unwrap()
}

/// Build a POST request that goes through the CSRF guard.
fn json_request(method: &str, uri: &str, cookie: &str, body: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header("Content-Type", "application/json")
        .header("Origin", "http://localhost:8080")
        .header("Cookie", cookie)
        .body(Body::from(body.to_string()))
        .unwrap()
}

// ---------------------------------------------------------------------------
// T-USER-001 / T-USER-005: provider credential set + get cycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_provider_credential_creates_row_and_get_reports_it() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let pre = audit_row_count(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            r#"{"api_key":"sk-ant-real-token-here"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // GET reports the credential.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/users/me/credentials")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Task #39: wire shape is now a bundle — `provider.api_key.kind` rather
    // than `provider.kind`. The bundle's `provider` field repeats the
    // active-provider name once so older clients keying off `provider.provider`
    // still resolve to a string.
    assert_eq!(json["provider"]["provider"], "claude");
    assert_eq!(json["provider"]["api_key"]["provider"], "claude");
    assert_eq!(json["provider"]["api_key"]["kind"], "api_key");
    assert_eq!(json["provider"]["api_key"]["active"], true);
    // Task #39: cli_state slot must be absent when only api_key is stored.
    assert!(
        json["provider"].get("cli_state").is_none(),
        "cli_state slot must be omitted when only api_key is set: {}",
        json["provider"]
    );
    // No leaked secrets — see secret_leak_guards test for the full allowlist.
    assert!(json["provider"]["api_key"].get("ciphertext").is_none());
    assert!(json["provider"]["api_key"].get("nonce").is_none());

    // Audit row written.
    let post = audit_row_count(&state).await;
    assert!(
        post > pre,
        "credential write must emit an audit row (pre={pre} post={post})"
    );
}

#[tokio::test]
async fn post_provider_credential_twice_rotates_and_returns_200() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    let app = build_router(state.clone());
    let r1 = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/cursor",
            &cookie,
            r#"{"api_key":"first-key"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r1.status(), StatusCode::CREATED);

    let app = build_router(state.clone());
    let r2 = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/cursor",
            &cookie,
            r#"{"api_key":"second-key"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(r2.status(), StatusCode::OK, "rotation must return 200");
}

// ---------------------------------------------------------------------------
// T-USER-020/021/022: input validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_api_key_returns_400_api_key_empty() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let app = build_router(state);
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            r#"{"api_key":"   "}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "api_key_empty");
}

#[tokio::test]
async fn oversized_api_key_returns_400_api_key_too_long() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let app = build_router(state);
    let huge = "x".repeat(5000);
    let body = format!(r#"{{"api_key":"{huge}"}}"#);
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "api_key_too_long");
}

#[tokio::test]
async fn null_byte_in_api_key_returns_400() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let app = build_router(state);
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            "{\"api_key\":\"ab\\u0000cd\"}",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "api_key_invalid_nul");
}

#[tokio::test]
async fn unknown_provider_returns_400_unknown_provider() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let app = build_router(state);
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/gemini",
            &cookie,
            r#"{"api_key":"x"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "unknown_provider");
}

// ---------------------------------------------------------------------------
// T-GH-PAT-SAVE-INVALID
// ---------------------------------------------------------------------------

#[tokio::test]
async fn invalid_pat_returns_400_invalid_pat_and_writes_audit_row() {
    let (state, cookie) = state_with_mock(MockGh::user(GhResponse {
        status: 401,
        headers: vec![],
        body: "{}".into(),
    }))
    .await;
    let pre = audit_row_count(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/github-pat",
            &cookie,
            r#"{"pat":"ghp_bogus"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "invalid_pat");

    // Audit row written with outcome=error.
    let post = audit_row_count(&state).await;
    assert_eq!(post, pre + 1);
}

// ---------------------------------------------------------------------------
// T-GH-PAT-SAVE-SCOPES
// ---------------------------------------------------------------------------

#[tokio::test]
async fn pat_missing_repo_scope_returns_insufficient_scopes() {
    // PAT validates as a user but only has `read:org` — neither classic nor
    // fine-grained sufficient.
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "read:org")).await;
    let pre = audit_row_count(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/github-pat",
            &cookie,
            r#"{"pat":"ghp_scoped_low"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "insufficient_scopes");
    let missing: Vec<String> = serde_json::from_value(json["missing_scopes"].clone()).unwrap();
    assert!(missing.contains(&"contents:write".to_string()));

    let post = audit_row_count(&state).await;
    assert_eq!(post, pre + 1);
}

// ---------------------------------------------------------------------------
// T-GH-PAT-SAVE-SUCCESS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn valid_pat_seal_upsert_and_returns_login_scopes_attribute_commits() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice-gh", "repo, read:org")).await;
    let pre = audit_row_count(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/github-pat",
            &cookie,
            r#"{"pat":"ghp_valid_token","attribute_commits":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["login"], "alice-gh");
    assert!(json["scopes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str() == Some("repo")));
    assert_eq!(json["attribute_commits"], false);

    let post = audit_row_count(&state).await;
    assert_eq!(post, pre + 1, "exactly one ok audit row after PAT save");
}

// ---------------------------------------------------------------------------
// T-GH-PAT-DELETE
// ---------------------------------------------------------------------------

#[tokio::test]
async fn delete_github_pat_clears_row_and_writes_audit() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice-gh", "repo")).await;
    // Save a PAT first.
    let app = build_router(state.clone());
    app.oneshot(json_request(
        "POST",
        "/api/users/me/github-pat",
        &cookie,
        r#"{"pat":"ghp_valid"}"#,
    ))
    .await
    .unwrap();
    let pre = audit_row_count(&state).await;

    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "DELETE",
            "/api/users/me/github-pat",
            &cookie,
            "",
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let post = audit_row_count(&state).await;
    assert_eq!(post, pre + 1, "delete should write exactly one audit row");
}

// ---------------------------------------------------------------------------
// T-GH-PATCH-ATTR-COMMITS (the column-rename verification)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn patch_attribute_commits_flips_sign_commits_column() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    // Save a PAT first so the PATCH has something to flip.
    let app = build_router(state.clone());
    app.oneshot(json_request(
        "POST",
        "/api/users/me/github-pat",
        &cookie,
        r#"{"pat":"ghp_valid","attribute_commits":true}"#,
    ))
    .await
    .unwrap();

    // Flip via the wire name.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "PATCH",
            "/api/users/me/github",
            &cookie,
            r#"{"attribute_commits":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // Verify the SQLite column is now 0.
    let db = state.db.as_ref().unwrap().clone();
    let value: i64 = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        conn.query_row(
            "SELECT sign_commits FROM user_github_credentials",
            [],
            |r| r.get(0),
        )
        .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(
        value, 0,
        "wire `attribute_commits=false` must clear the `sign_commits` column"
    );
}

#[tokio::test]
async fn patch_attribute_commits_returns_404_when_no_pat() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let app = build_router(state);
    let resp = app
        .oneshot(json_request(
            "PATCH",
            "/api/users/me/github",
            &cookie,
            r#"{"attribute_commits":false}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// T-ADMIN-GH-STATUS
// ---------------------------------------------------------------------------

#[tokio::test]
async fn admin_can_read_peer_github_status_non_admin_gets_403() {
    let (state, admin_cookie) = state_with_mock(MockGh::user_ok("admin-gh", "repo")).await;

    // Create a second user via the admin endpoint.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users",
            &admin_cookie,
            r#"{"username":"bob","password":"testpassword1234","role":"user"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let bob_json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // POST /api/users returns { user: {id, username, role, ...}, recovery_codes }.
    let bob_id = bob_json["user"]["id"]
        .as_str()
        .unwrap_or_else(|| panic!("missing user.id; got body: {bob_json}"))
        .to_string();

    // Admin reads bob's github-status — must succeed, return has_pat:false.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get(format!("/api/admin/users/{bob_id}/github-status"))
                .header("Cookie", &admin_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["has_pat"], false);

    // Bob logs in and tries to read his own status via the admin route → 403.
    let app = build_router(state.clone());
    let login = app
        .oneshot(
            Request::post("/api/auth/login")
                .header("Content-Type", "application/json")
                .header("Origin", "http://localhost:8080")
                .body(Body::from(
                    r#"{"username":"bob","password":"testpassword1234"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        login.status(),
        StatusCode::NO_CONTENT,
        "bob login should succeed"
    );
    let bob_cookie = login
        .headers()
        .get("set-cookie")
        .expect("set-cookie header")
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_string();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get(format!("/api/admin/users/{bob_id}/github-status"))
                .header("Cookie", &bob_cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
}

// ---------------------------------------------------------------------------
// T-DEGRADED-503
// ---------------------------------------------------------------------------

#[tokio::test]
async fn post_credential_returns_503_when_master_key_unavailable() {
    let (mut state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    break_master_key(&mut state);
    // Re-register because the new DB is empty — we need a valid session.
    let cookie2 = register_and_login(&state).await;

    let app = build_router(state);
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie2,
            r#"{"api_key":"x"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "master_key_unavailable");
    // The unused-import-killer cookie name keeps Clippy quiet on the helper.
    let _ = cookie;
}

// ---------------------------------------------------------------------------
// Secret-leak guard (the most important test)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_my_credentials_never_returns_sealed_bytes_or_tokens() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice-gh", "repo")).await;

    // Seed both a provider credential and a GitHub PAT.
    let app = build_router(state.clone());
    app.oneshot(json_request(
        "POST",
        "/api/users/me/credentials/claude",
        &cookie,
        r#"{"api_key":"sk-ant-secret-token"}"#,
    ))
    .await
    .unwrap();
    let app = build_router(state.clone());
    app.oneshot(json_request(
        "POST",
        "/api/users/me/github-pat",
        &cookie,
        r#"{"pat":"ghp_secret_pat","attribute_commits":true}"#,
    ))
    .await
    .unwrap();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/users/me/credentials")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let raw = std::str::from_utf8(&body).unwrap();

    // Hard guards: the response must NOT carry the actual sealed bytes or
    // the plaintext we pasted in. We only check for substrings that would
    // be unambiguous leaks; the literal token `"kind":"api_key"` is metadata
    // (the kind discriminator), not the credential bytes.
    for forbidden in &[
        "sk-ant-secret-token",
        "ghp_secret_pat",
        "ciphertext",
        "wrapped_dek",
        "wnonce",
        "\"nonce\"",
    ] {
        assert!(
            !raw.contains(forbidden),
            "GET /api/users/me/credentials must not include `{forbidden}` — \
             leaks would be catastrophic. Body: {raw}"
        );
    }

    // Sanity: the response DID include the metadata we expected.
    assert!(raw.contains("\"login\":\"alice-gh\""));
    assert!(raw.contains("\"provider\":\"claude\""));
}

// ---------------------------------------------------------------------------
// /api/auth/status — Phase 2b.1 mirror field
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_status_reflects_provider_credential_present_after_paste() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    // Before paste: present=false even for the authenticated caller.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["provider_credential_present"], false);

    // Paste a credential for the active provider (claude).
    let app = build_router(state.clone());
    app.oneshot(json_request(
        "POST",
        "/api/users/me/credentials/claude",
        &cookie,
        r#"{"api_key":"sk-ant-test"}"#,
    ))
    .await
    .unwrap();

    // After paste: present=true.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["provider_credential_present"], true);

    // Unauthenticated callers always see false.
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/auth/status")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["provider_credential_present"], false);
}

// ---------------------------------------------------------------------------
// /api/onboarding/status — Phase 2b.1 user_onboarding extension
// ---------------------------------------------------------------------------

#[tokio::test]
async fn onboarding_status_step4_flips_to_completed_when_credential_present() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    // No credential yet → no user_onboarding step_4_credentials entry.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/onboarding/status")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json.get("user_onboarding").is_some());
    // Before paste, step_4_credentials is null.
    assert!(
        json["user_onboarding"]["step_4_credentials"].is_null(),
        "expected null before paste, got {:?}",
        json["user_onboarding"]
    );

    // Paste a credential.
    let app = build_router(state.clone());
    app.oneshot(json_request(
        "POST",
        "/api/users/me/credentials/claude",
        &cookie,
        r#"{"api_key":"sk-ant"}"#,
    ))
    .await
    .unwrap();

    // Now step_4_credentials auto-flips to "completed".
    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/onboarding/status")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["user_onboarding"]["step_4_credentials"], "completed");
}

// ---------------------------------------------------------------------------
// Task #39: T-CLAUDE-CLI-STATE-*  —  kind=cli_state for Claude
// ---------------------------------------------------------------------------

/// Build a minimal valid `~/.claude.json` blob carrying the three required
/// oauthAccount keys plus extra fields the validator must ignore.
fn fixture_claude_session_json() -> String {
    serde_json::json!({
        "oauthAccount": {
            "accountUuid": "00000000-0000-0000-0000-000000000001",
            "emailAddress": "alice@example.com",
            "organizationUuid": "11111111-1111-1111-1111-111111111111",
            "organizationName": "Example Corp",
            "organizationType": "claude_team",
            "seatTier": "team_standard",
        },
        "lastUpdateCheck": "2026-05-19T00:00:00Z",
        "tipsHistory": {},
    })
    .to_string()
}

/// T-CLAUDE-CLI-STATE-001: POST valid cli_state → 201, GET reports it, audit row.
#[tokio::test]
async fn t_claude_cli_state_001_valid_post_creates_row_and_audit() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let pre = audit_row_count(&state).await;

    let body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": fixture_claude_session_json(),
    })
    .to_string();
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let resp_body = resp.into_body().collect().await.unwrap().to_bytes();
    let resp_json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(resp_json["provider"], "claude");
    assert_eq!(resp_json["kind"], "cli_state");

    // GET reports cli_state slot populated, api_key slot empty.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/users/me/credentials")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["provider"]["cli_state"]["kind"], "cli_state");
    assert_eq!(v["provider"]["cli_state"]["active"], true);
    assert!(v["provider"].get("api_key").is_none());

    // Audit row.
    let post = audit_row_count(&state).await;
    assert!(post > pre, "cli_state save must emit an audit row");
}

/// T-CLAUDE-CLI-STATE-002: missing `oauthAccount` → 400 + `claude_session_invalid`,
/// no DB write.
#[tokio::test]
async fn t_claude_cli_state_002_missing_oauthaccount_returns_400_no_side_effects() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let pre = audit_row_count(&state).await;

    // JSON parses fine, but oauthAccount is absent.
    let body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": r#"{"lastUpdateCheck":"2026-05-19T00:00:00Z"}"#,
    })
    .to_string();
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp_body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(v["error"], "claude_session_invalid");

    let post = audit_row_count(&state).await;
    assert_eq!(post, pre, "rejected save must NOT emit an audit row");
}

/// T-CLAUDE-CLI-STATE-003: non-JSON body → 400 `claude_session_json_invalid`.
#[tokio::test]
async fn t_claude_cli_state_003_non_json_body_returns_400() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    let body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": "not-valid-json-{[",
    })
    .to_string();
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let resp_body = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(v["error"], "claude_session_json_invalid");
}

/// T-CLAUDE-CLI-STATE-004: a user can have BOTH api_key AND cli_state rows
/// for claude. UNIQUE(user_id, provider, kind) allows independent slots.
#[tokio::test]
async fn t_claude_cli_state_004_user_can_have_both_kinds_simultaneously() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    // Save api_key first.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            r#"{"api_key":"sk-ant-token","kind":"api_key"}"#,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Then save cli_state — must NOT replace or conflict with api_key.
    let body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": fixture_claude_session_json(),
    })
    .to_string();
    let app = build_router(state.clone());
    let resp = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);

    // GET shows both slots populated.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/users/me/credentials")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["provider"]["api_key"]["kind"], "api_key");
    assert_eq!(v["provider"]["cli_state"]["kind"], "cli_state");
}

/// T-CLAUDE-CLI-STATE-005: non-Claude providers reject cli_state with
/// `kind_not_supported_for_provider` (cursor / codex / opencode).
#[tokio::test]
async fn t_claude_cli_state_005_non_claude_provider_rejects_cli_state() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;
    let body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": fixture_claude_session_json(),
    })
    .to_string();
    for provider in ["cursor", "codex", "opencode"] {
        let app = build_router(state.clone());
        let uri = format!("/api/users/me/credentials/{provider}");
        let resp = app
            .oneshot(json_request("POST", &uri, &cookie, &body))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "{provider}: cli_state must be rejected"
        );
        let rb = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&rb).unwrap();
        assert_eq!(v["error"], "kind_not_supported_for_provider");
    }
}

/// T-CLAUDE-CLI-STATE-006: `DELETE /api/users/me/credentials/claude?kind=cli_state`
/// removes only the cli_state row, leaves api_key intact. Without `?kind`,
/// both rows go (back-compat with the pre-task-#39 dashboard wipe).
#[tokio::test]
async fn t_claude_cli_state_006_delete_kind_query_scopes_deletion() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    // Seed both kinds.
    let app = build_router(state.clone());
    let _ = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            r#"{"api_key":"sk-ant"}"#,
        ))
        .await
        .unwrap();
    let body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": fixture_claude_session_json(),
    })
    .to_string();
    let app = build_router(state.clone());
    let _ = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &body,
        ))
        .await
        .unwrap();

    // DELETE with ?kind=cli_state.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/users/me/credentials/claude?kind=cli_state")
                .header("Origin", "http://localhost:8080")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    // GET: api_key still present, cli_state gone.
    let app = build_router(state.clone());
    let resp = app
        .oneshot(
            Request::get("/api/users/me/credentials")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["provider"]["api_key"]["kind"], "api_key");
    assert!(
        v["provider"].get("cli_state").is_none(),
        "cli_state must be wiped; api_key must survive: {}",
        v["provider"]
    );
}

/// T-CLAUDE-CLI-STATE-007: GET shape for a user with BOTH kinds. Bundle's
/// `provider` field carries the active-provider name; both slots populated.
#[tokio::test]
async fn t_claude_cli_state_007_get_shape_with_both_kinds_present() {
    let (state, cookie) = state_with_mock(MockGh::user_ok("alice", "repo")).await;

    // Seed both kinds.
    let app = build_router(state.clone());
    let _ = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            r#"{"api_key":"sk-ant"}"#,
        ))
        .await
        .unwrap();
    let cli_body = serde_json::json!({
        "kind": "cli_state",
        "claude_session_json": fixture_claude_session_json(),
    })
    .to_string();
    let app = build_router(state.clone());
    let _ = app
        .oneshot(json_request(
            "POST",
            "/api/users/me/credentials/claude",
            &cookie,
            &cli_body,
        ))
        .await
        .unwrap();

    let app = build_router(state);
    let resp = app
        .oneshot(
            Request::get("/api/users/me/credentials")
                .header("Cookie", &cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // Bundle shape: provider top-level + per-kind nested objects.
    assert_eq!(v["provider"]["provider"], "claude");
    assert_eq!(v["provider"]["api_key"]["provider"], "claude");
    assert_eq!(v["provider"]["api_key"]["kind"], "api_key");
    assert_eq!(v["provider"]["cli_state"]["provider"], "claude");
    assert_eq!(v["provider"]["cli_state"]["kind"], "cli_state");
}

// Suppress the "unused import" diagnostic on `Mutex` when the file grows.
#[allow(dead_code)]
fn _imports_in_scope(_m: std::marker::PhantomData<Mutex<()>>) {}
