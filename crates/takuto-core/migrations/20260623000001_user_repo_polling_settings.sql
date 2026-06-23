-- Per-(user, workspace) polling settings. Polling configuration is now
-- per-user, per-repository: which Jira projects / GitHub filters a repository
-- polls, how often, which flow to auto-start, and the parallel-item caps are
-- all owned by the user for that repository and edited from the Ticketing tab.
-- Deployment-wide limits (max_concurrent_manual_workflows,
-- pr_merge_poll_interval_secs, generate_report default, work_item log
-- retention) stay global in `[general]`.
--
-- `settings_json` is a JSON object (the `RepoPollingSettings` struct, default
-- `'{}'` so a freshly-inserted row decodes to all-defaults). One JSON column
-- rather than a wide table: the per-repo settings evolve together and are read
-- as a unit by the poller and the manual picker.
--
-- `user_id` references `users(id) ON DELETE CASCADE`. `workspace_name` is the
-- repository name (same key convention as `user_worktree_commands`).

CREATE TABLE user_repo_polling_settings (
    user_id VARCHAR(64) NOT NULL,
    workspace_name VARCHAR(255) NOT NULL,
    settings_json TEXT NOT NULL DEFAULT '{}',
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, workspace_name),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_user_repo_polling_settings_user
    ON user_repo_polling_settings(user_id, updated_at DESC);
