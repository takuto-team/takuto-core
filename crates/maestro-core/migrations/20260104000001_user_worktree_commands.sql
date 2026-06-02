-- Plan-11 step 2 — hand-translated port of MIGRATION_V4 (plan-09
-- per-user worktree settings). Drops plan-08's `workspace_commands`
-- (never released — see V3 file header) and replaces it with the
-- per-user table.
--
-- The DROP is grandfathered per plan-11 §7.5 ("non-destructiveness
-- guarantee") because the previous table never held production data.

DROP TABLE IF EXISTS workspace_commands;

CREATE TABLE user_worktree_commands (
    user_id VARCHAR(64) NOT NULL,
    workspace_name VARCHAR(255) NOT NULL,
    init_commands_json TEXT NOT NULL DEFAULT '[]',
    run_commands_json TEXT NOT NULL DEFAULT '[]',
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, workspace_name),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_user_worktree_commands_user
    ON user_worktree_commands(user_id, updated_at DESC);
