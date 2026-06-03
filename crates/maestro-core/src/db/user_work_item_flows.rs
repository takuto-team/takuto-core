// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-user, per-workspace work-item flows.
//!
//! A **flow** is the unit a user triggers on a work-item card. It groups one
//! or more ordered **steps**, each a single agent prompt with optional skills.
//! Every signed-in user owns their own flow list per workspace.
//!
//! Storage mirrors `user_worktree_commands`: one row per
//! `(user_id, workspace_name)` with the whole ordered list serialized into a
//! single JSON column (`flows_json`). The list is small (capped at
//! [`MAX_FLOWS_PER_WORKSPACE`]) and always read/written atomically by the UI,
//! so a single blob beats a row-per-flow table.
//!
//! Row presence carries meaning:
//! - **No row** — not yet seeded for this workspace.
//! - **`[]`** — seeded then emptied by the user (render the empty-state).
//!
//! The two states are kept distinct so seeding stays idempotent and never
//! re-fills a list the user intentionally cleared.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::SkillRef;
use crate::db::{DbAdapter, DbValue};
use crate::error::Result;

use super::DbError;

/// Hard cap on the number of flows a single user may keep per workspace.
pub const MAX_FLOWS_PER_WORKSPACE: usize = 20;

/// Maximum length of a flow's kebab-case slug. The slug is the
/// `workflow_def_runs` key and survives renames as long as it is unchanged.
const MAX_SLUG_LEN: usize = 64;

/// A single step within a flow: one agent prompt with optional skills.
///
/// `Serialize`/`Deserialize` round-trip this through the `flows_json` column.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserFlowStep {
    pub name: String,
    pub prompt: String,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
}

/// A named, ordered list of steps a user triggers on a work-item card.
///
/// `depends_on` references other flows by **name** (not slug): an upstream
/// flow must have completed at least once on a work item before this flow's
/// button becomes enabled.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserFlow {
    pub name: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    pub steps: Vec<UserFlowStep>,
}

impl UserFlow {
    /// Stable kebab-case slug used as the `workflow_def_runs` key. Lower-cased,
    /// every run of non-alphanumeric characters collapsed to a single `-`,
    /// trimmed of leading/trailing `-`, and length-capped.
    pub fn slug(&self) -> String {
        slugify(&self.name)
    }
}

/// Lower-case, kebab-case, length-capped slug for a flow name.
pub fn slugify(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    let mut slug: String = trimmed.chars().take(MAX_SLUG_LEN).collect();
    // A trailing dash can survive the length cap; trim once more.
    while slug.ends_with('-') {
        slug.pop();
    }
    slug
}

/// Why a proposed flow list was rejected. The REST layer maps each variant to
/// a 4xx with a structured body; the editor mirrors these checks client-side.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlowValidationError {
    #[error("too many flows: {count} exceeds the maximum of {max}")]
    TooManyFlows { count: usize, max: usize },

    #[error("a flow name must not be empty")]
    EmptyFlowName,

    #[error("duplicate flow name: {name}")]
    DuplicateFlowName { name: String },

    #[error("flows '{first}' and '{second}' produce the same slug '{slug}'")]
    DuplicateSlug {
        first: String,
        second: String,
        slug: String,
    },

    #[error("a flow name must contain at least one slug-able character: {name}")]
    EmptySlug { name: String },

    #[error("flow '{flow}' must have at least one step")]
    NoSteps { flow: String },

    #[error("flow '{flow}' has a step with an empty name")]
    EmptyStepName { flow: String },

    #[error("flow '{flow}', step '{step}' has an empty prompt")]
    EmptyStepPrompt { flow: String, step: String },

    #[error("flow '{flow}', step '{step}' has a skill with an empty name")]
    EmptySkillName { flow: String, step: String },

    #[error("flow '{flow}' depends on unknown flow '{dependency}'")]
    UnknownDependency { flow: String, dependency: String },

    #[error("dependency cycle detected involving flow '{flow}'")]
    DependencyCycle { flow: String },
}

