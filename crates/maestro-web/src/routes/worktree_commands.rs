// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user REST endpoints for worktree init + run commands (plan-09 Step 5).
//!
//! Mounted under `/api/worktree-commands/*` (no admin prefix). Every handler
//! reads `Extension<AuthenticatedUser>` and operates on `auth.user_id` ONLY —
//! the URL never carries a `user_id` and admins have no special path to
//! another user's data. The CSRF + session middleware mounted in `server.rs`
//! cover the rest; these handlers don't re-implement auth.
//!
//! Endpoints:
//!
//! | Method | Path                                       |
//! |--------|--------------------------------------------|
//! | GET    | `/api/worktree-commands`                   |
//! | GET    | `/api/worktree-commands/{workspace}`       |
//! | PUT    | `/api/worktree-commands/{workspace}`       |
//! | DELETE | `/api/worktree-commands/{workspace}`       |
//! | GET    | `/api/worktree-commands/_workspaces`       |
//!
//! Validation (PUT body):
//! - `init_commands`: ≤50 items, each ≤2000 chars after trim, non-empty,
//!   no NUL bytes.
//! - `run_commands`: ≤50 items, each `name` ≤100 chars after trim non-empty,
//!   each `command` ≤2000 chars after trim non-empty, no NUL bytes,
//!   duplicate `name`s within the list are rejected.
//! - Whole body capped at 1 MiB defense-in-depth.
//!
//! Audit log: PUT and DELETE emit a `tracing::info!` line with the user_id,
//! workspace_name, action, command counts, and SHA-256 hashes of each JSON.
//! The optional snippet is scrubbed of `*_TOKEN=…` style assignments; the DB
//! stores commands verbatim.

use std::collections::{BTreeSet, HashSet};

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use maestro_core::db::repositories;
use maestro_core::db::user_worktree_commands::{self, RunCommand, UserWorktreeCommandsRow};

use crate::auth::AuthenticatedUser;
use crate::state::AuthState;

// ---------------------------------------------------------------------------
// Constants — validation limits.
// ---------------------------------------------------------------------------

/// Hard ceiling on the number of commands per kind (init or run).
const MAX_COMMANDS: usize = 50;

/// Hard ceiling on a single command string's length (after `trim`). Multi-line
/// shell scripts inside a single command are allowed — newlines do NOT count
/// as command separators.
const MAX_COMMAND_LEN: usize = 2000;

/// Hard ceiling on a run-command `name`. Short labels: "Dashboard UI",
/// "Storybook" — 100 is more than enough.
const MAX_NAME_LEN: usize = 100;

/// Defense-in-depth ceiling on the serialized JSON body size. axum's default
/// JSON limit (currently 2 MiB) covers this for normal requests, but we
/// enforce it ourselves so the policy stays local.
const MAX_BODY_BYTES: usize = 1_024 * 1_024; // 1 MiB

// ---------------------------------------------------------------------------
// Wire types.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct UserCommandsEntry {
    pub workspace_name: String,
    pub init_commands: Vec<String>,
    pub run_commands: Vec<RunCommand>,
    pub updated_at: i64,
}

