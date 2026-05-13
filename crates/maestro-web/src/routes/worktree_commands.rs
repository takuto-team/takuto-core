// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Admin REST endpoints for per-workspace `worktree_init_commands` overrides
//! (plan-08 Step 5).
//!
//! All endpoints are admin-gated via [`require_admin_for`]. CSRF and session
//! authentication are enforced by the outer middleware stack mounted in
//! `server.rs` — no need to re-check them here.
//!
//! Trust boundary: only admins can write. Command strings may contain `sudo`,
//! shell-meta, and secrets (registry tokens, CI keys); the threat model is
//! "rogue admin", which is out of scope. The audit log line below scrubs
//! `*_TOKEN=…` style secrets from the message, but the DB stores the command
//! verbatim — operators legitimately need to read it back.

use std::collections::BTreeSet;

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use maestro_core::db::workspace_commands::{self, WorkspaceCommandsRow};

use crate::auth::AuthenticatedUser;
use crate::routes::admin::require_admin_for;
use crate::routes::repos::list_workspaces;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Constants — validation limits.
// ---------------------------------------------------------------------------

/// Hard ceiling on the number of commands per override.
const MAX_COMMANDS: usize = 50;

/// Hard ceiling on a single command's length (after `trim`). Multi-line shell
/// scripts inside a single command are allowed; newlines do NOT count as
/// command separators.
const MAX_COMMAND_LEN: usize = 2000;

/// Defense-in-depth ceiling on the serialized JSON body size. axum's default
/// JSON limit (currently 2 MiB) covers this for normal requests, but we
/// enforce it ourselves so the policy stays in this file.
const MAX_BODY_BYTES: usize = 1_024 * 1_024; // 1 MiB

// ---------------------------------------------------------------------------
// Wire types.
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
pub struct OverrideEntry {
    pub workspace_name: String,
    pub commands: Vec<String>,
    pub updated_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
}

