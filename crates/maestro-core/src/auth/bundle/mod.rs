// Copyright 2026 Alexandre Obellianne
// Licensed under the Functional Source License 1.1 (FSL-1.1-ALv2). See LICENSE.

//! Per-workflow worker secrets bundle.
//!
//! Builds an opaque container of tmpfs-mounted secret files + non-secret
//! env vars that the worker entrypoint sources, then deletes. The bundle's
//! `TempDir` field RAII-cleans the secrets directory when the workflow
//! teardown drops the value.
//!
//! Threat model: secrets MUST NOT be passed as `-e KEY=value` to `docker
//! run` (visible via `docker inspect`). Instead, the secret files live on
//! a host tmpfs (`mode 0700` parent, `0400` files) and are bind-mounted
//! read-only into the worker at `/run/maestro-secrets/`. The worker
//! entrypoint `source`s each file into an env var then `rm`s the on-disk
//! copy to shrink the blast radius if the worker is later compromised.
//!
//! Split into five files (previously a single 1244-LOC `auth/bundle.rs`):
//! - `mod.rs`           — re-exports + full test suite
//! - `types.rs`         — [`WorkerSecretsBundle`] struct + `SECRET_FILE_*` constants
//! - `tempdir.rs`       — [`cleanup_orphan_secrets`] + per-bundle dir
//! - `write_secret.rs`  — mode-0400 atomic write (Unix + non-Unix)
//! - `unseal.rs`        — open `provider_credential` + `cli_state` rows
//! - `assembler.rs`     — [`build`], [`build_for_endpoint`], [`pin_for_workflow`]

mod assembler;
mod opencode_config;
mod tempdir;
mod types;
mod unseal;
mod write_secret;

