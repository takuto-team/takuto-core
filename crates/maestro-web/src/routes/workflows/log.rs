// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Plan-07 slice 17 — `GET /work-items/{id}/log`.
//!
//! Paged read of `work_item_log_lines` for a work item. The
//! `step_id`, `limit`, and `offset` query params let the dashboard
//! both stream the live tail (poll with offset=count) and load full
//! history for a specific step. Pure-additive: there is no
//! in-memory analog the engine ever exposed via REST (the legacy
//! `terminal_lines` field on the workflow summary capped at 100
//! lines), so this endpoint cannot regress any existing reader.
//!
//! Returns an empty array when:
//!   * the engine has no DB attached (test paths),
//!   * the work item has no recorded log lines yet (plan-07 slice 18
//!     will wire the writer).

use axum::Extension;
use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};

use maestro_core::db::work_items;

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, EngineState};

use super::require_workflow_access;

/// Optional query string for `GET /work-items/{id}/log`.
#[derive(Debug, Deserialize, Default)]
pub struct LogQuery {
    /// Filter to a single step row's log lines.
    pub step_id: Option<i64>,
    /// Page size (rows, oldest first). Server caps at 5000 to keep
    /// any single response bounded.
    pub limit: Option<u32>,
    /// Skip this many rows. Pair with `limit` for the dashboard's
    /// "Load more history" button.
    pub offset: Option<u32>,
}

const MAX_LIMIT: u32 = 5000;
const DEFAULT_LIMIT: u32 = 500;

/// JSON projection of [`work_items::LogLine`]. Stream is a
/// lowercase string so clients don't need to know the Rust enum.
#[derive(Debug, Serialize)]
pub struct LogLineDto {
    pub id: i64,
    pub step_id: Option<i64>,
    pub stream: String,
    pub content: String,
    /// Unix milliseconds.
    pub emitted_at: i64,
}

impl From<work_items::LogLine> for LogLineDto {
    fn from(l: work_items::LogLine) -> Self {
        Self {
            id: l.id,
            step_id: l.step_id,
            stream: l.stream.as_str().to_string(),
            content: l.content,
            emitted_at: l.emitted_at,
        }
    }
}

/// `GET /work-items/{id}/log` — paged log lines, oldest first.
/// Access is gated by `require_workflow_access` (404 for missing
/// or unauthorised id, matching the convention from AC-2).
pub async fn get_log(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path(id): Path<String>,
    Query(q): Query<LogQuery>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<LogLineDto>>, StatusCode> {
    require_workflow_access(&engine, &auth_state, &auth, &id).await?;
    let Some(db) = engine.engine.db() else {
        return Ok(Json(Vec::new()));
    };
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT);
    let paging = work_items::LogPaging {
        step_id: q.step_id,
        limit,
        offset: q.offset.unwrap_or(0),
    };
    let lines = work_items::fetch_log_lines(db.adapter(), &id, paging)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(lines.into_iter().map(LogLineDto::from).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_line_dto_lowercase_stringifies_stream() {
        let row = work_items::LogLine {
            id: 1,
            work_item_id: "wf-x".into(),
            step_id: Some(7),
            stream: work_items::LogStream::Stderr,
            content: "boom".into(),
            emitted_at: 1_700_000_000_000,
        };
        let dto = LogLineDto::from(row);
        assert_eq!(dto.id, 1);
        assert_eq!(dto.step_id, Some(7));
        assert_eq!(dto.stream, "stderr");
        assert_eq!(dto.content, "boom");
        assert_eq!(dto.emitted_at, 1_700_000_000_000);
    }
}
