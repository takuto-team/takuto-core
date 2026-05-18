// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `onboarding_state` table — row shape + CRUD.
//!
//! Phase 2a defined the table; Phase 2b.1 grows the helpers consumed by the
//! `GET /api/onboarding/status` (per-user shim) and the per-step mutators
//! that Phase 2b.2 will turn into endpoints.

use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// Tri-state for a single wizard step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnboardingStepState {
    Completed,
    Skipped,
}

impl OnboardingStepState {
    pub fn as_str(self) -> &'static str {
        match self {
            OnboardingStepState::Completed => "completed",
            OnboardingStepState::Skipped => "skipped",
        }
    }
}

/// One row of onboarding wizard state. `None` for any step that has not been
/// reached yet (the wizard renders the unreached step normally).
#[derive(Debug, Clone)]
pub struct OnboardingStateRow {
    pub user_id: String,
    pub step_1_ticketing: Option<OnboardingStepState>,
    pub step_2_provider: Option<OnboardingStepState>,
    pub step_3_github: Option<OnboardingStepState>,
    pub step_4_credentials: Option<OnboardingStepState>,
    /// ISO-8601 UTC timestamp; `Some(_)` once the wizard is dismissed.
    /// Clearing this re-enters the wizard (FR-2.4).
    pub completed_at: Option<String>,
    pub updated_at: String,
}

/// Which step the caller is updating. Lower-case discriminator matches the
/// column suffix to keep the parser obvious.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingStep {
    Ticketing,
    Provider,
    Github,
    Credentials,
}

impl OnboardingStep {
    /// Stable wire identifier (matches the path parameter the Phase 2b.2
    /// `POST /api/onboarding/skip/{step}` endpoint will use).
    pub fn as_str(self) -> &'static str {
        match self {
            OnboardingStep::Ticketing => "ticketing",
            OnboardingStep::Provider => "provider",
            OnboardingStep::Github => "github",
            OnboardingStep::Credentials => "credentials",
        }
    }

    /// SQL column that stores this step's state.
    fn column(self) -> &'static str {
        match self {
            OnboardingStep::Ticketing => "step_1_ticketing",
            OnboardingStep::Provider => "step_2_provider",
            OnboardingStep::Github => "step_3_github",
            OnboardingStep::Credentials => "step_4_credentials",
        }
    }
}

fn parse_step(text: Option<String>) -> Option<OnboardingStepState> {
    match text.as_deref() {
        Some("completed") => Some(OnboardingStepState::Completed),
        Some("skipped") => Some(OnboardingStepState::Skipped),
        _ => None,
    }
}

fn row_from_query(row: &rusqlite::Row) -> rusqlite::Result<OnboardingStateRow> {
    Ok(OnboardingStateRow {
        user_id: row.get("user_id")?,
        step_1_ticketing: parse_step(row.get("step_1_ticketing")?),
        step_2_provider: parse_step(row.get("step_2_provider")?),
        step_3_github: parse_step(row.get("step_3_github")?),
        step_4_credentials: parse_step(row.get("step_4_credentials")?),
        completed_at: row.get("completed_at")?,
        updated_at: row.get("updated_at")?,
    })
}

/// Look up the user's onboarding state row, if any.
pub fn get(conn: &Connection, user_id: &str) -> Result<Option<OnboardingStateRow>> {
    let mut stmt = conn.prepare(
        "SELECT user_id, step_1_ticketing, step_2_provider, step_3_github, \
                step_4_credentials, completed_at, updated_at \
         FROM onboarding_state WHERE user_id = ?1",
    )?;
    let row = stmt
        .query_row(params![user_id], row_from_query)
        .optional()?;
    Ok(row)
}

