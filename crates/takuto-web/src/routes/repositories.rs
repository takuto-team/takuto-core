// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! User-facing repository endpoints.
//!
//! - `GET /api/repositories` — list MY repos.
//! - `GET /api/repositories/_available` — registered repos I haven't added yet.
//! - `POST /api/repositories` — clone-if-needed + add to my dashboard.
//! - `DELETE /api/repositories/{id}` — remove from my dashboard, always-purge
//!   on last-user (decision #2).
//!
//! All endpoints are authenticated; none are admin-gated except the optional
//! `force_purge` flag on DELETE.

use std::sync::atomic::Ordering;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use takuto_core::db::models::UserRole;
use takuto_core::workflow::snapshot::WORKSPACES_DIR;

use crate::auth::AuthenticatedUser;
use crate::routes::repos::{CloneGuard, do_clone, read_git_remote_url, sanitize_clone_error};
use crate::state::{AuthState, EngineState};

/// URL validation.
///
/// Equivalent to the regex `^https://github\.com/[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$`.
/// Implemented as a hand-rolled matcher in [`validate_repo_url`] to avoid
/// pulling in the `regex` crate. The function additionally rejects embedded
/// credentials (`user:pass@`), query strings, fragments, and `..` segments.
const MAX_URL_LEN: usize = 2048;
const GITHUB_PREFIX: &str = "https://github.com/";

fn is_valid_segment_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'
}

// ── Wire types ──────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct RepositoryDto {
    pub id: String,
    pub name: String,
    pub repo_url: Option<String>,
    pub local_path: String,
    pub default_branch: String,
    /// Present only on `GET /api/repositories`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added_at: Option<i64>,
    /// Number of OTHER users associated with this repository (excludes the
    /// caller). Drives the UI's "last user" warning + purge consequences.
    /// Present only on `GET /api/repositories`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub co_users_count: Option<i64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddRepositoryBody {
    #[serde(default)]
    pub repository_id: Option<String>,
    #[serde(default)]
    pub repo_url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct DeleteRepositoryBody {
    #[serde(default)]
    pub force_purge: bool,
}

#[derive(Serialize)]
pub struct DeleteRefusedResponse {
    pub error: &'static str,
    pub blocking_workflows: Vec<BlockingWorkflowEntry>,
}

#[derive(Serialize)]
pub struct BlockingWorkflowEntry {
    pub ticket_key: String,
    pub user_id: String,
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn db_error(e: takuto_core::error::TakutoError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

fn require_db(auth_state: &AuthState) -> Result<takuto_core::db::Database, (StatusCode, String)> {
    auth_state
        .db
        .as_ref()
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            "database unavailable".into(),
        ))
        .cloned()
}

/// Validate a GitHub repo URL.
fn validate_repo_url(url: &str) -> Result<(String, String), (StatusCode, String)> {
    if url.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "repo_url cannot be empty".into()));
    }
    if url.len() > MAX_URL_LEN {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("repo_url exceeds {MAX_URL_LEN} characters"),
        ));
    }
    if url.contains('@') {
        return Err((
            StatusCode::BAD_REQUEST,
            "repo_url must not contain credentials".into(),
        ));
    }
    if url.contains('?') || url.contains('#') {
        return Err((
            StatusCode::BAD_REQUEST,
            "repo_url must not contain query strings or fragments".into(),
        ));
    }
    if url.contains("..") {
        return Err((
            StatusCode::BAD_REQUEST,
            "repo_url must not contain `..` path segments".into(),
        ));
    }
    let rest = url.strip_prefix(GITHUB_PREFIX).ok_or((
        StatusCode::BAD_REQUEST,
        "repo_url must be of the form https://github.com/owner/repo".into(),
    ))?;
    let parts: Vec<&str> = rest.split('/').collect();
    if parts.len() != 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            "repo_url must have exactly owner/repo after the host".into(),
        ));
    }
    for seg in &parts {
        if seg.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                "repo_url path segments must be non-empty".into(),
            ));
        }
        if !seg.chars().all(is_valid_segment_char) {
            return Err((
                StatusCode::BAD_REQUEST,
                "repo_url path segments may contain only [A-Za-z0-9._-]".into(),
            ));
        }
    }
    let owner = parts[0].to_string();
    let repo = parts[1].trim_end_matches(".git").to_string();
    if repo.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "repo segment must be non-empty after stripping `.git`".into(),
        ));
    }
    Ok((owner, repo))
}

