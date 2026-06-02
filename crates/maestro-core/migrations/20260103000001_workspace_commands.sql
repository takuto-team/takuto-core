-- Plan-11 step 2 — hand-translated port of MIGRATION_V3 (plan-08
-- workspace_commands per-workspace override). Note: this table is
-- dropped by MIGRATION_V4 (plan-09) before any production deployment
-- ever wrote to it; the migration is kept so v3→v4 upgrades can
-- still play forward via plan-11's runner once it goes live.

CREATE TABLE IF NOT EXISTS workspace_commands (
    workspace_name VARCHAR(255) PRIMARY KEY,
    commands_json TEXT NOT NULL,
    updated_at BIGINT NOT NULL,
    updated_by VARCHAR(64),
    FOREIGN KEY (updated_by) REFERENCES users(id) ON DELETE SET NULL
);
CREATE INDEX IF NOT EXISTS idx_workspace_commands_updated
    ON workspace_commands(updated_at DESC);
