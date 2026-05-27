// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! `onboarding_state` table — row shape + CRUD.
//!
//! Phase 2a defined the table; Phase 2b.1 grows the helpers consumed by the
//! `GET /api/onboarding/status` (per-user shim) and the per-step mutators
//! that Phase 2b.2 will turn into endpoints.
//!
//! ### Plan-11 step 3 (commit `dc422de` + this commit)
//!
//! Second DAO migrated to the backend-agnostic [`DbAdapter`] API. Pattern
//! template lives in `login_attempts.rs`; the additions specific to this
//! DAO are:
//!
//! 1. **Timestamps computed in Rust.** The legacy SQL used
//!    `strftime('%Y-%m-%dT%H:%M:%SZ','now')`, which is SQLite-only. The
//!    migrated form binds an ISO-8601 string produced by `chrono::Utc::now`
//!    so the same query runs on SQLite, Postgres, and MySQL once those
//!    backends are wired (plan §10 step 4). The on-disk shape stays TEXT
//!    so the live rusqlite path's schema is unchanged.
//!
//! 2. **`INSERT ... ON CONFLICT(user_id) DO UPDATE` works on SQLite +
//!    Postgres.** MySQL uses `ON DUPLICATE KEY UPDATE` instead — when
//!    `MAESTRO_TEST_BACKEND=mysql` lands in CI, this DAO will need a
//!    per-backend `match adapter.backend()` branch. Documented inline.

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::db::{DbAdapter, DbValue};
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

/// Format `chrono::Utc::now()` in the same shape SQLite's
/// `strftime('%Y-%m-%dT%H:%M:%SZ','now')` produced. Keeps the on-disk
/// timestamp format byte-identical to pre-migration rows so the
/// rusqlite-path's TEXT columns round-trip without surprises.
fn now_iso() -> String {
    Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// Look up the user's onboarding state row, if any.
pub async fn get(adapter: &DbAdapter, user_id: &str) -> Result<Option<OnboardingStateRow>> {
    let row = adapter
        .query_optional(
            "SELECT user_id, step_1_ticketing, step_2_provider, step_3_github, \
                    step_4_credentials, completed_at, updated_at \
             FROM onboarding_state WHERE user_id = ?",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    Ok(Some(OnboardingStateRow {
        user_id: r.get_text(0)?,
        step_1_ticketing: parse_step(r.get_text_opt(1)?),
        step_2_provider: parse_step(r.get_text_opt(2)?),
        step_3_github: parse_step(r.get_text_opt(3)?),
        step_4_credentials: parse_step(r.get_text_opt(4)?),
        completed_at: r.get_text_opt(5)?,
        updated_at: r.get_text(6)?,
    }))
}

/// Insert (when no row exists for the user) or update the named step's
/// state. Idempotent; never touches `completed_at`. The `INSERT ... ON
/// CONFLICT DO UPDATE` form is used so the rest of the columns keep their
/// existing values across multiple step updates.
///
/// Backend support: works on SQLite (≥ 3.24) and Postgres (≥ 9.5 with
/// the same `excluded` keyword). MySQL uses `ON DUPLICATE KEY UPDATE`
/// — when MySQL lands as a supported backend (plan §10 step 4) this fn
/// will need a `match adapter.backend()` branch to emit the right form.
pub async fn mark_step(
    adapter: &DbAdapter,
    user_id: &str,
    step: OnboardingStep,
    status: OnboardingStepState,
) -> Result<()> {
    let column = step.column();
    // SAFETY: `column` comes from a closed enum, never user input.
    let sql = format!(
        "INSERT INTO onboarding_state (user_id, {column}, updated_at) \
         VALUES (?, ?, ?) \
         ON CONFLICT(user_id) DO UPDATE SET \
            {column} = excluded.{column}, \
            updated_at = excluded.updated_at"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(status.as_str().to_string()),
                DbValue::Text(now_iso()),
            ],
        )
        .await?;
    Ok(())
}

/// Mark the wizard finished. Sets `completed_at` to "now" if it was null;
/// otherwise leaves the previous timestamp intact (re-entries clear it via
/// [`clear_completed`]). Idempotent.
pub async fn mark_completed(adapter: &DbAdapter, user_id: &str) -> Result<()> {
    let now = now_iso();
    adapter
        .execute(
            "INSERT INTO onboarding_state (user_id, completed_at, updated_at) \
             VALUES (?, ?, ?) \
             ON CONFLICT(user_id) DO UPDATE SET \
                completed_at = COALESCE(onboarding_state.completed_at, excluded.completed_at), \
                updated_at = excluded.updated_at",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(now.clone()),
                DbValue::Text(now),
            ],
        )
        .await?;
    Ok(())
}

