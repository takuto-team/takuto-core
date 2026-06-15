// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `GET /work-items/{id}/steps`.
//!
//! Pure-read endpoint that returns the step history persisted in
//! `work_item_steps` by the shadow-writes. There is no in-memory analog
//! (the engine keeps `Workflow.steps_log` but never exposed it as an
//! endpoint), so this is additive — adding the route cannot regress any
//! existing reader.

use axum::Extension;
use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use serde::Serialize;

use takuto_core::db::work_items;

use crate::auth::AuthenticatedUser;
use crate::state::{AuthState, EngineState};

use super::require_workflow_access;

/// A single step row, projected for JSON. `status` is the lowercase
/// string variant from [`work_items::StepStatus`] so clients don't
/// have to know the Rust enum layout.
#[derive(Debug, Serialize)]
pub struct StepDto {
    pub id: i64,
    pub sequence: i64,
    pub name: String,
    pub definition_filename: Option<String>,
    pub status: String,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

impl From<work_items::StepRow> for StepDto {
    fn from(r: work_items::StepRow) -> Self {
        Self {
            id: r.id,
            sequence: r.sequence,
            name: r.name,
            definition_filename: r.definition_filename,
            status: r.status.as_str().to_string(),
            started_at: r.started_at,
            ended_at: r.ended_at,
            exit_code: r.exit_code,
            error_message: r.error_message,
        }
    }
}

/// `GET /work-items/{id}/steps` — returns the step history for a
/// work item, ascending by sequence. Access is gated by the same
/// `require_workflow_access` policy used by all other per-item
/// endpoints; on a missing or unauthorised id the route returns
/// `404 Not Found` (never 403).
///
/// When the engine has no DB attached the route returns an empty
/// array — same outcome as a work-item that has run zero steps.
pub async fn get_steps(
    State(engine): State<EngineState>,
    State(auth_state): State<AuthState>,
    Path(id): Path<String>,
    Extension(auth): Extension<AuthenticatedUser>,
) -> Result<Json<Vec<StepDto>>, StatusCode> {
    require_workflow_access(&engine, &auth_state, &auth, &id).await?;
    let Some(db) = engine.engine.db() else {
        return Ok(Json(Vec::new()));
    };
    let rows = work_items::list_steps(db.adapter(), &id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(rows.into_iter().map(StepDto::from).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `StepDto` projection drops the `work_item_id` (redundant with
    /// the path param) and lowercase-stringifies the status enum so
    /// the API surface doesn't leak the Rust variant layout.
    #[test]
    fn step_dto_projection_strips_work_item_and_stringifies_status() {
        let row = work_items::StepRow {
            id: 7,
            work_item_id: "wf-x".into(),
            sequence: 2,
            name: "build".into(),
            definition_filename: Some("ship.toml".into()),
            status: work_items::StepStatus::Success,
            started_at: 100,
            ended_at: Some(200),
            exit_code: Some(0),
            error_message: None,
        };
        let dto = StepDto::from(row);
        assert_eq!(dto.id, 7);
        assert_eq!(dto.sequence, 2);
        assert_eq!(dto.name, "build");
        assert_eq!(dto.definition_filename.as_deref(), Some("ship.toml"));
        assert_eq!(dto.status, "success");
        assert_eq!(dto.started_at, 100);
        assert_eq!(dto.ended_at, Some(200));
        assert_eq!(dto.exit_code, Some(0));
        assert_eq!(dto.error_message, None);
    }
}
