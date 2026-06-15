-- Per-user Jira API credential, modeled on user_github_credentials.
--
-- The Jira API token is the secret → sealed with the envelope scheme
-- (ciphertext/nonce/wrapped_dek/wnonce, BLOB columns rewritten to BYTEA on
-- Postgres by the DialectAware transformer). The Jira site base URL and the
-- account email are stored as plain metadata columns (they are not secrets;
-- they form the Basic-auth pair email:token and the REST base URL). The
-- account id / display name captured at validation time are also stored so
-- the dashboard can render "connected as <name>" without re-hitting Jira.
--
-- Singleton per user (PRIMARY KEY user_id), matching the GitHub PAT table.
-- Timestamp columns stay TEXT (RFC3339 strings) bound explicitly by the DAO.

CREATE TABLE user_jira_credentials (
    user_id VARCHAR(64) PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    ciphertext BLOB NOT NULL,
    nonce BLOB NOT NULL,
    wrapped_dek BLOB NOT NULL,
    wnonce BLOB NOT NULL,
    site VARCHAR(512) NOT NULL,
    email VARCHAR(320) NOT NULL,
    account_id VARCHAR(128) NOT NULL DEFAULT '',
    account_name VARCHAR(255) NOT NULL DEFAULT '',
    last_validated_at TEXT,
    created_at TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT ''
);
