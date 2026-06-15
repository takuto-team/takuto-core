-- Per-user, per-workspace work-item flows.
--
-- One row per (user, workspace). `flows_json` holds the user's full
-- ordered flow list as a JSON array; an absent row means "not yet
-- seeded for this workspace", an empty array means "seeded then
-- emptied by the user". `updated_at` is Unix milliseconds.

CREATE TABLE user_work_item_flows (
    user_id VARCHAR(64) NOT NULL,
    workspace_name VARCHAR(255) NOT NULL,
    flows_json TEXT NOT NULL DEFAULT '[]',
    updated_at BIGINT NOT NULL,
    PRIMARY KEY (user_id, workspace_name),
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);
CREATE INDEX idx_user_work_item_flows_user
    ON user_work_item_flows(user_id, updated_at DESC);