/// Clear `completed_at` so the wizard re-enters on next dashboard load
/// (FR-2.4). Phase 2b.2 wires the `POST /api/onboarding/re-enter` button to
/// this helper.
pub async fn clear_completed(adapter: &DbAdapter, user_id: &str) -> Result<()> {
    adapter
        .execute(
            "UPDATE onboarding_state SET completed_at = NULL, updated_at = ? \
             WHERE user_id = ?",
            vec![
                DbValue::Text(now_iso()),
                DbValue::Text(user_id.to_string()),
            ],
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

    /// Build a fresh in-memory SQLite adapter with the portable
    /// migration set applied + a single seed user (FK target for
    /// onboarding_state.user_id). Mirrors the helper in
    /// `db/login_attempts.rs::tests`.
    async fn fresh_adapter() -> DbAdapter {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .expect("connect in-memory sqlite");
        let source = DialectAwareMigrationSource::for_backend(DbBackend::Sqlite);
        sqlx::migrate::Migrator::new(source)
            .await
            .expect("build migrator")
            .run(&pool)
            .await
            .expect("run migrations");
        let adapter = DbAdapter::new(DbPool::Sqlite(pool));
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, ?)",
                vec![
                    DbValue::Text("u-alice".into()),
                    DbValue::Text("alice".into()),
                    DbValue::Text("user".into()),
                ],
            )
            .await
            .expect("seed user");
        adapter
    }

    #[tokio::test]
    async fn mark_step_inserts_then_updates() {
        let a = fresh_adapter().await;
        mark_step(
            &a,
            "u-alice",
            OnboardingStep::Ticketing,
            OnboardingStepState::Completed,
        )
        .await
        .unwrap();
        let row = get(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(
            row.step_1_ticketing,
            Some(OnboardingStepState::Completed)
        );
        assert!(row.step_2_provider.is_none());

        // Updating a different step preserves the first.
        mark_step(
            &a,
            "u-alice",
            OnboardingStep::Provider,
            OnboardingStepState::Skipped,
        )
        .await
        .unwrap();
        let row2 = get(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(
            row2.step_1_ticketing,
            Some(OnboardingStepState::Completed)
        );
        assert_eq!(row2.step_2_provider, Some(OnboardingStepState::Skipped));
    }

    #[tokio::test]
    async fn mark_completed_idempotent_and_clear_resets() {
        let a = fresh_adapter().await;
        mark_completed(&a, "u-alice").await.unwrap();
        let row = get(&a, "u-alice").await.unwrap().unwrap();
        let first = row.completed_at.clone().expect("completed_at set");

        // Second call keeps the same timestamp (COALESCE preserves the
        // existing value). The sleep here is necessary because the
        // migrated DAO computes timestamps in Rust at second-precision —
        // without it, the two `now_iso()` calls would collide and the
        // COALESCE behaviour would be invisible.
        tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        mark_completed(&a, "u-alice").await.unwrap();
        let row2 = get(&a, "u-alice").await.unwrap().unwrap();
        assert_eq!(row2.completed_at.as_deref(), Some(first.as_str()));

        // Clearing nukes it.
        clear_completed(&a, "u-alice").await.unwrap();
        let row3 = get(&a, "u-alice").await.unwrap().unwrap();
        assert!(row3.completed_at.is_none());
    }

    #[tokio::test]
    async fn get_returns_none_for_unknown_user() {
        let a = fresh_adapter().await;
        assert!(get(&a, "u-nobody").await.unwrap().is_none());
    }
}