pub use assembler::{build, build_for_endpoint, pin_for_workflow};
pub use tempdir::cleanup_orphan_secrets;
pub use types::{
    SECRET_FILE_CLAUDE, SECRET_FILE_CLAUDE_SESSION, SECRET_FILE_CODEX, SECRET_FILE_CURSOR,
    SECRET_FILE_GH, SECRET_FILE_OPENCODE, SECRETS_DIR_REL, WORKER_SECRETS_MOUNTPOINT,
    WorkerSecretsBundle,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{MasterKey, seal};
    use crate::config::{AiAgentProvider, Config};
    use crate::db::{Database, provider_credentials};
    use crate::github::auth_resolver::GitAuthResolver;
    use crate::workflow::snapshot::AuthPin;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn fixed_config(provider: AiAgentProvider) -> Config {
        let mut cfg = Config::default();
        cfg.agent.provider = provider;
        cfg
    }

    /// OpenCode requires non-empty base_url + model (validator enforces);
    /// helper centralises the minimum-viable fixture for bundle tests.
    fn fixed_opencode_config(base_url: &str, model: &str) -> Config {
        let mut cfg = fixed_config(AiAgentProvider::OpenCode);
        cfg.agent.providers.opencode.base_url = base_url.into();
        cfg.agent.providers.opencode.model = model.into();
        cfg
    }

    fn fixed_pin(provider: AiAgentProvider) -> AuthPin {
        AuthPin {
            provider: provider.as_str().to_string(),
            provider_credential_row_id: None,
            github_mode: "app".to_string(),
            github_credential_row_id: None,
            started_at: "2026-05-18T00:00:00Z".to_string(),
        }
    }

    fn db_with_master_key() -> Database {
        Database::open_in_memory()
            .unwrap()
            .with_test_master_key(MasterKey::from_bytes([0x42; 32]))
    }

    async fn seed_user(db: &Database, user_id: &str) {
        use crate::db::DbValue;
        db.adapter()
            .execute(
                "INSERT INTO users (id, username, role) VALUES (?, ?, 'user')",
                vec![
                    DbValue::Text(user_id.to_string()),
                    DbValue::Text(user_id.to_string()),
                ],
            )
            .await
            .unwrap();
    }

    async fn seed_provider_credential(
        db: &Database,
        user_id: &str,
        provider: &str,
        plaintext: &[u8],
    ) {
        let mk = db.master_key().unwrap().key.clone();
        let sealed = seal(&mk, plaintext).unwrap();
        let adapter = db.adapter();
        let mut tx = adapter.begin().await.unwrap();
        provider_credentials::upsert(
            &mut tx,
            user_id,
            provider,
            provider_credentials::ProviderCredentialKind::ApiKey,
            &sealed,
            "{}",
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    fn make_resolver(db: Database) -> Arc<GitAuthResolver> {
        Arc::new(GitAuthResolver::new(db, None))
    }

    /// Seed a cli_state row carrying a minimal valid Claude session blob.
    /// Uses the test DB's master key so `seal()`/`open()` round-trip
    /// cleanly.
    async fn seed_claude_cli_state(db: &Database, user_id: &str, json: &[u8]) {
        let mk = db.master_key().unwrap().key.clone();
        let sealed = seal(&mk, json).unwrap();
        let adapter = db.adapter();
        let mut tx = adapter.begin().await.unwrap();
        provider_credentials::upsert(
            &mut tx,
            user_id,
            "claude",
            provider_credentials::ProviderCredentialKind::CliState,
            &sealed,
            r#"{"kind":"cli_state"}"#,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
    }

    #[tokio::test]
    async fn build_writes_provider_secret_file_when_credential_present() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant-test-token").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let secret_path = bundle
            .provider_secret_file
            .as_ref()
            .expect("provider secret file");
        let bytes = std::fs::read(secret_path).expect("read secret file");
        assert_eq!(bytes, b"sk-ant-test-token");

        // host_dir is the parent of the secret file.
        assert!(secret_path.starts_with(bundle.host_dir()));
    }

    #[tokio::test]
    async fn build_returns_none_secret_when_no_credential_and_default_allowed() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let mut cfg = fixed_config(AiAgentProvider::Claude);
        cfg.agent.providers.claude.allow_shared_default = true;
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build with shared default fallback");
        assert!(bundle.provider_secret_file.is_none());
    }

    #[tokio::test]
    async fn build_errors_when_no_credential_and_default_disallowed() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude); // allow_shared_default defaults to false
        let pin = fixed_pin(AiAgentProvider::Claude);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("no credential + no fallback must error");
        assert!(err.to_string().contains("provider_credential_missing"));
    }

    #[tokio::test]
    async fn build_errors_when_master_key_unavailable() {
        let db = Database::open_in_memory().unwrap();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("must error when master key not loaded");
        assert!(err.to_string().contains("master_key_unavailable"));
    }

    #[tokio::test]
    async fn temp_dir_cleanup_removes_secret_files_on_drop() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice").await.unwrap();
        let secret_path = bundle.provider_secret_file.clone().unwrap();
        assert!(secret_path.exists());

        // Drop the bundle; RAII should remove the directory.
        drop(bundle);
        assert!(
            !secret_path.exists(),
            "secret file must be cleaned up when WorkerSecretsBundle drops"
        );
    }

    #[tokio::test]
    async fn build_emits_anthropic_base_url_env_for_claude() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let mut cfg = fixed_config(AiAgentProvider::Claude);
        cfg.agent.providers.claude.base_url = "https://proxy.example.com".into();
        cfg.agent.providers.claude.extra_args = vec!["--max-turns".into(), "50".into()];
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice").await.unwrap();
        assert!(
            bundle
                .extra_env
                .iter()
                .any(|(k, v)| k == "ANTHROPIC_BASE_URL" && v == "https://proxy.example.com")
        );
        assert_eq!(bundle.extra_args, vec!["--max-turns", "50"]);
        // MAESTRO_AUTH_BUNDLE is the worker entrypoint's discriminator.
        assert!(
            bundle
                .extra_env
                .iter()
                .any(|(k, v)| k == "MAESTRO_AUTH_BUNDLE" && v == "1")
        );
    }

    #[tokio::test]
    async fn pin_for_workflow_captures_provider_and_github_mode() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let cfg = fixed_config(AiAgentProvider::Claude);

        let pin = pin_for_workflow(&cfg, &db, "u-alice").await.expect("pin");
        assert_eq!(pin.provider, "claude");
        assert!(pin.provider_credential_row_id.is_some());
        assert_eq!(pin.github_mode, "app"); // No GitHub PAT seeded.
        assert!(!pin.started_at.is_empty());
    }

    #[tokio::test]
    async fn pin_for_workflow_with_no_credential_returns_none_id() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let cfg = fixed_config(AiAgentProvider::Claude);

        let pin = pin_for_workflow(&cfg, &db, "u-alice").await.unwrap();
        assert_eq!(pin.provider, "claude");
        assert!(pin.provider_credential_row_id.is_none());
    }

    // ─── build_for_endpoint ──────────────────────────────────────────────
    //
    // The endpoint-side wrapper synthesizes an ephemeral pin internally.
    // It must behave identically to `build` for credential lookup, but
    // requires no caller-supplied pin (so improve_ticket / open_editor
    // / start_run_command can be wired without first computing a pin).

    #[tokio::test]
    async fn build_for_endpoint_returns_bundle_when_credential_present() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant-endpoint").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);

        let bundle = build_for_endpoint(&cfg, &db, &resolver, "u-alice")
            .await
            .expect("endpoint bundle build");
        let secret_path = bundle
            .provider_secret_file
            .as_ref()
            .expect("provider secret file");
        let bytes = std::fs::read(secret_path).expect("read secret file");
        assert_eq!(bytes, b"sk-ant-endpoint");
    }

    #[tokio::test]
    async fn build_for_endpoint_surfaces_credential_required_for_no_cred_and_no_default() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        // Default config has `allow_shared_default = false` for every
        // provider — so a user with no credential MUST surface the
        // structured `provider_credential_missing` error so the dashboard
        // can prompt them to paste an API key.
        let cfg = fixed_config(AiAgentProvider::Claude);

        let err = build_for_endpoint(&cfg, &db, &resolver, "u-alice")
            .await
            .expect_err("must error when caller has no credential");
        assert!(err.to_string().contains("provider_credential_missing"));
    }

    /// `apply_secrets_bundle_to_args` (defined in `container.rs`) must:
    ///   1. Bind-mount the bundle's host_dir RO at /run/maestro-secrets, AND
    ///   2. Copy every `extra_env` pair as `-e KEY=VALUE`, AND
    ///   3. NEVER write secret bytes into the argv (those live in tmpfs).
    ///
    /// We exercise the helper from bundle.rs because `WorkerSecretsBundle`'s
    /// `_temp_dir` field is private to this module, so the by-hand
    /// constructor is only reachable here.
    #[test]
    fn apply_secrets_bundle_to_args_mounts_ro_and_copies_extra_env_only() {
        use std::path::Path;
        let dir = TempDir::new().unwrap();
        let host_dir_path = dir.path().to_path_buf();
        let bundle = WorkerSecretsBundle {
            provider: AiAgentProvider::Claude,
            provider_secret_file: Some(host_dir_path.join("claude")),
            claude_session_file: None,
            github_token_file: Some(host_dir_path.join("gh")),
            git_author_name: Some("alice".into()),
            git_author_email: Some("alice@noreply".into()),
            base_url: Some("https://proxy.example".into()),
            extra_args: vec![],
            extra_env: vec![
                ("MAESTRO_AUTH_BUNDLE".into(), "1".into()),
                ("ANTHROPIC_BASE_URL".into(), "https://proxy.example".into()),
                ("GIT_AUTHOR_NAME".into(), "alice".into()),
            ],
            opencode_config_dir: None,
            _temp_dir: dir,
        };

        let mut args: Vec<String> = Vec::new();
        crate::container::apply_secrets_bundle_to_args(&mut args, &bundle);

        // The mount must be RO and point at the bundle's host_dir.
        let mount_expected = format!(
            "{}:/run/maestro-secrets:ro",
            Path::new(&host_dir_path).to_string_lossy()
        );
        let has_volume = args
            .windows(2)
            .any(|w| w[0] == "-v" && w[1] == mount_expected);
        assert!(
            has_volume,
            "expected RO mount {mount_expected:?} in args = {args:?}"
        );

        // All extra_env entries must be present as -e KEY=VALUE.
        let has_env = |k: &str, v: &str| -> bool {
            let needle = format!("{k}={v}");
            args.windows(2).any(|w| w[0] == "-e" && w[1] == needle)
        };
        assert!(has_env("MAESTRO_AUTH_BUNDLE", "1"));
        assert!(has_env("ANTHROPIC_BASE_URL", "https://proxy.example"));
        assert!(has_env("GIT_AUTHOR_NAME", "alice"));

        // CRITICAL: argv must NOT carry the bundled secret env names —
        // those flow through tmpfs files only. Token bytes never appear
        // in argv at all because the bundle exposes only file paths,
        // not byte slices.
        let argv_joined = args.join(" ");
        assert!(
            !argv_joined.contains("CLAUDE_CODE_OAUTH_TOKEN"),
            "secret env name must not appear in argv"
        );
        assert!(
            !argv_joined.contains("CURSOR_API_KEY"),
            "secret env name must not appear in argv"
        );
        assert!(
            !argv_joined.contains("GH_TOKEN="),
            "secret env name must not appear in argv"
        );
    }

    #[test]
    fn debug_does_not_leak_token_bytes() {
        // Build a stub bundle by hand so we can inspect Debug without going
        // through the async builder.
        let dir = TempDir::new().unwrap();
        let bundle = WorkerSecretsBundle {
            provider: AiAgentProvider::Claude,
            provider_secret_file: Some(dir.path().join("claude")),
            claude_session_file: None,
            github_token_file: Some(dir.path().join("gh")),
            git_author_name: Some("alice".into()),
            git_author_email: Some("alice@noreply".into()),
            base_url: Some("https://proxy".into()),
            extra_args: vec![],
            extra_env: vec![("MAESTRO_AUTH_BUNDLE".into(), "1".into())],
            opencode_config_dir: None,
            _temp_dir: dir,
        };
        let s = format!("{bundle:?}");
        // Paths can appear; token bytes cannot — they're never set as fields.
        assert!(s.contains("WorkerSecretsBundle"));
        assert!(s.contains("has_provider_secret"));
        assert!(s.contains("has_claude_session"));
        assert!(s.contains("has_github_token"));
    }

    // ─── claude cli_state in bundle ──────────────────────────────────────

    /// Minimal valid Claude session blob — three required oauthAccount
    /// keys + a couple of harmless extras the validator must ignore.
    fn fixture_session_json() -> Vec<u8> {
        serde_json::json!({
            "oauthAccount": {
                "accountUuid": "00000000-0000-0000-0000-000000000001",
                "emailAddress": "alice@example.com",
                "organizationUuid": "11111111-1111-1111-1111-111111111111",
                "organizationType": "claude_team",
                "seatTier": "team_standard",
            },
            "lastUpdateCheck": "2026-05-19T00:00:00Z",
        })
        .to_string()
        .into_bytes()
    }

    /// Happy path: both api_key AND cli_state rows present → bundle has
    /// BOTH files populated and the session file contents round-trip.
    #[tokio::test]
    async fn build_writes_claude_session_file_when_cli_state_row_present() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let session_blob = fixture_session_json();
        seed_claude_cli_state(&db, "u-alice", &session_blob).await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let session_path = bundle
            .claude_session_file
            .as_ref()
            .expect("claude_session_file must be Some when cli_state row exists");
        let on_disk = std::fs::read(session_path).expect("read session file");
        assert_eq!(on_disk, session_blob);
        // The mount filename must match the documented constant so
        // BUNDLE_SOURCING_SH's `cp` reads it.
        assert!(
            session_path.ends_with(SECRET_FILE_CLAUDE_SESSION),
            "session file must use the SECRET_FILE_CLAUDE_SESSION name; got {session_path:?}"
        );
        // API key file is also present (the user has both rows).
        assert!(bundle.provider_secret_file.is_some());
    }

    /// No cli_state row → `claude_session_file` is None (api_key path
    /// unchanged). Saves succeed without it; this is the most common
    /// case for direct-API users.
    #[tokio::test]
    async fn build_omits_session_file_when_no_cli_state_row() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        assert!(
            bundle.claude_session_file.is_none(),
            "claude_session_file must be None when no cli_state row exists"
        );
    }

    /// Non-Claude provider with a (somehow) seeded cli_state row → bundle
    /// MUST NOT write the session file (the unseal helper is gated on
    /// `provider == Claude`). Defence-in-depth.
    #[tokio::test]
    async fn build_does_not_emit_session_file_for_non_claude_provider() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "cursor", b"sk-curs").await;
        // Sneak a claude cli_state row in (UI rejects this at POST time,
        // but the bundle builder must not key off non-active providers).
        seed_claude_cli_state(&db, "u-alice", &fixture_session_json()).await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Cursor);
        let pin = fixed_pin(AiAgentProvider::Cursor);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        assert!(
            bundle.claude_session_file.is_none(),
            "non-Claude bundle must NOT include claude_session_file even when \
             a rogue cli_state row exists in the DB"
        );
    }

    /// Corrupt cli_state blob (valid bytes through `seal`, but the
    /// plaintext isn't valid JSON) → typed error at unseal time. Defence
    /// in depth — the validator catches this at save time, but the
    /// bundle builder must not silently ship garbage to the worker.
    #[tokio::test]
    async fn build_errors_when_cli_state_blob_is_not_json() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        // Skip the validator: write garbage bytes via the seed helper.
        seed_claude_cli_state(&db, "u-alice", b"this is not json {[").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("invalid JSON must surface a typed error");
        assert!(
            err.to_string().contains("cli_state blob is not valid JSON"),
            "error must explain the cli_state JSON problem; got: {err}"
        );
    }

    // ─── data_dir-based secrets dir + cleanup ──────────────────────────

    /// Build a real Database backed by a temp data_dir (not in-memory) so
    /// `data_dir()` returns a real path the bundle can sit under.
    fn db_with_master_key_and_disk_data_dir() -> (Database, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("disk-backed tempdir");
        let db = Database::open(dir.path(), true)
            .expect("open disk DB")
            .with_test_master_key(MasterKey::from_bytes([0xAA; 32]));
        (db, dir)
    }

    /// When a disk-backed `data_dir` is available, the bundle's TempDir
    /// is created under `<data_dir>/runtime/secrets/<random>` — NOT
    /// under the process `/tmp` (the task #43 bug).
    #[tokio::test]
    async fn bundle_temp_dir_is_under_data_dir_runtime_secrets() {
        let (db, data_dir_keepalive) = db_with_master_key_and_disk_data_dir();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let host_dir = bundle.host_dir().to_path_buf();
        let expected_root = data_dir_keepalive.path().join(SECRETS_DIR_REL);
        assert!(
            host_dir.starts_with(&expected_root),
            "bundle's host_dir must live under {} (got: {})",
            expected_root.display(),
            host_dir.display()
        );
        // The dir is a real tempfile-style random child of secrets/.
        assert!(host_dir.is_dir());
    }

    /// `cleanup_orphan_secrets` is best-effort: missing dir → Ok(0).
    #[test]
    fn cleanup_orphan_secrets_returns_zero_when_dir_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nonexistent = dir.path().join("does-not-exist");
        let n = cleanup_orphan_secrets(&nonexistent).expect("must not error");
        assert_eq!(n, 0);
    }

    /// Pre-seed `<data_dir>/runtime/secrets/` with two fake orphan
    /// directories and a stray file; the sweep removes both dirs and
    /// leaves the file alone (real-world it'd be empty anyway).
    #[test]
    fn cleanup_orphan_secrets_removes_subdirs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(SECRETS_DIR_REL);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("orphan-a")).unwrap();
        std::fs::write(root.join("orphan-a/claude"), "stale-token").unwrap();
        std::fs::create_dir_all(root.join("orphan-b")).unwrap();
        // A stray file at the same depth — sweep skips files (its `is_dir()`
        // guard) so callers don't accidentally lose metadata.
        std::fs::write(root.join("stray-file"), "metadata").unwrap();

        let n = cleanup_orphan_secrets(dir.path()).expect("sweep ok");
        assert_eq!(n, 2, "must remove both orphan dirs");
        assert!(!root.join("orphan-a").exists());
        assert!(!root.join("orphan-b").exists());
        assert!(
            root.join("stray-file").exists(),
            "files outside dir entries survive"
        );
    }

    // ─── OpenCode self-hosted spec (2026-05-27) ──────────────────────────

    /// Spec §2.3 happy path: provider=opencode + base_url + model + a
    /// user bearer → bundle holds `opencode_config_dir`, the file at
    /// `<dir>/opencode.json` parses, baseURL/apiKey/models match.
    #[tokio::test]
    async fn build_opencode_emits_config_dir_with_bearer_baseurl_model() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "opencode", b"user-bearer").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_opencode_config("http://lm-studio:1234/v1", "lmstudio/qwen3-coder");
        let pin = fixed_pin(AiAgentProvider::OpenCode);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let cfg_dir = bundle
            .opencode_config_dir
            .as_ref()
            .expect("opencode_config_dir must be Some for OpenCode");
        let cfg_file = cfg_dir.join("opencode.json");
        assert!(cfg_file.exists(), "opencode.json must exist in cfg dir");

        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&cfg_file).unwrap()).unwrap();
        let p = &v["provider"]["self_hosted"];
        assert_eq!(p["options"]["baseURL"], "http://lm-studio:1234/v1");
        assert_eq!(p["options"]["apiKey"], "user-bearer");
        assert!(p["models"]["lmstudio/qwen3-coder"].is_object());

        // Spec §2.1: OpenCode workflows do NOT receive `/run/maestro-secrets/opencode`.
        assert!(
            bundle.provider_secret_file.is_none(),
            "OpenCode bundles must not write /run/maestro-secrets/opencode \
             (spec 2026-05-27 §2.1 — bearer lives in opencode.json instead)"
        );
        // Spec §2.2: `OPENCODE_PROVIDER_BASE_URL` env is dropped.
        assert!(
            !bundle
                .extra_env
                .iter()
                .any(|(k, _)| k == "OPENCODE_PROVIDER_BASE_URL"),
            "OpenCode bundles must not emit OPENCODE_PROVIDER_BASE_URL env \
             (spec 2026-05-27 §2.2 — replaced by opencode.json)"
        );
    }

    /// Spec §2.3: no bearer saved → apiKey defaults to "lm-studio" (LM
    /// Studio dummy). Validates the most common deployment shape (single
    /// admin + LM Studio without auth).
    #[tokio::test]
    async fn build_opencode_uses_dummy_apikey_when_user_has_no_bearer() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        let resolver = make_resolver(db.clone());
        let mut cfg = fixed_opencode_config("http://lm-studio:1234/v1", "lmstudio/qwen3-coder");
        // Allow the shared-default fallback — the user has no credential
        // and LM Studio doesn't care about the key value.
        cfg.agent.providers.opencode.allow_shared_default = true;
        let pin = fixed_pin(AiAgentProvider::OpenCode);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        let cfg_file = bundle
            .opencode_config_dir
            .as_ref()
            .expect("opencode_config_dir")
            .join("opencode.json");
        let v: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&cfg_file).unwrap()).unwrap();
        assert_eq!(
            v["provider"]["self_hosted"]["options"]["apiKey"],
            "lm-studio"
        );
    }

    /// Defence in depth: an OpenCode bundle built with a hand-crafted
    /// Config that bypasses the validator (empty base_url) MUST surface
    /// a typed error rather than silently writing a broken
    /// opencode.json. The validator catches this on Config::load and on
    /// PUT /api/config/agent, but unit-test paths can construct Configs
    /// directly.
    #[tokio::test]
    async fn build_opencode_errors_when_base_url_empty() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "opencode", b"key").await;
        let resolver = make_resolver(db.clone());
        let mut cfg = fixed_opencode_config("", "lmstudio/qwen3-coder");
        // Bypass validator — direct field write.
        cfg.agent.providers.opencode.base_url.clear();
        let pin = fixed_pin(AiAgentProvider::OpenCode);

        let err = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect_err("empty base_url must surface a typed shim error");
        assert!(
            err.to_string().contains("base_url is empty"),
            "error must explain the violation; got: {err}"
        );
    }

    /// Non-OpenCode providers MUST NOT get an `opencode_config_dir`. The
    /// assembler should leave it None for Claude / Cursor / Codex — the
    /// shim is OpenCode-only.
    #[tokio::test]
    async fn build_claude_does_not_emit_opencode_config_dir() {
        let db = db_with_master_key();
        seed_user(&db, "u-alice").await;
        seed_provider_credential(&db, "u-alice", "claude", b"sk-ant").await;
        let resolver = make_resolver(db.clone());
        let cfg = fixed_config(AiAgentProvider::Claude);
        let pin = fixed_pin(AiAgentProvider::Claude);

        let bundle = build(&cfg, &db, &resolver, &pin, "u-alice")
            .await
            .expect("build bundle");
        assert!(
            bundle.opencode_config_dir.is_none(),
            "non-OpenCode bundles must not carry opencode_config_dir"
        );
    }

    /// Arc-storage strategy proof. The route handlers stash an
    /// `Arc<WorkerSecretsBundle>` clone in AppState so the bundle's
    /// `TempDir` outlives the route's stack scope. This test asserts the
    /// expected lifetime semantics: cloning the Arc does NOT trigger
    /// cleanup, only dropping the LAST clone does. If anyone refactors
    /// the bundle storage to a non-Arc shape (e.g. `Box<Bundle>`),
    /// this test fails loudly.
    #[test]
    fn bundle_temp_dir_survives_clone_drop_until_last_arc_released() {
        let bundle = Arc::new(WorkerSecretsBundle::for_tests(
            AiAgentProvider::Claude,
            vec![("MAESTRO_AUTH_BUNDLE".into(), "1".into())],
        ));
        let host_dir = bundle.host_dir().to_path_buf();
        assert!(
            host_dir.exists(),
            "bundle's TempDir must exist immediately after construction"
        );

        // Clone the Arc twice — simulates AppState stashing one clone
        // and the route handler passing a second to the container runner.
        let arc1 = bundle.clone();
        let arc2 = bundle.clone();
        // Drop the original — TempDir must still survive.
        drop(bundle);
        assert!(
            host_dir.exists(),
            "TempDir must survive while clones are held"
        );
        // Drop arc1 — still one clone alive.
        drop(arc1);
        assert!(
            host_dir.exists(),
            "TempDir must survive while at least one Arc clone exists"
        );
        // Drop the last clone — RAII fires now.
        drop(arc2);
        assert!(
            !host_dir.exists(),
            "TempDir must be rm-rf'd when the last Arc clone drops"
        );
    }
}