/// Validate a proposed flow list. Enforces the cap, unique names, unique
/// slugs, per-flow/step required fields, dependency references, and the
/// absence of dependency cycles.
///
/// NUL-byte rejection is the DAO's job ([`set`]); this function checks the
/// structural rules the UI and REST layer share.
pub fn validate_user_flows(flows: &[UserFlow]) -> std::result::Result<(), FlowValidationError> {
    if flows.len() > MAX_FLOWS_PER_WORKSPACE {
        return Err(FlowValidationError::TooManyFlows {
            count: flows.len(),
            max: MAX_FLOWS_PER_WORKSPACE,
        });
    }

    let mut seen_names: HashSet<&str> = HashSet::new();
    let mut seen_slugs: HashMap<String, &str> = HashMap::new();

    for flow in flows {
        if flow.name.trim().is_empty() {
            return Err(FlowValidationError::EmptyFlowName);
        }
        if !seen_names.insert(flow.name.as_str()) {
            return Err(FlowValidationError::DuplicateFlowName {
                name: flow.name.clone(),
            });
        }

        let slug = slugify(&flow.name);
        if slug.is_empty() {
            return Err(FlowValidationError::EmptySlug {
                name: flow.name.clone(),
            });
        }
        if let Some(&first) = seen_slugs.get(&slug) {
            return Err(FlowValidationError::DuplicateSlug {
                first: first.to_string(),
                second: flow.name.clone(),
                slug,
            });
        }
        seen_slugs.insert(slug, flow.name.as_str());

        if flow.steps.is_empty() {
            return Err(FlowValidationError::NoSteps {
                flow: flow.name.clone(),
            });
        }
        for step in &flow.steps {
            if step.name.trim().is_empty() {
                return Err(FlowValidationError::EmptyStepName {
                    flow: flow.name.clone(),
                });
            }
            if step.prompt.trim().is_empty() {
                return Err(FlowValidationError::EmptyStepPrompt {
                    flow: flow.name.clone(),
                    step: step.name.clone(),
                });
            }
            for skill in &step.skills {
                if skill.name.trim().is_empty() {
                    return Err(FlowValidationError::EmptySkillName {
                        flow: flow.name.clone(),
                        step: step.name.clone(),
                    });
                }
            }
        }
    }

    // Dependency references must point at siblings.
    for flow in flows {
        for dep in &flow.depends_on {
            if !seen_names.contains(dep.as_str()) {
                return Err(FlowValidationError::UnknownDependency {
                    flow: flow.name.clone(),
                    dependency: dep.clone(),
                });
            }
        }
    }

    detect_cycle(flows)
}

/// DFS three-colour cycle detection over the flow dependency graph (by name).
fn detect_cycle(flows: &[UserFlow]) -> std::result::Result<(), FlowValidationError> {
    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    let deps: HashMap<&str, &[String]> = flows
        .iter()
        .map(|f| (f.name.as_str(), f.depends_on.as_slice()))
        .collect();
    let mut colors: HashMap<&str, Color> = deps.keys().map(|&k| (k, Color::White)).collect();

    fn visit<'a>(
        node: &'a str,
        deps: &HashMap<&'a str, &'a [String]>,
        colors: &mut HashMap<&'a str, Color>,
    ) -> std::result::Result<(), FlowValidationError> {
        colors.insert(node, Color::Gray);
        if let Some(children) = deps.get(node) {
            for child in *children {
                match colors.get(child.as_str()).copied() {
                    Some(Color::Gray) => {
                        return Err(FlowValidationError::DependencyCycle {
                            flow: child.clone(),
                        });
                    }
                    Some(Color::White) => {
                        // The child is a known sibling (validated earlier).
                        let key = deps
                            .get_key_value(child.as_str())
                            .map(|(k, _)| *k)
                            .unwrap_or(child.as_str());
                        visit(key, deps, colors)?;
                    }
                    _ => {}
                }
            }
        }
        colors.insert(node, Color::Black);
        Ok(())
    }

    let nodes: Vec<&str> = deps.keys().copied().collect();
    for node in nodes {
        if colors.get(node).copied() == Some(Color::White) {
            visit(node, &deps, &mut colors)?;
        }
    }
    Ok(())
}

