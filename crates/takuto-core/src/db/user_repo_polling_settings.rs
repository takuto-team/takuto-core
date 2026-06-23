// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user-per-workspace polling settings.
//!
//! Polling is now a per-user, per-repository concern: which Jira projects /
//! GitHub filters a repository draws from, how often it polls, which flow to
//! auto-start, and the parallel-item caps are all owned by the user for that
//! repository (edited from the Ticketing tab). Deployment-wide limits stay
//! global in `[general]` (see `config/general.rs`).
//!
//! User-scoped table keyed by `(user_id, workspace_name)` with a single
//! `settings_json` column holding the [`RepoPollingSettings`] struct (default
//! `'{}'` → all-defaults). `user_id` references `users(id) ON DELETE CASCADE`.
//!
//! Validation (key shape, interval floor) lives in the REST layer; this layer
//! rejects NUL bytes as a last-line guardrail and surfaces a corrupt JSON read
//! as `DbError::CommandsJsonDecode`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::config::LinkedItemsPromptMode;
use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::DbError;

fn default_item_types() -> Vec<String> {
    vec!["Task".to_string(), "Bug".to_string()]
}

fn default_done_status() -> String {
    "Done".to_string()
}

/// Per-`(user, workspace)` polling configuration. Serialized as the
/// `settings_json` column; every field has a serde default so partial JSON and
/// schema additions decode cleanly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "RepoPollingSettings.ts")]
pub struct RepoPollingSettings {
    /// When `true` (and the global pause switch is off), the poller polls this
    /// repository each cycle. Default `false` — polling is opt-in per repo so a
    /// fresh deploy never auto-starts workflows.
    #[serde(default)]
    pub auto_polling: bool,
    /// Flow slug to auto-start for each polled item. Empty = start all
    /// dependency-free flows (legacy behavior).
    #[serde(default)]
    pub auto_start_flow: String,
    /// Maximum items occupying a concurrency slot for this repo. `0` =
    /// unlimited (the global ceilings still apply).
    #[serde(default)]
    pub max_parallel_items: u32,
    /// Jira project keys polled / offered in the manual picker for this repo.
    #[serde(default)]
    pub project_keys: Vec<String>,
    /// Jira issue types polled.
    #[serde(default = "default_item_types")]
    pub item_types: Vec<String>,
    /// Case-insensitive ANY-substring match against the Jira ticket summary.
    #[serde(default)]
    pub jira_summary_keywords: Vec<String>,
    /// Exact GitHub label membership, ANY match.
    #[serde(default)]
    pub github_labels: Vec<String>,
    /// Case-insensitive ANY-substring match against the GitHub issue title.
    #[serde(default)]
    pub github_title_keywords: Vec<String>,
    /// How linked Jira issues appear in `{ticket_context}` for agent prompts.
    #[serde(default)]
    pub linked_items_in_prompt: LinkedItemsPromptMode,
    /// Max UTF-8 bytes for the primary ticket description in prompts (`0` =
    /// unlimited).
    #[serde(default)]
    pub ticket_context_max_description_bytes: usize,
    /// Max UTF-8 bytes per linked issue description when mode is `full`.
    #[serde(default)]
    pub linked_issue_description_max_bytes: usize,
    /// Extra JQL `AND`-merged into the manual picker / poll query.
    #[serde(default)]
    pub jql_filter: String,
    /// Jira transition target for **Mark as Done**.
    #[serde(default = "default_done_status")]
    pub done_status: String,
}

impl Default for RepoPollingSettings {
    fn default() -> Self {
        Self {
            auto_polling: false,
            auto_start_flow: String::new(),
            max_parallel_items: 0,
            project_keys: Vec::new(),
            item_types: default_item_types(),
            jira_summary_keywords: Vec::new(),
            github_labels: Vec::new(),
            github_title_keywords: Vec::new(),
            linked_items_in_prompt: LinkedItemsPromptMode::default(),
            ticket_context_max_description_bytes: 0,
            linked_issue_description_max_bytes: 0,
            jql_filter: String::new(),
            done_status: default_done_status(),
        }
    }
}

/// One per-user-per-workspace row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRepoPollingSettingsRow {
    pub user_id: String,
    pub workspace_name: String,
    pub settings: RepoPollingSettings,
    pub updated_at: i64,
}

