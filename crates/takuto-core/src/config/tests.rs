// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Cross-cutting Config integration tests (load, validate, runtime patches,
//! redaction). Per-submodule unit tests live next to their code in
//! `agent.rs`, `general.rs`, `git.rs`, `web.rs`, `template.rs`.

use super::*;
use std::io::Write;
use std::path::Path;
use tempfile::NamedTempFile;

fn valid_config_toml() -> &'static str {
    r#"
[general]
dry_mode = true
auto_polling = false
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["PROJ", "CORE"]
item_types = ["Task", "Bug"]

[git]
base_branch = "main"
repo_path = "/workspace"

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#
}

#[test]
fn test_load_valid_config() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(valid_config_toml().as_bytes()).unwrap();
    let config = Config::load(f.path()).unwrap();
    assert!(config.general.dry_mode);
    assert!(!config.general.auto_polling);
    assert_eq!(config.general.poll_interval_secs, 30);
    assert_eq!(config.jira.project_keys, vec!["PROJ", "CORE"]);
}

#[test]
fn test_load_missing_file() {
    let result = Config::load(Path::new("/nonexistent/config.toml"));
    assert!(result.is_err());
}

#[test]
fn test_defaults() {
    let config = Config::default();
    assert!(!config.general.dry_mode);
    assert!(config.general.auto_polling);
    assert_eq!(config.general.poll_interval_secs, 60);
    assert_eq!(config.web.port, 8080);
    assert_eq!(config.agent.effective_cursor_model(), "Auto");
    assert_eq!(config.git.remote, "origin");
}

#[test]
fn test_validate_poll_interval_too_low() {
    let mut config = Config::default();
    config.general.poll_interval_secs = 5;
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_empty_item_types() {
    let mut config = Config::default();
    config.jira.item_types.clear();
    assert!(config.validate().is_err());
}

#[test]
fn test_validate_empty_git_remote() {
    let mut config = Config::default();
    config.git.remote = "   ".to_string();
    assert!(config.validate().is_err());
}

#[test]
fn legacy_commands_table_is_silently_ignored() {
    // Stale `[commands]` in a user's config.toml is ignored at load time.
    // The startup warning is logged but the config still parses cleanly
    // (no panic, no error).
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(
        br#"
[general]
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[commands]
worktree_init_commands = ["echo legacy"]
pre_install = ["should be ignored"]

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
    )
    .unwrap();
    // Must load without error — the legacy [commands] table is dropped.
    Config::load(f.path()).expect("load must succeed with stale [commands]");
}

#[test]
fn legacy_run_commands_array_is_silently_ignored() {
    // Stale `[[run_commands]]` entries are ignored at load time.
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(
        br#"
[general]
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
step_timeout_secs = 600

[[run_commands]]
name = "Dev Server"
command = "npm run dev"
"#,
    )
    .unwrap();
    Config::load(f.path()).expect("load must succeed with stale [[run_commands]]");
}

#[test]
fn runtime_patch_json_unknown_top_level_field_fails() {
    let err = serde_json::from_str::<RuntimeDashboardConfigPatch>(r#"{"jira":{}}"#).unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("unknown field") || s.contains("Unknown field"),
        "unexpected serde error: {s}"
    );
}

#[test]
fn runtime_patch_merge_general_only() {
    let mut c = Config::default();
    let patch: RuntimeDashboardConfigPatch =
        serde_json::from_str(r#"{"general":{"max_concurrent_workflows":7}}"#).unwrap();
    c.apply_runtime_dashboard_patch(patch).unwrap();
    assert_eq!(c.general.max_concurrent_workflows, 7);
}

#[test]
fn runtime_patch_empty_top_level_errors() {
    let mut c = Config::default();
    let patch: RuntimeDashboardConfigPatch = serde_json::from_str("{}").unwrap();
    assert!(c.apply_runtime_dashboard_patch(patch).is_err());
}

#[test]
fn runtime_patch_web_field_now_unknown() {
    // `web` is no longer an accepted top-level patch key.
    let err = serde_json::from_str::<RuntimeDashboardConfigPatch>(r#"{"web":{}}"#).unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("unknown field") || s.contains("Unknown field"),
        "expected unknown field error: {s}"
    );
}

// -- CORS origin tests --

#[test]
fn cors_origins_defaults_to_empty_vec() {
    let config = Config::default();
    assert!(config.web.cors_origins.is_empty());
}

#[test]
fn cors_origins_deserialized_from_toml() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(
        br#"
[general]
poll_interval_secs = 30
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
repo_path = "/workspace"
[web]
port = 8080
cors_origins = ["http://example.com:3000"]
[agent]
step_timeout_secs = 600
"#,
    )
    .unwrap();
    let config = Config::load(f.path()).unwrap();
    assert_eq!(config.web.cors_origins, vec!["http://example.com:3000"]);
}

#[test]
fn cors_origins_invalid_in_toml_rejected_by_load() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(
        br#"
[general]
poll_interval_secs = 30
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
repo_path = "/workspace"
[web]
port = 8080
cors_origins = ["localhost:3000"]
[agent]
step_timeout_secs = 600
"#,
    )
    .unwrap();
    let err = Config::load(f.path()).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("http://") || msg.contains("https://"),
        "expected scheme error from Config::load, got: {msg}"
    );
}