/// Get the flow list for `(user_id, workspace_name)`.
///
/// `None` means no row exists (not yet seeded for this workspace);
/// `Some(vec![])` means the user has an explicitly empty list. JSON parse
/// failures surface as [`DbError::CommandsJsonDecode`].
pub async fn get(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
) -> Result<Option<Vec<UserFlow>>> {
    let row = adapter
        .query_optional(
            "SELECT flows_json FROM user_work_item_flows \
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
    let flows_json = r.get_text(0)?;
    let flows = decode_flows(&flows_json, user_id, workspace_name)?;
    Ok(Some(flows))
}

/// Insert or replace the flow list for `(user_id, workspace_name)`.
///
/// An empty list is a legitimate state. Rejects any string containing a NUL
/// byte before persisting. `updated_at` is set to the current Unix
/// milliseconds. This is the last-line guardrail against physically corrupt
/// data; structural validation lives in [`validate_user_flows`].
pub async fn set(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
    flows: &[UserFlow],
) -> Result<()> {
    guard_nul(user_id, workspace_name, flows)?;

    let flows_json = encode_flows(flows)?;
    let now = chrono::Utc::now().timestamp_millis();

    let tail = super::upsert::build_update_tail(
        adapter.backend(),
        &["user_id", "workspace_name"],
        &["flows_json", "updated_at"],
    );
    let sql = format!(
        "INSERT INTO user_work_item_flows \
            (user_id, workspace_name, flows_json, updated_at) \
         VALUES (?, ?, ?, ?) {tail}"
    );
    adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
                DbValue::Text(flows_json),
                DbValue::I64(now),
            ],
        )
        .await?;
    Ok(())
}

/// Seed the defaults for `(user_id, workspace_name)` only if no row exists.
///
/// `INSERT ... ON CONFLICT DO NOTHING`. Returns `true` if a row was created,
/// `false` if one already existed. Idempotent: never overwrites a list the
/// user has customized or intentionally emptied.
pub async fn seed_if_absent(
    adapter: &DbAdapter,
    user_id: &str,
    workspace_name: &str,
    defaults: &[UserFlow],
) -> Result<bool> {
    guard_nul(user_id, workspace_name, defaults)?;

    let flows_json = encode_flows(defaults)?;
    let now = chrono::Utc::now().timestamp_millis();

    let tail = super::upsert::build_ignore_tail(adapter.backend(), &["user_id", "workspace_name"]);
    let sql = format!(
        "INSERT INTO user_work_item_flows \
            (user_id, workspace_name, flows_json, updated_at) \
         VALUES (?, ?, ?, ?) {tail}"
    );
    let affected = adapter
        .execute(
            &sql,
            vec![
                DbValue::Text(user_id.to_string()),
                DbValue::Text(workspace_name.to_string()),
                DbValue::Text(flows_json),
                DbValue::I64(now),
            ],
        )
        .await?;
    Ok(affected > 0)
}