/// Get the settings for `(user_id, workspace_name)`, or `None` if no row exists.
pub async fn get(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
) -> Result<Option<RepoPollingSettings>> {
    let row = adapter
        .query_optional(
            "SELECT settings_json FROM user_repo_polling_settings \
             WHERE user_id = ? AND workspace_name = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
            ],
        )
        .await?;
    let Some(r) = row else {
        return Ok(None);
    };
    let json = r.get_text(0)?;
    Ok(Some(decode_settings(&json, user_id, workspace_name)?))
}

/// List every row owned by `user_id`, most-recently-updated first.
pub async fn list_for_user(
    adapter: &DbAdapter,
    user_id: &str,
) -> Result<Vec<UserRepoPollingSettingsRow>> {
    let rows = adapter
        .query_all(
            "SELECT user_id, workspace_name, settings_json, updated_at \
             FROM user_repo_polling_settings WHERE user_id = ? ORDER BY updated_at DESC",
            vec![DbValue::Text(user_id.to_string())],
        )
        .await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        out.push(decode_full_row(r)?);
    }
    Ok(out)
}

/// Insert or replace the row for `(user_id, workspace_name)`.
///
/// NUL-byte guardrail across the string fields. The REST layer enforces all
/// other validation (key shape, interval floor, caps). `updated_at` = now.
pub async fn set(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
    settings: &RepoPollingSettings,
) -> Result<()> {
    if user_id.contains('\0') || workspace_name.contains('\0') {
        return Err(DbError::NulByte {
            field: "user_id_or_workspace_name",
        }
        .into());
    }
    let strings = settings
        .project_keys
        .iter()
        .chain(&settings.item_types)
        .chain(&settings.jira_summary_keywords)
        .chain(&settings.github_labels)
        .chain(&settings.github_title_keywords)
        .chain(std::iter::once(&settings.auto_start_flow))
        .chain(std::iter::once(&settings.jql_filter))
        .chain(std::iter::once(&settings.done_status));
    for s in strings {
        if s.contains('\0') {
            return Err(DbError::NulByte {
                field: "polling_settings_string",
            }
            .into());
        }
    }

    let settings_json =
        serde_json::to_string(settings).map_err(|e| DbError::CommandsJsonEncode {
            column: "settings_json",
            source: e,
        })?;
    let now = chrono::Utc::now().timestamp();

    let tail = super::upsert::build_update_tail(
        adapter.backend(),
        &["user_id", "workspace_name"],
        &["settings_json", "updated_at"],
    );
    let sql = format!(
        "INSERT INTO user_repo_polling_settings \
            (user_id, workspace_name, settings_json, updated_at) \
         VALUES (?, ?, ?, ?) {tail}"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
                DbValue::Text(settings_json),
                DbValue::I64(now),
            ],
        )
        .await?;
    Ok(())
}

/// Remove the row for `(user_id, workspace_name)`. Returns `true` if a row was
/// deleted, `false` if none existed.
pub async fn delete(adapter: &DbAdapter, user_id: &str, workspace_name: &str) -> Result<bool> {
    let affected = adapter
        .execute(
            "DELETE FROM user_repo_polling_settings WHERE user_id = ? AND workspace_name = ?",
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
            ],
        )
        .await?;
    Ok(affected > 0)
}

fn decode_full_row(row: &crate::db::DbRow) -> Result<UserRepoPollingSettingsRow> {
    let user_id = row.get_text(0)?;
    let workspace_name = row.get_text(1)?;
    let json = row.get_text(2)?;
    let updated_at = row.get_i64(3)?;
    let settings = decode_settings(&json, &user_id, &workspace_name)?;
    Ok(UserRepoPollingSettingsRow {
        user_id,
        workspace_name,
        settings,
        updated_at,
    })
}

