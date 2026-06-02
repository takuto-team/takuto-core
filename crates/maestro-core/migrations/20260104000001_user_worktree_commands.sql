-- Per-user worktree settings. Drops the earlier `workspace_commands`
-- table (never released — see its file header) and replaces it with
-- the per-user table.
--
-- The DROP is safe because the previous table never held production data.

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
