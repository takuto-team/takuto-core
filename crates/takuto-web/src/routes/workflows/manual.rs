// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Dashboard "Add to Dashboard" / "Start manual workflow" endpoint.

use axum::Extension;
use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use takuto_core::workflow::state::WorkflowState;

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, ConfigState, EngineState};

#[derive(Deserialize)]
pub struct StartManualWorkflowBody {
    pub ticket_key: String,
    pub ticket_summary: String,
    /// Optional ticket description (used when Jira is unavailable and the user pastes the description).
    #[serde(default)]
    pub ticket_description: Option<String>,
    /// Direct URL to the issue in the ticketing system (e.g. GitHub issue `html_url`).
    /// Used so clicking the issue key on the dashboard opens the correct URL for GitHub workflows.
    #[serde(default)]
    pub issue_url: Option<String>,
    /// Id of a `repositories` row the caller has added. When omitted, the
    /// server picks the caller's most-recently-added repo (or rejects
    /// when the caller has none).
    #[serde(default)]
    pub repository_id: Option<String>,
}

#[derive(Serialize)]
pub struct StartManualWorkflowResponse {
    pub workflow_id: String,
    pub ticket_key: String,
}

/// Start a ticket workflow from the dashboard (same pipeline as the poller). Respects **`[general] max_concurrent_manual_workflows`**.
///
/// When Jira is unavailable (`jira_available = false`), `ticket_key` may be empty — a synthetic
/// `MANUAL-{timestamp}` key is generated. The `ticket_description` field is stored on the workflow
/// so the agent prompt can use it.
pub async fn start_manual_workflow(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    State(cfg): State<ConfigState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<StartManualWorkflowBody>,
) -> Result<Json<StartManualWorkflowResponse>, (StatusCode, String)> {
    let jira_on = cfg
        .jira_available
        .load(std::sync::atomic::Ordering::Relaxed);

    let ticket_key = {
        let k = body.ticket_key.trim().to_string();
        if k.is_empty() {
            if jira_on {
                return Err((StatusCode::BAD_REQUEST, "ticket_key is required".into()));
            }
            // Auto-generate a synthetic key when Jira is unavailable.
            format!("MANUAL-{}", chrono::Utc::now().timestamp_millis())
        } else {
            k
        }
    };
    let ticket_summary = {
        let s = body.ticket_summary.trim();
        if s.is_empty() {
            if jira_on {
                ticket_key.clone()
            } else {
                "Manual item".to_string()
            }
        } else {
            s.to_string()
        }
    };

    let (max_manual, max_parallel_per_user) = {
        let cfg_guard = cfg.config.read().await;
        (
            cfg_guard.general.max_concurrent_manual_workflows,
            cfg_guard.general.max_parallel_per_user,
        )
    };

    {
        let wf_arc = engine.engine.workflows_arc();
        let map = wf_arc.read().await;
        if let Some(existing) = map.get(&ticket_key) {
            // Only a `Done` item is "handled in the past" and therefore safe to
            // re-add (a fresh run opens its own PR). Items still on the board —
            // in-progress, `Paused`, `Stopped`, or `Error` — are "Already added"
            // and must not be replaced; the picker disables them and this guard
            // keeps the server authoritative against a direct API call.
            //
            // Orphan rows (user_id = None) carried over from legacy snapshots are
            // invisible to the caller (per-user isolation); allow replacing those
            // regardless of state so they don't become undeletable zombies.
            let is_orphan = existing.user_id.is_none();
            let replaceable = matches!(existing.state, WorkflowState::Done) || is_orphan;
            if !replaceable {
                return Err((
                    StatusCode::CONFLICT,
                    format!("{ticket_key} is already on your board"),
                ));
            }
            tracing::info!(
                ticket = %ticket_key,
                prev_state = %existing.state,
                prev_owner = ?existing.user_id,
                new_owner = %auth.user_id,
                "Replacing re-addable workflow with a fresh add"
            );
        }
    }

    if max_manual > 0 {
        // Count per-user, not global.
        let wf_arc = engine.engine.workflows_arc();
        let map = wf_arc.read().await;
        let n = map
            .values()
            .filter(|w| w.user_id.as_deref() == Some(&auth.user_id))
            .filter(|w| w.started_manually && w.state.occupies_concurrency_slot())
            .count();
        if n >= max_manual as usize {
            return Err((
                StatusCode::CONFLICT,
                format!(
                    "Maximum concurrent manual items ({max_manual}) reached; complete, stop, or delete a manual item first"
                ),
            ));
        }
    }

    // The per-repo `max_parallel_items` ceiling is enforced below, after the
    // selected repository (and its polling settings) is resolved — the cap is a
    // per-repository setting now, scoped per-user when the global
    // `[general] max_parallel_per_user` is set.

    let description = body
        .ticket_description
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    let issue_url = body
        .issue_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from);

    // Resolve the workflow's repository_id. When the body specifies one,
    // validate the caller has it associated; otherwise, default to the
    // most-recently-added repo. Reject when the caller has zero repos.
    let repository_id = if let Some(database) = auth_state.db.as_ref() {
        let user_repos =
            takuto_core::db::repositories::list_for_user(database.adapter(), &auth.user_id)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        if user_repos.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "Add a repository before starting an item.".into(),
            ));
        }
        let chosen = match body
            .repository_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(requested) => match user_repos.iter().find(|r| r.id == requested) {
                Some(repo) => repo.clone(),
                None => {
                    return Err((
                        StatusCode::FORBIDDEN,
                        "You do not have access to that repository".into(),
                    ));
                }
            },
            None => user_repos
                .iter()
                .max_by_key(|r| r.created_at)
                .cloned()
                // `user_repos.is_empty()` is rejected with 400 above, so the
                // iterator is non-empty and `max_by_key` returns `Some`. The
                // `?` keeps the impossible case off the handler's panic path.
                .ok_or_else(|| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        "no repository available after non-empty check".to_string(),
                    )
                })?,
        };

        // Per-repo polling settings for the selected repository drive both the
        // Jira-keys guard (Jira mode) and the per-repo `max_parallel_items` cap.
        let settings = takuto_core::db::user_repo_polling_settings::get(
            database.adapter(),
            &auth.user_id,
            &chosen.name,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .unwrap_or_default();

        // In Jira mode, the selected repository must have at least one Jira
        // project key configured for this user (per-repo, replacing the old
        // global `[jira] project_keys`). Gate on the configured ticketing
        // system, NOT acli auth (`jira_on`) — a REST-only user (acli not logged
        // in) is still in Jira mode and must pass the same guard the picker
        // applies. GitHub / no-ticketing modes skip this.
        if cfg.ticketing_system == takuto_core::config::TicketingSystem::Jira
            && settings.project_keys.is_empty()
        {
            return Err((
                StatusCode::BAD_REQUEST,
                "No Jira project keys configured for this repository".into(),
            ));
        }

        // Per-repo parallel-item ceiling (0 = unlimited), scoped per-user when
        // the global `[general] max_parallel_per_user` is set. Reuses the
        // existing 409 → toast path on the dashboard.
        if settings.max_parallel_items > 0 {
            let scope = if max_parallel_per_user {
                Some(auth.user_id.as_str())
            } else {
                None
            };
            let in_use = engine.engine.active_item_count(scope).await;
            if in_use >= settings.max_parallel_items as usize {
                return Err((
                    StatusCode::CONFLICT,
                    format!(
                        "Maximum parallel items ({}) reached for this repository; complete, stop, or delete an item first",
                        settings.max_parallel_items
                    ),
                ));
            }
        }

        Some(chosen.id)
    } else {
        // No DB attached (legacy test paths). Fall through with None — the
        // engine will derive workspace_name from cfg.git.repo_path.
        None
    };

    let workflow_id = engine
        .engine
        .add_to_dashboard(
            ticket_key.clone(),
            ticket_summary,
            true,
            description,
            issue_url,
            Some(auth.user_id),
            repository_id,
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(StartManualWorkflowResponse {
        workflow_id,
        ticket_key,
    }))
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::server::build_router;
    use crate::state::AppState;
    use crate::test_helpers::{TEST_ORIGIN, register_and_login, test_state_with_db};

    async fn user_id_for(state: &AppState, username: &str) -> String {
        let db = state.auth.db.as_ref().expect("db");
        takuto_core::db::users::get_user_by_username(db.adapter(), username)
            .await
            .expect("query user")
            .expect("user exists")
            .id
    }

    /// Register a repository and associate it with `user_id`. Returns the
    /// repository id.
    async fn seed_repo(state: &AppState, user_id: &str, name: &str) -> String {
        let db = state.auth.db.as_ref().expect("db");
        let id = takuto_core::db::repositories::upsert(
            db.adapter(),
            name,
            None,
            &format!("/workspaces/{name}"),
            "main",
            Some(user_id),
        )
        .await
        .expect("upsert repo");
        takuto_core::db::repositories::add_for_user(db.adapter(), user_id, &id)
            .await
            .expect("add repo for user");
        id
    }

    /// Seed a slot-occupying (Pending) workflow owned by `owner_id`.
    async fn seed_item(state: &AppState, ticket_key: &str, owner_id: &str) {
        state
            .engine
            .engine
            .start_workflow(
                ticket_key.to_string(),
                "seeded".to_string(),
                false,
                None,
                None,
                Some(owner_id.to_string()),
                None,
            )
            .await
            .expect("seed start_workflow");
    }

    fn start_manual_request(cookie: &str) -> Request<Body> {
        Request::post("/api/workflows/start-manual")
            .header("Content-Type", "application/json")
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", cookie)
            .body(Body::from(
                r#"{"ticket_key":"NEW-1","ticket_summary":"new item"}"#,
            ))
            .unwrap()
    }

    fn start_manual_request_for(cookie: &str, ticket_key: &str) -> Request<Body> {
        let body = format!(r#"{{"ticket_key":"{ticket_key}","ticket_summary":"re-add"}}"#);
        Request::post("/api/workflows/start-manual")
            .header("Content-Type", "application/json")
            .header("Origin", TEST_ORIGIN)
            .header("Cookie", cookie)
            .body(Body::from(body))
            .unwrap()
    }

    async fn set_state(
        state: &AppState,
        ticket_key: &str,
        s: takuto_core::workflow::state::WorkflowState,
    ) {
        let arc = state.engine.engine.workflows_arc();
        let mut map = arc.write().await;
        map.get_mut(ticket_key).expect("seeded item present").state = s;
    }

    /// Seed the selected repo's per-repo polling settings with a parallel-item
    /// cap (the cap is per-repository now).
    async fn seed_repo_max_parallel(state: &AppState, user_id: &str, repo_name: &str, cap: u32) {
        let db = state.auth.db.as_ref().expect("db");
        let settings = takuto_core::db::user_repo_polling_settings::RepoPollingSettings {
            max_parallel_items: cap,
            ..takuto_core::db::user_repo_polling_settings::RepoPollingSettings::default()
        };
        takuto_core::db::user_repo_polling_settings::set(
            db.adapter(),
            user_id,
            repo_name,
            &settings,
        )
        .await
        .expect("seed settings");
    }

    #[tokio::test]
    async fn manual_start_409_when_global_parallel_cap_reached() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let admin_id = user_id_for(&state, "admin").await;

        // max_parallel_per_user defaults false (global scope). Seeded repo with
        // a per-repo cap of 1, one active item anywhere → next start hits 409.
        seed_repo(&state, &admin_id, "takuto-core").await;
        seed_repo_max_parallel(&state, &admin_id, "takuto-core", 1).await;
        seed_item(&state, "SEED-1", &admin_id).await;

        let app = build_router(state);
        let resp = app.oneshot(start_manual_request(&cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn manual_start_409_when_per_user_parallel_cap_reached() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let admin_id = user_id_for(&state, "admin").await;

        {
            let mut cfg = state.config.config.write().await;
            cfg.general.max_parallel_per_user = true;
        }
        seed_repo(&state, &admin_id, "takuto-core").await;
        seed_repo_max_parallel(&state, &admin_id, "takuto-core", 1).await;
        // An item owned by the caller (admin) fills the caller's single slot.
        seed_item(&state, "SEED-1", &admin_id).await;

        let app = build_router(state);
        let resp = app.oneshot(start_manual_request(&cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn manual_start_per_user_cap_ignores_other_users_items() {
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let admin_id = user_id_for(&state, "admin").await;

        // A second user owns the only slot-occupying item.
        let other_id = "other-user";
        {
            let db = state.auth.db.as_ref().unwrap();
            db.adapter()
                .execute(
                    "INSERT INTO users (id, username, role) VALUES (?, 'other', 'user')",
                    vec![other_id.into()],
                )
                .await
                .expect("seed other user");
        }
        {
            let mut cfg = state.config.config.write().await;
            cfg.general.max_parallel_per_user = true;
        }
        seed_repo(&state, &admin_id, "takuto-core").await;
        seed_repo_max_parallel(&state, &admin_id, "takuto-core", 1).await;
        seed_item(&state, "SEED-1", other_id).await;

        // The admin has zero items, so the per-user parallel cap does NOT fire;
        // with a repo configured (Jira off in the harness) the request succeeds
        // (200), proving the cap counted only the caller's items.
        let app = build_router(state);
        let resp = app.oneshot(start_manual_request(&cookie)).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn manual_start_409_when_item_already_on_board() {
        // A ticket the caller currently has on the board in a non-Done state
        // ("Already added") must be rejected — even Stopped/Error, which used to
        // be replaceable.
        use takuto_core::workflow::state::WorkflowState;
        for blocking in [
            WorkflowState::Stopped,
            WorkflowState::Error {
                source_state: Box::new(WorkflowState::Reviewing),
                message: "boom".into(),
            },
            WorkflowState::Paused {
                source_state: Box::new(WorkflowState::AddressingTicket { pass: 1 }),
            },
        ] {
            let state = test_state_with_db();
            let cookie = register_and_login(&state).await;
            let admin_id = user_id_for(&state, "admin").await;
            seed_item(&state, "DUP-1", &admin_id).await;
            set_state(&state, "DUP-1", blocking).await;

            let app = build_router(state);
            let resp = app
                .oneshot(start_manual_request_for(&cookie, "DUP-1"))
                .await
                .unwrap();
            assert_eq!(
                resp.status(),
                StatusCode::CONFLICT,
                "a non-Done board item must block re-add"
            );
        }
    }

    #[tokio::test]
    async fn manual_start_400_when_jira_mode_and_repo_has_no_keys() {
        let mut state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let admin_id = user_id_for(&state, "admin").await;
        let repo_id = seed_repo(&state, &admin_id, "takuto-core").await;
        // Jira mode (NOT acli auth) is what gates the no-keys guard now.
        state.config_mut().ticketing_system = takuto_core::config::TicketingSystem::Jira;

        let body = format!(
            r#"{{"ticket_key":"PROJ-1","ticket_summary":"x","repository_id":"{repo_id}"}}"#
        );
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/workflows/start-manual")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn manual_start_succeeds_when_jira_mode_and_repo_has_keys() {
        let mut state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let admin_id = user_id_for(&state, "admin").await;
        let repo_id = seed_repo(&state, &admin_id, "takuto-core").await;
        let settings = takuto_core::db::user_repo_polling_settings::RepoPollingSettings {
            project_keys: vec!["PROJ".to_string()],
            ..takuto_core::db::user_repo_polling_settings::RepoPollingSettings::default()
        };
        takuto_core::db::user_repo_polling_settings::set(
            state.auth.db.as_ref().unwrap().adapter(),
            &admin_id,
            "takuto-core",
            &settings,
        )
        .await
        .expect("seed settings");
        state.config_mut().ticketing_system = takuto_core::config::TicketingSystem::Jira;

        let body = format!(
            r#"{{"ticket_key":"PROJ-1","ticket_summary":"x","repository_id":"{repo_id}"}}"#
        );
        let app = build_router(state);
        let resp = app
            .oneshot(
                Request::post("/api/workflows/start-manual")
                    .header("Content-Type", "application/json")
                    .header("Origin", TEST_ORIGIN)
                    .header("Cookie", &cookie)
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn manual_start_allows_re_add_of_done_item() {
        // A Done item is past work and re-addable: the request must pass the
        // duplicate guard and only fail later for having no repo (400).
        let state = test_state_with_db();
        let cookie = register_and_login(&state).await;
        let admin_id = user_id_for(&state, "admin").await;
        seed_item(&state, "DONE-1", &admin_id).await;
        set_state(
            &state,
            "DONE-1",
            takuto_core::workflow::state::WorkflowState::Done,
        )
        .await;

        let app = build_router(state);
        let resp = app
            .oneshot(start_manual_request_for(&cookie, "DONE-1"))
            .await
            .unwrap();
        assert_eq!(
            resp.status(),
            StatusCode::BAD_REQUEST,
            "Done re-add must clear the duplicate guard (then 400 for no repo)"
        );
    }
}