#[test]
fn cors_origins_omitted_in_toml_defaults_to_empty() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(valid_config_toml().as_bytes()).unwrap();
    let config = Config::load(f.path()).unwrap();
    assert!(config.web.cors_origins.is_empty());
}

// -- Validation: valid origins --

#[test]
fn validate_accepts_http_origin() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["http://localhost:3000".into()];
    assert!(config.validate().is_ok());
}

#[test]
fn validate_accepts_https_origin() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["https://app.example.com".into()];
    assert!(config.validate().is_ok());
}

#[test]
fn validate_accepts_multiple_origins() {
    let mut config = Config::default();
    config.web.cors_origins = vec![
        "http://localhost:3000".into(),
        "https://prod.example.com".into(),
    ];
    assert!(config.validate().is_ok());
}

#[test]
fn validate_accepts_empty_cors_origins() {
    let config = Config::default();
    assert!(config.validate().is_ok());
}

// -- Validation: invalid origins --

#[test]
fn validate_rejects_origin_without_scheme() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["localhost:3000".into()];
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("http://") || err.to_string().contains("https://"),
        "expected scheme error: {}",
        err
    );
}

#[test]
fn validate_rejects_ftp_scheme() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["ftp://files.example.com".into()];
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("http://") || err.to_string().contains("https://"),
        "expected scheme error: {}",
        err
    );
}

#[test]
fn validate_rejects_origin_with_path() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["http://localhost:3000/api".into()];
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("path"),
        "expected path error: {}",
        err
    );
}

#[test]
fn validate_rejects_origin_with_trailing_slash() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["http://localhost:3000/".into()];
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("path"),
        "expected path error: {}",
        err
    );
}

#[test]
fn validate_rejects_empty_string_origin() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["".into()];
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("empty"),
        "expected empty error: {}",
        err
    );
}

#[test]
fn validate_rejects_whitespace_only_origin() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["   ".into()];
    let err = config.validate().unwrap_err();
    assert!(
        err.to_string().contains("empty"),
        "expected empty error: {}",
        err
    );
}

#[test]
fn validate_rejects_if_any_origin_invalid() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["http://localhost:3000".into(), "bad".into()];
    assert!(config.validate().is_err());
}

// -- Redaction --

#[test]
fn redacted_clone_preserves_cors_origins() {
    let mut config = Config::default();
    config.web.cors_origins = vec!["http://localhost:3000".into()];
    let redacted = config.redacted_for_api_clone();
    assert_eq!(redacted.web.cors_origins, vec!["http://localhost:3000"]);
}

// -- Runtime patch rejection --

#[test]
fn runtime_patch_rejects_cors_origins_field() {
    let err = serde_json::from_str::<RuntimeDashboardConfigPatch>(
        r#"{"web":{"cors_origins":["http://x"]}}"#,
    )
    .unwrap_err();
    let s = err.to_string();
    assert!(
        s.contains("unknown field") || s.contains("Unknown field"),
        "expected unknown field error: {s}"
    );
}

// -- generate_report --

#[test]
fn generate_report_defaults_to_false() {
    let config = Config::default();
    assert!(!config.general.generate_report);
}

#[test]
fn generate_report_true_from_toml() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(
        br#"
[general]
generate_report = true
poll_interval_secs = 30

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"
repo_path = "/workspace"

[web]
port = 8080

[agent]
step_timeout_secs = 600
"#,
    )
    .unwrap();
    let config = Config::load(f.path()).unwrap();
    assert!(config.general.generate_report);
}

#[test]
fn generate_report_false_when_omitted() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(valid_config_toml().as_bytes()).unwrap();
    let config = Config::load(f.path()).unwrap();
    assert!(!config.general.generate_report);
}