impl From<WorkspaceCommandsRow> for OverrideEntry {
    fn from(row: WorkspaceCommandsRow) -> Self {
        OverrideEntry {
            workspace_name: row.workspace_name,
            commands: row.commands,
            updated_at: row.updated_at,
            updated_by: row.updated_by,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TopLevelResponse {
    /// The global `Config.commands.worktree_init_commands` value.
    pub default: Vec<String>,
    /// All per-workspace overrides, sorted by `workspace_name`.
    pub overrides: Vec<OverrideEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PutOverrideBody {
    pub commands: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WorkspaceWithOverride {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    pub active: bool,
    pub has_override: bool,
}

// ---------------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------------

/// SHA-256 hex digest. Empty input → empty string (used as the `prev_hash`
/// when no override existed before, and as the `new_hash` after a DELETE).
fn sha256_hex(input: &[u8]) -> String {
    if input.is_empty() {
        return String::new();
    }
    let mut hasher = Sha256::new();
    hasher.update(input);
    hex::encode(hasher.finalize())
}

/// Serialize a command list to JSON for hashing. Stable for a given Vec since
/// `serde_json::to_vec` produces deterministic output for `Vec<String>`.
fn commands_json_bytes(commands: &[String]) -> Vec<u8> {
    serde_json::to_vec(commands).unwrap_or_default()
}

/// Scrub `*_TOKEN=…` style assignments from a command snippet so they don't
/// leak into the audit log. The DB-stored copy is untouched.
fn scrub_secrets_for_log(commands: &[String]) -> Vec<String> {
    // We do this inline with a small state machine rather than pulling in
    // `regex` (already a transitive dep, but the inline scan avoids the
    // import cost in this hot path). Match the regex:
    //   `\b[A-Z_]*TOKEN\b\s*=\s*\S+`  →  `<NAME>=***`
    commands
        .iter()
        .map(|cmd| {
            let mut out = String::with_capacity(cmd.len());
            let bytes = cmd.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                // Find the start of a potential TOKEN identifier: a run of
                // [A-Z_] characters at a word boundary that ends in TOKEN.
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
                    // Optional whitespace then '='.
                    let mut k = j;
                    while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                        k += 1;
                    }
                    if k < bytes.len() && bytes[k] == b'=' {
                        k += 1;
                        while k < bytes.len() && (bytes[k] == b' ' || bytes[k] == b'\t') {
                            k += 1;
                        }
                        // Consume non-whitespace value (\S+).
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
                // No match: copy this byte and advance one.
                // (Using char boundaries via `str` slicing instead of bytes
                // to avoid splitting a multi-byte char.)
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
/// and empty names; mirrors the policy in `routes/repos.rs::switch_workspace`.
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

/// Validate a command list per the spec:
/// - `commands.len() <= MAX_COMMANDS`
/// - each `cmd.trim()` non-empty
/// - each command ≤ `MAX_COMMAND_LEN` chars after trim
/// - reject `\0` bytes anywhere
/// - the entire commands-as-JSON must fit in `MAX_BODY_BYTES`
fn validate_commands(commands: &[String]) -> Result<(), (StatusCode, String)> {
    if commands.len() > MAX_COMMANDS {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Too many commands: {} > {MAX_COMMANDS}", commands.len()),
        ));
    }
    for (i, cmd) in commands.iter().enumerate() {
        if cmd.contains('\0') {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Command #{}: contains NUL byte", i + 1),
            ));
        }
        let trimmed = cmd.trim();
        if trimmed.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Command #{}: empty after trim", i + 1),
            ));
        }
        if trimmed.chars().count() > MAX_COMMAND_LEN {
            return Err((
                StatusCode::BAD_REQUEST,
                format!(
                    "Command #{}: {} chars exceeds {MAX_COMMAND_LEN}",
                    i + 1,
                    trimmed.chars().count()
                ),
            ));
        }
    }
    // Defense-in-depth: even with each field within bounds, fail if the
    // serialized JSON exceeds the body ceiling.
    if commands_json_bytes(commands).len() > MAX_BODY_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("Commands JSON exceeds {MAX_BODY_BYTES} bytes"),
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

/// `GET /api/admin/worktree-commands` — global default + all overrides.
pub async fn list_top_level(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<TopLevelResponse>, (StatusCode, String)> {
    require_admin_for(&state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;

    let default = state
        .config
        .read()
        .await
        .commands
        .worktree_init_commands
        .clone();

    let db = state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();
    let rows = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        workspace_commands::list(&conn)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    let mut overrides: Vec<OverrideEntry> = rows.into_iter().map(OverrideEntry::from).collect();
    overrides.sort_by(|a, b| a.workspace_name.cmp(&b.workspace_name));

    Ok(Json(TopLevelResponse { default, overrides }))
}

/// `GET /api/admin/worktree-commands/{workspace}` — one override row.
pub async fn get_one(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
) -> Result<Json<OverrideEntry>, (StatusCode, String)> {
    require_admin_for(&state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;
    validate_workspace_name(&workspace)?;

    let db = state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();
    let lookup_name = workspace.clone();
    let row = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        workspace_commands::get(&conn, &lookup_name)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    match row {
        Some(r) => Ok(Json(r.into())),
        None => Err((StatusCode::NOT_FOUND, "No override".to_string())),
    }
}

/// `PUT /api/admin/worktree-commands/{workspace}` — upsert an override.
pub async fn put_override(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
    Json(body): Json<PutOverrideBody>,
) -> Result<Json<OverrideEntry>, (StatusCode, String)> {
    require_admin_for(&state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;
    validate_workspace_name(&workspace)?;
    validate_commands(&body.commands)?;

    let db = state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();

    let writer_id = auth.user_id.clone();
    let commands = body.commands.clone();
    let lookup_name = workspace.clone();

    let result = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let prev = workspace_commands::get(&conn, &lookup_name)?;
        workspace_commands::upsert(&conn, &lookup_name, &commands, Some(&writer_id))?;
        let row = workspace_commands::get(&conn, &lookup_name)?
            .expect("row was just upserted");
        Ok::<_, maestro_core::error::MaestroError>((prev, row))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    let (prev, row) = result;
    let prev_bytes = prev
        .as_ref()
        .map(|r| commands_json_bytes(&r.commands))
        .unwrap_or_default();
    let new_bytes = commands_json_bytes(&row.commands);

    // Audit log — scrubbed snippet (first 3 entries, each truncated) plus
    // hashes for plan-03 backfill.
    let scrubbed = scrub_secrets_for_log(&row.commands);
    tracing::info!(
        actor_user_id = %auth.user_id,
        workspace_name = %workspace,
        action = "set",
        prev_hash = %sha256_hex(&prev_bytes),
        new_hash = %sha256_hex(&new_bytes),
        command_count = row.commands.len(),
        snippet = ?scrubbed.iter().take(3).map(|s| {
            let mut t = s.clone();
            if t.len() > 80 {
                t.truncate(80);
                t.push('…');
            }
            t
        }).collect::<Vec<_>>(),
        "worktree_commands override changed"
    );

    Ok(Json(row.into()))
}

/// `DELETE /api/admin/worktree-commands/{workspace}` — drop the override.
pub async fn delete_override(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
    Path(workspace): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin_for(&state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;
    validate_workspace_name(&workspace)?;

    let db = state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();

    let lookup_name = workspace.clone();
    let (prev, deleted) = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        let prev = workspace_commands::get(&conn, &lookup_name)?;
        let removed = workspace_commands::delete(&conn, &lookup_name)?;
        Ok::<_, maestro_core::error::MaestroError>((prev, removed))
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?;

    if !deleted {
        return Err((StatusCode::NOT_FOUND, "No override to delete".to_string()));
    }

    let prev_bytes = prev
        .as_ref()
        .map(|r| commands_json_bytes(&r.commands))
        .unwrap_or_default();

    tracing::info!(
        actor_user_id = %auth.user_id,
        workspace_name = %workspace,
        action = "delete",
        prev_hash = %sha256_hex(&prev_bytes),
        new_hash = "",
        "worktree_commands override changed"
    );

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/admin/worktree-commands/_workspaces` — workspaces with the
/// `has_override` flag merged in. Delegates the filesystem scan to
/// `routes/repos.rs::list_workspaces` so we don't duplicate the scan logic.
pub async fn list_workspaces_with_override_flag(
    State(state): State<AppState>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<WorkspaceWithOverride>>, (StatusCode, String)> {
    require_admin_for(&state, &auth)
        .await
        .map_err(|s| (s, String::new()))?;

    let ws = list_workspaces(State(state.clone())).await.0;

    let db = state
        .db
        .as_ref()
        .ok_or((StatusCode::SERVICE_UNAVAILABLE, "database unavailable".into()))?
        .clone();
    let names: BTreeSet<String> = tokio::task::spawn_blocking(move || {
        let conn = db.conn().blocking_lock();
        workspace_commands::list_workspaces_with_overrides(&conn)
    })
    .await
    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "join error".into()))?
    .map_err(db_error)?
    .into_iter()
    .collect();

    let merged: Vec<WorkspaceWithOverride> = ws
        .into_iter()
        .map(|w| WorkspaceWithOverride {
            has_override: names.contains(&w.name),
            name: w.name,
            html_url: w.html_url,
            active: w.active,
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
        // SHA-256("abc") = ba7816bf...
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
        // "NOT_A_TOKEN_VALUE" is one ident token, doesn't end in TOKEN (it ends
        // in VALUE), so it should NOT be scrubbed.
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
    fn validate_commands_rejects_too_many() {
        let cmds: Vec<String> = (0..51).map(|i| format!("echo {i}")).collect();
        assert!(validate_commands(&cmds).is_err());
    }

    #[test]
    fn validate_commands_rejects_empty_after_trim() {
        let cmds = vec!["   ".to_string()];
        assert!(validate_commands(&cmds).is_err());
    }

    #[test]
    fn validate_commands_rejects_nul_byte() {
        let cmds = vec!["echo a\0b".to_string()];
        assert!(validate_commands(&cmds).is_err());
    }

    #[test]
    fn validate_commands_rejects_oversize_cmd() {
        let cmds = vec!["x".repeat(MAX_COMMAND_LEN + 1)];
        assert!(validate_commands(&cmds).is_err());
    }

    #[test]
    fn validate_commands_accepts_multi_line() {
        let cmds = vec!["set -e\necho one\necho two".to_string()];
        assert!(validate_commands(&cmds).is_ok());
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