fn decode_settings(json: &str, user_id: &str, workspace_name: &str) -> Result<RepoPollingSettings> {
    serde_json::from_str::<RepoPollingSettings>(json).map_err(|e| {
        DbError::CommandsJsonDecode {
            column: "settings_json",
            user_id: user_id.to_string(),
            workspace_name: workspace_name.to_string(),
            source: e,
        }
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::adapter::DbAdapter;
    use crate::db::migrate::DialectAwareMigrationSource;
    use crate::db::pool::{DbBackend, DbPool};
    use sqlx::sqlite::SqlitePool;

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
        DbAdapter::new(DbPool::Sqlite(pool))
    }

    async fn seed_user(adapter: &DbAdapter, username: &str, role: &str) -> String {
        let id = format!("u-{username}");
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, ?)",
                vec![
                    DbValue::Text(id.clone()),
                    DbValue::Text(username.to_string()),
                    DbValue::Text(role.to_string()),
                ],
            )
            .await
            .expect("seed user");
        id
    }

    fn sample() -> RepoPollingSettings {
        RepoPollingSettings {
            auto_polling: true,
            project_keys: vec!["PROJ".into(), "OPS".into()],
            jql_filter: "labels = urgent".into(),
            done_status: "Closed".into(),
            ..RepoPollingSettings::default()
        }
    }

    #[tokio::test]
    async fn get_returns_none_on_missing() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        assert!(get(&a, &alice, "frontend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_then_get_roundtrips() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let s = sample();
        set(&a, &alice, "frontend", &s).await.unwrap();
        let got = get(&a, &alice, "frontend").await.unwrap().unwrap();
        assert_eq!(got, s);
    }

    #[tokio::test]
    async fn empty_object_decodes_to_defaults() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        // Insert a row with the schema default '{}'.
        a.execute(
            "INSERT INTO user_repo_polling_settings (user_id, workspace_name, updated_at) \
             VALUES (?, 'frontend', 1)",
            vec![DbValue::Text(alice.clone())],
        )
        .await
        .unwrap();
        let got = get(&a, &alice, "frontend").await.unwrap().unwrap();
        assert_eq!(got, RepoPollingSettings::default());
        assert_eq!(got.item_types, vec!["Task", "Bug"]);
        assert!(!got.auto_polling);
        assert_eq!(got.done_status, "Done");
    }

    #[tokio::test]
    async fn set_overwrites_and_delete_works() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        set(&a, &alice, "frontend", &sample()).await.unwrap();
        let other = RepoPollingSettings {
            project_keys: vec!["CORE".into()],
            ..RepoPollingSettings::default()
        };
        set(&a, &alice, "frontend", &other).await.unwrap();
        assert_eq!(
            get(&a, &alice, "frontend")
                .await
                .unwrap()
                .unwrap()
                .project_keys,
            vec!["CORE".to_string()]
        );
        assert!(delete(&a, &alice, "frontend").await.unwrap());
        assert!(!delete(&a, &alice, "frontend").await.unwrap());
        assert!(get(&a, &alice, "frontend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn list_for_user_isolates_per_user() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let bob = seed_user(&a, "bob", "user").await;
        set(&a, &alice, "frontend", &sample()).await.unwrap();
        set(&a, &alice, "backend", &RepoPollingSettings::default())
            .await
            .unwrap();
        set(&a, &bob, "frontend", &sample()).await.unwrap();

        assert_eq!(list_for_user(&a, &alice).await.unwrap().len(), 2);
        let bob_rows = list_for_user(&a, &bob).await.unwrap();
        assert_eq!(bob_rows.len(), 1);
        assert_eq!(bob_rows[0].workspace_name, "frontend");
    }

    #[tokio::test]
    async fn nul_byte_rejected() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "user").await;
        let s = RepoPollingSettings {
            project_keys: vec!["PR\0OJ".into()],
            ..RepoPollingSettings::default()
        };
        assert!(set(&a, &alice, "frontend", &s).await.is_err());
        assert!(get(&a, &alice, "frontend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fk_cascade_on_user_delete() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice", "admin").await;
        let _bob = seed_user(&a, "bob", "admin").await;
        set(&a, &alice, "frontend", &sample()).await.unwrap();
        a.execute(
            "DELETE FROM users WHERE id = ?",
            vec![DbValue::Text(alice.clone())],
        )
        .await
        .unwrap();
        assert_eq!(list_for_user(&a, &alice).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn fk_rejects_unknown_user() {
        let a = fresh_adapter().await;
        assert!(set(&a, "nobody", "frontend", &sample()).await.is_err());
    }
}
