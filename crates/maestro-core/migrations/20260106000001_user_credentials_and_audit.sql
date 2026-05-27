-- Plan-11 step 2 — hand-translated port of MIGRATION_V6 (Phase 2a
-- per-user credentials foundation: 04_architecture.md §3.1).
--
-- Four new tables — provider credentials, GitHub PAT, credential audit,
-- onboarding state. Envelope encryption uses the BLOB-typed `ciphertext`,
-- `nonce`, `wrapped_dek`, `wnonce` columns (the DialectAware transformer
-- rewrites `BLOB` to `BYTEA` for Postgres).
--
-- Notes on portability changes vs V6's SQLite original:
--   • Auto-incrementing PKs (`id INTEGER PRIMARY KEY AUTOINCREMENT`) are
--     rewritten per-backend by the transformer:
--       SQLite → unchanged
--       Postgres → `id BIGSERIAL PRIMARY KEY`
--       MySQL → `id BIGINT AUTO_INCREMENT PRIMARY KEY`
--   • Timestamps switch from `TEXT DEFAULT (strftime(...))` to
--     `BIGINT` columns with no default. Application code (after
--     plan-11 step 3) sets the unix-seconds value explicitly.
--     `last_validated_at` / `last_used_at` / `expires_at` are nullable.

CREATE TABLE user_provider_credentials (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider VARCHAR(32) NOT NULL,
    kind VARCHAR(32) NOT NULL,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    wrapped_dek BLOB NOT NULL,
    wnonce BLOB NOT NULL,
    metadata_json TEXT NOT NULL DEFAULT '{}',
    inactive INTEGER NOT NULL DEFAULT 0,
    last_validated_at BIGINT,
    last_used_at BIGINT,
    created_at BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL DEFAULT 0,
    expires_at BIGINT,
    UNIQUE(user_id, provider, kind)
);
CREATE INDEX idx_user_provider_credentials_lookup
    ON user_provider_credentials(user_id, provider, inactive);

CREATE TABLE user_github_credentials (
    user_id VARCHAR(64) PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    wrapped_dek BLOB NOT NULL,
    wnonce BLOB NOT NULL,
    github_login VARCHAR(255) NOT NULL,
    scopes_json TEXT NOT NULL,
    sign_commits INTEGER NOT NULL DEFAULT 1,
    last_validated_at BIGINT,
    created_at BIGINT NOT NULL DEFAULT 0,
    updated_at BIGINT NOT NULL DEFAULT 0
);

CREATE TABLE credential_audit (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    actor_user_id VARCHAR(64) REFERENCES users(id),
    kind VARCHAR(32) NOT NULL,
    provider VARCHAR(32),
    event VARCHAR(64) NOT NULL,
    outcome VARCHAR(32) NOT NULL,
    error_code VARCHAR(64),
    at BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX idx_credential_audit_user ON credential_audit(user_id, at);

CREATE TABLE onboarding_state (
    user_id VARCHAR(64) PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    step_1_ticketing VARCHAR(32),
    step_2_provider VARCHAR(32),
    step_3_github VARCHAR(32),
    step_4_credentials VARCHAR(32),
    completed_at BIGINT,
    updated_at BIGINT NOT NULL DEFAULT 0
);
