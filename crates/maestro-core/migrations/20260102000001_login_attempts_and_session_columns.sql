-- Auth hardening: per-user login attempts + sliding-window /
-- absolute-TTL session columns.
--
-- Adds the `login_attempts` audit table and two new columns on the
-- existing `sessions` row.

CREATE TABLE IF NOT EXISTS login_attempts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id VARCHAR(64) NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind VARCHAR(16) NOT NULL CHECK(kind IN ('password','recovery')),
    attempted_at BIGINT NOT NULL,
    success INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS idx_login_attempts_user_kind_time
    ON login_attempts(user_id, kind, attempted_at);

-- ALTER TABLE … ADD COLUMN is portable across SQLite, Postgres, MySQL —
-- no transform needed. Both columns default to 0 so V1 sessions backfill
-- with a timestamp older than the 30-day absolute TTL; the auth path
-- will reject and lazily delete them on next use.
ALTER TABLE sessions ADD COLUMN last_seen_at BIGINT NOT NULL DEFAULT 0;
ALTER TABLE sessions ADD COLUMN created_at_unix BIGINT NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS idx_sessions_last_seen_at ON sessions(last_seen_at);