/// Reject any NUL byte across every string we are about to persist.
fn guard_nul(user_id: &str, workspace_name: &str, flows: &[UserFlow]) -> Result<()> {
    if user_id.contains('\0') || workspace_name.contains('\0') {
        return Err(DbError::NulByte {
            field: "user_id_or_workspace_name",
        }
        .into());
    }
    for flow in flows {
        if flow.name.contains('\0') {
            return Err(DbError::NulByte { field: "flow_name" }.into());
        }
        for dep in &flow.depends_on {
            if dep.contains('\0') {
                return Err(DbError::NulByte {
                    field: "flow_depends_on",
                }
                .into());
            }
        }
        for step in &flow.steps {
            if step.name.contains('\0') || step.prompt.contains('\0') {
                return Err(DbError::NulByte {
                    field: "flow_step_name_or_prompt",
                }
                .into());
            }
            for skill in &step.skills {
                if skill.name.contains('\0') || skill.args.iter().any(|a| a.contains('\0')) {
                    return Err(DbError::NulByte {
                        field: "flow_step_skill",
                    }
                    .into());
                }
            }
        }
    }
    Ok(())
}

fn encode_flows(flows: &[UserFlow]) -> Result<String> {
    serde_json::to_string(flows).map_err(|e| {
        DbError::CommandsJsonEncode {
            column: "flows_json",
            source: e,
        }
        .into()
    })
}