// ─── Phase 1: provider sub-tables, migration, validation ─────────────

#[test]
fn load_ignores_legacy_flat_agent_keys() {
    // The removed flat `[agent]` keys (cursor_cli / cursor_model / model) are
    // now unknown fields: a config still carrying them loads cleanly and the
    // keys are inert (the sub-tables keep their defaults).
    let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "cursor"
cursor_cli = "agent-custom"
cursor_model = "gpt-4.1"
model = "claude-3-5"
"#;
    let cfg = Config::load_from_str(toml).expect("load");
    assert!(cfg.agent.providers.cursor.cli.is_empty());
    assert!(cfg.agent.providers.cursor.model.is_empty());
    assert!(cfg.agent.providers.claude.model.is_empty());
    // Effective accessors fall back to their hard-coded defaults.
    assert_eq!(cfg.agent.effective_cursor_cli(), "agent");
    assert_eq!(cfg.agent.effective_cursor_model(), "Auto");
}

#[test]
fn load_reads_cursor_sub_table_values() {
    let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "cursor"

[agent.providers.cursor]
cli = "sub-table-agent"
"#;
    let cfg = Config::load_from_str(toml).expect("load");
    assert_eq!(cfg.agent.providers.cursor.cli, "sub-table-agent");
    assert_eq!(cfg.agent.effective_cursor_cli(), "sub-table-agent");
}

/// T-CFG-002 (Phase 1, P1): a non-empty `[agent.providers.cursor].base_url`
/// is rejected with a stable, user-visible message — Cursor's CLI does not
/// honour custom endpoints (amendment A1).
#[test]
fn load_rejects_cursor_base_url_with_friendly_message() {
    let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"

[agent.providers.cursor]
base_url = "https://proxy.example.com"
"#;
    let err = Config::load_from_str(toml).expect_err("cursor base_url must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("Cursor CLI custom endpoints not supported"),
        "expected friendly message, got: {msg}"
    );
}

/// Empty / default `[agent.providers.cursor].base_url` continues to load
/// (the validator only fires on non-empty values) — guarantees the new
/// check doesn't break clean configs.
#[test]
fn load_accepts_empty_cursor_base_url() {
    let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"

[agent.providers.cursor]
base_url = ""
"#;
    Config::load_from_str(toml).expect("empty cursor.base_url must load");
}

#[test]
fn load_rejects_denied_extra_arg_in_subtable() {
    let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"

[agent.providers.claude]
extra_args = ["--dangerously-skip-permissions"]
"#;
    let err = Config::load_from_str(toml).expect_err("denied flag must reject");
    let msg = err.to_string();
    assert!(
        msg.contains("extra_args_denied"),
        "expected extra_args_denied in error, got: {msg}"
    );
}

#[test]
fn load_rejects_unknown_available_provider() {
    let toml = r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2

[jira]
project_keys = ["X"]
item_types = ["Task"]

[git]
base_branch = "main"

[web]
port = 8080

[agent]
provider = "claude"
available_providers = ["claude", "bogus"]
"#;
    let err = Config::load_from_str(toml).expect_err("unknown provider must reject");
    assert!(err.to_string().contains("bogus"));
}

#[test]
fn default_available_providers_lists_all_v1() {
    let cfg = Config::default();
    assert_eq!(
        cfg.agent.available_providers,
        vec!["claude", "cursor", "codex", "opencode"]
    );
}

#[test]
fn to_toml_round_trip_preserves_provider_sub_tables() {
    let mut cfg = Config::default();
    cfg.agent.providers.claude.model = "claude-3-5".into();
    cfg.agent.providers.claude.base_url = "https://proxy.example.com".into();
    cfg.agent.providers.cursor.cli = "agent-custom".into();
    cfg.agent.providers.cursor.model = "gpt-4.1".into();
    cfg.agent.providers.codex.provider_name = "lmstudio".into();
    cfg.agent.providers.codex.base_url = "http://lm-studio:1234/v1".into();
    cfg.agent.providers.opencode.model = "anthropic/claude-3-5-sonnet".into();

    let serialized = cfg.to_toml_string().expect("serialize");
    let parsed: Config = toml::from_str(&serialized).expect("re-parse");

    assert_eq!(parsed.agent.providers.claude.model, "claude-3-5");
    assert_eq!(
        parsed.agent.providers.claude.base_url,
        "https://proxy.example.com"
    );
    assert_eq!(parsed.agent.providers.cursor.cli, "agent-custom");
    assert_eq!(parsed.agent.providers.cursor.model, "gpt-4.1");
    assert_eq!(parsed.agent.providers.codex.provider_name, "lmstudio");
    assert_eq!(
        parsed.agent.providers.codex.base_url,
        "http://lm-studio:1234/v1"
    );
    assert_eq!(
        parsed.agent.providers.opencode.model,
        "anthropic/claude-3-5-sonnet"
    );
}

