-- Initial schema: users, credentials, recovery. This file is the
-- canonical source of truth for the V1 shape.
--
-- Portability rules applied:
--   • TEXT PRIMARY KEY → VARCHAR(64) PRIMARY KEY (MySQL rejects TEXT PK)
--   • TEXT (free-form values) stays TEXT
--   • Timestamp columns: TEXT (RFC3339 strings) — matches the app DAOs.
--     The original schema.rs default `strftime('...','now')` is dropped
--     because the application always binds an explicit timestamp.
--   • BLOB is left as `BLOB` in the source file; the DialectAware
--     transformer rewrites it to `BYTEA` for Postgres only.
--   • INTEGER PRIMARY KEY AUTOINCREMENT is left as-is; the transformer
--     rewrites it per backend.

CREATE TABLE IF NOT EXISTS users (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    username VARCHAR(255) UNIQUE NOT NULL,
    role VARCHAR(16) NOT NULL DEFAULT 'user' CHECK(role IN ('admin', 'user')),
    suspended INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS credentials (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind VARCHAR(16) NOT NULL CHECK(kind IN ('password', 'passkey')),
    data BLOB NOT NULL,
    label TEXT,
    created_at TEXT NOT NULL DEFAULT '',
    last_used_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_credentials_user_id ON credentials(user_id);

CREATE TABLE IF NOT EXISTS recovery_codes (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    code_hash BLOB NOT NULL,
    used INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL DEFAULT ''
);
CREATE INDEX IF NOT EXISTS idx_recovery_codes_user_id ON recovery_codes(user_id);

CREATE TABLE IF NOT EXISTS user_repositories (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    repo_url TEXT NOT NULL,
    local_path TEXT NOT NULL,
    added_at TEXT NOT NULL DEFAULT '',
    UNIQUE(user_id, repo_url)
);
CREATE INDEX IF NOT EXISTS idx_user_repositories_user_id ON user_repositories(user_id);

CREATE TABLE IF NOT EXISTS container_users (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    container_id VARCHAR(128) NOT NULL,
    container_type VARCHAR(16) NOT NULL CHECK(container_type IN ('workflow', 'terminal', 'editor')),
    os_username VARCHAR(64) NOT NULL,
    created_at TEXT NOT NULL DEFAULT '',
    destroyed_at TEXT,
    UNIQUE(user_id, container_id)
);
CREATE INDEX IF NOT EXISTS idx_container_users_user_id ON container_users(user_id);

CREATE TABLE IF NOT EXISTS sessions (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    data BLOB NOT NULL,
    expires_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_sessions_user_id ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_expires_at ON sessions(expires_at);