fn decode_flows(json: &str, user_id: &str, workspace_name: &str) -> Result<Vec<UserFlow>> {
    serde_json::from_str::<Vec<UserFlow>>(json).map_err(|e| {
        DbError::CommandsJsonDecode {
            column: "flows_json",
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

    async fn seed_user(adapter: &DbAdapter, username: &str) -> String {
        let id = format!("u-{username}");
        adapter
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, ?)",
                vec![
                    DbValue::Text(id.clone()),
                    DbValue::Text(username.to_string()),
                    DbValue::Text("user".to_string()),
                ],
            )
            .await
            .expect("seed user");
        id
    }

    fn step(name: &str, prompt: &str) -> UserFlowStep {
        UserFlowStep {
            name: name.to_string(),
            prompt: prompt.to_string(),
            skills: Vec::new(),
        }
    }

    fn flow(name: &str, deps: &[&str], steps: Vec<UserFlowStep>) -> UserFlow {
        UserFlow {
            name: name.to_string(),
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            steps,
        }
    }

    // ── slug ────────────────────────────────────────────────────────────

    #[test]
    fn slugify_kebab_cases_and_collapses() {
        assert_eq!(slugify("Implement Ticket"), "implement-ticket");
        assert_eq!(slugify("implement-ticket"), "implement-ticket");
        assert_eq!(slugify("  Fix   the   Bug!! "), "fix-the-bug");
        assert_eq!(slugify("Café (v2)"), "caf-v2");
    }

    #[test]
    fn slugify_length_caps_without_trailing_dash() {
        let long = "a".repeat(100);
        assert_eq!(slugify(&long).len(), MAX_SLUG_LEN);
        // A name whose cap boundary lands on a separator must not keep it.
        let name = format!("{}  tail", "b".repeat(MAX_SLUG_LEN - 1));
        let slug = slugify(&name);
        assert!(!slug.ends_with('-'), "slug must not end with dash: {slug}");
    }

    // ── validation: happy path ────────────────────────────────────────────

    #[test]
    fn validate_accepts_valid_list_with_deps() {
        let flows = vec![
            flow("Build", &[], vec![step("compile", "cargo build")]),
            flow("Test", &["Build"], vec![step("run", "cargo test")]),
        ];
        assert!(validate_user_flows(&flows).is_ok());
    }

    #[test]
    fn validate_accepts_empty_list() {
        assert!(validate_user_flows(&[]).is_ok());
    }

    #[test]
    fn validate_accepts_step_with_skills() {
        let flows = vec![flow(
            "Implement",
            &[],
            vec![UserFlowStep {
                name: "code".to_string(),
                prompt: "do it".to_string(),
                skills: vec![SkillRef {
                    name: "address-ticket".to_string(),
                    args: vec!["--headless".to_string()],
                }],
            }],
        )];
        assert!(validate_user_flows(&flows).is_ok());
    }

    // ── validation: each error ────────────────────────────────────────────

    #[test]
    fn validate_rejects_over_cap() {
        let flows: Vec<UserFlow> = (0..=MAX_FLOWS_PER_WORKSPACE)
            .map(|i| flow(&format!("flow {i}"), &[], vec![step("s", "p")]))
            .collect();
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::TooManyFlows { .. })
        ));
    }

    #[test]
    fn validate_accepts_exactly_cap() {
        let flows: Vec<UserFlow> = (0..MAX_FLOWS_PER_WORKSPACE)
            .map(|i| flow(&format!("flow {i}"), &[], vec![step("s", "p")]))
            .collect();
        assert!(validate_user_flows(&flows).is_ok());
    }

    #[test]
    fn validate_rejects_empty_name() {
        let flows = vec![flow("   ", &[], vec![step("s", "p")])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::EmptyFlowName)
        ));
    }

    #[test]
    fn validate_rejects_duplicate_name() {
        let flows = vec![
            flow("Build", &[], vec![step("s", "p")]),
            flow("Build", &[], vec![step("s", "p")]),
        ];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::DuplicateFlowName { .. })
        ));
    }

    #[test]
    fn validate_rejects_colliding_slugs() {
        let flows = vec![
            flow("Implement Ticket", &[], vec![step("s", "p")]),
            flow("implement-ticket", &[], vec![step("s", "p")]),
        ];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::DuplicateSlug { .. })
        ));
    }

    #[test]
    fn validate_rejects_name_with_no_slug_chars() {
        let flows = vec![flow("---", &[], vec![step("s", "p")])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::EmptySlug { .. })
        ));
    }

    #[test]
    fn validate_rejects_no_steps() {
        let flows = vec![flow("Build", &[], vec![])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::NoSteps { .. })
        ));
    }

    #[test]
    fn validate_rejects_empty_step_name() {
        let flows = vec![flow("Build", &[], vec![step("  ", "p")])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::EmptyStepName { .. })
        ));
    }

    #[test]
    fn validate_rejects_empty_step_prompt() {
        let flows = vec![flow("Build", &[], vec![step("compile", "   ")])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::EmptyStepPrompt { .. })
        ));
    }

    #[test]
    fn validate_rejects_empty_skill_name() {
        let flows = vec![flow(
            "Build",
            &[],
            vec![UserFlowStep {
                name: "compile".to_string(),
                prompt: "p".to_string(),
                skills: vec![SkillRef {
                    name: "  ".to_string(),
                    args: vec![],
                }],
            }],
        )];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::EmptySkillName { .. })
        ));
    }

    #[test]
    fn validate_rejects_unknown_dependency() {
        let flows = vec![flow("Test", &["Build"], vec![step("s", "p")])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::UnknownDependency { .. })
        ));
    }

    #[test]
    fn validate_rejects_direct_cycle() {
        let flows = vec![
            flow("A", &["B"], vec![step("s", "p")]),
            flow("B", &["A"], vec![step("s", "p")]),
        ];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::DependencyCycle { .. })
        ));
    }

    #[test]
    fn validate_rejects_self_cycle() {
        let flows = vec![flow("A", &["A"], vec![step("s", "p")])];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::DependencyCycle { .. })
        ));
    }

    #[test]
    fn validate_rejects_transitive_cycle() {
        let flows = vec![
            flow("A", &["B"], vec![step("s", "p")]),
            flow("B", &["C"], vec![step("s", "p")]),
            flow("C", &["A"], vec![step("s", "p")]),
        ];
        assert!(matches!(
            validate_user_flows(&flows),
            Err(FlowValidationError::DependencyCycle { .. })
        ));
    }

    // ── DAO round-trips ────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_returns_none_when_absent() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        assert!(get(&a, &alice, "backend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn set_then_get_round_trips_in_order() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        let flows = vec![
            flow("Build", &[], vec![step("compile", "cargo build")]),
            flow(
                "Test",
                &["Build"],
                vec![step("unit", "cargo test"), step("e2e", "run e2e")],
            ),
        ];
        set(&a, &alice, "backend", &flows).await.unwrap();

        let got = get(&a, &alice, "backend").await.unwrap().unwrap();
        assert_eq!(got, flows, "order and content must round-trip");
    }

    #[tokio::test]
    async fn set_empty_list_is_distinct_from_absent() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        set(&a, &alice, "backend", &[]).await.unwrap();

        let got = get(&a, &alice, "backend").await.unwrap();
        assert_eq!(got, Some(vec![]), "empty list must persist as Some([])");
    }

    #[tokio::test]
    async fn set_overwrites_existing() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        set(
            &a,
            &alice,
            "backend",
            &[flow("Build", &[], vec![step("s", "p")])],
        )
        .await
        .unwrap();
        set(
            &a,
            &alice,
            "backend",
            &[flow("Deploy", &[], vec![step("s", "p")])],
        )
        .await
        .unwrap();

        let got = get(&a, &alice, "backend").await.unwrap().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "Deploy");
    }

    #[tokio::test]
    async fn seed_if_absent_inserts_then_is_idempotent() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        let defaults = vec![flow("Build", &[], vec![step("s", "p")])];

        assert!(
            seed_if_absent(&a, &alice, "backend", &defaults)
                .await
                .unwrap(),
            "first seed creates a row"
        );
        assert!(
            !seed_if_absent(&a, &alice, "backend", &defaults)
                .await
                .unwrap(),
            "second seed is a no-op"
        );
    }

    #[tokio::test]
    async fn seed_if_absent_does_not_refill_emptied_list() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        // User deliberately empties their list.
        set(&a, &alice, "backend", &[]).await.unwrap();

        let created = seed_if_absent(
            &a,
            &alice,
            "backend",
            &[flow("Build", &[], vec![step("s", "p")])],
        )
        .await
        .unwrap();
        assert!(
            !created,
            "seed must not overwrite an intentionally empty list"
        );
        assert_eq!(get(&a, &alice, "backend").await.unwrap(), Some(vec![]));
    }

    #[tokio::test]
    async fn flows_are_scoped_per_workspace() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        set(
            &a,
            &alice,
            "cargo-repo",
            &[flow("Cargo Build", &[], vec![step("s", "cargo build")])],
        )
        .await
        .unwrap();
        set(
            &a,
            &alice,
            "npm-repo",
            &[flow("NPM Build", &[], vec![step("s", "npm run build")])],
        )
        .await
        .unwrap();

        assert_eq!(
            get(&a, &alice, "cargo-repo").await.unwrap().unwrap()[0].name,
            "Cargo Build"
        );
        assert_eq!(
            get(&a, &alice, "npm-repo").await.unwrap().unwrap()[0].name,
            "NPM Build"
        );
    }

    #[tokio::test]
    async fn set_rejects_nul_bytes() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;

        let bad_name = vec![flow("Bad\0Name", &[], vec![step("s", "p")])];
        assert!(set(&a, &alice, "backend", &bad_name).await.is_err());

        let bad_prompt = vec![flow("Build", &[], vec![step("s", "pro\0mpt")])];
        assert!(set(&a, &alice, "backend", &bad_prompt).await.is_err());

        // Nothing persisted by the failures.
        assert!(get(&a, &alice, "backend").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn fk_cascade_drops_rows_on_user_delete() {
        let a = fresh_adapter().await;
        let alice = seed_user(&a, "alice").await;
        set(
            &a,
            &alice,
            "backend",
            &[flow("Build", &[], vec![step("s", "p")])],
        )
        .await
        .unwrap();

        a.execute(
            "DELETE FROM users WHERE id = ?",
            vec![DbValue::Text(alice.clone())],
        )
        .await
        .unwrap();

        assert!(get(&a, &alice, "backend").await.unwrap().is_none());
    }
}