/// Determine an available `local_path` under `/workspaces/<derived>/`.
/// Suffixes `-2`, `-3`, … when the base directory already exists with a
/// different `repo_url` (path collision resolver).
fn pick_clone_target(base_name: &str) -> std::path::PathBuf {
    let workspaces = std::path::Path::new(WORKSPACES_DIR);
    let primary = workspaces.join(base_name);
    if !primary.exists() {
        return primary;
    }
    for suffix in 2..1000 {
        let candidate = workspaces.join(format!("{base_name}-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    // Improbable; fall back to primary and let the clone fail clearly.
    primary
}

fn row_to_dto(
    row: takuto_core::db::repositories::RepositoryRow,
    added_at: Option<i64>,
    co_users_count: Option<i64>,
) -> RepositoryDto {
    RepositoryDto {
        id: row.id,
        name: row.name,
        repo_url: row.repo_url,
        local_path: row.local_path,
        default_branch: row.default_branch,
        added_at,
        co_users_count,
    }
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// `GET /api/repositories` — list MY repositories with `added_at` + per-row
/// co-user counts so the UI can warn the caller before a purge.
pub async fn list_mine(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<RepositoryDto>>, (StatusCode, String)> {
    let db = require_db(&auth_state)?;
    let adapter = db.adapter();
    let user_id = auth.user_id.clone();
    let rows = takuto_core::db::repositories::list_for_user(adapter, &user_id)
        .await
        .map_err(db_error)?;
    let mut dtos = Vec::with_capacity(rows.len());
    for row in rows {
        let added_at = adapter
            .query_optional(
                "SELECT added_at FROM user_repositories WHERE user_id = ? AND repository_id = ?",
                vec![
                    takuto_core::db::DbValue::Text(user_id.clone()),
                    takuto_core::db::DbValue::Text(row.id.clone()),
                ],
            )
            .await
            .map_err(|e| db_error(e.into()))?
            .map(|r| r.get_i64(0).unwrap_or(0))
            .unwrap_or(0);
        let total = adapter
            .query_one(
                "SELECT COUNT(*) FROM user_repositories WHERE repository_id = ?",
                vec![takuto_core::db::DbValue::Text(row.id.clone())],
            )
            .await
            .map_err(|e| db_error(e.into()))?
            .get_i64(0)
            .unwrap_or(1);
        let co = (total - 1).max(0);
        dtos.push(row_to_dto(row, Some(added_at), Some(co)));
    }

    Ok(Json(dtos))
}

/// `GET /api/repositories/_available` — registered repos the caller has not
/// yet added. Same shape as `GET /api/repositories` minus `added_at`.
pub async fn list_available(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<RepositoryDto>>, (StatusCode, String)> {
    let db = require_db(&auth_state)?;
    let user_id = auth.user_id.clone();
    let rows = takuto_core::db::repositories::list_available_for_user(db.adapter(), &user_id)
        .await
        .map_err(db_error)?;

    let dtos = rows
        .into_iter()
        .map(|r| row_to_dto(r, None, None))
        .collect();
    Ok(Json(dtos))
}

/// `POST /api/repositories` — dispatch on `{repository_id}` vs `{repo_url}`.
pub async fn add(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Json(body): Json<AddRepositoryBody>,
) -> Result<(StatusCode, Json<RepositoryDto>), (StatusCode, String)> {
    let db = require_db(&auth_state)?;

    match (body.repository_id, body.repo_url) {
        (Some(_), Some(_)) => Err((
            StatusCode::BAD_REQUEST,
            "Provide either repository_id or repo_url, not both".into(),
        )),
        (None, None) => Err((
            StatusCode::BAD_REQUEST,
            "Provide either repository_id or repo_url".into(),
        )),
        (Some(repo_id), None) => add_existing(&db, &auth, &repo_id).await,
        (None, Some(repo_url)) => add_via_clone(&engine, &auth_state, &db, &auth, &repo_url).await,
    }
}

async fn add_existing(
    db: &takuto_core::db::Database,
    auth: &AuthenticatedUser,
    repo_id: &str,
) -> Result<(StatusCode, Json<RepositoryDto>), (StatusCode, String)> {
    let adapter = db.adapter();
    let row = match takuto_core::db::repositories::get(adapter, repo_id)
        .await
        .map_err(db_error)?
    {
        None => {
            return Err((StatusCode::NOT_FOUND, "repository not found".to_string()));
        }
        Some(r) => r,
    };
    let inserted = takuto_core::db::repositories::add_for_user(adapter, &auth.user_id, &row.id)
        .await
        .map_err(db_error)?;

    info!(
        actor_user_id = %auth.user_id,
        repository_id = %row.id,
        repository_name = %row.name,
        action = "add",
        repo_url = ?row.repo_url,
        was_already_added = !inserted,
        "user added existing repository to dashboard"
    );

    Ok((StatusCode::OK, Json(row_to_dto(row, None, None))))
}

async fn add_via_clone(
    engine: &EngineState,
    auth_state: &AuthState,
    db: &takuto_core::db::Database,
    auth: &AuthenticatedUser,
    repo_url: &str,
) -> Result<(StatusCode, Json<RepositoryDto>), (StatusCode, String)> {
    // 1. Validate URL.
    let (owner, repo_name) = validate_repo_url(repo_url)?;
    let full_name = format!("{owner}/{repo_name}");
    let normalized_url = format!("https://github.com/{full_name}");

    // 2. Acquire process-level clone lock.
    if engine
        .clone_in_progress
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err((
            StatusCode::CONFLICT,
            "clone already in progress; retry shortly".to_string(),
        ));
    }
    // From here on, every early-return must release via `_guard`.
    let _guard = CloneGuard(engine.clone_in_progress.clone());

    // 3. Look up an existing `repositories` row by repo_url. If found,
    //    just associate (idempotent) and return 200.
    {
        let adapter = db.adapter();
        let row = adapter
            .query_optional(
                "SELECT id, name, repo_url, local_path, default_branch, created_at, created_by \
                 FROM repositories WHERE repo_url = ? LIMIT 1",
                vec![takuto_core::db::DbValue::Text(normalized_url.clone())],
            )
            .await
            .map_err(|e| db_error(e.into()))?;
        let existing: Option<takuto_core::db::repositories::RepositoryRow> = match row {
            None => None,
            Some(r) => Some(takuto_core::db::repositories::RepositoryRow {
                id: r.get_text(0).map_err(|e| db_error(e.into()))?,
                name: r.get_text(1).map_err(|e| db_error(e.into()))?,
                repo_url: r.get_text_opt(2).map_err(|e| db_error(e.into()))?,
                local_path: r.get_text(3).map_err(|e| db_error(e.into()))?,
                default_branch: r.get_text(4).map_err(|e| db_error(e.into()))?,
                created_at: r.get_i64(5).map_err(|e| db_error(e.into()))?,
                created_by: r.get_text_opt(6).map_err(|e| db_error(e.into()))?,
            }),
        };

        if let Some(row) = existing {
            takuto_core::db::repositories::add_for_user(adapter, &auth.user_id, &row.id)
                .await
                .map_err(db_error)?;

            info!(
                actor_user_id = %auth.user_id,
                repository_id = %row.id,
                repository_name = %row.name,
                action = "add",
                repo_url = ?row.repo_url,
                clone_skipped = true,
                "user added existing repository via repo_url (no clone needed)"
            );
            return Ok((StatusCode::OK, Json(row_to_dto(row, None, None))));
        }
    }

    // 4. Pick the clone target and perform the clone.
    let clone_target = pick_clone_target(&repo_name);
    let token_cwd = std::path::Path::new(WORKSPACES_DIR);

    // Pass the authenticated caller so do_clone can ask the
    // GitAuthResolver to pick App vs user PAT per the §4.2 matrix.
    let clone_result = do_clone(
        engine,
        auth_state,
        &full_name,
        token_cwd,
        &clone_target,
        Some(&auth.user_id),
    )
    .await;
    if let Err(err) = clone_result {
        let stderr = err.to_string();
        warn!(
            actor_user_id = %auth.user_id,
            repo_url = %normalized_url,
            error = %stderr,
            "clone failed"
        );
        // GitHub App permission-denied UX.
        if stderr.contains("Resource not accessible by integration") {
            return Err((
                StatusCode::BAD_GATEWAY,
                format!(
                    "GitHub App installation cannot access {owner}/{repo_name}. Ask an admin to grant the App access to the repo."
                ),
            ));
        }
        return Err((StatusCode::BAD_GATEWAY, sanitize_clone_error(&stderr)));
    }

    // 5. Resolve default branch (best-effort).
    let default_branch = read_default_branch(&clone_target).unwrap_or_else(|| "main".to_string());
    let local_path = clone_target.to_string_lossy().into_owned();
    let actual_url = read_git_remote_url(&clone_target).unwrap_or_else(|| normalized_url.clone());
    let actor_id = auth.user_id.clone();
    let name_owned = repo_name.clone();
    let local_path_owned = local_path.clone();
    let default_branch_owned = default_branch.clone();
    let actual_url_owned = actual_url.clone();

    let adapter = db.adapter();
    let repo_id = takuto_core::db::repositories::upsert(
        adapter,
        &name_owned,
        Some(&actual_url_owned),
        &local_path_owned,
        &default_branch_owned,
        Some(&actor_id),
    )
    .await
    .map_err(db_error)?;
    let _inserted = takuto_core::db::repositories::add_for_user(adapter, &actor_id, &repo_id)
        .await
        .map_err(db_error)?;
    // Re-read to get the canonical row (in case of a racing peer).
    // The three calls above ran sequentially on the adapter; the upsert
    // either committed our new row or observed the racing peer's, and the
    // follow-up get() returns whichever the DB now has.
    let row = takuto_core::db::repositories::get(adapter, &repo_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "repository row missing immediately after upsert".to_string(),
            )
        })?;

    info!(
        actor_user_id = %auth.user_id,
        repository_id = %row.id,
        repository_name = %row.name,
        action = "clone",
        repo_url = ?row.repo_url,
        local_path = %row.local_path,
        "repository cloned and registered"
    );
    info!(
        actor_user_id = %auth.user_id,
        repository_id = %row.id,
        repository_name = %row.name,
        action = "add",
        "user associated newly-cloned repository"
    );

    Ok((StatusCode::CREATED, Json(row_to_dto(row, None, None))))
}

/// `DELETE /api/repositories/{id}` — remove the caller's association.
/// Always-purge behaviour: drop the on-disk clone when the caller was the
/// last user.
pub async fn delete(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(repository_id): Path<String>,
    body: Option<Json<DeleteRepositoryBody>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let force_purge = body.map(|b| b.0.force_purge).unwrap_or(false);
    if force_purge && !matches!(auth.role, UserRole::Admin) {
        return Err((StatusCode::FORBIDDEN, "force_purge requires admin".into()));
    }

    let db = require_db(&auth_state)?;

    // 1. Active-workflow check, scoped correctly:
    //    - force_purge (admin): refuse if ANY user has an active workflow on
    //      the repo — admin is destroying it for everyone, so every user's
    //      worktrees would orphan.
    //    - non-force-purge: refuse only if the CALLER has an active workflow
    //      on the repo. Other users' workflows are irrelevant — the caller is
    //      just dropping their own association. Other users keep theirs and
    //      their worktrees stay valid.
    let adapter = db.adapter();
    let all_blockers =
        takuto_core::db::repositories::repository_has_active_workflow(adapter, &repository_id)
            .await
            .map_err(db_error)?;

    let relevant_blockers: Vec<(String, String)> = if force_purge {
        all_blockers
    } else {
        all_blockers
            .into_iter()
            .filter(|(_, uid)| uid == &auth.user_id)
            .collect()
    };

    if !relevant_blockers.is_empty() {
        let entries: Vec<BlockingWorkflowEntry> = relevant_blockers
            .into_iter()
            .map(|(ticket_key, user_id)| BlockingWorkflowEntry {
                ticket_key,
                user_id,
            })
            .collect();
        let error_msg = if force_purge {
            "active workflows reference this repository — stop or finish them first"
        } else {
            "you have active workflows on this repository — stop or finish them before removing it from your dashboard"
        };
        let body = serde_json::to_string(&DeleteRefusedResponse {
            error: error_msg,
            blocking_workflows: entries,
        })
        .unwrap_or_default();
        return Err((StatusCode::CONFLICT, body));
    }

    // 2. Try to remove the caller's association. 404 if no row.
    if !force_purge {
        let removed =
            takuto_core::db::repositories::remove_for_user(adapter, &auth.user_id, &repository_id)
                .await
                .map_err(db_error)?;

        if !removed {
            return Err((
                StatusCode::NOT_FOUND,
                "repository not in your dashboard".into(),
            ));
        }
    }

    // 3. Decide whether to purge (last user OR force_purge).
    let row = takuto_core::db::repositories::get(adapter, &repository_id)
        .await
        .map_err(db_error)?;
    let purge_info = match row {
        Some(r) => {
            let rows = adapter
                .query_all(
                    "SELECT user_id FROM user_repositories WHERE repository_id = ?",
                    vec![takuto_core::db::DbValue::Text(repository_id.clone())],
                )
                .await
                .map_err(|e| db_error(e.into()))?;
            let mut affected_users: Vec<String> =
                rows.iter().filter_map(|row| row.get_text(0).ok()).collect();
            affected_users.sort();
            Some((r, affected_users))
        }
        None => None,
    };

    let Some((row, remaining_users)) = purge_info else {
        // No row left; caller already had it removed via cascade. 204.
        return Ok(StatusCode::NO_CONTENT);
    };

    let should_purge = force_purge || remaining_users.is_empty();
    if !should_purge {
        // Other users still have this repo; we're done.
        info!(
            actor_user_id = %auth.user_id,
            repository_id = %row.id,
            repository_name = %row.name,
            action = "remove",
            repo_url = ?row.repo_url,
            "user removed repository association (no purge)"
        );
        return Ok(StatusCode::NO_CONTENT);
    }

    // 4. Purge: delete the DB row (cascades to user_repositories) then remove
    //    the on-disk clone.
    if force_purge {
        for user in &remaining_users {
            info!(
                actor_user_id = %auth.user_id,
                affected_user_id = %user,
                repository_id = %row.id,
                repository_name = %row.name,
                action = "force_purge_drop_association",
                "admin force_purge removed association from another user"
            );
        }
    }

    let deleted = takuto_core::db::repositories::delete(adapter, &row.id)
        .await
        .map_err(db_error)?;

    if deleted {
        // Remove on-disk clone. Best-effort — log on failure.
        let local_path = std::path::Path::new(&row.local_path).to_path_buf();
        let local_path_owned = local_path.clone();
        let rm_result =
            tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&local_path_owned))
                .await
                .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?;
        match rm_result {
            Ok(()) => info!(
                actor_user_id = %auth.user_id,
                repository_id = %row.id,
                repository_name = %row.name,
                action = "purge",
                local_path = %row.local_path,
                "purged on-disk clone after last-user removal"
            ),
            Err(e) => warn!(
                actor_user_id = %auth.user_id,
                repository_id = %row.id,
                repository_name = %row.name,
                error = %e,
                local_path = %row.local_path,
                "failed to purge on-disk clone; DB row is gone"
            ),
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

// ── default-branch detection ─────────────────────────────────────────────────

/// Read the default branch from a freshly cloned repo by shelling out to
/// `git symbolic-ref refs/remotes/origin/HEAD`. Returns `None` on any error
/// (e.g. tests, missing git, no origin) so the caller can fall back to "main".
fn read_default_branch(repo_path: &std::path::Path) -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["symbolic-ref", "refs/remotes/origin/HEAD"])
        .current_dir(repo_path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&output.stdout);
    let trimmed = s.trim();
    // Expected form: "refs/remotes/origin/<branch>"
    trimmed
        .strip_prefix("refs/remotes/origin/")
        .map(|b| b.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_repo_url_accepts_canonical_form() {
        let (owner, repo) = validate_repo_url("https://github.com/owner/repo").unwrap();
        assert_eq!(owner, "owner");
        assert_eq!(repo, "repo");
    }

    #[test]
    fn validate_repo_url_strips_git_suffix_internally() {
        // The regex disallows ".git" in the path because dots are allowed but
        // the path must contain exactly two segments. ".git" suffix produces a
        // valid match — the suffix is stripped when extracting `repo`.
        let (_, repo) = validate_repo_url("https://github.com/owner/repo.git").unwrap();
        assert_eq!(repo, "repo");
    }

    #[test]
    fn validate_repo_url_rejects_credentials() {
        let err = validate_repo_url("https://user:pass@github.com/owner/repo");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_query_strings() {
        let err = validate_repo_url("https://github.com/owner/repo?ref=main");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_fragments() {
        let err = validate_repo_url("https://github.com/owner/repo#readme");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_path_traversal() {
        let err = validate_repo_url("https://github.com/owner/..");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_non_github_host() {
        let err = validate_repo_url("https://gitlab.com/owner/repo");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_extra_path_segments() {
        let err = validate_repo_url("https://github.com/owner/repo/tree/main");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_empty() {
        let err = validate_repo_url("");
        assert!(err.is_err());
    }

    #[test]
    fn validate_repo_url_rejects_oversize() {
        let big = format!("https://github.com/owner/{}", "a".repeat(MAX_URL_LEN + 10));
        let err = validate_repo_url(&big);
        assert!(err.is_err());
    }
}
