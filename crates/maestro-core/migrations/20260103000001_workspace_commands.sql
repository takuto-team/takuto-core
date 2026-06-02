-- Per-workspace command override. Note: this table is dropped by the
-- next migration before any production deployment ever wrote to it;
-- the migration is kept so upgrades can still play forward through
-- this version via the runner.

CREATE TABLE IF NOT EXISTS workspace_commands (
    workspace_name VARCHAR(255) PRIMARY KEY,
    commands_json TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    updated_by VARCHAR(64),
    FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_workspace_commands_updated
    ON workspace_commands(updated_at DESC);