impl From<UserWorktreeCommandsRow> for UserCommandsEntry {
    fn from(row: UserWorktreeCommandsRow) -> Self {
        UserCommandsEntry {
            workspace_name: row.workspace_name,
            init_commands: row.init_commands,
            run_commands: row.run_commands,
            updated_at: row.updated_at,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutBody {
    #[serde(default)]
    pub init_commands: Vec<String>,
    #[serde(default)]
    pub run_commands: Vec<RunCommand>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceWithHasCommands {
    pub name: String,
    /// `repo_url` from the `repositories` row when available — the old field
    /// name `html_url` is kept on the wire so the UI doesn't need to change.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    pub has_my_commands: bool,
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// SHA-256 hex digest. Empty input → empty string.
fn sha256_hex(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

fn init_json_bytes(commands: &[String]) -> Vec<u8> {
    serde_json::to_vec(commands).unwrap_or_default()
}

fn run_json_bytes(commands: &[RunCommand]) -> Vec<u8> {
    serde_json::to_vec(commands).unwrap_or_default()
}

/// Scrub `*_TOKEN=…` style assignments from a command snippet so they don't
/// leak into the audit log. The DB-stored copy is untouched. Matches the
/// regex `\b[A-Z_]*TOKEN\b\s*=\s*\S+` → `<NAME>=***`.
fn scrub_secrets_for_log(snippets: &[String]) -> Vec<String> {
    snippets
        .iter()
        .map(|cmd| {
            let mut out = String::with_capacity(cmd.len());
            let bytes = cmd.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let id_start = i;
                let mut j = i;
                while j < bytes.len() && (bytes[j].is_ascii_uppercase() || bytes[j] == b'_') {
                    j += 1;
                }
                let is_word_boundary =
                    id_start == 0 || !is_ident_byte(bytes[id_start.saturating_sub(1)]);
                let ident = &bytes[id_start..j];
                let ends_with_token = ident.ends_with(b"TOKEN");
                if j > id_start && is_word_boundary && ends_with_token {
                    let mut k = j;
                    while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'=' {
                        k += 1;
                        while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                            k += 1;
                        }
                        let val_start = k;
                        while k < bytes.len() && !bytes[k].is_ascii_whitespace() {
                            k += 1;
                        }
                        if k > val_start {
                            out.push_str(std::str::from_utf8(ident).unwrap_or(""));
                            out.push_str("=***");
                            i = k;
                            continue;
                        }
                    }
                }
                let ch_end = next_char_boundary(cmd, i);
                out.push_str(&cmd[i..ch_end]);
                i = ch_end;
            }
            out
        })
        .collect()
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn next_char_boundary(s: &str, i: usize) -> usize {
    let mut j = i + 1;
    while j < s.len() && !s.is_char_boundary(j) {
        j += 1;
    }
    j
}

/// Validate the workspace name segment from the URL. Rejects path traversal
/// and empty names; mirrors `routes/repos.rs::switch_workspace`.
fn validate_workspace_name(name: &str) -> Result<(), (StatusCode, String)> {
    if name.is_empty()
        || name.contains('/')
        || name.contains("..")
        || name.starts_with('.')
        || name.contains('\0')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid workspace name".to_string(),
        ));
    }
    Ok(())
}

/// Validate the init-commands list.
fn validate_init_commands(commands: &[String]) -> Result<(), (StatusCode, String)> {
    if commands.len() > MAX_COMMANDS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Too many init commands: {} > {MAX_COMMANDS}",
                commands.len()
            ),
        ));
    }
    for (i, cmd) in commands.iter().enumerate() {
        if cmd.contains('\0') {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Init command #{}: contains NUL byte", i + 1),
            ));
        }
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Init command #{}: empty after trim", i + 1),
            ));
        }
        if trimmed.chars().count() > MAX_COMMAND_LEN {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Init command #{}: {} chars exceeds {MAX_COMMAND_LEN}",
                    i + 1,
                    trimmed.chars().count()
                ),
            ));
        }
    }
    Ok(())
}

/// Validate the run-commands list.
fn validate_run_commands(commands: &[RunCommand]) -> Result<(), (StatusCode, String)> {
    if commands.len() > MAX_COMMANDS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "Too many run commands: {} > {MAX_COMMANDS}",
                commands.len()
            ),
        ));
    }
    let mut seen_names: HashSet<&str> = HashSet::with_capacity(commands.len());
    for (i, rc) in commands.iter().enumerate() {
        if rc.name.contains('\0') {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Run command #{}: name contains NUL byte", i + 1),
            ));
        }
        if rc.command.contains('\0') {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Run command #{}: command contains NUL byte", i + 1),
            ));
        }
        let trimmed_name = rc.name.trim();
        if trimmed_name.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Run command #{}: name empty after trim", i + 1),
            ));
        }
        if trimmed_name.chars().count() > MAX_NAME_LEN {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Run command #{}: name {} chars exceeds {MAX_NAME_LEN}",
                    i + 1,
                    trimmed_name.chars().count()
                ),
            ));
        }
        let trimmed_cmd = rc.command.trim();
        if trimmed_cmd.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Run command #{}: command empty after trim", i + 1),
            ));
        }
        if trimmed_cmd.chars().count() > MAX_COMMAND_LEN {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Run command #{}: command {} chars exceeds {MAX_COMMAND_LEN}",
                    i + 1,
                    trimmed_cmd.chars().count()
                ),
            ));
        }
        if !seen_names.insert(trimmed_name) {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Run command #{}: duplicate name \"{}\"",
                    i + 1,
                    trimmed_name
                ),
            ));
        }
    }
    Ok(())
}

