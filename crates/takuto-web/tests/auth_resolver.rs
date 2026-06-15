// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

// Copyright (C) 2026 Alexandre Obellianne
//
// Integration tests for GitAuthResolver wiring on AppState.
//
// These tests cover the *integration* of the resolver with AppState — the
// resolver itself is exhaustively unit-tested in
// `crates/takuto-core/src/github/auth_resolver.rs::tests` (28 decision-matrix
// cells × actions, plus error / mode / SSO / audit-debounce cases).

use std::sync::Arc;

use takuto_core::auth::{MasterKey, seal};
use takuto_core::github::auth_resolver::{GitAction, GitAuthResolver, GithubAuthMode, TokenSource};

// ---------------------------------------------------------------------------
// Mode A — App only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appstate_resolver_mode_a_clone_picks_app() {
    let state = takuto_web::test_helpers::test_state_with_db();
    // test_state_with_db wires the resolver with no App. Build a fresh
    // resolver with a fake App attached to surface Mode A behaviour.
    let db = state.auth().db.as_ref().expect("db").clone();
    let app = Some(Arc::new(
        takuto_core::github_app::GitHubAppTokenManager::for_tests(7, 9),
    ));
    let resolver = GitAuthResolver::new(db.clone(), app);

    // Seed a user but no PAT.
    db.adapter()
        .execute(
            "INSERT INTO users (id, username, role) VALUES ('u-mode-a', 'mode-a', 'user')",
            vec![],
        )
        .await
        .unwrap();

    assert_eq!(
        resolver.mode_for_user("u-mode-a").await.unwrap(),
        GithubAuthMode::AppOnly
    );

    // We can't actually fetch an App token without a real PEM, but the
    // decision function is what we're verifying — the materialise path
    // failing on the encoding key is fine and proves the right branch was
    // taken.
    let err = resolver
        .token_for(GitAction::Clone, "u-mode-a")
        .await
        .expect_err("App token fetch must fail with the test-only key");
    // Confirm we hit the App branch (not the unauthenticated branch).
    assert_eq!(
        err.code(),
        "github_app_token_fetch_failed",
        "expected App-token-fetch error code, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Mode B — App + user PAT, push picks user PAT when attribute_commits=true
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appstate_resolver_mode_b_push_with_attribution_picks_user_pat() {
    let state = takuto_web::test_helpers::test_state_with_db();
    let db = state.auth().db.as_ref().expect("db").clone();
    let app = Some(Arc::new(
        takuto_core::github_app::GitHubAppTokenManager::for_tests(7, 9),
    ));
    let resolver = GitAuthResolver::new(db.clone(), app);

    // Seed user + PAT with sign_commits = 1.
    seed_user(&db, "u-mode-b").await;
    seed_pat(&db, "u-mode-b", true, "alice-bot").await;

    assert_eq!(
        resolver.mode_for_user("u-mode-b").await.unwrap(),
        GithubAuthMode::AppPlusPat
    );

    let token = resolver
        .token_for(GitAction::Push, "u-mode-b")
        .await
        .expect("Mode B push must succeed via PAT");
    assert_eq!(token.source, TokenSource::UserPat);
    assert_eq!(token.bearer.expose(), "ghp_alice_pat");
    assert_eq!(token.author_name.as_deref(), Some("alice-bot"));
    assert_eq!(
        token.author_email.as_deref(),
        Some("alice-bot@users.noreply.github.com")
    );
}

// ---------------------------------------------------------------------------
// Mode B — push with attribute_commits=false falls back to App
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appstate_resolver_mode_b_push_without_attribution_picks_app() {
    let state = takuto_web::test_helpers::test_state_with_db();
    let db = state.auth().db.as_ref().expect("db").clone();
    let app = Some(Arc::new(
        takuto_core::github_app::GitHubAppTokenManager::for_tests(11, 22),
    ));
    let resolver = GitAuthResolver::new(db.clone(), app);

    seed_user(&db, "u-mode-b2").await;
    seed_pat(&db, "u-mode-b2", false, "bob").await;

    let err = resolver
        .token_for(GitAction::Push, "u-mode-b2")
        .await
        .expect_err("App branch must be taken when attribute_commits=false");
    assert_eq!(err.code(), "github_app_token_fetch_failed");
}

// ---------------------------------------------------------------------------
// Mode C — PAT only, every action uses the PAT
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appstate_resolver_mode_c_every_action_uses_user_pat() {
    let state = takuto_web::test_helpers::test_state_with_db();
    let db = state.auth().db.as_ref().expect("db").clone();
    // No App configured — pass None to surface Mode C.
    let resolver = GitAuthResolver::new(db.clone(), None);

    seed_user(&db, "u-mode-c").await;
    seed_pat(&db, "u-mode-c", true, "carol").await;

    assert_eq!(
        resolver.mode_for_user("u-mode-c").await.unwrap(),
        GithubAuthMode::PatOnly
    );

    for action in [
        GitAction::Clone,
        GitAction::Fetch,
        GitAction::Push,
        GitAction::PullRequestCreate,
        GitAction::PullRequestComment,
        GitAction::PullRequestReview,
        GitAction::IssueComment,
        GitAction::WebhookEventIngest,
    ] {
        let token = resolver
            .token_for(action, "u-mode-c")
            .await
            .unwrap_or_else(|e| panic!("Mode C {action:?} must succeed; got {e:?}"));
        assert_eq!(
            token.source,
            TokenSource::UserPat,
            "Mode C must route {action:?} to user PAT"
        );
    }
}

// ---------------------------------------------------------------------------
// Missing — every action errors UnauthenticatedGit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appstate_resolver_missing_every_action_errors_unauthenticated() {
    let state = takuto_web::test_helpers::test_state_with_db();
    let db = state.auth().db.as_ref().expect("db").clone();
    let resolver = GitAuthResolver::new(db.clone(), None);

    seed_user(&db, "u-missing").await;

    for action in [
        GitAction::Clone,
        GitAction::Fetch,
        GitAction::Push,
        GitAction::PullRequestCreate,
        GitAction::PullRequestComment,
        GitAction::PullRequestReview,
        GitAction::IssueComment,
        GitAction::WebhookEventIngest,
    ] {
        let err = resolver
            .token_for(action, "u-missing")
            .await
            .expect_err(&format!("Mode Missing must reject {action:?}"));
        assert_eq!(
            err.code(),
            "unauthenticated_git",
            "wrong error for {action:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// AppState.git_auth_resolver: wiring is present and is Some when db is Some.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn appstate_git_auth_resolver_is_wired_when_db_present() {
    let state = takuto_web::test_helpers::test_state_with_db();
    assert!(
        state.auth().git_auth_resolver.is_some(),
        "test_state_with_db must wire a resolver (db is Some)"
    );
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn seed_user(db: &takuto_core::db::Database, user_id: &str) {
    use takuto_core::db::DbValue;
    db.adapter()
        .execute(
            "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(user_id.to_string()),
            ],
        )
        .await
        .unwrap();
}

async fn seed_pat(
    db: &takuto_core::db::Database,
    user_id: &str,
    sign_commits: bool,
    github_login: &str,
) {
    // Upsert via the adapter wrapped in a short transaction (matches the
    // route's atomicity contract).
    let mk = db.master_key().expect("test mk").key.clone();
    let _ = MasterKey::from_bytes([0u8; 32]); // keep import used
    let sealed = seal(&mk, b"ghp_alice_pat").unwrap();
    let adapter = db.adapter();
    let mut tx = adapter.begin().await.unwrap();
    takuto_core::db::github_credentials::upsert(
        &mut tx,
        user_id,
        &sealed,
        github_login,
        "[\"repo\"]",
        sign_commits,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
}