#[test]
fn codex_provider_serde_round_trips_lowercase() {
    let cfg: Config = toml::from_str(
        r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
[web]
port = 8080
[agent]
provider = "codex"
"#,
    )
    .expect("parse");
    assert_eq!(cfg.agent.provider, AiAgentProvider::Codex);
    assert_eq!(cfg.agent.provider.as_str(), "codex");
}

#[test]
fn opencode_provider_serde_round_trips_lowercase() {
    let cfg: Config = toml::from_str(
        r#"
[general]
poll_interval_secs = 30
max_concurrent_workflows = 2
[jira]
project_keys = ["X"]
item_types = ["Task"]
[git]
base_branch = "main"
[web]
port = 8080
[agent]
provider = "opencode"
"#,
    )
    .expect("parse");
    assert_eq!(cfg.agent.provider, AiAgentProvider::OpenCode);
    assert_eq!(cfg.agent.provider.as_str(), "opencode");
}

// ─── provisioning_sha ───────────────────────────────────────────────

fn config_with_provisioning(cmds: &[&str]) -> Config {
    let mut cfg = Config::default();
    cfg.provisioning.install_commands = cmds.iter().map(|s| s.to_string()).collect();
    cfg
}

/// T-PROV-SHA-001: same list → same SHA (the boot-side fast-path
/// gate works deterministically across restarts and machines).
#[test]
fn provisioning_sha_is_stable_for_same_content() {
    let a = config_with_provisioning(&["cmd-1", "cmd-2"]);
    let b = config_with_provisioning(&["cmd-1", "cmd-2"]);
    assert_eq!(a.provisioning_sha(), b.provisioning_sha());
    // And it's not a random uuid pretending to be a SHA — must be
    // 64 lowercase hex chars (sha256 hex digest).
    let sha = a.provisioning_sha();
    assert_eq!(sha.len(), 64, "sha must be 64 hex chars; got {sha}");
    assert!(
        sha.chars()
            .all(|c| c.is_ascii_hexdigit() && (!c.is_ascii_alphabetic() || c.is_ascii_lowercase()))
    );
}

/// T-PROV-SHA-002: edit a command → SHA changes (cache invalidation).
#[test]
fn provisioning_sha_changes_when_command_text_changes() {
    let a = config_with_provisioning(&["install foo"]);
    let b = config_with_provisioning(&["install bar"]);
    assert_ne!(a.provisioning_sha(), b.provisioning_sha());
}

/// T-PROV-SHA-003: order matters (later commands can depend on
/// artifacts from earlier ones — `[a, b]` is NOT the same install
/// as `[b, a]`). The SHA must reflect that.
#[test]
fn provisioning_sha_order_sensitive() {
    let a = config_with_provisioning(&["cmd-1", "cmd-2"]);
    let b = config_with_provisioning(&["cmd-2", "cmd-1"]);
    assert_ne!(a.provisioning_sha(), b.provisioning_sha());
}

/// T-PROV-SHA-004: empty list yields a known stable SHA so the
/// entrypoint can fast-path-skip even on the empty-config case
/// without re-running every boot.
#[test]
fn provisioning_sha_empty_list_is_stable_known_value() {
    let cfg = Config::default();
    assert!(cfg.provisioning.install_commands.is_empty());
    // sha256 of `[]` (the JSON-encoded empty array) is well-known.
    // Recompute on the fly so a change to the canonicalization
    // scheme fails this test loudly rather than silently shifting
    // the gate value.
    use sha2::{Digest, Sha256};
    let expected = format!("{:x}", Sha256::digest(b"[]"));
    assert_eq!(cfg.provisioning_sha(), expected);
}

/// T-PROV-SHA-005: whitespace inside a command is part of the
/// command's identity — the canonicalizer must NOT collapse spaces
/// (admins may rely on multi-space formatting inside a heredoc /
/// args list).
#[test]
fn provisioning_sha_preserves_inner_whitespace() {
    let a = config_with_provisioning(&["cmd  --flag  value"]);
    let b = config_with_provisioning(&["cmd --flag value"]);
    assert_ne!(a.provisioning_sha(), b.provisioning_sha());
}