/// Defense-in-depth body-size check on the combined JSON shape. Both lists
/// are independently capped above; this guards against pathological JSON
/// (e.g. 50 × 2000-char commands × 2 kinds = ~200 KiB — still well below
/// 1 MiB, but operators may save smaller-than-cap lists in odd shapes).
fn validate_combined_size(
    init_bytes: &[u8],
    run_bytes: &[u8],
) -> Result<(), (StatusCode, String)> {
    if init_bytes.len().saturating_add(run_bytes.len()) > MAX_BODY_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Body exceeds {MAX_BODY_BYTES} bytes"),
        ));
    }
    Ok(())
}

/// Map a `MaestroError` from the DB layer to an HTTP response.
fn db_error(e: maestro_core::error::MaestroError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

// ---------------------------------------------------------------------------
// Handlers.
// ---------------------------------------------------------------------------

/// `GET /api/worktree-commands` — the caller's rows.
pub async fn list_my_rows(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<UserCommandsEntry>>, (StatusCode, String)> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();
    let user_id = auth.user_id.clone();

    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        user_worktree_commands::list_for_user(&conn, &user_id)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    let mut entries: Vec<UserCommandsEntry> =
        rows.into_iter().map(UserCommandsEntry::from).collect();
    entries.sort_by(|a, b| a.workspace_name.cmp(&b.workspace_name));
    Ok(Json(entries))
}

/// `GET /api/worktree-commands/{workspace}` — the caller's row for that
/// workspace, or 404 if absent.
pub async fn get_my_row(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
) -> Result<Json<UserCommandsEntry>, (StatusCode, String)> {
    validate_workspace_name(&workspace)?;

    let db = auth_state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();
    let user_id = auth.user_id.clone();
    let lookup_name = workspace.clone();

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        user_worktree_commands::get(&conn, &user_id, &lookup_name)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    match row {
        Some(r) => Ok(Json(r.into())),
        None => Err((StatusCode::NOT_FOUND, "No commands set".to_string())),
    }
}

/// `PUT /api/worktree-commands/{workspace}` — upsert both kinds atomically.
pub async fn put_my_row(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
    Json(body): Json<PutBody>,
) -> Result<Json<UserCommandsEntry>, (StatusCode, String)> {
    validate_workspace_name(&workspace)?;
    validate_init_commands(&body.init_commands)?;
    validate_run_commands(&body.run_commands)?;
    let init_bytes = init_json_bytes(&body.init_commands);
    let run_bytes = run_json_bytes(&body.run_commands);
    validate_combined_size(&init_bytes, &run_bytes)?;

    let db = auth_state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();

    let user_id = auth.user_id.clone();
    let lookup_name = workspace.clone();
    let init_commands = body.init_commands.clone();
    let run_commands = body.run_commands.clone();

    let row = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        user_worktree_commands::upsert(
            &conn,
            &user_id,
            &lookup_name,
            &init_commands,
            &run_commands,
        )?;
        user_worktree_commands::get(&conn, &user_id, &lookup_name)?.ok_or_else(|| {
            maestro_core::error::MaestroError::DatabaseStr(
                "row was just upserted but vanished".into(),
            )
        })
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    // Audit log — scrubbed init snippets + hashes of both kinds.
    let scrubbed_init = scrub_secrets_for_log(&row.init_commands);
    let scrubbed_run: Vec<String> = row
        .run_commands
        .iter()
        .map(|rc| format!("{}={}", rc.name, rc.command))
        .collect();
    let scrubbed_run = scrub_secrets_for_log(&scrubbed_run);

    tracing::info!(
        user_id = %auth.user_id,
        workspace_name = %workspace,
        action = "set",
        init_count = row.init_commands.len(),
        run_count = row.run_commands.len(),
        init_hash = %sha256_hex(&init_bytes),
        run_hash = %sha256_hex(&run_bytes),
        init_snippet = ?scrubbed_init.iter().take(3).map(|s| truncate_80(s)).collect::<Vec<_>>(),
        run_snippet = ?scrubbed_run.iter().take(3).map(|s| truncate_80(s)).collect::<Vec<_>>(),
        "user_worktree_commands changed"
    );

    Ok(Json(row.into()))
}

fn truncate_80(s: &str) -> String {
    if s.chars().count() <= 80 {
        return s.to_string();
    }
    let mut out: String = s.chars().take(80).collect();
    out.push('…');
    out
}

/// `DELETE /api/worktree-commands/{workspace}` — remove the caller's row.
pub async fn delete_my_row(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    validate_workspace_name(&workspace)?;

    let db = auth_state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();

    let user_id = auth.user_id.clone();
    let lookup_name = workspace.clone();

    let (prev, deleted) = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let prev = user_worktree_commands::get(&conn, &user_id, &lookup_name)?;
        let removed = user_worktree_commands::delete(&conn, &user_id, &lookup_name)?;
        Ok::<_, maestro_core::error::MaestroError>((prev, removed))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "No commands to delete".to_string()));
    }

    let (prev_init_hash, prev_run_hash, init_count, run_count) = match prev {
        Some(r) => (
            sha256_hex(&init_json_bytes(&r.init_commands)),
            sha256_hex(&run_json_bytes(&r.run_commands)),
            r.init_commands.len(),
            r.run_commands.len(),
        ),
        None => (String::new(), String::new(), 0, 0),
    };

    tracing::info!(
        user_id = %auth.user_id,
        workspace_name = %workspace,
        action = "delete",
        init_count = init_count,
        run_count = run_count,
        init_hash = %prev_init_hash,
        run_hash = %prev_run_hash,
        "user_worktree_commands changed"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/worktree-commands/_workspaces` — repositories the caller has
/// added (plan-10) augmented with `has_my_commands`.
///
/// Plan-09 listed every workspace on disk (filesystem scan). Plan-10 deletes
/// the global workspace list and replaces it with per-user repositories
/// (`db::repositories::list_for_user`). The wire shape keeps `name`,
/// `html_url`, and `has_my_commands` so the existing UI keeps working; the
/// old `active` field is dropped (there is no "active repo" concept after
/// plan-10).
pub async fn list_workspaces_with_has_commands(
    State(auth_state): State<AuthState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<WorkspaceWithHasCommands>>, (StatusCode, String)> {
    let db = auth_state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();
    let user_id = auth.user_id.clone();

    let (repos, command_rows) = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let repos = repositories::list_for_user(&conn, &user_id)?;
        let cmds = user_worktree_commands::list_for_user(&conn, &user_id)?;
        Ok::<_, maestro_core::error::MaestroError>((repos, cmds))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    let my_workspaces: BTreeSet<String> =
        command_rows.into_iter().map(|r| r.workspace_name).collect();

    let merged: Vec<WorkspaceWithHasCommands> = repos
        .into_iter()
        .map(|r| WorkspaceWithHasCommands {
            has_my_commands: my_workspaces.contains(&r.name),
            html_url: r.repo_url,
            name: r.name,
        })
        .collect();

    Ok(Json(merged))
}

// ---------------------------------------------------------------------------
// Unit tests for helpers.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_hex_known_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(sha256_hex(b""), "");
    }

    #[test]
    fn scrub_replaces_token_assignments() {
        let cmds = vec![
            "export GITHUB_TOKEN=ghp_secretvalue".to_string(),
            "echo NOT_A_TOKEN_VALUE".to_string(),
            "MY_TOKEN  =   xyz123 && echo done".to_string(),
        ];
        let out = scrub_secrets_for_log(&cmds);
        assert!(out[0].contains("GITHUB_TOKEN=***"), "got: {}", out[0]);
        assert!(!out[0].contains("ghp_secretvalue"));
        assert_eq!(out[1], "echo NOT_A_TOKEN_VALUE");
        assert!(out[2].contains("MY_TOKEN=***"), "got: {}", out[2]);
        assert!(!out[2].contains("xyz123"));
    }

    #[test]
    fn scrub_handles_unicode() {
        let cmds = vec!["echo héllo TOKEN=x".to_string()];
        let out = scrub_secrets_for_log(&cmds);
        assert!(out[0].contains("héllo"));
        assert!(out[0].contains("TOKEN=***"));
    }

    #[test]
    fn validate_init_rejects_too_many() {
        let cmds: Vec<String> = (0..51).map(|i| format!("echo {i}")).collect();
        assert!(validate_init_commands(&cmds).is_err());
    }

    #[test]
    fn validate_init_rejects_empty_after_trim() {
        let cmds = vec!["   ".to_string()];
        assert!(validate_init_commands(&cmds).is_err());
    }

    #[test]
    fn validate_init_rejects_nul_byte() {
        let cmds = vec!["echo a\0b".to_string()];
        assert!(validate_init_commands(&cmds).is_err());
    }

    #[test]
    fn validate_init_rejects_oversize_cmd() {
        let cmds = vec!["x".repeat(MAX_COMMAND_LEN + 1)];
        assert!(validate_init_commands(&cmds).is_err());
    }

    #[test]
    fn validate_init_accepts_multi_line() {
        let cmds = vec!["set -e\necho one\necho two".to_string()];
        assert!(validate_init_commands(&cmds).is_ok());
    }

    #[test]
    fn validate_run_rejects_too_many() {
        let rcs: Vec<RunCommand> = (0..51)
            .map(|i| RunCommand {
                name: format!("n{i}"),
                command: "echo".to_string(),
            })
            .collect();
        assert!(validate_run_commands(&rcs).is_err());
    }

    #[test]
    fn validate_run_rejects_duplicate_names() {
        let rcs = vec![
            RunCommand {
                name: "Storybook".to_string(),
                command: "echo a".to_string(),
            },
            RunCommand {
                name: "Storybook".to_string(),
                command: "echo b".to_string(),
            },
        ];
        let err = validate_run_commands(&rcs).expect_err("dup names should error");
        assert_eq!(err.0, StatusCode::BAD_REQUEST);
        assert!(err.1.contains("duplicate"));
    }

    #[test]
    fn validate_run_rejects_empty_name() {
        let rcs = vec![RunCommand {
            name: "   ".to_string(),
            command: "echo".to_string(),
        }];
        assert!(validate_run_commands(&rcs).is_err());
    }

    #[test]
    fn validate_run_rejects_empty_command() {
        let rcs = vec![RunCommand {
            name: "ok".to_string(),
            command: "  ".to_string(),
        }];
        assert!(validate_run_commands(&rcs).is_err());
    }

    #[test]
    fn validate_run_rejects_long_name() {
        let rcs = vec![RunCommand {
            name: "x".repeat(MAX_NAME_LEN + 1),
            command: "echo".to_string(),
        }];
        assert!(validate_run_commands(&rcs).is_err());
    }

    #[test]
    fn validate_run_rejects_long_command() {
        let rcs = vec![RunCommand {
            name: "ok".to_string(),
            command: "x".repeat(MAX_COMMAND_LEN + 1),
        }];
        assert!(validate_run_commands(&rcs).is_err());
    }

    #[test]
    fn validate_run_rejects_nul_byte() {
        let rcs = vec![RunCommand {
            name: "ok".to_string(),
            command: "echo a\0b".to_string(),
        }];
        assert!(validate_run_commands(&rcs).is_err());
        let rcs = vec![RunCommand {
            name: "a\0b".to_string(),
            command: "echo".to_string(),
        }];
        assert!(validate_run_commands(&rcs).is_err());
    }

    #[test]
    fn validate_workspace_name_rejects_traversal() {
        assert!(validate_workspace_name("").is_err());
        assert!(validate_workspace_name("a/b").is_err());
        assert!(validate_workspace_name("..").is_err());
        assert!(validate_workspace_name(".hidden").is_err());
        assert!(validate_workspace_name("ok-name").is_ok());
    }
}
