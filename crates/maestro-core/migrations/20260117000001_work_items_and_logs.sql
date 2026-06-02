-- Work-item state, steps, definition runs, log lines, port mappings,
-- and run-command state in the database.
--
-- These tables become the truth-of-record for everything that today
-- lives in:
--   • `workspaces/*/workflow_snapshot.json` (rewritten every 60 s)
--   • `{repo_path}/logs/<TICKET>.log` (per-ticket append-only logs)
--   • the engine's in-memory `HashMap<String, Workflow>`
--
-- Portability notes (matching the existing migration set):
--   • IDs use VARCHAR(64) — MySQL rejects TEXT PRIMARY KEY.
--   • Autoincrement `id` columns are declared as
--     `INTEGER PRIMARY KEY AUTOINCREMENT`; the DialectAware
--     transformer rewrites per backend (BIGSERIAL on PG, BIGINT
--     AUTO_INCREMENT on MySQL).
--   • Unix-seconds timestamps are BIGINT (8 bytes) so values past
--     the 2038 i32 ceiling fit on Postgres, where plain INTEGER is
--     INT4.
--   • Composite primary keys avoid nullable columns (Postgres rejects
--     NULL in PK). The port-mapping table uses an `id` surrogate PK +
--     a non-unique index in place of the natural composite key.

-- ── work_items ──────────────────────────────────────────────────────
-- One row per Jira/GitHub ticket or manually-pasted item.
CREATE TABLE work_items (
    id VARCHAR(64) PRIMARY KEY NOT NULL,
    ticket_key VARCHAR(255) NOT NULL,
    workspace_name VARCHAR(255) NOT NULL,
    user_id VARCHAR(64),
    private INTEGER NOT NULL DEFAULT 0,
    started_manually INTEGER NOT NULL DEFAULT 0,
    counts_toward_manual_cap INTEGER NOT NULL DEFAULT 0,
    driver_started INTEGER NOT NULL DEFAULT 0,
    jira_available INTEGER NOT NULL DEFAULT 1,

    -- Ticket metadata
    ticket_summary TEXT,
    ticket_description TEXT,
    ticket_type VARCHAR(64),
    ticket_url TEXT,
    acceptance_criteria TEXT,

    -- Git / PR state
    base_branch VARCHAR(255),
    branch_name VARCHAR(255),
    worktree_path TEXT,
    pr_url TEXT,
    pr_merged INTEGER NOT NULL DEFAULT 0,

    -- Agent state
    last_session_id VARCHAR(255),

    -- State machine
    state_kind VARCHAR(32) NOT NULL,
    state_payload TEXT,
    current_step_label TEXT,

    -- Timestamps (Unix seconds — BIGINT keeps values past 2038 on PG).
    created_at BIGINT NOT NULL,
    started_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL,

    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE SET NULL
);

CREATE UNIQUE INDEX idx_work_items_workspace_key
    ON work_items(workspace_name, ticket_key);
CREATE INDEX idx_work_items_user
    ON work_items(user_id, workspace_name, started_at DESC);
CREATE INDEX idx_work_items_state
    ON work_items(state_kind, workspace_name);
CREATE INDEX idx_work_items_started
    ON work_items(started_at DESC);

-- ── work_item_steps ─────────────────────────────────────────────────
-- Per-step execution log. Replaces the in-memory `steps_log`.
CREATE TABLE work_item_steps (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    work_item_id VARCHAR(64) NOT NULL,
    sequence INTEGER NOT NULL,
    name VARCHAR(255) NOT NULL,
    definition_filename VARCHAR(255),
    status VARCHAR(32) NOT NULL,
    started_at BIGINT NOT NULL,
    ended_at BIGINT,
    exit_code INTEGER,
    error_message TEXT,
    FOREIGN KEY (work_item_id) REFERENCES work_items(id) ON DELETE CASCADE
);
CREATE INDEX idx_work_item_steps_work_item
    ON work_item_steps(work_item_id, sequence);

-- ── work_item_definition_runs ───────────────────────────────────────
-- Per-definition run state. Replaces the in-memory `workflow_def_runs`
-- HashMap on `Workflow`.
CREATE TABLE work_item_definition_runs (
    work_item_id VARCHAR(64) NOT NULL,
    definition_filename VARCHAR(255) NOT NULL,
    state VARCHAR(32) NOT NULL,
    error_message TEXT,
    started_at BIGINT,
    ended_at BIGINT,
    PRIMARY KEY (work_item_id, definition_filename),
    FOREIGN KEY (work_item_id) REFERENCES work_items(id) ON DELETE CASCADE
);

-- ── work_item_log_lines ─────────────────────────────────────────────
-- Replaces both:
--   • the 100 most recent `terminal_lines` in the snapshot
--   • the {repo_path}/logs/<TICKET>.log files
--
-- `emitted_at` is unix milliseconds (not seconds) — the agent can emit
-- hundreds of lines per second; second-resolution would lose ordering.
CREATE TABLE work_item_log_lines (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    work_item_id VARCHAR(64) NOT NULL,
    step_id BIGINT,
    stream VARCHAR(16) NOT NULL,
    content TEXT NOT NULL,
    emitted_at BIGINT NOT NULL,
    FOREIGN KEY (work_item_id) REFERENCES work_items(id) ON DELETE CASCADE,
    FOREIGN KEY (step_id) REFERENCES work_item_steps(id) ON DELETE SET NULL
);
CREATE INDEX idx_work_item_log_lines_work_item
    ON work_item_log_lines(work_item_id, emitted_at);
CREATE INDEX idx_work_item_log_lines_step
    ON work_item_log_lines(step_id, emitted_at);

-- ── work_item_port_mappings ─────────────────────────────────────────
-- Today the engine rebuilds these on every load from container labels.
-- Persisting them lets the dashboard restore port buttons immediately
-- after a server restart without re-discovery latency.
--
-- The plan sketches a composite PK including `run_command_index`,
-- but that column is NULL for non-run-command rows and Postgres
-- rejects NULL in PK. Use a surrogate `id` PK and a non-unique index
-- on `(work_item_id, container_port, kind)`; uniqueness is enforced
-- in application code via upsert (one mapping per kind per container
-- port per work item).
CREATE TABLE work_item_port_mappings (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    work_item_id VARCHAR(64) NOT NULL,
    container_port INTEGER NOT NULL,
    host_port INTEGER NOT NULL,
    proxy_url TEXT NOT NULL,
    path_token VARCHAR(255) NOT NULL,
    kind VARCHAR(32) NOT NULL,
    run_command_index INTEGER,
    created_at BIGINT NOT NULL,
    FOREIGN KEY (work_item_id) REFERENCES work_items(id) ON DELETE CASCADE
);
CREATE INDEX idx_work_item_port_mappings_work_item
    ON work_item_port_mappings(work_item_id, container_port, kind);
CREATE INDEX idx_work_item_port_mappings_token
    ON work_item_port_mappings(path_token);

-- ── work_item_run_commands ──────────────────────────────────────────
-- Run-command state. Today this is rebuilt from container labels on
-- every load; persisting it lets the dashboard show stale-but-correct
-- buttons immediately after a server restart.
CREATE TABLE work_item_run_commands (
    work_item_id VARCHAR(64) NOT NULL,
    command_index INTEGER NOT NULL,
    name VARCHAR(255) NOT NULL,
    running INTEGER NOT NULL DEFAULT 0,
    container_id VARCHAR(128),
    started_at BIGINT,
    ended_at BIGINT,
    PRIMARY KEY (work_item_id, command_index),
    FOREIGN KEY (work_item_id) REFERENCES work_items(id) ON DELETE CASCADE
);