// ─── [polling] section ──────────────────────────────────────────────

#[test]
fn polling_defaults_are_empty_and_unlimited() {
    let cfg = Config::default();
    assert!(cfg.polling.auto_start_flow.is_empty());
    assert_eq!(cfg.polling.max_parallel_items, 0);
    assert!(!cfg.polling.max_parallel_per_user);
    assert!(cfg.polling.jira.summary_keywords.is_empty());
    assert!(cfg.polling.github.labels.is_empty());
    assert!(cfg.polling.github.title_keywords.is_empty());
}

#[test]
fn polling_omitted_in_toml_uses_defaults() {
    let mut f = NamedTempFile::new().unwrap();
    f.write_all(valid_config_toml().as_bytes()).unwrap();
    let cfg = Config::load(f.path()).unwrap();
    assert!(cfg.polling.auto_start_flow.is_empty());
    assert_eq!(cfg.polling.max_parallel_items, 0);
}

#[test]
fn polling_round_trips_through_toml() {
    let mut cfg = Config::default();
    cfg.polling.auto_start_flow = "implement-ticket".to_string();
    cfg.polling.max_parallel_items = 5;
    cfg.polling.max_parallel_per_user = true;
    cfg.polling.jira.summary_keywords = vec!["crash".into(), "urgent".into()];
    cfg.polling.github.labels = vec!["bug".into()];
    cfg.polling.github.title_keywords = vec!["panic".into()];

    let serialized = cfg.to_toml_string().expect("serialize");
    let parsed: Config = toml::from_str(&serialized).expect("re-parse");

    assert_eq!(parsed.polling.auto_start_flow, "implement-ticket");
    assert_eq!(parsed.polling.max_parallel_items, 5);
    assert!(parsed.polling.max_parallel_per_user);
    assert_eq!(
        parsed.polling.jira.summary_keywords,
        vec!["crash", "urgent"]
    );
    assert_eq!(parsed.polling.github.labels, vec!["bug"]);
    assert_eq!(parsed.polling.github.title_keywords, vec!["panic"]);
}

#[test]
fn validate_rejects_blank_jira_summary_keyword() {
    let mut cfg = Config::default();
    cfg.polling.jira.summary_keywords = vec!["ok".into(), "   ".into()];
    let err = cfg.validate().unwrap_err();
    assert!(
        err.to_string().contains("summary_keywords"),
        "expected summary_keywords error: {err}"
    );
}

#[test]
fn validate_rejects_blank_github_label() {
    let mut cfg = Config::default();
    cfg.polling.github.labels = vec!["".into()];
    let err = cfg.validate().unwrap_err();
    assert!(
        err.to_string().contains("labels"),
        "expected labels error: {err}"
    );
}

#[test]
fn validate_rejects_blank_github_title_keyword() {
    let mut cfg = Config::default();
    cfg.polling.github.title_keywords = vec!["\t".into()];
    let err = cfg.validate().unwrap_err();
    assert!(
        err.to_string().contains("title_keywords"),
        "expected title_keywords error: {err}"
    );
}

#[test]
fn validate_accepts_non_empty_polling_filters() {
    let mut cfg = Config::default();
    cfg.polling.auto_start_flow = "anything".into();
    cfg.polling.max_parallel_items = 0; // 0 = unlimited is valid
    cfg.polling.jira.summary_keywords = vec!["crash".into()];
    cfg.polling.github.labels = vec!["bug".into()];
    cfg.polling.github.title_keywords = vec!["panic".into()];
    assert!(cfg.validate().is_ok());
}

#[test]
fn matches_any_keyword_empty_list_matches_everything() {
    assert!(matches_any_keyword("anything at all", &[]));
}

#[test]
fn matches_any_keyword_is_case_insensitive_substring() {
    let kws = vec!["Crash".to_string()];
    assert!(matches_any_keyword("App CRASHED on launch", &kws));
    assert!(matches_any_keyword("crash report", &kws));
    assert!(!matches_any_keyword("everything is fine", &kws));
}

#[test]
fn matches_any_keyword_any_of_many() {
    let kws = vec!["bug".to_string(), "urgent".to_string()];
    assert!(matches_any_keyword("URGENT: fix login", &kws));
    assert!(!matches_any_keyword("nice to have", &kws));
}

#[test]
fn matches_any_keyword_trims_keywords() {
    let kws = vec!["  crash  ".to_string()];
    assert!(matches_any_keyword("a crash happened", &kws));
}