/// Insert (when no row exists for the user) or update the named step's
/// state. Idempotent; never touches `completed_at`. The `INSERT ON CONFLICT
/// DO UPDATE` form is used so the rest of the columns keep their existing
/// values across multiple step updates.
pub fn mark_step(
    conn: &Connection,
    user_id: &str,
    step: OnboardingStep,
    status: OnboardingStepState,
) -> Result<()> {
    let column = step.column();
    // SAFETY: `column` comes from a closed enum, never user input.
    let sql = format!(
        "INSERT INTO onboarding_state (user_id, {column}) VALUES (?1, ?2) \
         ON CONFLICT(user_id) DO UPDATE SET \
            {column} = excluded.{column}, \
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')"
    );
    conn.execute(&sql, params![user_id, status.as_str()])?;
    Ok(())
}

/// Mark the wizard finished. Sets `completed_at` to "now" if it was null;
/// otherwise leaves the previous timestamp intact (re-entries clear it via
/// [`clear_completed`]).
pub fn mark_completed(conn: &Connection, user_id: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO onboarding_state (user_id, completed_at, updated_at) \
         VALUES (?1, strftime('%Y-%m-%dT%H:%M:%SZ','now'), strftime('%Y-%m-%dT%H:%M:%SZ','now')) \
         ON CONFLICT(user_id) DO UPDATE SET \
            completed_at = COALESCE(onboarding_state.completed_at, strftime('%Y-%m-%dT%H:%M:%SZ','now')), \
            updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now')",
        params![user_id],
    )?;
    Ok(())
}

/// Clear `completed_at` so the wizard re-enters on next dashboard load
/// (FR-2.4). Phase 2b.2 wires the `POST /api/onboarding/re-enter` button to
/// this helper.
pub fn clear_completed(conn: &Connection, user_id: &str) -> Result<()> {
    conn.execute(
        "UPDATE onboarding_state SET completed_at = NULL, \
         updated_at = strftime('%Y-%m-%dT%H:%M:%SZ','now') \
         WHERE user_id = ?1",
        params![user_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::schema;

    fn fresh_db() -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        schema::run_migrations(&conn).unwrap();
        conn.execute(
            "INSERT INTO users (id, username, role) VALUES ('u-alice', 'alice', 'user')",
            [],
        )
        .unwrap();
        conn
    }

    #[test]
    fn mark_step_inserts_then_updates() {
        let conn = fresh_db();
        mark_step(
            &conn,
            "u-alice",
            OnboardingStep::Ticketing,
            OnboardingStepState::Completed,
        )
        .unwrap();
        let row = get(&conn, "u-alice").unwrap().unwrap();
        assert_eq!(
            row.step_1_ticketing,
            Some(OnboardingStepState::Completed)
        );
        assert!(row.step_2_provider.is_none());

        // Updating a different step preserves the first.
        mark_step(
            &conn,
            "u-alice",
            OnboardingStep::Provider,
            OnboardingStepState::Skipped,
        )
        .unwrap();
        let row2 = get(&conn, "u-alice").unwrap().unwrap();
        assert_eq!(
            row2.step_1_ticketing,
            Some(OnboardingStepState::Completed)
        );
        assert_eq!(row2.step_2_provider, Some(OnboardingStepState::Skipped));
    }

    #[test]
    fn mark_completed_idempotent_and_clear_resets() {
        let conn = fresh_db();
        mark_completed(&conn, "u-alice").unwrap();
        let row = get(&conn, "u-alice").unwrap().unwrap();
        let first = row.completed_at.clone().expect("completed_at set");

        // Second call keeps the same timestamp (COALESCE preserves the
        // existing value).
        std::thread::sleep(std::time::Duration::from_millis(1100));
        mark_completed(&conn, "u-alice").unwrap();
        let row2 = get(&conn, "u-alice").unwrap().unwrap();
        assert_eq!(row2.completed_at.as_deref(), Some(first.as_str()));

        // Clearing nukes it.
        clear_completed(&conn, "u-alice").unwrap();
        let row3 = get(&conn, "u-alice").unwrap().unwrap();
        assert!(row3.completed_at.is_none());
    }
}
